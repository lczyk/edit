//! The `--list-languages` surface: enumerate the syntax definitions lsh
//! was built with, in a few formats.
//!
//! Lives in the library rather than under `eat` because both personas
//! expose it -- `eat -L` and `edit -L` are the same listing -- and it has
//! nothing to do with catting a file.

use std::io::{self, IsTerminal, Write};
use std::process::ExitCode;

use lsh::runtime::Language;
use lsh_defs::{FILE_ASSOCIATIONS, LANGUAGES};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListFormat {
    Pretty,
    Plain,
    Json,
}

impl ListFormat {
    /// Parse a format name. Used by both eat's argh-driven cli and edit's
    /// hand-rolled parser when they expose `-L [<format>]`.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "pretty" => Ok(ListFormat::Pretty),
            "plain" => Ok(ListFormat::Plain),
            "json" => Ok(ListFormat::Json),
            _ => Err(format!("invalid list format: {value}. expected pretty, plain, or json")),
        }
    }
}

/// gather all languages with their file associations and shebangs, sorted by name.
fn collect_language_rows() -> Vec<(&'static Language, Vec<&'static str>)> {
    let mut rows: Vec<(&'static Language, Vec<&'static str>)> = Vec::new();
    for lang in LANGUAGES {
        let exts: Vec<&str> = FILE_ASSOCIATIONS
            .iter()
            .filter(|(_, l)| std::ptr::eq(*l, lang))
            .map(|(pat, _)| *pat)
            .collect();
        rows.push((lang, exts));
    }
    rows.sort_by_key(|(lang, _)| lang.name.to_ascii_lowercase());
    rows
}

/// detect terminal width: $COLUMNS env var, fallback 80.
fn term_width() -> usize {
    std::env::var("COLUMNS").ok().and_then(|s| s.parse().ok()).unwrap_or(80)
}

/// list-languages: plain "Name: ext1, ext2" -- one per line, no colour, no shebang info.
/// preserved verbatim from the original behaviour for scripting.
fn list_languages_plain() -> ExitCode {
    let rows = collect_language_rows();
    for (lang, exts) in &rows {
        print!("{}", lang.name);
        if !exts.is_empty() {
            print!(": {}", exts.join(", "));
        }
        println!();
    }
    ExitCode::from(0)
}

/// list-languages: pretty bat-style two-column output with shebangs.
fn list_languages_pretty() -> ExitCode {
    let rows = collect_language_rows();
    // No --color flag reaches here (both personas expose -L as a
    // standalone "print and exit"), so tty plus NO_COLOR is the whole
    // decision. eat's fuller precedence chain covers its own output.
    let use_color = io::stdout().is_terminal() && !crate::glyphs::env_disables_color();

    // name column = max name length, capped, plus padding.
    let max_name = rows.iter().map(|(l, _)| l.name.len()).max().unwrap_or(0);
    let name_col = max_name.min(24) + 2;
    let total = term_width().max(name_col + 20);
    let body_width = total.saturating_sub(name_col).max(20);

    let bold_cyan = if use_color { "\x1b[1;36m" } else { "" };
    let yellow = if use_color { "\x1b[33m" } else { "" };
    let reset = if use_color { "\x1b[m" } else { "" };

    let stdout = io::stdout();
    let mut w = stdout.lock();

    for (lang, exts) in &rows {
        // name column -- overflow onto its own line if too long.
        if lang.name.len() + 2 > name_col {
            let _ = writeln!(w, "{bold_cyan}{}{reset}", lang.name);
            print_wrapped(&mut w, "", name_col, body_width, exts, "", reset);
        } else {
            let pad = name_col - lang.name.len();
            let prefix = format!("{bold_cyan}{}{reset}{}", lang.name, " ".repeat(pad));
            print_wrapped(&mut w, &prefix, name_col, body_width, exts, "", reset);
        }

        if !lang.shebangs.is_empty() {
            let shebang_items: Vec<String> =
                lang.shebangs.iter().map(|s| format!("#!{s}")).collect();
            let shebang_refs: Vec<&str> = shebang_items.iter().map(|s| s.as_str()).collect();
            print_wrapped(
                &mut w,
                &" ".repeat(name_col),
                name_col,
                body_width,
                &shebang_refs,
                yellow,
                reset,
            );
        }
    }

    ExitCode::from(0)
}

