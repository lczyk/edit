//! eat -- a bat-like syntax-highlighting cat persona for edit.
//!
//! when invoked as `eat` (via symlink or standalone binary), reads files,
//! syntax-highlights via lsh, writes to stdout, optionally pages.

pub mod follow;
pub mod gutter_view;
pub mod theme;
pub mod viewer;
pub mod views;

use std::fs::File;
use std::io::{self, BufRead, BufReader, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use argh::FromArgs;
use lsh::runtime::{Language, Runtime};
use stdext::arena::scratch_arena;

use crate::langlist::ListFormat;
use lsh_defs::detect::{
    disambiguate_language, find_language, language_from_content, language_from_shebang,
    match_file_associations,
};
use lsh_defs::{ASSEMBLY, CHARSETS, FILE_ASSOCIATIONS, STRINGS};

/// argh glue for the shared [`ListFormat`]. The wrapper exists so
/// `langlist` stays free of the cli framework -- argh is eat's
/// dependency, not the library's.
#[derive(Debug, PartialEq, Eq)]
struct ListFormatArg(ListFormat);

impl argh::FromArgValue for ListFormatArg {
    fn from_arg_value(value: &str) -> Result<Self, String> {
        ListFormat::parse(value).map(ListFormatArg)
    }
}

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

    /// when to wrap long lines: auto, always, never (never chops instead)
    #[argh(option, default = "WrapMode::Auto")]
    wrap: WrapMode,

    /// follow file appends and emit new lines as they arrive (like `tail -F`).
    /// optional value sets the poll interval, e.g. `-f 30s`, `-f 500ms`,
    /// `-f 2` (bare number = seconds). bare `-f` defaults to 250ms.
    #[argh(option, short = 'f')]
    follow: Option<FollowDuration>,

    /// print known languages and exit (format: pretty, plain, json; defaults to pretty)
    #[argh(option, short = 'L')]
    list_languages: Option<ListFormatArg>,

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

/// resolve whether to emit ansi colour, following the de-facto-standard
/// precedence (highest wins):
///
///   1. explicit cli flag (`--color always|never`).
///   2. `FORCE_COLOR` env var (any non-empty value other than `0`) -> on.
///   3. `NO_COLOR` env var (any non-empty value, per <https://no-color.org>) -> off.
///   4. fall back to whether the output stream is a tty.
///
/// kept as a free function so the various entry points (bulk render, follow,
/// list-languages) can all share it. testable via the `_with_env` variant
/// that takes the env values as parameters.
fn resolve_use_color(mode: ColorMode, output_is_tty: bool) -> bool {
    let force = std::env::var_os("FORCE_COLOR");
    let no = std::env::var_os("NO_COLOR");
    resolve_use_color_with_env(mode, output_is_tty, force.as_deref(), no.as_deref())
}

// The `NO_COLOR`-only half of this precedence lives in `crate::glyphs`,
// where the editor -- which has no `ColorMode` -- reads it.
fn resolve_use_color_with_env(
    mode: ColorMode,
    output_is_tty: bool,
    force_color: Option<&std::ffi::OsStr>,
    no_color: Option<&std::ffi::OsStr>,
) -> bool {
    // explicit cli flag wins.
    match mode {
        ColorMode::Always => return true,
        ColorMode::Never => return false,
        ColorMode::Auto => {}
    }
    // FORCE_COLOR overrides NO_COLOR + tty status. accept any non-empty value
    // except literal "0" (matches the convention used by chalk, supports-color,
    // and friends in the js ecosystem).
    if let Some(v) = force_color
        && !v.is_empty()
        && v != "0"
    {
        return true;
    }
    // NO_COLOR: any non-empty value disables, per https://no-color.org.
    if let Some(v) = no_color
        && !v.is_empty()
    {
        return false;
    }
    // fall back to terminal detection.
    output_is_tty
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
enum WrapMode {
    Auto,
    Always,
    Never,
}

impl argh::FromArgValue for WrapMode {
    fn from_arg_value(value: &str) -> Result<Self, String> {
        match value {
            "auto" => Ok(WrapMode::Auto),
            "always" => Ok(WrapMode::Always),
            "never" => Ok(WrapMode::Never),
            _ => Err(format!("invalid wrap mode: {value}. expected auto, always, or never")),
        }
    }
}

impl WrapMode {
    /// Wrap state the tui views start in. `auto` wraps: the tui only ever runs
    /// on a tty, and clipping long lines by default hides content.
    fn resolve(self) -> bool {
        !matches!(self, WrapMode::Never)
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
    let name = prog_name();
    let eat_flag = if std::env::var("EDIT_EAT_VIA_FLAG").is_ok() { " --eat" } else { "" };
    eprintln!(
        "usage: {name}{eat_flag} [-l <lang>] [-p] [-n] [-L] [--line-range <RANGE>] [--color <WHEN>] [--paging <WHEN>] [--wrap <WHEN>] [-f [<DUR>]] [--version] [FILES...]"
    );
    eprintln!("try `{name}{eat_flag} --help` for more details.");
    ExitCode::from(0)
}

/// Join the first 64 lines of a `Vec<String>` (BufRead::lines() strips `\n`)
/// back into a contiguous byte buffer suitable for the shared `lsh_defs::detect`
/// fns, which take `head: &[u8]`. Allocates -- only called once per file on the
/// detection path.
fn head_bytes(lines: &[String]) -> Vec<u8> {
    let take = lines.iter().take(64);
    let mut out = Vec::with_capacity(take.clone().map(|l| l.len() + 1).sum());
    for l in take {
        out.extend_from_slice(l.as_bytes());
        out.push(b'\n');
    }
    out
}

/// Resolve a language from a path using the same content-aware dialect
/// disambiguation the editor uses (`documents.rs`): collect every glob
/// candidate, then probe each dialect's `detect_entrypoint` against `head`.
/// `disambiguate_language` only reads `head` when more than one definition
/// shares the glob -- e.g. yaml + slice_yaml both on `**/*.yaml`. Returns
/// `None` if no glob matched, so callers chain shebang/content fallbacks.
fn detect_by_path(path: &Path, head: &[u8]) -> Option<&'static Language> {
    let candidates = match_file_associations(FILE_ASSOCIATIONS, path);
    if candidates.is_empty() {
        return None;
    }
    disambiguate_language(&candidates, head)
}

/// Read up to 4 KiB from the start of `path` for content-based language
/// detection. Best-effort: returns empty on any error.
fn read_head(path: &Path) -> Vec<u8> {
    use std::io::Read as _;
    let mut buf = vec![0u8; 4096];
    match File::open(path).and_then(|mut f| f.read(&mut buf)) {
        Ok(n) => {
            buf.truncate(n);
            buf
        }
        Err(_) => Vec::new(),
    }
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

/// render the highlighted body bytes for a single line into `out`. no gutter
/// prefix and no trailing newline -- the caller composes those. this is the
/// shared core that `write_highlighted_line` (writes through to a stream)
/// and the tui's `LineSink` (stores per-line for later composition) both
/// build on.
pub(crate) fn render_body(
    runtime: Option<&mut Runtime>,
    color_map: &[&str],
    line: &str,
    use_color: bool,
    out: &mut Vec<u8>,
) {
    use std::io::Write as _;
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
                    let _ = write!(out, "{color}");
                    out.extend_from_slice(text);
                    out.extend_from_slice(b"\x1b[m");
                } else {
                    out.extend_from_slice(text);
                }
            }
        }
        None => {
            out.extend_from_slice(line.as_bytes());
        }
    }
}

