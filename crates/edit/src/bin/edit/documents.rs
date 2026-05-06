use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

static ALLOW_CREATE: AtomicBool = AtomicBool::new(false);

/// Set by [`crate::parse_args`] from `--quirks=allow-create`. When false (the
/// default), edit refuses to open a path that doesn't exist and refuses to
/// save into a file that doesn't exist on disk. Directory creation is never
/// permitted -- not even with the quirk on.
pub fn set_allow_create(enabled: bool) {
    ALLOW_CREATE.store(enabled, Ordering::Relaxed);
}

pub fn allow_create() -> bool {
    ALLOW_CREATE.load(Ordering::Relaxed)
}

use edit::buffer::{RcTextBuffer, TextBuffer};
use edit::framebuffer::IndexedColor;
use edit::helpers::{CoordType, Point};
use edit::lsh::{FILE_ASSOCIATIONS, Language, language_from_shebang, process_file_associations};
use edit::{path, sys};

use gutter::gutter_diff::{self, BaselineState};

use crate::apperr;
use crate::minimap::MinimapState;
use crate::settings::Settings;

pub struct Document {
    pub buffer: RcTextBuffer,
    pub path: PathBuf,
    pub filename: String,
    pub file_id: Option<sys::FileId>,
    pub language_override: Option<Option<&'static Language>>,
    pub read_only: bool,

    /// `None` until first refresh attempt; `Some` carries the cached
    /// baseline blob (or a `disabled` flag if the file isn't in a git repo
    /// or otherwise can't be diffed).
    baseline: Option<BaselineState>,
    /// Buffer generation at last gutter recompute. Compared each tick to
    /// decide whether marks need refreshing.
    last_gutter_generation: u32,
    /// True after the buffer changed and we haven't yet recomputed marks.
    /// Used together with [`Self::gutter_dirty_since`] for debouncing.
    gutter_dirty: bool,
    gutter_dirty_since: Option<std::time::Instant>,

    /// Lazily-built minimap data. Rebuilt under the same dirty/debounce
    /// rhythm as the gutter.
    minimap: MinimapState,
    last_minimap_generation: u32,
    minimap_dirty: bool,
    minimap_dirty_since: Option<std::time::Instant>,
    minimap_target_width: u8,
}

impl Document {
    /// Opens the file at `path` into a new document. `path` may carry a
    /// `:line[:column]` suffix (parsed with [`parse_filename_goto`]).
    pub fn open(path: &Path) -> apperr::Result<Self> {
        let (path, goto) = parse_filename_goto(path);
        let path = path::normalize(path);

        let mut file = match File::open(&path) {
            Ok(file) => Some(file),
            Err(err) if err.kind() == io::ErrorKind::NotFound => None,
            Err(err) => return Err(err.into()),
        };

        let file_id = if file.is_some() { Some(sys::file_id(file.as_ref(), &path)?) } else { None };
        let read_only = file.is_some() && !sys::is_path_writable(&path);

        let buffer = create_buffer()?;
        if let Some(file) = &mut file {
            let mut tb = buffer.borrow_mut();
            tb.read_file(file)?;
            if let Some(goto) = goto
                && goto != Default::default()
            {
                tb.cursor_move_to_logical(goto);
            }
            tb.set_read_only(read_only);
        }

        let filename = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let mut doc = Document {
            buffer,
            path,
            filename,
            file_id,
            language_override: None,
            read_only,
            baseline: None,
            last_gutter_generation: 0,
            gutter_dirty: true,
            gutter_dirty_since: None,
            minimap: MinimapState::new(),
            last_minimap_generation: 0,
            minimap_dirty: true,
            minimap_dirty_since: None,
            minimap_target_width: 2,
        };
        doc.apply_path_metadata();
        // Build the minimap eagerly so the very first frame already has it.
        // Without this the rail flips in 300ms after open (debounce window),
        // which reads as "scrollbar shows briefly then snaps to minimap".
        doc.minimap_refresh();
        Ok(doc)
    }

