//! Microsoft Edit's adapter to LSH.

pub mod cache;
mod definitions;
mod highlighter;

use std::path::Path;

pub use definitions::{FILE_ASSOCIATIONS, HighlightKind, LANGUAGES};
pub use highlighter::*;
pub use lsh::runtime::Language;
use stdext::glob::glob_match;

pub fn process_file_associations<T>(
    associations: &[(T, &'static Language)],
    path: &Path,
) -> Option<&'static Language>
where
    T: AsRef<[u8]>,
{
    let path = path.as_os_str().as_encoded_bytes();

    for a in associations {
        if glob_match(a.0.as_ref(), path) {
            return Some(a.1);
        }
    }

    None
}

/// Try to identify the language of a buffer from its shebang (`#!...`) line.
/// Used as a fallback for files whose extension didn't match any association --
/// executable scripts w/out a file extension are the main motivating case.
///
/// `head` should be the first chunk of the buffer; only the first line is
/// inspected. Returns `None` if there's no shebang or the interpreter isn't
/// recognised.
pub fn language_from_shebang(head: &[u8]) -> Option<&'static Language> {
    let line = head.split(|&b| b == b'\n').next()?;
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    if !line.starts_with(b"#!") {
        return None;
    }
    let body = &line[2..];

    // Walk whitespace-separated tokens. Skip flags (`-S`, `-u`, ...) and the
    // `env` wrapper itself; the first remaining token names the interpreter.
    let mut interpreter: Option<&[u8]> = None;
    for tok in body.split(|&b| b == b' ' || b == b'\t') {
        if tok.is_empty() || tok.starts_with(b"-") {
            continue;
        }
        let base = basename(tok);
        if base == b"env" {
            continue;
        }
        interpreter = Some(base);
        break;
    }
    let interpreter = interpreter?;
    let id = interpreter_to_language_id(strip_version_suffix(interpreter))?;
    LANGUAGES.iter().find(|l| l.id == id)
}

fn basename(path: &[u8]) -> &[u8] {
    match path.iter().rposition(|&b| b == b'/') {
        Some(i) => &path[i + 1..],
        None => path,
    }
}

/// Drop a trailing version suffix made of digits and dots, so `python3.11` and
/// `python3` both map to `python`.
fn strip_version_suffix(name: &[u8]) -> &[u8] {
    let mut end = name.len();
    while end > 0 {
        let c = name[end - 1];
        if c.is_ascii_digit() || c == b'.' {
            end -= 1;
        } else {
            break;
        }
    }
    if end == 0 { name } else { &name[..end] }
}

fn interpreter_to_language_id(name: &[u8]) -> Option<&'static str> {
    Some(match name {
        b"sh" | b"bash" | b"zsh" | b"dash" | b"ksh" | b"ash" | b"mksh" | b"fish" => "shellscript",
        b"python" => "python",
        b"node" | b"deno" | b"bun" => "javascript",
        b"pwsh" | b"powershell" => "powershell",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(line: &[u8]) -> Option<&'static str> {
        language_from_shebang(line).map(|l| l.id)
    }

    #[test]
    fn shebang_shellscript() {
        assert_eq!(id(b"#!/bin/sh\n"), Some("shellscript"));
        assert_eq!(id(b"#!/bin/bash"), Some("shellscript"));
        assert_eq!(id(b"#!/usr/bin/env bash\n"), Some("shellscript"));
        assert_eq!(id(b"#!/usr/bin/env -S bash -e\n"), Some("shellscript"));
        assert_eq!(id(b"#!/usr/bin/zsh\n"), Some("shellscript"));
        assert_eq!(id(b"#!/usr/bin/env fish\n"), Some("shellscript"));
        assert_eq!(id(b"#!/bin/sh\r\n"), Some("shellscript"));
        assert_eq!(id(b"#!/bin/sh\nfollowed by other text"), Some("shellscript"));
        assert_eq!(id(b"#!\t/bin/bash\n"), Some("shellscript"));
    }

    #[test]
    fn shebang_other_interpreters() {
        assert_eq!(id(b"#!/usr/bin/env python3\n"), Some("python"));
        assert_eq!(id(b"#!/usr/bin/python3.11\n"), Some("python"));
        assert_eq!(id(b"#!/usr/bin/env -S python3 -u\n"), Some("python"));
        assert_eq!(id(b"#!/usr/bin/env node\n"), Some("javascript"));
        assert_eq!(id(b"#!/usr/bin/env pwsh\n"), Some("powershell"));
    }

    #[test]
    fn shebang_none() {
        assert_eq!(id(b""), None);
        assert_eq!(id(b"hello world\n"), None);
        assert_eq!(id(b"# not a shebang\n"), None);
        assert_eq!(id(b"#!/usr/bin/env\n"), None);
        assert_eq!(id(b"#!/usr/bin/env nonesuch\n"), None);
        assert_eq!(id(b"  #!/bin/sh\n"), None);
    }
}