/// write one line to `writer`, optionally with a leading line number and ansi
/// colour escapes from `color_map`. when `runtime` is `None`, the line is
/// emitted as-is (no highlighting). when `gutter` is `Some((g, n))`, prepend
/// the gutter prefix (right-aligned line number + separator) using mark
/// information from `g`. used by the bulk path (`print_highlighted`) and the
/// streaming follow path; the tui follow path stores raw bodies and composes
/// the prefix at render time so the gutter can update without re-rendering.
pub(crate) fn write_highlighted_line(
    writer: &mut dyn Write,
    runtime: Option<&mut Runtime>,
    color_map: &[&str],
    line: &str,
    gutter: Option<(&gutter_view::Gutter, usize)>,
    use_color: bool,
) -> io::Result<()> {
    if let Some((g, n)) = gutter {
        gutter_view::write_prefix(writer, n, g.width, g.mark(n), use_color)?;
    }
    let mut body = Vec::with_capacity(line.len() + 16);
    render_body(runtime, color_map, line, use_color, &mut body);
    writer.write_all(&body)?;
    writeln!(writer)?;
    Ok(())
}

/// write one file's highlighted lines (plus optional header) to a writer.
/// caller owns sink + pager lifecycle so a multi-file run shares one pager.
#[allow(clippy::too_many_arguments)]
fn print_highlighted(
    writer: &mut dyn Write,
    runtime: &mut Runtime,
    lines: &[String],
    color_map: &[&str],
    show_numbers: bool,
    header: Option<&str>,
    use_color: bool,
    gutter: Option<&gutter_view::Gutter>,
) -> io::Result<()> {
    if let Some(hdr) = header {
        if use_color {
            writeln!(writer, "\x1b[1m--- {hdr} ---\x1b[m")?;
        } else {
            writeln!(writer, "--- {hdr} ---")?;
        }
    }

    for (i, line) in lines.iter().enumerate() {
        let g = if show_numbers { gutter.map(|g| (g, i + 1)) } else { None };
        write_highlighted_line(writer, Some(runtime), color_map, line, g, use_color)?;
    }

    Ok(())
}

