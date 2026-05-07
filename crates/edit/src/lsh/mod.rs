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

/// Try to identify the language of a buffer from its content alone, when
/// neither path nor shebang detection produced a hit. Currently only
/// recognises markdown -- common case is a `README` / `NOTES` / `LICENSE`
/// file w/out an `.md` suffix, or a brand-new buffer where the user has
/// started typing markdown before saving under a recognisable name.
///
/// `head` should be a chunk of the buffer (a few KB is plenty); only the
/// first ~64 lines are inspected.
pub fn language_from_content(head: &[u8]) -> Option<&'static Language> {
    if looks_like_markdown(head) {
        return LANGUAGES.iter().find(|l| l.id == "markdown");
    }
    None
}

fn looks_like_markdown(head: &[u8]) -> bool {
    let mut score: i32 = 0;
    let mut nonempty_lines: u32 = 0;
    let mut in_code_fence = false;
    let mut first = true;

    for raw in head.split(|&b| b == b'\n').take(64) {
        let line = raw.strip_suffix(b"\r").unwrap_or(raw);
        let trimmed = trim_left_ws(line);

        if first {
            first = false;
            // YAML frontmatter delimiter at very start of file
            if line == b"---" {
                score += 3;
            }
        }

        if trimmed.is_empty() {
            continue;
        }
        nonempty_lines += 1;

        // fenced code block toggles -- count the fence itself, but skip
        // scoring lines inside the fence (they look like prose / code, not md).
        if trimmed.starts_with(b"```") || trimmed.starts_with(b"~~~") {
            score += 2;
            in_code_fence = !in_code_fence;
            continue;
        }
        if in_code_fence {
            continue;
        }

        // ATX heading: 1-6 `#`s followed by a space
        if trimmed.starts_with(b"#") {
            let hashes = trimmed.iter().take_while(|&&b| b == b'#').count();
            if (1..=6).contains(&hashes) && trimmed.get(hashes).copied() == Some(b' ') {
                score += 3;
                continue;
            }
        }

        // bullet list marker `- `, `* `, `+ `
        if matches!(trimmed.first(), Some(b'-') | Some(b'*') | Some(b'+'))
            && trimmed.get(1).copied() == Some(b' ')
        {
            score += 1;
        }

        // numbered list `1. `
        let digits = trimmed.iter().take_while(|&&b| b.is_ascii_digit()).count();
        if digits > 0
            && trimmed.get(digits).copied() == Some(b'.')
            && trimmed.get(digits + 1).copied() == Some(b' ')
        {
            score += 1;
        }

        // blockquote
        if trimmed.starts_with(b"> ") || trimmed == b">" {
            score += 1;
        }

        // setext heading underline
        if trimmed.len() >= 3
            && (trimmed.iter().all(|&b| b == b'=') || trimmed.iter().all(|&b| b == b'-'))
        {
            score += 2;
        }

        // inline link `[text](url)` -- crude but effective
        if let Some(p) = find_subseq(trimmed, b"](")
            && trimmed[..p].contains(&b'[')
        {
            score += 2;
        }

        // reference-style link definition `[label]: url`
        if trimmed.starts_with(b"[") && find_subseq(trimmed, b"]: ").is_some() {
            score += 2;
        }
    }

    nonempty_lines >= 2 && score >= 4
}

fn trim_left_ws(line: &[u8]) -> &[u8] {
    let mut i = 0;
    while i < line.len() && (line[i] == b' ' || line[i] == b'\t') {
        i += 1;
    }
    &line[i..]
}

fn find_subseq(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
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

    fn content_id(text: &[u8]) -> Option<&'static str> {
        language_from_content(text).map(|l| l.id)
    }

    #[test]
    fn content_markdown_obvious() {
        // multiple heading-shaped lines + a link is plenty
        let s = b"# Title\n\nsome intro text.\n\n## Section\n\nsee [here](https://x).\n";
        assert_eq!(content_id(s), Some("markdown"));
    }

    #[test]
    fn content_markdown_frontmatter() {
        let s = b"---\ntitle: hi\n---\n\n# heading\n\nbody.\n";
        assert_eq!(content_id(s), Some("markdown"));
    }

    #[test]
    fn content_markdown_list_and_quote() {
        let s = b"# notes\n\n- one\n- two\n- three\n\n> quoted bit\n";
        assert_eq!(content_id(s), Some("markdown"));
    }

    #[test]
    fn content_not_markdown() {
        // single short line, no signals
        assert_eq!(content_id(b"hello world\n"), None);
        // a single bullet by itself isn't enough
        assert_eq!(content_id(b"- one item\n"), None);
        // plain prose
        assert_eq!(
            content_id(b"this is just prose with no markdown markers in it at all.\n"),
            None
        );
        // empty
        assert_eq!(content_id(b""), None);
    }

    #[test]
    fn content_code_fence_does_not_score_inner_lines() {
        // body inside a fence shouldn't accumulate score from `# comment`
        let s = b"```\n# this is a python comment\n# another\n# yet another\n```\n";
        // only the two fences score (2+2 = 4) and there are 5 nonempty lines,
        // so this just barely qualifies. Tighten if it becomes a problem.
        // The real defence is requiring multiple distinct signal types in the
        // wild; a bare fenced block on its own is fine to call markdown.
        assert_eq!(content_id(s), Some("markdown"));
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
