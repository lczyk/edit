// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ffi::OsStr;
use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;
use std::{fs, io};

use edit::buffer::{RcTextBuffer, TextBuffer};
use edit::helpers::{CoordType, Point};
use edit::lsh::{FILE_ASSOCIATIONS, Language, process_file_associations};
use edit::{path, sys};

use crate::apperr;
use crate::diff_mode::{self, DiffState};
use crate::git;
use crate::settings::Settings;

pub struct DiffMode {
    pub state: DiffState,
    /// Bumped by user edits; cleared after a rediff rebuild.
    pub dirty_generation: u32,
    /// Generation of the buffer the last time we ran a rediff.
    pub last_rebuilt_generation: u32,
    /// `Instant` at which `dirty_generation` last changed; used for debounce.
    pub dirty_since: Option<Instant>,
    /// Running counts of added / deleted lines for the status badge.
    pub hunk_adds: u32,
    pub hunk_dels: u32,
}

pub struct Document {
    pub buffer: RcTextBuffer,
    pub path: PathBuf,
    pub filename: String,
    pub file_id: Option<sys::FileId>,
    pub language_override: Option<Option<&'static Language>>,
    pub read_only: bool,
    pub diff: Option<DiffMode>,
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
            diff: None,
        };
        doc.apply_path_metadata();
        Ok(doc)
    }

    pub fn save(&mut self) -> apperr::Result<()> {
        if self.read_only {
            return Err(apperr::Error::from(io::Error::from(io::ErrorKind::PermissionDenied)));
        }

        let mut file = open_for_writing(&self.path)?;

        if self.diff.is_some() {
            // In diff mode the view buffer contains deleted-line stripes; we
            // must save only the editable portion.
            let real = {
                let tb = self.buffer.borrow();
                diff_mode::extract_real(&tb)
            };
            file.write_all(&real)?;
            self.buffer.borrow_mut().mark_as_clean();
        } else {
            let mut tb = self.buffer.borrow_mut();
            tb.write_file(&mut file)?;
        }

        if let Ok(id) = sys::file_id(None, &self.path) {
            self.file_id = Some(id);
        }

        Ok(())
    }

    /// Enters diff mode: locate repo, read baseline, build the interleaved
    /// view and install it on the buffer. Caller should display returned
    /// errors via the error log.
    pub fn enter_diff_mode(&mut self) -> apperr::Result<()> {
        if self.diff.is_some() {
            return Ok(());
        }
        if self.read_only {
            return Err(apperr::Error::from(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "file is read-only",
            )));
        }

        let info = git::locate(&self.path)?;
        if !git::is_tracked(&info)? {
            return Err(apperr::Error::from(io::Error::new(
                io::ErrorKind::NotFound,
                "file is not tracked by git",
            )));
        }
        let baseline = git::read_baseline(&info)?;
        if baseline.len() > diff_mode::MAX_DIFF_BYTES {
            return Err(apperr::Error::from(io::Error::new(
                io::ErrorKind::InvalidInput,
                "baseline exceeds diff-mode size cap",
            )));
        }
        if looks_binary(&baseline) {
            return Err(apperr::Error::from(io::Error::new(
                io::ErrorKind::InvalidData,
                "baseline looks binary",
            )));
        }

        let current = {
            let tb = self.buffer.borrow();
            read_all(&tb)
        };
        if current.len() > diff_mode::MAX_DIFF_BYTES {
            return Err(apperr::Error::from(io::Error::new(
                io::ErrorKind::InvalidInput,
                "file exceeds diff-mode size cap",
            )));
        }

        let state = DiffState::new(baseline);
        let view = state.build_view(&current);
        let (adds, dels) = count_hunks(&view);

        {
            let mut tb = self.buffer.borrow_mut();
            tb.set_locked_content(&view.bytes, view.locked_ranges);
            tb.set_line_decorations(view.decorations);
        }

        self.diff = Some(DiffMode {
            state,
            dirty_generation: 0,
            last_rebuilt_generation: self.buffer.borrow().generation(),
            dirty_since: None,
            hunk_adds: adds,
            hunk_dels: dels,
        });

        Ok(())
    }

    /// Leaves diff mode. Replaces the view with just the editable content
    /// (stripes gone) and clears locks / decorations.
    pub fn exit_diff_mode(&mut self) {
        if self.diff.is_none() {
            return;
        }

        let real = {
            let tb = self.buffer.borrow();
            diff_mode::extract_real(&tb)
        };

        {
            let mut tb = self.buffer.borrow_mut();
            tb.set_locked_content(&real, Vec::new());
            tb.clear_line_decorations();
        }

        self.diff = None;
    }

    /// Re-runs the diff against the stored baseline and rebuilds the view.
    /// Preserves the cursor by round-tripping through a real-content anchor.
    pub fn rediff(&mut self) {
        let Some(diff) = self.diff.as_mut() else { return };

        let (cursor_real, current_bytes) = {
            let tb = self.buffer.borrow();
            let cursor_real =
                diff_mode::view_to_real_off(tb.cursor_offset(), tb.locked_ranges());
            let current = diff_mode::extract_real(&tb);
            (cursor_real, current)
        };

        let view = diff.state.build_view(&current_bytes);
        let (adds, dels) = count_hunks(&view);

        let mut tb = self.buffer.borrow_mut();
        // `refresh_view_content` preserves undo/redo and the dirty flag.
        tb.refresh_view_content(&view.bytes, view.locked_ranges);
        tb.set_line_decorations(view.decorations);

        let new_view_off =
            diff_mode::real_to_view_off(cursor_real, tb.locked_ranges(), tb.text_length());
        tb.cursor_move_to_offset(new_view_off);

        diff.last_rebuilt_generation = tb.generation();
        diff.dirty_since = None;
        diff.hunk_adds = adds;
        diff.hunk_dels = dels;
    }

    /// Call after each input batch in diff mode. If the buffer has changed
    /// since the last rebuild, records the dirtying time for debouncing.
    pub fn diff_mark_dirty_if_changed(&mut self) {
        let Some(diff) = self.diff.as_mut() else { return };
        let current_gen = self.buffer.borrow().generation();
        if current_gen != diff.last_rebuilt_generation && diff.dirty_since.is_none() {
            diff.dirty_since = Some(Instant::now());
        }
    }

    /// Returns `true` if enough idle time has elapsed since the last edit to
    /// warrant a rediff.
    pub fn diff_should_rebuild(&self) -> bool {
        let Some(diff) = self.diff.as_ref() else { return false };
        let Some(since) = diff.dirty_since else { return false };
        since.elapsed().as_millis() as u64 >= diff_mode::REDIFF_DEBOUNCE_MS
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
        self.buffer.borrow_mut().set_language(self.get_language());
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

        None
    }
}