/// spawn the pager (if any) and return its stdin + child handle.
/// caller must drop the writer to signal EOF, then wait on the child.
fn open_pager_sink(
    pager_path: &str,
    wrap: bool,
) -> io::Result<(Box<dyn Write>, std::process::Child)> {
    let mut args: Vec<&str> = Vec::new();
    let pager_name = Path::new(pager_path).file_name().and_then(|n| n.to_str()).unwrap_or("");

    // -R: pass ansi colour escapes through raw. -F: exit if content fits one screen.
    // we deliberately do NOT pass -X (--no-init): without alt-screen, terminals
    // can't forward mouse-wheel events to less via xterm alternate-scroll, so the
    // page won't scroll under the cursor. less >=530 fixed the old -F-clears-screen
    // bug that motivated -X; older less is rare enough not to chase.
    if pager_name == "less" {
        args.extend_from_slice(&["-R", "-F"]);
        // less wraps by default, so only the chop case needs a flag.
        if !wrap {
            args.push("-S");
        }
    }

    let mut child = std::process::Command::new(pager_path)
        .args(&args)
        .stdin(std::process::Stdio::piped())
        .spawn()?;
    let stdin = child.stdin.take().expect("piped");
    Ok((Box::new(stdin), child))
}

/// read a file into a Vec of lines.
pub(crate) fn read_file(path: &Path) -> io::Result<Vec<String>> {
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
#[allow(clippy::too_many_arguments)]
fn run(
    files: &[String],
    language_override: Option<&str>,
    plain: bool,
    show_numbers: bool,
    line_range: Option<LineRange>,
    color_mode: ColorMode,
    paging_mode: PagingMode,
    wrap_mode: WrapMode,
) -> ExitCode {
    let mut has_error = false;

    // resolve overridden language
    let lang_override = language_override.and_then(find_language);
    if let Some(name) = language_override
        && lang_override.is_none()
    {
        eprintln!("{}: unknown language '{name}'", prog_name());
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

    // decide paging + colour once for the whole run so a multi-file invocation
    // shares one pager rather than spawning one per file.
    let stdout_is_tty = io::stdout().is_terminal();
    let want_page = match paging_mode {
        PagingMode::Always => true,
        PagingMode::Never => false,
        PagingMode::Auto => stdout_is_tty,
    };
    let pager_path = if want_page { resolve_pager() } else { None };
    let use_color = match color_mode {
        ColorMode::Never => false,
        // through the pager pipe, stdout-is-tty would read false; treat paging
        // as a tty for colour purposes. otherwise consult env + tty.
        _ => pager_path.is_some() || resolve_use_color(color_mode, stdout_is_tty),
    };

    let mut pager_child: Option<std::process::Child> = None;
    let mut sink: Box<dyn Write> = match pager_path.as_deref() {
        Some(path) => match open_pager_sink(path, wrap_mode.resolve()) {
            Ok((w, c)) => {
                pager_child = Some(c);
                w
            }
            Err(_) => Box::new(io::stdout()),
        },
        None => Box::new(io::stdout()),
    };

    // tracks whether the pager (or downstream consumer) has hung up; once it
    // has, subsequent writes are pointless -- stop iterating.
    let mut sink_closed = false;

    for input in &inputs {
        if sink_closed {
            break;
        }
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
                    eprintln!("{}: {}: {e}", prog_name(), path.display());
                    has_error = true;
                    continue;
                }
            },
            EatInput::Stdin => match read_stdin() {
                Ok(lines) => (lines, None, None),
                Err(e) => {
                    eprintln!("{}: stdin: {e}", prog_name());
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
            let head = head_bytes(&lines);
            detect_by_path(p, &head)
                .or_else(|| language_from_shebang(&head))
                .or_else(|| language_from_content(&head))
        } else {
            // stdin with no path: shebang sniff, then content sniff
            let head = head_bytes(&lines);
            language_from_shebang(&head).or_else(|| language_from_content(&head))
        };

        if plain {
            // plain mode: cat to shared sink. no header, no decorations.
            for line in &lines {
                if let Err(e) = writeln!(sink, "{line}") {
                    if e.kind() == io::ErrorKind::BrokenPipe {
                        sink_closed = true;
                        break;
                    }
                    eprintln!("{}: {e}", prog_name());
                    has_error = true;
                    sink_closed = true;
                    break;
                }
            }
            continue;
        }

        let header = if stdout_is_tty && inputs.len() > 1 { header_label.as_deref() } else { None };

        // build the gutter once per file when -n is on and we have a path to
        // resolve a baseline against. for stdin or with -n off, no gutter.
        let gutter = if show_numbers && let Some(p) = path_for_detection {
            // re-join the lines into a contiguous byte buffer for diffing.
            // BufRead::lines() already stripped \n, so we need to put them back.
            let mut bytes = Vec::with_capacity(lines.iter().map(|l| l.len() + 1).sum());
            for l in &lines {
                bytes.extend_from_slice(l.as_bytes());
                bytes.push(b'\n');
            }
            Some(gutter_view::Gutter::compute(p, &bytes, 1))
        } else {
            None
        };

        if let Some(lang) = lang {
            let mut runtime = Runtime::new(&ASSEMBLY, &STRINGS, &CHARSETS, lang.entrypoint);
            match print_highlighted(
                sink.as_mut(),
                &mut runtime,
                &lines,
                &color_map,
                show_numbers,
                header,
                use_color,
                gutter.as_ref(),
            ) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {
                    sink_closed = true;
                }
                Err(e) => {
                    eprintln!("{}: {e}", prog_name());
                    has_error = true;
                }
            }
        } else {
            // no language detected -- plain output, but still through shared sink
            // so we share the pager with siblings.
            if let Some(hdr) = header {
                let _ = if use_color {
                    writeln!(sink, "\x1b[1m--- {hdr} ---\x1b[m")
                } else {
                    writeln!(sink, "--- {hdr} ---")
                };
            }
            for line in &lines {
                if let Err(e) = writeln!(sink, "{line}") {
                    if e.kind() == io::ErrorKind::BrokenPipe {
                        sink_closed = true;
                        break;
                    }
                    eprintln!("{}: {e}", prog_name());
                    has_error = true;
                    sink_closed = true;
                    break;
                }
            }
        }
    }

    // close sink (drops pager stdin so it sees EOF), then wait on pager.
    drop(sink);
    if let Some(mut c) = pager_child {
        let _ = c.wait();
    }

    if has_error { ExitCode::from(1) } else { ExitCode::from(0) }
}

