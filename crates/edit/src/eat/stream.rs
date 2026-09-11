//! The non-tty output pipeline: highlight to ansi and write through.
//!
//! Kept deliberately separate from the alt-screen views. This half serves
//! pipes, redirects and pagers, where a framebuffer is meaningless --
//! `eat foo.go | grep` cannot consume one -- so it composes escape
//! sequences directly instead.

use std::fs::File;
use std::io::{self, BufRead, BufReader, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use lsh::runtime::Runtime;
use lsh_defs::{ASSEMBLY, CHARSETS, STRINGS};
use stdext::arena::scratch_arena;

use super::cli::{
    ColorMode, LineRange, PagingMode, WrapMode, print_short_help, prog_name, resolve_use_color,
};
use super::detect::{detect_by_path, head_bytes};
use super::{gutter_view, theme};
use lsh_defs::detect::{find_language, language_from_content, language_from_shebang};

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
/// prefix and no trailing newline -- `write_highlighted_line` composes those
/// around it.
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
/// emitted as-is (no highlighting). when `gutter` is `Some`, prepend the
/// gutter prefix (right-aligned line number + separator) using mark
/// information from it. used by the bulk path (`print_highlighted`) and the
/// streaming follow path.
pub(crate) fn write_highlighted_line(
    writer: &mut dyn Write,
    mut runtime: Option<&mut Runtime>,
    color_map: &[&str],
    line_no: usize,
    line: &str,
    gutter: Option<&gutter_view::Gutter>,
    use_color: bool,
) -> io::Result<()> {
    if let Some(g) = gutter {
        gutter_view::write_prefix(writer, line_no, g.width, g.mark(line_no), use_color)?;
    }
    // Position-sensitive constructs (a line-1 frontmatter fence) read the
    // line number from the vm, and a top-level return clears it.
    if let Some(rt) = runtime.as_deref_mut() {
        rt.set_line_number(line_no as u32);
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
        let g = if show_numbers { gutter } else { None };
        write_highlighted_line(writer, Some(runtime), color_map, i + 1, line, g, use_color)?;
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
pub(crate) fn run(
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