    pub fn save(&mut self) -> apperr::Result<()> {
        if self.read_only {
            return Err(apperr::Error::from(io::Error::from(io::ErrorKind::PermissionDenied)));
        }

        let mut file = open_for_writing(&self.path)?;

        {
            let mut tb = self.buffer.borrow_mut();
            tb.write_file(&mut file)?;
        }

        if let Ok(id) = sys::file_id(None, &self.path) {
            self.file_id = Some(id);
        }

        // Saving doesn't change HEAD, so the cached baseline is still
        // valid. Mark gutter dirty so the next tick recomputes immediately.
        self.gutter_dirty = true;
        self.gutter_dirty_since = Some(std::time::Instant::now());
        Ok(())
    }

    /// Mark gutter for recompute if the buffer generation has advanced.
    pub fn gutter_check_dirty(&mut self) {
        let buf_gen = self.buffer.borrow().generation();
        if buf_gen != self.last_gutter_generation {
            self.last_gutter_generation = buf_gen;
            self.gutter_dirty = true;
            self.gutter_dirty_since = Some(std::time::Instant::now());
        }
    }

    /// Returns true iff debounce has elapsed and we should recompute now.
    pub fn gutter_should_rebuild(&self, debounce: std::time::Duration) -> bool {
        self.gutter_dirty && self.gutter_dirty_since.is_some_and(|t| t.elapsed() >= debounce)
    }

    /// Recompute marks. Cheap when the baseline is disabled.
    pub fn gutter_refresh(&mut self) {
        if self.baseline.is_none() {
            self.baseline = Some(BaselineState::load(&self.path));
        }
        let baseline = self.baseline.as_ref().unwrap();
        self.gutter_dirty = false;
        self.gutter_dirty_since = None;
        let Some(bytes) = baseline.bytes.as_deref() else {
            self.buffer.borrow_mut().clear_gutter_marks();
            return;
        };
        let mut tb = self.buffer.borrow_mut();
        let lines = tb.logical_line_count() as u32;
        let len = tb.text_length();
        if len > gutter_diff::MAX_DIFF_BYTES {
            tb.clear_gutter_marks();
            return;
        }
        let mut current = Vec::with_capacity(len);
        tb.copy_all_bytes(&mut current);
        let marks = gutter_diff::compute_marks(bytes, &current, lines);
        tb.set_gutter_marks(marks);
    }

    pub fn minimap_check_dirty(&mut self) {
        let buf_gen = self.buffer.borrow().generation();
        if buf_gen != self.last_minimap_generation {
            self.last_minimap_generation = buf_gen;
            self.minimap_dirty = true;
            self.minimap_dirty_since = Some(std::time::Instant::now());
        }
    }

    /// Set the desired minimap cell width (0 disables, 1 narrow, 2 wide).
    /// Marks the minimap dirty if the value changes.
    pub fn set_minimap_target_width(&mut self, width: u8) {
        if self.minimap_target_width != width {
            self.minimap_target_width = width;
            self.minimap_dirty = true;
            self.minimap_dirty_since = Some(std::time::Instant::now());
        }
    }

    pub fn minimap_should_rebuild(&self, debounce: std::time::Duration) -> bool {
        self.minimap_dirty && self.minimap_dirty_since.is_some_and(|t| t.elapsed() >= debounce)
    }

