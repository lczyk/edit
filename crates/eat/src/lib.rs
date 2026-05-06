//! eat -- a bat-like syntax-highlighting cat persona for edit.
//!
//! when invoked as `eat` (via symlink or standalone binary), reads files,
//! syntax-highlights via lsh, writes to stdout, optionally pages.

pub mod definitions;
pub mod follow;
pub mod theme;

use std::fs::File;
use std::io::{self, BufRead, BufReader, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use argh::FromArgs;
use lsh::runtime::{Language, Runtime};
use stdext::arena::scratch_arena;
use stdext::glob::glob_match;

use definitions::{ASSEMBLY, CHARSETS, FILE_ASSOCIATIONS, LANGUAGES, STRINGS};

/// eat -- a bat-like syntax-highlighting cat.
#[derive(FromArgs, PartialEq, Debug)]
#[argh(name = "eat")]
struct Cli {
    /// override syntax detection (required for stdin if no shebang)
    #[argh(option, short = 'l')]
    language: Option<String>,

    /// disable highlighting, decorations, and paging -- act like cat
    #[argh(switch, short = 'p')]
    plain: bool,

    /// show line numbers
    #[argh(switch, short = 'n')]
    number: bool,

    /// line range: N | N: | :M | N:M
    #[argh(option)]
    line_range: Option<String>,

    /// when to use colors: auto, always, never
    #[argh(option, default = "ColorMode::Auto")]
    color: ColorMode,

    /// when to use a pager: auto, always, never
    #[argh(option, default = "PagingMode::Auto")]
    paging: PagingMode,

    /// follow file appends and emit new lines as they arrive (like `tail -F`).
    /// optional value sets the poll interval, e.g. `-f 30s`, `-f 500ms`,
    /// `-f 2` (bare number = seconds). bare `-f` defaults to 250ms.
    #[argh(option, short = 'f')]
    follow: Option<FollowDuration>,

    /// print known languages and exit (format: pretty, plain, json; defaults to pretty)
    #[argh(option, short = 'L')]
    list_languages: Option<ListFormat>,

    /// print version and exit
    #[argh(switch)]
    version: bool,

    /// files to read (use - for stdin)
    #[argh(positional)]
    files: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColorMode {
    Auto,
    Always,
    Never,
}

impl argh::FromArgValue for ColorMode {
    fn from_arg_value(value: &str) -> Result<Self, String> {
        match value {
            "auto" => Ok(ColorMode::Auto),
            "always" => Ok(ColorMode::Always),
            "never" => Ok(ColorMode::Never),
            _ => Err(format!("invalid color mode: {value}. expected auto, always, or never")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PagingMode {
    Auto,
    Always,
    Never,
}

impl argh::FromArgValue for PagingMode {
    fn from_arg_value(value: &str) -> Result<Self, String> {
        match value {
            "auto" => Ok(PagingMode::Auto),
            "always" => Ok(PagingMode::Always),
            "never" => Ok(PagingMode::Never),
            _ => Err(format!("invalid paging mode: {value}. expected auto, always, or never")),
        }
    }
}

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

impl argh::FromArgValue for ListFormat {
    fn from_arg_value(value: &str) -> Result<Self, String> {
        Self::parse(value)
    }
}

/// poll interval for `--follow`, parsed off the cli. accepts `30s`, `500ms`,
/// `1m`, `1.5s`, or a bare number (= seconds). minimum 50ms; smaller values
/// are silently clamped. zero / negative / non-finite values are rejected.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FollowDuration(pub std::time::Duration);

impl FollowDuration {
    /// minimum permitted poll interval. 50ms is enough headroom for a tui
    /// redraw + tick; anything below is just busy-looping w/out user value.
    pub const MIN: std::time::Duration = std::time::Duration::from_millis(50);

    /// default poll interval applied when `-f` is given w/out a value.
    pub const DEFAULT: std::time::Duration = std::time::Duration::from_millis(250);

    pub fn parse(value: &str) -> Result<Self, String> {
        let s = value.trim();
        if s.is_empty() {
            return Err("empty duration".into());
        }
        // split numeric prefix from unit suffix.
        let split = s.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(s.len());
        let (num_part, unit) = s.split_at(split);
        let n: f64 = num_part.parse().map_err(|_| format!("invalid duration: {value}"))?;
        if !n.is_finite() || n < 0.0 {
            return Err(format!("invalid duration: {value}"));
        }
        let ms = match unit {
            "" => n * 1000.0, // bare number = seconds
            "s" => n * 1000.0,
            "ms" => n,
            "m" => n * 60_000.0,
            other => {
                return Err(format!("invalid duration unit: '{other}' (expected ms, s, or m)"));
            }
        };
        if ms <= 0.0 {
            return Err("duration must be positive".into());
        }
        let mut d = std::time::Duration::from_millis(ms.round() as u64);
        if d < Self::MIN {
            d = Self::MIN;
        }
        Ok(FollowDuration(d))
    }
}

impl argh::FromArgValue for FollowDuration {
    fn from_arg_value(value: &str) -> Result<Self, String> {
        Self::parse(value)
    }
}

struct LineRange {
    start: Option<usize>,
    end: Option<usize>,
}

fn parse_line_range(s: &str) -> Result<LineRange, String> {
    if s.is_empty() {
        return Err("empty line range".into());
    }

    let (start_str, end_str) = match s.split_once(':') {
        Some((l, r)) => (l, r),
        None => (s, ""),
    };

    let start = if start_str.is_empty() {
        None
    } else {
        Some(start_str.parse::<usize>().map_err(|e| format!("invalid line number: {e}"))?)
    };
    let end = if end_str.is_empty() {
        None
    } else {
        Some(end_str.parse::<usize>().map_err(|e| format!("invalid line number: {e}"))?)
    };

    Ok(LineRange { start, end })
}

/// print short help and exit.
fn print_short_help() -> ExitCode {
    eprintln!(
        "usage: eat [-l <lang>] [-p] [-n] [-L] [--line-range <RANGE>] [--color <WHEN>] [--paging <WHEN>] [--version] [FILES...]"
    );
    eprintln!("try `eat --help` for more details.");
    ExitCode::from(0)
}

/// find a language by name (case-insensitive prefix match on id and display_name).
fn find_language(name: &str) -> Option<&'static Language> {
    let name_lower = name.to_ascii_lowercase();
    LANGUAGES.iter().find(|lang| {
        lang.id.to_ascii_lowercase() == name_lower || lang.name.to_ascii_lowercase() == name_lower
    })
}

/// detect language from a file path: path glob matching.
fn detect_language_by_path(path: &Path) -> Option<&'static Language> {
    let bytes = path.as_os_str().as_encoded_bytes();
    for (pattern, lang) in FILE_ASSOCIATIONS {
        if glob_match(pattern.as_bytes(), bytes) {
            return Some(lang);
        }
    }
    None
}

/// extract the interpreter token from a shebang line.
/// handles `#!/usr/bin/python` and `#!/usr/bin/env python`.
fn extract_shebang_token(line: &str) -> Option<&str> {
    let line = line.strip_prefix("#!")?;
    let line = line.trim();
    if line.is_empty() {
        return None;
    }

    // split by whitespace -- first token is the interpreter path
    let first_word = line.split_ascii_whitespace().next()?;

    // if the first word's basename is "env", look at the second word
    if Path::new(first_word).file_name().is_some_and(|n| n == "env") {
        let second_word = line.split_ascii_whitespace().nth(1)?;
        return Some(second_word);
    }

    // otherwise, take the basename
    Path::new(first_word).file_name().and_then(|n| n.to_str())
}

/// detect language from a shebang line by prefix-matching against entrypoint shebangs.
fn detect_language_by_shebang(first_line: &str) -> Option<&'static Language> {
    let token = extract_shebang_token(first_line)?;
    let token_lower = token.to_ascii_lowercase();
    for lang in LANGUAGES {
        for shebang in lang.shebangs {
            if token_lower.starts_with(shebang) {
                return Some(lang);
            }
        }
    }
    None
}

/// resolve the pager binary path.
fn resolve_pager() -> Option<String> {
    // EAT_PAGER > PAGER > less
    if let Ok(p) = std::env::var("EAT_PAGER")
        && !p.is_empty()
    {
        return Some(p);
    }
    if let Ok(p) = std::env::var("PAGER")
        && !p.is_empty()
    {
        return Some(p);
    }

    // manual $PATH walk for less
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            let candidate = Path::new(dir).join("less");
            if candidate.is_file() {
                return Some(candidate.to_string_lossy().into_owned());
            }
        }
    }

    None
}

/// write one line to `writer`, optionally with a leading line number and ansi
/// colour escapes from `color_map`. when `runtime` is `None`, the line is
/// emitted as-is (no highlighting). `num_width` is only consulted when
/// `line_no` is `Some` -- it left-pads the number column. used by both the
/// bulk path (`print_highlighted`) and the follow path.
pub(crate) fn write_highlighted_line(
    writer: &mut dyn Write,
    runtime: Option<&mut Runtime>,
    color_map: &[&str],
    line: &str,
    line_no: Option<usize>,
    num_width: usize,
    use_color: bool,
) -> io::Result<()> {
    if let Some(n) = line_no {
        if use_color {
            write!(writer, "\x1b[90m{:<num_width$} \x1b[m", n)?;
        } else {
            write!(writer, "{:<num_width$} ", n)?;
        }
    }

    match runtime {
        Some(rt) => {
            let scratch = scratch_arena(None);
            let highlights = rt.parse_next_line::<u32>(&scratch, line.as_bytes());
            // NOTE: lsh emits byte indices that may not land on utf-8 char
            // boundaries, so slice via as_bytes() and write_all -- string
            // slicing would panic on multi-byte codepoints (e.g. man pages
            // with em-dashes / smart quotes).
            let line_bytes = line.as_bytes();
            for w in highlights.windows(2) {
                let curr = &w[0];
                let next = &w[1];
                let start = curr.start;
                let end = next.start.min(line_bytes.len());
                let kind = curr.kind;
                let text = &line_bytes[start..end];

                if use_color
                    && let Some(color) = color_map.get(kind as usize)
                    && !color.is_empty()
                {
                    write!(writer, "{color}")?;
                    writer.write_all(text)?;
                    write!(writer, "\x1b[m")?;
                } else {
                    writer.write_all(text)?;
                }
            }
        }
        None => {
            writer.write_all(line.as_bytes())?;
        }
    }
    writeln!(writer)?;
    Ok(())
}

/// print highlighted lines from a reader to stdout or a pager.
#[allow(clippy::too_many_arguments)]
fn print_highlighted(
    runtime: &mut Runtime,
    lines: &[String],
    color_map: &[&str],
    show_numbers: bool,
    header: Option<&str>,
    color_mode: ColorMode,
    paging_mode: PagingMode,
) -> io::Result<()> {
    let stdout = io::stdout();
    let is_tty = stdout.is_terminal();

    let should_page = match paging_mode {
        PagingMode::Always => true,
        PagingMode::Never => false,
        PagingMode::Auto => is_tty,
    };

    // if paging, force color to always (stdout-is-tty returns false through the pipe)
    let use_color = if should_page {
        true
    } else {
        match color_mode {
            ColorMode::Always => true,
            ColorMode::Never => false,
            ColorMode::Auto => is_tty,
        }
    };

    let pager = if should_page { resolve_pager() } else { None };

    let mut write_output = move |writer: &mut dyn Write| -> io::Result<()> {
        if let Some(hdr) = header {
            if use_color {
                writeln!(writer, "\x1b[1m--- {hdr} ---\x1b[m")?;
            } else {
                writeln!(writer, "--- {hdr} ---")?;
            }
        }

        let num_width =
            if show_numbers { lines.len().checked_ilog10().unwrap_or(0) as usize + 1 } else { 0 };

        for (i, line) in lines.iter().enumerate() {
            let line_no = if show_numbers { Some(i + 1) } else { None };
            write_highlighted_line(
                writer,
                Some(runtime),
                color_map,
                line,
                line_no,
                num_width,
                use_color,
            )?;
        }

        Ok(())
    };

    match pager {
        Some(pager_path) => {
            let mut args: Vec<&str> = Vec::new();
            let pager_name =
                Path::new(&pager_path).file_name().and_then(|n| n.to_str()).unwrap_or("");

            // -R: pass ansi colour escapes through raw. -F: exit if content fits one screen.
            // we deliberately do NOT pass -X (--no-init): without alt-screen, terminals
            // can't forward mouse-wheel events to less via xterm alternate-scroll, so the
            // page won't scroll under the cursor. less >=530 fixed the old -F-clears-screen
            // bug that motivated -X; older less is rare enough not to chase.
            if pager_name == "less" {
                args.extend_from_slice(&["-R", "-F"]);
            }

            let mut child = std::process::Command::new(&pager_path)
                .args(&args)
                .stdin(std::process::Stdio::piped())
                .spawn()?;

            if let Some(stdin) = child.stdin.as_mut() {
                let result = write_output(stdin);
                // close stdin so the pager sees EOF
                drop(child.stdin.take());
                match result {
                    Ok(_) => {}
                    Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {
                        // user quit pager early -- ok
                    }
                    Err(e) => return Err(e),
                }
            }

            let _ = child.wait();
        }
        None => {
            let result = write_output(&mut io::stdout());
            match result {
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {
                    // output was piped and consumer quit -- ok
                }
                Err(e) => return Err(e),
            }
        }
    }

    Ok(())
}

/// read a file into a Vec of lines.
fn read_file(path: &Path) -> io::Result<Vec<String>> {
    let file = File::open(path)?;
    let reader = BufReader::with_capacity(128 * 1024, file);
    reader.lines().collect()
}

/// read stdin into a Vec of lines.
fn read_stdin() -> io::Result<Vec<String>> {
    let stdin = io::stdin();
    let reader = BufReader::with_capacity(128 * 1024, stdin.lock());
    reader.lines().collect()
}

/// run the cat-like path over a list of files and optional stdin.
fn run(
    files: &[String],
    language_override: Option<&str>,
    plain: bool,
    show_numbers: bool,
    line_range: Option<LineRange>,
    color_mode: ColorMode,
    paging_mode: PagingMode,
) -> ExitCode {
    let mut has_error = false;

    // resolve overridden language
    let lang_override = language_override.and_then(find_language);
    if let Some(name) = language_override
        && lang_override.is_none()
    {
        eprintln!("eat: unknown language '{name}'");
        return ExitCode::from(2);
    }

    let color_map = theme::color_map();

    let stdin_is_tty = io::stdin().is_terminal();

    // tty + no args -> short help
    if stdin_is_tty && files.is_empty() {
        return print_short_help();
    }

    // collect inputs: expand "-" to stdin at its position
    let mut inputs: Vec<EatInput> = Vec::new();
    if files.is_empty() {
        // no args, stdin piped
        inputs.push(EatInput::Stdin);
    } else {
        for f in files {
            if f == "-" {
                inputs.push(EatInput::Stdin);
            } else {
                inputs.push(EatInput::File(PathBuf::from(f)));
            }
        }
    }

    // filter to actual files (not stdin) for multi-file header logic
    let file_count = inputs.iter().filter(|i| matches!(i, EatInput::File(_))).count();

    for input in &inputs {
        let (lines, path_for_detection, header_label) = match input {
            EatInput::File(path) => match read_file(path) {
                Ok(lines) => {
                    let label = if file_count > 1
                        || inputs.len() > 1
                        || inputs.iter().any(|i| matches!(i, EatInput::Stdin))
                    {
                        Some(path.display().to_string())
                    } else {
                        None
                    };
                    (lines, Some(path.as_path()), label)
                }
                Err(e) => {
                    eprintln!("eat: {}: {e}", path.display());
                    has_error = true;
                    continue;
                }
            },
            EatInput::Stdin => match read_stdin() {
                Ok(lines) => (lines, None, None),
                Err(e) => {
                    eprintln!("eat: stdin: {e}");
                    has_error = true;
                    continue;
                }
            },
        };

        // apply line range
        let lines = if let Some(ref range) = line_range {
            let start = range.start.unwrap_or(1).saturating_sub(1);
            let end = range.end.unwrap_or(lines.len()).min(lines.len());
            lines[start..end].to_vec()
        } else {
            lines
        };

        // language detection
        let lang = if let Some(l) = lang_override {
            Some(l)
        } else if let Some(p) = path_for_detection {
            detect_language_by_path(p).or_else(|| {
                // shebang sniff: read first line from already-loaded lines
                lines.first().and_then(|l| detect_language_by_shebang(l))
            })
        } else {
            // stdin with no path: try shebang sniff, otherwise plain
            lines.first().and_then(|l| detect_language_by_shebang(l))
        };

        if plain {
            // plain mode: just cat the lines
            let stdout = io::stdout();
            let is_tty = stdout.is_terminal();

            let should_page = match paging_mode {
                PagingMode::Always => true,
                PagingMode::Never => false,
                PagingMode::Auto => is_tty && resolve_pager().is_some(),
            };

            if should_page {
                if let Some(pager_path) = resolve_pager() {
                    let spawn_result = std::process::Command::new(&pager_path)
                        .args(if Path::new(&pager_path).file_name().is_some_and(|n| n == "less") {
                            vec!["-R", "-F"]
                        } else {
                            vec![]
                        })
                        .stdin(std::process::Stdio::piped())
                        .spawn();
                    match spawn_result {
                        Ok(mut c) => {
                            if let Some(stdin) = c.stdin.as_mut() {
                                for line in &lines {
                                    let _ = writeln!(stdin, "{line}");
                                }
                            }
                            let _ = c.wait();
                        }
                        Err(_) => {
                            for line in &lines {
                                println!("{line}");
                            }
                        }
                    }
                } else {
                    for line in &lines {
                        println!("{line}");
                    }
                }
            } else {
                for line in &lines {
                    println!("{line}");
                }
            }
            continue;
        }

        let header = if io::stdout().is_terminal() && inputs.len() > 1 {
            header_label.as_deref()
        } else {
            None
        };

        if let Some(lang) = lang {
            let mut runtime = Runtime::new(&ASSEMBLY, &STRINGS, &CHARSETS, lang.entrypoint);
            if let Err(e) = print_highlighted(
                &mut runtime,
                &lines,
                &color_map,
                show_numbers,
                header,
                color_mode,
                paging_mode,
            ) {
                eprintln!("eat: {e}");
                has_error = true;
            }
        } else {
            // no language detected -- plain output
            for line in &lines {
                println!("{line}");
            }
        }
    }

    if has_error { ExitCode::from(1) } else { ExitCode::from(0) }
}

enum EatInput {
    File(PathBuf),
    Stdin,
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- shebang token extraction ---

    #[test]
    fn shebang_simple_interpreter() {
        assert_eq!(extract_shebang_token("#!/usr/bin/python"), Some("python"));
        assert_eq!(extract_shebang_token("#!/bin/bash"), Some("bash"));
        assert_eq!(extract_shebang_token("#!/usr/bin/ruby"), Some("ruby"));
    }

    #[test]
    fn shebang_env_style() {
        assert_eq!(extract_shebang_token("#!/usr/bin/env python"), Some("python"));
        assert_eq!(extract_shebang_token("#!/usr/bin/env bash"), Some("bash"));
        assert_eq!(extract_shebang_token("#!/usr/bin/env python3"), Some("python3"));
    }

    #[test]
    fn shebang_with_args() {
        assert_eq!(extract_shebang_token("#!/usr/bin/python -i"), Some("python"));
        assert_eq!(extract_shebang_token("#!/usr/bin/env python -S"), Some("python"));
    }

    #[test]
    fn shebang_whitespace_only() {
        assert_eq!(extract_shebang_token("#!   /usr/bin/python"), Some("python"));
        assert_eq!(extract_shebang_token("#!  /usr/bin/env python"), Some("python"));
    }

    #[test]
    fn shebang_no_interpreter() {
        assert_eq!(extract_shebang_token("not a shebang"), None);
        assert_eq!(extract_shebang_token("#!"), None);
    }

    #[test]
    fn shebang_empty_env() {
        assert_eq!(extract_shebang_token("#!/usr/bin/env"), None);
    }

    // --- line range parsing ---

    #[test]
    fn line_range_single() {
        let r = parse_line_range("5").unwrap();
        assert_eq!(r.start, Some(5));
        assert_eq!(r.end, None);
    }

    #[test]
    fn line_range_start_only() {
        let r = parse_line_range("5:").unwrap();
        assert_eq!(r.start, Some(5));
        assert_eq!(r.end, None);
    }

    #[test]
    fn line_range_end_only() {
        let r = parse_line_range(":10").unwrap();
        assert_eq!(r.start, None);
        assert_eq!(r.end, Some(10));
    }

    #[test]
    fn line_range_both() {
        let r = parse_line_range("5:10").unwrap();
        assert_eq!(r.start, Some(5));
        assert_eq!(r.end, Some(10));
    }

    #[test]
    fn line_range_empty() {
        assert!(parse_line_range("").is_err());
    }

    // --- color mode parsing ---

    #[test]
    fn color_mode_parsing() {
        assert_eq!(
            <ColorMode as argh::FromArgValue>::from_arg_value("auto").unwrap(),
            ColorMode::Auto
        );
        assert_eq!(
            <ColorMode as argh::FromArgValue>::from_arg_value("always").unwrap(),
            ColorMode::Always
        );
        assert_eq!(
            <ColorMode as argh::FromArgValue>::from_arg_value("never").unwrap(),
            ColorMode::Never
        );
    }

    #[test]
    fn color_mode_invalid() {
        assert!(<ColorMode as argh::FromArgValue>::from_arg_value("nope").is_err());
    }

    // --- paging mode parsing ---

    #[test]
    fn paging_mode_parsing() {
        assert_eq!(
            <PagingMode as argh::FromArgValue>::from_arg_value("auto").unwrap(),
            PagingMode::Auto
        );
        assert_eq!(
            <PagingMode as argh::FromArgValue>::from_arg_value("always").unwrap(),
            PagingMode::Always
        );
        assert_eq!(
            <PagingMode as argh::FromArgValue>::from_arg_value("never").unwrap(),
            PagingMode::Never
        );
    }

    // --- list format ---

    #[test]
    fn list_format_parsing() {
        assert_eq!(
            <ListFormat as argh::FromArgValue>::from_arg_value("pretty").unwrap(),
            ListFormat::Pretty
        );
        assert_eq!(
            <ListFormat as argh::FromArgValue>::from_arg_value("plain").unwrap(),
            ListFormat::Plain
        );
        assert_eq!(
            <ListFormat as argh::FromArgValue>::from_arg_value("json").unwrap(),
            ListFormat::Json
        );
        assert!(<ListFormat as argh::FromArgValue>::from_arg_value("xml").is_err());
    }

    #[test]
    fn json_str_escapes() {
        assert_eq!(json_str("hello"), "\"hello\"");
        assert_eq!(json_str("a\"b"), "\"a\\\"b\"");
        assert_eq!(json_str("a\\b"), "\"a\\\\b\"");
        assert_eq!(json_str("a\nb"), "\"a\\nb\"");
        assert_eq!(json_str("a\tb"), "\"a\\tb\"");
    }

    // --- follow duration ---

    #[test]
    fn follow_duration_parsing() {
        use std::time::Duration;
        let p = |s: &str| FollowDuration::parse(s).map(|d| d.0);
        assert_eq!(p("250ms"), Ok(Duration::from_millis(250)));
        assert_eq!(p("2"), Ok(Duration::from_secs(2)));
        assert_eq!(p("2s"), Ok(Duration::from_secs(2)));
        assert_eq!(p("1.5s"), Ok(Duration::from_millis(1500)));
        assert_eq!(p("1m"), Ok(Duration::from_secs(60)));
        assert_eq!(p("30s"), Ok(Duration::from_secs(30)));
        // clamping at MIN
        assert_eq!(p("10ms"), Ok(FollowDuration::MIN));
        assert_eq!(p("0.001s"), Ok(FollowDuration::MIN));
    }

    #[test]
    fn follow_duration_rejections() {
        assert!(FollowDuration::parse("").is_err());
        assert!(FollowDuration::parse("abc").is_err());
        assert!(FollowDuration::parse("10x").is_err());
        assert!(FollowDuration::parse("-5s").is_err());
        assert!(FollowDuration::parse("0").is_err());
        assert!(FollowDuration::parse("0ms").is_err());
    }

    // --- theme ---

    #[test]
    fn theme_color_map_non_empty() {
        let map = theme::color_map();
        assert!(!map.is_empty());
        // check a few known entries
        assert_eq!(map[definitions::HighlightKind::Comment as usize], "\x1b[32m");
        assert_eq!(map[definitions::HighlightKind::String as usize], "\x1b[91m");
        assert_eq!(map[definitions::HighlightKind::Other as usize], "");
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
    let use_color = io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();

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

/// parse argv with one ergonomic adjustment: bare `-L` / `--list-languages` (no value
/// following, or a non-format value following) is rewritten to `-L pretty` so the user
/// can type `eat -L` and get the default pretty listing.
fn parse_cli() -> Cli {
    use argh::FromArgs;
    let argv: Vec<String> = std::env::args().collect();
    if argv.is_empty() {
        eprintln!("eat: empty argv");
        std::process::exit(1);
    }
    let mut rewritten: Vec<String> = Vec::with_capacity(argv.len() + 1);
    rewritten.push(argv[0].clone());
    let mut i = 1;
    while i < argv.len() {
        let a = &argv[i];
        if a == "-L" || a == "--list-languages" {
            rewritten.push(a.clone());
            let next_is_format =
                argv.get(i + 1).is_some_and(|n| matches!(n.as_str(), "pretty" | "plain" | "json"));
            if !next_is_format {
                rewritten.push("pretty".to_string());
            }
            i += 1;
        } else if a == "-f" || a == "--follow" {
            // argh treats `-f` as an option taking a value -- supply the default
            // when the next token isn't parseable as a duration. lets users type
            // bare `-f` (most common) and still combine with explicit `-f 30s`.
            rewritten.push(a.clone());
            let next = argv.get(i + 1);
            let has_dur = next.is_some_and(|n| FollowDuration::parse(n).is_ok());
            if has_dur {
                rewritten.push(next.unwrap().clone());
                i += 2;
            } else {
                let env_default = std::env::var("EAT_FOLLOW_INTERVAL_MS")
                    .ok()
                    .and_then(|s| s.parse::<u64>().ok())
                    .map(|ms| format!("{ms}ms"))
                    .unwrap_or_else(|| "250ms".to_string());
                rewritten.push(env_default);
                i += 1;
            }
        } else {
            rewritten.push(a.clone());
            i += 1;
        }
    }
    let strs: Vec<&str> = rewritten.iter().map(|s| s.as_str()).collect();
    match Cli::from_args(&[strs[0]], &strs[1..]) {
        Ok(c) => c,
        Err(early_exit) => match early_exit.status {
            Ok(()) => {
                println!("{}", early_exit.output);
                std::process::exit(0);
            }
            Err(()) => {
                eprintln!("{}\nRun {} --help for more information.", early_exit.output, strs[0]);
                std::process::exit(1);
            }
        },
    }
}

/// Print the language list in the requested format and return an exit code.
///
/// Exposed publicly so the editor's own cli can offer the same `-L` surface
/// without re-implementing the formatters.
pub fn list_languages(format: ListFormat) -> ExitCode {
    match format {
        ListFormat::Pretty => list_languages_pretty(),
        ListFormat::Plain => list_languages_plain(),
        ListFormat::Json => list_languages_json(),
    }
}

/// validate --follow combos, resolve language, dispatch to `follow::run`.
/// `--paging` is silently ignored in follow mode (forced off); paging-while-
/// following is a deliberate v2 once we have a scrollback ux for it.
fn run_follow_cli(cli: &Cli, has_line_range: bool) -> ExitCode {
    // disallowed combos
    if cli.files.is_empty() || cli.files.iter().any(|f| f == "-") {
        eprintln!("eat: --follow requires a file path (stdin is not supported)");
        return ExitCode::from(2);
    }
    if cli.files.len() > 1 {
        eprintln!("eat: --follow takes a single file (multi-file follow is not supported)");
        return ExitCode::from(2);
    }
    if has_line_range {
        eprintln!("eat: --follow cannot be combined with --line-range");
        return ExitCode::from(2);
    }
    if cli.plain {
        // plain + follow could work (just write raw bytes), but we'd need a
        // separate path; v1 keeps the surface tight. error rather than
        // silently doing one of the two.
        eprintln!("eat: --follow cannot be combined with --plain");
        return ExitCode::from(2);
    }

    let path = PathBuf::from(&cli.files[0]);

    // resolve language: -l override, then path-glob, then shebang sniff
    // off the file's first line.
    let lang: Option<&'static Language> = match cli.language.as_deref() {
        Some(name) => match find_language(name) {
            Some(l) => Some(l),
            None => {
                eprintln!("eat: unknown language '{name}'");
                return ExitCode::from(2);
            }
        },
        None => detect_language_by_path(&path).or_else(|| {
            let f = File::open(&path).ok()?;
            let mut br = BufReader::new(f);
            let mut first = String::new();
            let _ = std::io::BufRead::read_line(&mut br, &mut first);
            detect_language_by_shebang(first.trim_end_matches(['\n', '\r']))
        }),
    };

    let use_color = match cli.color {
        ColorMode::Always => true,
        ColorMode::Never => false,
        ColorMode::Auto => io::stdout().is_terminal(),
    };

    // poll interval comes from the parsed `--follow` value (already defaulted
    // by parse_cli's bare-`-f` rewrite, which also honours EAT_FOLLOW_INTERVAL_MS).
    let poll = cli.follow.map(|fd| fd.0).unwrap_or(FollowDuration::DEFAULT);

    match follow::run(path, lang, cli.number, use_color, poll) {
        Ok(()) => ExitCode::from(0),
        Err(e) => {
            eprintln!("eat: {e}");
            ExitCode::from(1)
        }
    }
}

/// main entry point for eat. called from edit's argv0 dispatch and from the
/// standalone `bin/eat` binary.
pub fn main() -> ExitCode {
    stdext::arena::init(128 * 1024 * 1024).unwrap();

    let cli: Cli = parse_cli();

    if let Some(fmt) = cli.list_languages {
        return list_languages(fmt);
    }

    if cli.version {
        println!("eat {}", version::version!());
        return ExitCode::from(0);
    }

    let line_range = if let Some(ref s) = cli.line_range {
        match parse_line_range(s) {
            Ok(r) => Some(r),
            Err(e) => {
                eprintln!("eat: invalid --line-range: {e}");
                return ExitCode::from(2);
            }
        }
    } else {
        None
    };

    if cli.follow.is_some() {
        return run_follow_cli(&cli, line_range.is_some());
    }

    run(
        &cli.files,
        cli.language.as_deref(),
        cli.plain,
        cli.number,
        line_range,
        cli.color,
        cli.paging,
    )
}