enum EatInput {
    File(PathBuf),
    Stdin,
}

// pre-existing layout: `mod tests` sits in the middle of the file, with
// more pub items following. didn't trigger clippy when this was a standalone
// crate; after the absorption (0f0d8f7) it now lives as an inner module of
// edit and the lint fires. moving the test block to the bottom of the file
// would be a big mechanical churn; allow until that cleanup pass.
#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;

    // shebang + content-sniff tests now live in `lsh_defs::detect::tests` --
    // the impls moved to the shared crate so both `eat` and `edit` consume the
    // same detection logic.

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

    // --- NO_COLOR / FORCE_COLOR resolution ---

    fn rc(mode: ColorMode, tty: bool, force: Option<&str>, no: Option<&str>) -> bool {
        use std::ffi::OsStr;
        resolve_use_color_with_env(mode, tty, force.map(OsStr::new), no.map(OsStr::new))
    }

    #[test]
    fn cli_always_wins_over_everything() {
        assert!(rc(ColorMode::Always, false, None, Some("1")));
        assert!(rc(ColorMode::Always, false, Some("0"), Some("yes")));
    }

    #[test]
    fn cli_never_wins_over_everything() {
        assert!(!rc(ColorMode::Never, true, Some("1"), None));
        assert!(!rc(ColorMode::Never, true, None, None));
    }

    #[test]
    fn force_color_overrides_no_color_and_tty() {
        assert!(rc(ColorMode::Auto, false, Some("1"), Some("1")));
        assert!(rc(ColorMode::Auto, false, Some("3"), None));
        assert!(rc(ColorMode::Auto, false, Some("true"), None));
    }

    #[test]
    fn force_color_zero_or_empty_does_not_force() {
        assert!(!rc(ColorMode::Auto, false, Some("0"), None));
        assert!(!rc(ColorMode::Auto, false, Some(""), None));
    }

    #[test]
    fn no_color_disables_for_any_non_empty_value() {
        assert!(!rc(ColorMode::Auto, true, None, Some("1")));
        assert!(!rc(ColorMode::Auto, true, None, Some("yes")));
        assert!(!rc(ColorMode::Auto, true, None, Some("anything")));
    }

    #[test]
    fn no_color_empty_does_not_disable() {
        // per the spec, only non-empty values count.
        assert!(rc(ColorMode::Auto, true, None, Some("")));
    }

    #[test]
    fn auto_falls_back_to_tty_when_env_silent() {
        assert!(rc(ColorMode::Auto, true, None, None));
        assert!(!rc(ColorMode::Auto, false, None, None));
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
    fn list_format_arg_parsing() {
        let parse = <ListFormatArg as argh::FromArgValue>::from_arg_value;
        assert_eq!(parse("pretty").unwrap().0, ListFormat::Pretty);
        assert_eq!(parse("plain").unwrap().0, ListFormat::Plain);
        assert_eq!(parse("json").unwrap().0, ListFormat::Json);
        assert!(parse("xml").is_err());
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

    // theme tests live in `lsh_defs::theme::tests` -- the colourmap impl
    // moved to the shared crate alongside the canonical
    // `HighlightKind::default_color` table.
}

/// parse argv with one ergonomic adjustment: bare `-L` / `--list-languages` (no value
/// following, or a non-format value following) is rewritten to `-L pretty` so the user
/// can type `eat -L` and get the default pretty listing.
/// Print argh help output, inserting ` --eat` into the Usage line when invoked
/// via `edit --eat` so the user sees `Usage: edit --eat [options]`.
fn print_help_maybe_eat(help: &str, via_eat_flag: bool, argv0: &str) {
    if !via_eat_flag {
        println!("{help}");
        return;
    }
    if let Some((first, rest)) = help.split_once('\n') {
        let prefix = format!("Usage: {argv0}");
        if let Some(args) = first.strip_prefix(&prefix) {
            println!("Usage: {argv0} --eat{args}");
        } else {
            println!("{first}");
        }
        println!("{rest}");
    } else {
        println!("{help}");
    }
}

/// File stem of argv[0], used as the program name prefix in messages.
fn prog_name() -> String {
    std::env::args_os()
        .next()
        .as_deref()
        .and_then(|p| std::path::Path::new(p).file_stem())
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "eat".to_string())
}

