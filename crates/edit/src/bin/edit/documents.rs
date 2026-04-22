// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ffi::OsStr;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::{fs, io};

use edit::buffer::{RcTextBuffer, TextBuffer};
use edit::helpers::{CoordType, Point};
use edit::lsh::{FILE_ASSOCIATIONS, Language, process_file_associations};
use edit::{path, sys};

use crate::apperr;
use crate::settings::Settings;

pub struct Document {
    pub buffer: RcTextBuffer,
    pub path: PathBuf,
    pub filename: String,
    pub file_id: Option<sys::FileId>,
    pub language_override: Option<Option<&'static Language>>,
    pub read_only: bool,
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
        let mut doc =
            Document { buffer, path, filename, file_id, language_override: None, read_only };
        doc.apply_path_metadata();
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

        Ok(())
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