fn create_buffer() -> apperr::Result<RcTextBuffer> {
    let buffer = TextBuffer::new_rc(false)?;
    {
        let mut tb = buffer.borrow_mut();
        tb.set_insert_final_newline(true); // POSIX.
        tb.set_margin_enabled(true);
        tb.set_line_highlight_enabled(true);
    }
    Ok(buffer)
}

fn read_all(tb: &TextBuffer) -> Vec<u8> {
    let mut out = Vec::with_capacity(tb.text_length());
    let mut off = 0;
    loop {
        let chunk = tb.read_forward(off);
        if chunk.is_empty() {
            break;
        }
        out.extend_from_slice(chunk);
        off += chunk.len();
    }
    out
}

fn looks_binary(bytes: &[u8]) -> bool {
    let window = &bytes[..bytes.len().min(8 * 1024)];
    window.contains(&0)
}

fn count_hunks(view: &diff_mode::BuiltView) -> (u32, u32) {
    let mut adds = 0;
    let mut dels = 0;
    for d in &view.decorations {
        match d {
            edit::buffer::LineDecoration::Added => adds += 1,
            edit::buffer::LineDecoration::Deleted => dels += 1,
            edit::buffer::LineDecoration::None => {}
        }
    }
    (adds, dels)
}

fn open_for_writing(path: &Path) -> apperr::Result<File> {
    // It is worth doing an existence check because it is significantly
    // faster than calling mkdir() and letting it fail.
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        fs::create_dir_all(parent)?;
    }

    File::create(path).map_err(apperr::Error::from)
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