fn parse_cli() -> Cli {
    use argh::FromArgs;
    let argv: Vec<String> = std::env::args().collect();
    if argv.is_empty() {
        eprintln!("{}: empty argv", prog_name());
        std::process::exit(1);
    }

    let via_eat_flag = std::env::var("EDIT_EAT_VIA_FLAG").is_ok();

    // --help/-h anywhere in args prints help and exits, position-independent.
    if argv.iter().skip(1).any(|a| a == "-h" || a == "--help") {
        let help_argv: [&str; 2] = [argv[0].as_str(), "--help"];
        match Cli::from_args(&[help_argv[0]], &help_argv[1..]) {
            Ok(_) => unreachable!(),
            Err(early_exit) => match early_exit.status {
                Ok(()) => {
                    print_help_maybe_eat(&early_exit.output, via_eat_flag, argv[0].as_str());
                    std::process::exit(0);
                }
                Err(()) => {
                    eprintln!("{}", early_exit.output);
                    std::process::exit(1);
                }
            },
        }
    }

    let mut rewritten: Vec<String> = Vec::with_capacity(argv.len() + 1);
    rewritten.push(argv[0].clone());
    let mut i = 1;
    while i < argv.len() {
        let a = &argv[i];
        if via_eat_flag && a == "--eat" {
            // strip --eat injected by `edit --eat` before argh sees it
            i += 1;
        } else if a == "-L" || a == "--list-languages" {
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
                print_help_maybe_eat(&early_exit.output, via_eat_flag, strs[0]);
                std::process::exit(0);
            }
            Err(()) => {
                eprintln!(
                    "{}\nRun {}{} for more information.",
                    early_exit.output,
                    strs[0],
                    if via_eat_flag { " --eat --help" } else { " --help" },
                );
                std::process::exit(1);
            }
        },
    }
}