/// write a comma-separated item list, hard-wrapping by greedy fill.
/// `first_prefix` is printed at the start of the first line. continuation lines are
/// indented to `name_col`. items are joined with ", ".
fn print_wrapped(
    w: &mut dyn Write,
    first_prefix: &str,
    name_col: usize,
    body_width: usize,
    items: &[&str],
    item_color: &str,
    reset: &str,
) {
    if items.is_empty() {
        let _ = writeln!(w, "{first_prefix}");
        return;
    }

    let indent = " ".repeat(name_col);
    let mut col = 0usize;
    let mut first_on_line = true;
    let _ = write!(w, "{first_prefix}");

    for item in items {
        let chunk_len = if first_on_line { item.len() } else { 2 + item.len() };

        if !first_on_line && col + chunk_len > body_width {
            let _ = writeln!(w, ",");
            let _ = write!(w, "{indent}");
            col = 0;
            first_on_line = true;
        }

        if first_on_line {
            let _ = write!(w, "{item_color}{item}{reset}");
            col = item.len();
            first_on_line = false;
        } else {
            let _ = write!(w, ", {item_color}{item}{reset}");
            col += chunk_len;
        }
    }
    let _ = writeln!(w);
}

/// list-languages: json array of objects with id, name, extensions, shebangs.
fn list_languages_json() -> ExitCode {
    let rows = collect_language_rows();
    let stdout = io::stdout();
    let mut w = stdout.lock();
    let _ = writeln!(w, "[");
    for (i, (lang, exts)) in rows.iter().enumerate() {
        let _ = write!(w, "  {{");
        let _ = write!(w, "\"id\": {}, ", json_str(lang.id));
        let _ = write!(w, "\"name\": {}, ", json_str(lang.name));
        let _ = write!(w, "\"extensions\": [");
        for (j, e) in exts.iter().enumerate() {
            if j > 0 {
                let _ = write!(w, ", ");
            }
            let _ = write!(w, "{}", json_str(e));
        }
        let _ = write!(w, "], \"shebangs\": [");
        for (j, s) in lang.shebangs.iter().enumerate() {
            if j > 0 {
                let _ = write!(w, ", ");
            }
            let _ = write!(w, "{}", json_str(s));
        }
        let _ = write!(w, "]}}");
        if i + 1 < rows.len() {
            let _ = writeln!(w, ",");
        } else {
            let _ = writeln!(w);
        }
    }
    let _ = writeln!(w, "]");
    ExitCode::from(0)
}

/// minimal JSON string escape for ASCII-only language identifiers/extensions.
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Print the language list in the requested format and return an exit code.
pub fn list_languages(format: ListFormat) -> ExitCode {
    match format {
        ListFormat::Pretty => list_languages_pretty(),
        ListFormat::Plain => list_languages_plain(),
        ListFormat::Json => list_languages_json(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_parsing() {
        assert_eq!(ListFormat::parse("pretty"), Ok(ListFormat::Pretty));
        assert_eq!(ListFormat::parse("plain"), Ok(ListFormat::Plain));
        assert_eq!(ListFormat::parse("json"), Ok(ListFormat::Json));
        assert!(ListFormat::parse("xml").is_err());
    }

    #[test]
    fn json_str_escapes() {
        assert_eq!(json_str("hello"), "\"hello\"");
        assert_eq!(json_str("a\"b"), "\"a\\\"b\"");
        assert_eq!(json_str("a\\b"), "\"a\\\\b\"");
        assert_eq!(json_str("a\nb"), "\"a\\nb\"");
        assert_eq!(json_str("a\tb"), "\"a\\tb\"");
    }

    #[test]
    fn every_language_is_listed_once() {
        let rows = collect_language_rows();
        assert_eq!(rows.len(), LANGUAGES.len());
        let mut names: Vec<_> = rows.iter().map(|(l, _)| l.name).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before, "duplicate language in the listing");
    }

    #[test]
    fn rows_are_sorted_case_insensitively() {
        let rows = collect_language_rows();
        let keys: Vec<String> = rows.iter().map(|(l, _)| l.name.to_ascii_lowercase()).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
    }
}