    pub fn minimap_refresh(&mut self) {
        self.minimap_dirty = false;
        self.minimap_dirty_since = None;
        let mut tb = self.buffer.borrow_mut();
        if self.minimap_target_width == 0 {
            tb.clear_minimap_cells();
            self.minimap.cells.clear();
            return;
        }
        let len = tb.text_length();
        let tab_size = tb.tab_size() as u32;
        let mut bytes = Vec::with_capacity(len);
        tb.copy_all_bytes(&mut bytes);
        self.minimap.rebuild(&bytes, tab_size, self.minimap_target_width);
        if self.minimap.is_suppressed() || self.minimap.cells.is_empty() {
            tb.clear_minimap_cells();
            return;
        }
        // Per-line dominant lsh colour, aggregated over each chunk of source
        // lines that maps to one minimap cell. Skipped only when colour
        // output is suppressed entirely (`no_color`) or no language is set
        // (`dominant_color_per_line` returns empty).
        if !edit::glyphs::no_color() {
            let per_line = tb.dominant_color_per_line();
            if !per_line.is_empty() {
                let chunk = edit::buffer::MINIMAP_SOURCE_ROWS_PER_CELL as usize;
                for (i, cell) in self.minimap.cells.iter_mut().enumerate() {
                    let start = i * chunk;
                    let end = (start + chunk).min(per_line.len());
                    cell.fg = dominant_in_range(&per_line[start..end]);
                }
            }
        }
        tb.set_minimap_cells(self.minimap.cells.clone(), self.minimap.content_rows);
    }

    fn apply_path_metadata(&mut self) {
        self.buffer.borrow_mut().set_ruler(if self.filename == "COMMIT_EDITMSG" { 72 } else { 0 });
        self.update_language();
    }

    pub fn auto_detect_language(&mut self) {
        self.language_override = None;
        self.update_language();
    }

    pub fn override_language(&mut self, lang: Option<&'static Language>) {
        self.language_override = Some(lang);
        self.update_language();
    }

    fn update_language(&mut self) {
        // Resolve before grabbing the buffer mutably -- shebang detection
        // borrows the buffer immutably.
        let lang = self.get_language();
        self.buffer.borrow_mut().set_language(lang);
    }

    fn get_language(&self) -> Option<&'static Language> {
        if let Some(lang) = self.language_override {
            return lang;
        }

        let settings = Settings::borrow();
        if let Some(lang) = process_file_associations(&settings.file_associations, &self.path) {
            return Some(lang);
        }
        if let Some(lang) = process_file_associations(FILE_ASSOCIATIONS, &self.path) {
            return Some(lang);
        }

        // Path-based detection missed -- fall back to the shebang. Catches
        // shell scripts w/out a recognised extension and similar.
        let mut head = Vec::new();
        self.buffer.borrow().copy_first_bytes(256, &mut head);
        language_from_shebang(&head)
    }
}

fn dominant_in_range(slice: &[Option<IndexedColor>]) -> Option<IndexedColor> {
    let mut tally: Vec<(IndexedColor, usize)> = Vec::new();
    for c in slice.iter().flatten().copied() {
        if let Some(s) = tally.iter_mut().find(|(k, _)| *k as u8 == c as u8) {
            s.1 += 1;
        } else {
            tally.push((c, 1));
        }
    }
    tally.into_iter().max_by_key(|(_, n)| *n).map(|(c, _)| c)
}

fn create_buffer() -> apperr::Result<RcTextBuffer> {
    let buffer = TextBuffer::new_rc(false)?;
    {
        let mut tb = buffer.borrow_mut();
        tb.set_insert_final_newline(true); // POSIX.
        tb.set_margin_enabled(true);
        tb.set_line_highlight_enabled(true);
        tb.set_word_wrap(true);
    }
    Ok(buffer)
}

fn open_for_writing(path: &Path) -> apperr::Result<File> {
    // edit never creates parent directories. Saving into a path with a
    // missing parent always fails -- regardless of the `allow-create` quirk.
    let mut opts = OpenOptions::new();
    opts.write(true).truncate(true);
    if allow_create() {
        opts.create(true);
    } else {
        opts.create(false);
    }
    opts.open(path).map_err(apperr::Error::from)
}