/// validate --follow combos, resolve language, dispatch to `follow::run`.
/// `--paging` is silently ignored in follow mode (forced off); paging-while-
/// following is a deliberate v2 once we have a scrollback ux for it.
fn run_follow_cli(cli: &Cli, has_line_range: bool) -> ExitCode {
    // disallowed combos
    if cli.files.is_empty() || cli.files.iter().any(|f| f == "-") {
        eprintln!("{}: --follow requires a file path (stdin is not supported)", prog_name());
        return ExitCode::from(2);
    }
    if cli.files.len() > 1 {
        eprintln!(
            "{}: --follow takes a single file (multi-file follow is not supported)",
            prog_name()
        );
        return ExitCode::from(2);
    }
    if has_line_range {
        eprintln!("{}: --follow cannot be combined with --line-range", prog_name());
        return ExitCode::from(2);
    }
    if cli.plain {
        // plain + follow could work (just write raw bytes), but we'd need a
        // separate path; v1 keeps the surface tight. error rather than
        // silently doing one of the two.
        eprintln!("{}: --follow cannot be combined with --plain", prog_name());
        return ExitCode::from(2);
    }

    let path = PathBuf::from(&cli.files[0]);

    // resolve language: -l override, then path-glob, then shebang sniff
    // off the file's first line.
    let lang: Option<&'static Language> = match cli.language.as_deref() {
        Some(name) => match find_language(name) {
            Some(l) => Some(l),
            None => {
                eprintln!("{}: unknown language '{name}'", prog_name());
                return ExitCode::from(2);
            }
        },
        None => {
            let head = read_head(&path);
            detect_by_path(&path, &head).or_else(|| language_from_shebang(&head))
        }
    };

    let use_color = resolve_use_color(cli.color, io::stdout().is_terminal());

    // poll interval comes from the parsed `--follow` value (already defaulted
    // by parse_cli's bare-`-f` rewrite, which also honours EAT_FOLLOW_INTERVAL_MS).
    let poll = cli.follow.map(|fd| fd.0).unwrap_or(FollowDuration::DEFAULT);

    // tty -> live alt-screen pager (mount-based, phase C.1/C.2/C.4);
    // non-tty (piped) -> stream lines as before.
    // EAT_FOLLOW_NO_TUI=1 forces streaming even on a tty (debug / scripting).
    let force_no_tui = std::env::var("EAT_FOLLOW_NO_TUI").is_ok_and(|v| !v.is_empty());
    let result = if io::stdout().is_terminal() && !force_no_tui {
        views::run_follow_mount(path, lang, cli.number, use_color, poll, cli.wrap.resolve())
    } else {
        follow::run(path, lang, cli.number, use_color, poll)
    };

    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => {
            eprintln!("{}: {e}", prog_name());
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
        return crate::langlist::list_languages(fmt.0);
    }

    if cli.version {
        println!("{} {}", prog_name(), version::version!());
        return ExitCode::from(0);
    }

    let line_range = if let Some(ref s) = cli.line_range {
        match parse_line_range(s) {
            Ok(r) => Some(r),
            Err(e) => {
                eprintln!("{}: invalid --line-range: {e}", prog_name());
                return ExitCode::from(2);
            }
        }
    } else {
        None
    };

    if cli.follow.is_some() {
        return run_follow_cli(&cli, line_range.is_some());
    }

    // single file, tty, not plain, no line-range -> snapshot TUI pager
    let use_snapshot_tui = io::stdout().is_terminal()
        && !cli.plain
        && line_range.is_none()
        && cli.files.len() == 1
        && cli.files[0] != "-";

    if use_snapshot_tui {
        let path = PathBuf::from(&cli.files[0]);
        let lang: Option<&'static Language> = match cli.language.as_deref() {
            Some(name) => match find_language(name) {
                Some(l) => Some(l),
                None => {
                    eprintln!("{}: unknown language '{name}'", prog_name());
                    return ExitCode::from(2);
                }
            },
            None => {
                let head = read_head(&path);
                detect_by_path(&path, &head).or_else(|| language_from_shebang(&head))
            }
        };
        let use_color = resolve_use_color(cli.color, true);
        return match views::run_snapshot(path, lang, cli.number, use_color, cli.wrap.resolve()) {
            Ok(()) => ExitCode::from(0),
            Err(e) => {
                eprintln!("{}: {e}", prog_name());
                ExitCode::from(1)
            }
        };
    }

    run(
        &cli.files,
        cli.language.as_deref(),
        cli.plain,
        cli.number,
        line_range,
        cli.color,
        cli.paging,
        cli.wrap,
    )
}