/// Hardcoded line-comment token for files lsh doesn't recognise. lsh stays
/// the canonical source -- this only covers extensions we don't yet have
/// syntax definitions for.
pub fn fallback_line_comment(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?;
    Some(match ext {
        "c" | "h" | "cpp" | "cc" | "cxx" | "hpp" | "hxx" | "java" | "kt" | "swift" | "ts"
        | "tsx" | "scala" | "dart" | "zig" => "//",
        "bash" | "zsh" | "fish" | "conf" | "cfg" | "service" | "dockerfile" => "#",
        _ => return None,
    })
}

/// Parse a filename like `foo.txt:123:45` into `(foo.txt, Some(Point { y: 122, x: 44 }))`.
pub fn parse_filename_goto(path: &Path) -> (&Path, Option<Point>) {
    fn parse(s: &[u8]) -> Option<CoordType> {
        if s.is_empty() {
            return None;
        }

        let mut num: CoordType = 0;
        for &b in s {
            if !b.is_ascii_digit() {
                return None;
            }
            let digit = (b - b'0') as CoordType;
            num = num.checked_mul(10)?.checked_add(digit)?;
        }
        Some(num)
    }

    fn find_colon_rev(bytes: &[u8], offset: usize) -> Option<usize> {
        (0..offset.min(bytes.len())).rev().find(|&i| bytes[i] == b':')
    }

    let bytes = path.as_os_str().as_encoded_bytes();
    let colend = match find_colon_rev(bytes, bytes.len()) {
        Some(colend) if colend > 0 => colend,
        _ => return (path, None),
    };

    let last = match parse(&bytes[colend + 1..]) {
        Some(last) => last,
        None => return (path, None),
    };
    let last = (last - 1).max(0);
    let mut len = colend;
    let mut goto = Point { x: 0, y: last };

    if let Some(colbeg) = find_colon_rev(bytes, colend)
        && colbeg != 0
        && let Some(first) = parse(&bytes[colbeg + 1..colend])
    {
        let first = (first - 1).max(0);
        len = colbeg;
        goto = Point { x: last, y: first };
    }

    let path = &bytes[..len];
    let path = unsafe { OsStr::from_encoded_bytes_unchecked(path) };
    let path = Path::new(path);
    (path, Some(goto))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_last_numbers() {
        fn parse(s: &str) -> (&str, Option<Point>) {
            let (p, g) = parse_filename_goto(Path::new(s));
            (p.to_str().unwrap(), g)
        }

        assert_eq!(parse("123"), ("123", None));
        assert_eq!(parse("abc"), ("abc", None));
        assert_eq!(parse(":123"), (":123", None));
        assert_eq!(parse("abc:123"), ("abc", Some(Point { x: 0, y: 122 })));
        assert_eq!(parse("45:123"), ("45", Some(Point { x: 0, y: 122 })));
        assert_eq!(parse(":45:123"), (":45", Some(Point { x: 0, y: 122 })));
        assert_eq!(parse("abc:45:123"), ("abc", Some(Point { x: 122, y: 44 })));
        assert_eq!(parse("abc:def:123"), ("abc:def", Some(Point { x: 0, y: 122 })));
        assert_eq!(parse("1:2:3"), ("1", Some(Point { x: 2, y: 1 })));
        assert_eq!(parse("::3"), (":", Some(Point { x: 0, y: 2 })));
        assert_eq!(parse("1::3"), ("1:", Some(Point { x: 0, y: 2 })));
        assert_eq!(parse(""), ("", None));
        assert_eq!(parse(":"), (":", None));
        assert_eq!(parse("::"), ("::", None));
        assert_eq!(parse("a:1"), ("a", Some(Point { x: 0, y: 0 })));
        assert_eq!(parse("1:a"), ("1:a", None));
        assert_eq!(parse("file.txt:10"), ("file.txt", Some(Point { x: 0, y: 9 })));
        assert_eq!(parse("file.txt:10:5"), ("file.txt", Some(Point { x: 4, y: 9 })));
    }
}
