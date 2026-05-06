//! `eat -f` live tui (alt-screen pager). attached when stdout is a tty;
//! otherwise the streaming `follow::run` path is used. modelled on the same
//! shape as `gg tree --follow`: dim header line on top, body fills the rest, default
//! behaviour is "tail mode" (auto-pin to bottom on append). j/k g/G PgUp/PgDn
//! navigate; G re-enters tail mode; q/esc/ctrl-c exit.
//!
//! the heavy lifting (file polling, line emission, runtime state) lives in
//! `follow::tick` -- this module only owns terminal i/o, key parsing, the
//! `View` (scroll state + buffered lines), and the redraw routine.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use lsh::runtime::{Language, Runtime};
use stdext::arena::{Arena, scratch_arena};

use crate::definitions::{ASSEMBLY, CHARSETS, STRINGS};
use crate::follow::{
    DEFAULT_MISS_BUDGET, FileSource, FollowSource, FollowState, TickOutcome, tick,
};

// --- ansi -----------------------------------------------------------------

const ALT_SCREEN_ENTER: &str = "\x1b[?1049h";
const ALT_SCREEN_LEAVE: &str = "\x1b[?1049l";
const CURSOR_HIDE: &str = "\x1b[?25l";
const CURSOR_SHOW: &str = "\x1b[?25h";
const CLEAR_SCREEN: &str = "\x1b[2J";
const RESET: &str = "\x1b[m";
const DIM: &str = "\x1b[2m";

/// move cursor to 1-indexed (row, col).
fn cursor_to(buf: &mut String, row: u16, col: u16) {
    use std::fmt::Write;
    let _ = write!(buf, "\x1b[{row};{col}H");
}

/// clear the current line from cursor to end.
fn clear_eol(buf: &mut String) {
    buf.push_str("\x1b[K");
}

// --- keys -----------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Quit,
    Up,
    Down,
    PgUp,
    PgDn,
    Home, // jump to top + leave tail mode
    End,  // enter tail mode
    Redraw,
    /// resize sequence: \x1b[8;H;Wt -- gives us the new (cols, rows).
    Resize(u16, u16),
    Other,
}

/// parse a chunk of bytes from stdin into a sequence of `Key`s. consumes
/// every byte of input -- complete sequences become specific `Key`s, partial
/// or unknown CSI sequences become `Key::Other` (caller usually ignores).
///
/// SIGWINCH injection from the `tty` crate appears as `\x1b[8;H;Wt` -- we
/// intercept it here and surface it as `Key::Resize`. callers don't need to
/// query the size separately.
pub fn parse_keys(bytes: &[u8]) -> Vec<Key> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            // ctrl-c, ctrl-d, ctrl-q -> quit
            0x03 | 0x04 | 0x11 => {
                out.push(Key::Quit);
                i += 1;
            }
            // ctrl-l -> redraw
            0x0c => {
                out.push(Key::Redraw);
                i += 1;
            }
            // ctrl-u -> page up; ctrl-f / ctrl-d are awkward (ctrl-d already quit)
            0x15 => {
                out.push(Key::PgUp);
                i += 1;
            }
            // ESC -- start of a CSI sequence, or a bare ESC = quit.
            0x1b => {
                if i + 1 >= bytes.len() {
                    out.push(Key::Quit);
                    i += 1;
                } else if bytes[i + 1] == b'[' {
                    // CSI: \x1b[ ... <final-byte>
                    let start = i + 2;
                    let mut j = start;
                    // params: digits, semicolons
                    while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b';') {
                        j += 1;
                    }
                    if j >= bytes.len() {
                        // truncated; bail out
                        break;
                    }
                    let final_byte = bytes[j];
                    let params = &bytes[start..j];
                    out.push(classify_csi(params, final_byte));
                    i = j + 1;
                } else {
                    // ESC followed by something else -- treat as quit and consume both.
                    out.push(Key::Quit);
                    i += 2;
                }
            }
            // printable ascii
            b'q' => {
                out.push(Key::Quit);
                i += 1;
            }
            b'j' => {
                out.push(Key::Down);
                i += 1;
            }
            b'k' => {
                out.push(Key::Up);
                i += 1;
            }
            b'g' => {
                out.push(Key::Home);
                i += 1;
            }
            b'G' => {
                out.push(Key::End);
                i += 1;
            }
            b' ' => {
                out.push(Key::PgDn);
                i += 1;
            }
            _ => {
                out.push(Key::Other);
                i += 1;
            }
        }
    }
    out
}

fn classify_csi(params: &[u8], final_byte: u8) -> Key {
    match final_byte {
        b'A' => Key::Up,
        b'B' => Key::Down,
        b'H' => Key::Home,
        b'F' => Key::End,
        b'~' => match std::str::from_utf8(params).ok().and_then(|s| s.split(';').next()) {
            Some("5") => Key::PgUp,
            Some("6") => Key::PgDn,
            Some("1" | "7") => Key::Home,
            Some("4" | "8") => Key::End,
            _ => Key::Other,
        },
        b't' => {
            // window-size injection from sigwinch handler: ESC [ 8 ; rows ; cols t
            let s = match std::str::from_utf8(params) {
                Ok(s) => s,
                Err(_) => return Key::Other,
            };
            let mut it = s.split(';');
            if it.next() != Some("8") {
                return Key::Other;
            }
            let rows: u16 = match it.next().and_then(|p| p.parse().ok()) {
                Some(n) => n,
                None => return Key::Other,
            };
            let cols: u16 = match it.next().and_then(|p| p.parse().ok()) {
                Some(n) => n,
                None => return Key::Other,
            };
            Key::Resize(cols, rows)
        }
        _ => Key::Other,
    }
}

// --- view -----------------------------------------------------------------

/// view-side state: the buffered lines (one per element, ansi-coloured
/// bytes), the current scroll offset, and tail-mode flag. terminal-agnostic;
/// every test exercise the `View` directly.
pub struct View {
    pub lines: Vec<Vec<u8>>,
    pub scroll_offset: usize,
    pub tail_mode: bool,
    pub width: u16,
    pub height: u16,
}

impl View {
    pub fn new(width: u16, height: u16) -> Self {
        Self { lines: Vec::new(), scroll_offset: 0, tail_mode: true, width, height }
    }

    /// number of body rows visible (excludes header).
    pub fn body_rows(&self) -> usize {
        self.height.saturating_sub(1) as usize
    }

    /// page size for PgUp/PgDn -- leave one row of overlap.
    fn page(&self) -> usize {
        self.body_rows().saturating_sub(1).max(1)
    }

    pub fn max_offset(&self) -> usize {
        self.lines.len().saturating_sub(self.body_rows())
    }

    /// when tail_mode is on, refresh `scroll_offset` to pin to the bottom.
    /// always called before rendering.
    pub fn settle_offset(&mut self) {
        if self.tail_mode {
            self.scroll_offset = self.max_offset();
        } else {
            let max = self.max_offset();
            if self.scroll_offset > max {
                self.scroll_offset = max;
            }
        }
    }

    /// apply a key. returns false iff the loop should exit.
    pub fn apply_key(&mut self, key: Key) -> bool {
        match key {
            Key::Quit => return false,
            Key::Up => {
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
                self.tail_mode = false;
            }
            Key::Down => {
                self.scroll_offset = self.scroll_offset.saturating_add(1);
                self.tail_mode = false;
                // re-enter tail mode if we just scrolled past the bottom.
                if self.scroll_offset >= self.max_offset() {
                    self.scroll_offset = self.max_offset();
                    self.tail_mode = true;
                }
            }
            Key::PgUp => {
                self.scroll_offset = self.scroll_offset.saturating_sub(self.page());
                self.tail_mode = false;
            }
            Key::PgDn => {
                self.scroll_offset = self.scroll_offset.saturating_add(self.page());
                self.tail_mode = false;
                if self.scroll_offset >= self.max_offset() {
                    self.scroll_offset = self.max_offset();
                    self.tail_mode = true;
                }
            }
            Key::Home => {
                self.scroll_offset = 0;
                self.tail_mode = false;
            }
            Key::End => {
                self.tail_mode = true;
            }
            Key::Resize(w, h) => {
                self.width = w;
                self.height = h;
            }
            Key::Redraw | Key::Other => {}
        }
        true
    }

    /// extend the buffered lines with a slice of newly-arrived lines.
    pub fn extend_lines(&mut self, new: &[Vec<u8>]) {
        self.lines.extend_from_slice(new);
    }

    /// reset for rotation/truncation.
    pub fn reset_lines(&mut self) {
        self.lines.clear();
        self.scroll_offset = 0;
        self.tail_mode = true;
    }
}

// --- redraw ---------------------------------------------------------------

fn render_frame(view: &mut View, path_label: &str, last_update: &str, interval_ms: u128) -> String {
    view.settle_offset();
    let mut buf = String::with_capacity(8 * 1024);
    // home the cursor; per-row clear_eol does the actual erasing. avoids the
    // full-screen flicker that \x1b[2J would cause on every tick.
    cursor_to(&mut buf, 1, 1);

    // header
    buf.push_str(DIM);
    let header = format!(
        "last update: {last_update}  --  {path_label}  ({interval_ms}ms, j/k g/G PgUp/PgDn scroll, q exit)"
    );
    push_truncated(&mut buf, &header, view.width as usize);
    buf.push_str(RESET);
    clear_eol(&mut buf);

    // body
    let body_rows = view.body_rows();
    let start = view.scroll_offset;
    let end = (start + body_rows).min(view.lines.len());
    for (i, line) in view.lines[start..end].iter().enumerate() {
        cursor_to(&mut buf, 2 + i as u16, 1);
        // best-effort utf-8: lines are produced by our highlighter and are utf-8 + ansi.
        match std::str::from_utf8(line) {
            Ok(s) => push_truncated_ansi(&mut buf, s, view.width as usize),
            Err(_) => buf.push_str(&String::from_utf8_lossy(line)),
        }
        clear_eol(&mut buf);
    }
    // clear any leftover rows below the body
    for i in (end - start)..body_rows {
        cursor_to(&mut buf, 2 + i as u16, 1);
        clear_eol(&mut buf);
    }
    buf
}

/// push at most `width` *display* columns of `s` into `buf`. only counts
/// ascii printable chars; this is a safe lower bound for our cli-style header.
fn push_truncated(buf: &mut String, s: &str, width: usize) {
    for (n, ch) in s.chars().enumerate() {
        if n >= width {
            break;
        }
        buf.push(ch);
    }
}

/// push `s` into `buf` while passing through ansi CSI sequences w/out
/// counting them against the column budget. truncates at `width` display
/// columns. simple model: a `\x1b[...<letter>` consumes no width; everything
/// else counts as 1 column per char. good enough for our highlighted output;
/// not a full unicode width pass.
fn push_truncated_ansi(buf: &mut String, s: &str, width: usize) {
    let mut n = 0;
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            // copy the whole CSI sequence verbatim.
            buf.push(ch);
            if let Some(&'[') = chars.peek() {
                buf.push('[');
                chars.next();
                while let Some(&c2) = chars.peek() {
                    chars.next();
                    buf.push(c2);
                    if c2.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        if n >= width {
            break;
        }
        buf.push(ch);
        n += 1;
    }
    // ensure ansi state doesn't bleed past the truncation.
    buf.push_str(RESET);
}

// --- the LineSink wrapper around tick's writer interface -----------------

/// in-memory writer that splits `\n`-terminated chunks into separate entries.
/// each `writeln!` call lands as one element in `lines`. `tick`'s emit_lines
/// already calls `writeln!` exactly once per highlighted line, so this gives
/// us one `Vec<u8>` per logical line w/out parsing.
struct LineBuf {
    pub lines: Vec<Vec<u8>>,
    /// in-progress partial -- `tick` writes a line then writes `\n`; we close
    /// the current entry on `\n` and start a fresh one.
    cur: Vec<u8>,
}

impl LineBuf {
    fn new() -> Self {
        Self { lines: Vec::new(), cur: Vec::new() }
    }
    fn take_new(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.lines)
    }
}

impl io::Write for LineBuf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        for &b in buf {
            if b == b'\n' {
                let line = std::mem::take(&mut self.cur);
                self.lines.push(line);
            } else {
                self.cur.push(b);
            }
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

// --- driver --------------------------------------------------------------

/// run the live tui pager. returns when the user exits or the file is gone
/// past the miss budget. the caller must have ensured stdout is a tty (the
/// loop relies on terminal escapes + raw mode).
pub fn run(
    path: PathBuf,
    lang: Option<&'static Language>,
    show_numbers: bool,
    use_color: bool,
    poll_interval: Duration,
) -> io::Result<()> {
    // initial sanity check: surface "no such file" before we wreck the screen.
    let mut src = FileSource::new(path.clone());
    src.stat()?;

    // bring up the terminal. Deinit drop restores termios; alt-screen is
    // exited explicitly at the end.
    let _deinit = tty::init();
    tty::switch_modes()?;
    // drive a fake sigwinch so the first read_stdin returns size right away.
    tty::inject_window_size_into_stdin();

    tty::write_stdout(&format!("{ALT_SCREEN_ENTER}{CURSOR_HIDE}{CLEAR_SCREEN}"));
    let cleanup_screen = || {
        tty::write_stdout(&format!("{CURSOR_SHOW}{ALT_SCREEN_LEAVE}"));
    };

    let result = run_loop(&mut src, &path, lang, show_numbers, use_color, poll_interval);
    cleanup_screen();
    result
}

fn run_loop(
    src: &mut FileSource,
    path: &Path,
    lang: Option<&'static Language>,
    show_numbers: bool,
    use_color: bool,
    poll_interval: Duration,
) -> io::Result<()> {
    let color_map = crate::theme::color_map();
    let entrypoint = lang.map(|l| l.entrypoint).unwrap_or(0);
    let mut runtime = lang.map(|l| Runtime::new(&ASSEMBLY, &STRINGS, &CHARSETS, l.entrypoint));

    let mut state = FollowState::new(DEFAULT_MISS_BUDGET);
    let mut view = View::new(80, 24); // updated on first sigwinch read
    let mut sink = LineBuf::new();
    let path_label = path.display().to_string();

    let mut last_tick = Instant::now() - poll_interval; // ensures first iteration ticks
    let arena_main = Arena::new(64 * 1024)?;

    loop {
        // 1. tick if it's time
        let now = Instant::now();
        if now.duration_since(last_tick) >= poll_interval {
            let outcome = tick(
                &mut state,
                src,
                runtime.as_mut(),
                entrypoint,
                &color_map,
                show_numbers,
                use_color,
                &mut sink,
            )?;
            match outcome {
                TickOutcome::Reset(_) => {
                    view.reset_lines();
                    view.extend_lines(&sink.take_new());
                }
                TickOutcome::Wrote(_) | TickOutcome::Idle => {
                    view.extend_lines(&sink.take_new());
                }
                TickOutcome::GoneTooLong => {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        "followed file disappeared",
                    ));
                }
            }
            last_tick = now;
            redraw(&mut view, &path_label, poll_interval);
        }

        // 2. wait for input until the next tick deadline
        let wait = poll_interval.saturating_sub(Instant::now().duration_since(last_tick));
        let scratch = scratch_arena(Some(&arena_main));
        let chunk = tty::read_stdin(&scratch, wait);
        match chunk {
            None => {
                // stdin closed -- exit cleanly.
                return Ok(());
            }
            Some(s) if s.is_empty() => {
                // timeout: loop, will tick at top.
            }
            Some(s) => {
                let mut should_redraw = false;
                let mut quit = false;
                for k in parse_keys(s.as_bytes()) {
                    if !view.apply_key(k) {
                        quit = true;
                        break;
                    }
                    should_redraw = true;
                }
                if quit {
                    return Ok(());
                }
                if should_redraw {
                    redraw(&mut view, &path_label, poll_interval);
                }
            }
        }
    }
}

fn redraw(view: &mut View, path_label: &str, poll_interval: Duration) {
    let now_str = format_clock(Instant::now());
    let frame = render_frame(view, path_label, &now_str, poll_interval.as_millis());
    tty::write_stdout(&frame);
}

/// approximate hh:mm:ss using system time. avoids a chrono dep.
fn format_clock(_now: Instant) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = (secs / 3600) % 24;
    format!("{h:02}:{m:02}:{s:02}")
}

// -------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(s: &[u8]) -> Vec<Key> {
        parse_keys(s)
    }

    // --- key parsing ---

    #[test]
    fn quit_keys() {
        assert_eq!(keys(b"q"), vec![Key::Quit]);
        assert_eq!(keys(&[0x03]), vec![Key::Quit]); // ctrl-c
        assert_eq!(keys(&[0x04]), vec![Key::Quit]); // ctrl-d
        assert_eq!(keys(&[0x1b]), vec![Key::Quit]); // bare esc
    }

    #[test]
    fn nav_keys() {
        assert_eq!(keys(b"j"), vec![Key::Down]);
        assert_eq!(keys(b"k"), vec![Key::Up]);
        assert_eq!(keys(b"g"), vec![Key::Home]);
        assert_eq!(keys(b"G"), vec![Key::End]);
        assert_eq!(keys(b" "), vec![Key::PgDn]);
    }

    #[test]
    fn arrow_keys() {
        assert_eq!(keys(b"\x1b[A"), vec![Key::Up]);
        assert_eq!(keys(b"\x1b[B"), vec![Key::Down]);
        assert_eq!(keys(b"\x1b[H"), vec![Key::Home]);
        assert_eq!(keys(b"\x1b[F"), vec![Key::End]);
    }

    #[test]
    fn page_keys() {
        assert_eq!(keys(b"\x1b[5~"), vec![Key::PgUp]);
        assert_eq!(keys(b"\x1b[6~"), vec![Key::PgDn]);
        assert_eq!(keys(&[0x15]), vec![Key::PgUp]); // ctrl-u
    }

    #[test]
    fn resize_sequence() {
        assert_eq!(keys(b"\x1b[8;24;80t"), vec![Key::Resize(80, 24)]);
        assert_eq!(keys(b"\x1b[8;100;200t"), vec![Key::Resize(200, 100)]);
    }

    #[test]
    fn batched_input() {
        // common case: one read pulls multiple keys
        assert_eq!(keys(b"jjk q"), vec![Key::Down, Key::Down, Key::Up, Key::PgDn, Key::Quit,]);
    }

    #[test]
    fn truncated_csi_ignored() {
        // partial sequence at end of buffer -- shouldn't panic.
        let _ = keys(b"\x1b[");
        let _ = keys(b"\x1b[5");
    }

    // --- view ---

    fn make_view(width: u16, height: u16, n_lines: usize) -> View {
        let mut v = View::new(width, height);
        for i in 0..n_lines {
            v.lines.push(format!("line {i}").into_bytes());
        }
        v
    }

    #[test]
    fn tail_mode_pins_to_bottom_after_extend() {
        let mut v = make_view(80, 6, 100); // body_rows = 5
        assert!(v.tail_mode);
        v.settle_offset();
        assert_eq!(v.scroll_offset, 95); // 100 - 5

        v.extend_lines(&[b"x".to_vec(), b"y".to_vec()]);
        v.settle_offset();
        assert_eq!(v.scroll_offset, 97);
    }

    #[test]
    fn k_scrolls_up_and_drops_tail_mode() {
        let mut v = make_view(80, 6, 100);
        v.settle_offset();
        v.apply_key(Key::Up);
        assert!(!v.tail_mode);
        assert_eq!(v.scroll_offset, 94);
    }

    #[test]
    fn j_at_bottom_re_enters_tail_mode() {
        let mut v = make_view(80, 6, 100);
        v.settle_offset();
        v.apply_key(Key::Up); // off tail
        assert!(!v.tail_mode);
        assert_eq!(v.scroll_offset, 94);
        v.apply_key(Key::Down);
        // scroll back to bottom -- re-enters tail mode.
        assert!(v.tail_mode);
        assert_eq!(v.scroll_offset, 95);
    }

    #[test]
    fn home_jumps_to_top() {
        let mut v = make_view(80, 6, 100);
        v.settle_offset();
        v.apply_key(Key::Home);
        assert!(!v.tail_mode);
        assert_eq!(v.scroll_offset, 0);
    }

    #[test]
    fn end_re_enters_tail_mode() {
        let mut v = make_view(80, 6, 100);
        v.apply_key(Key::Home);
        assert!(!v.tail_mode);
        v.apply_key(Key::End);
        assert!(v.tail_mode);
        v.settle_offset();
        assert_eq!(v.scroll_offset, 95);
    }

    #[test]
    fn page_keys_use_page_size() {
        let mut v = make_view(80, 12, 100); // body_rows=11, page=10
        v.apply_key(Key::Home);
        assert_eq!(v.scroll_offset, 0);
        v.apply_key(Key::PgDn);
        assert_eq!(v.scroll_offset, 10);
        v.apply_key(Key::PgUp);
        assert_eq!(v.scroll_offset, 0);
    }

    #[test]
    fn quit_returns_false() {
        let mut v = make_view(80, 24, 10);
        assert!(!v.apply_key(Key::Quit));
    }

    #[test]
    fn reset_clears_lines_and_re_enters_tail() {
        let mut v = make_view(80, 6, 100);
        v.apply_key(Key::Home);
        v.reset_lines();
        assert_eq!(v.lines.len(), 0);
        assert_eq!(v.scroll_offset, 0);
        assert!(v.tail_mode);
    }

    #[test]
    fn resize_updates_dims() {
        let mut v = make_view(80, 24, 10);
        v.apply_key(Key::Resize(120, 40));
        assert_eq!(v.width, 120);
        assert_eq!(v.height, 40);
    }

    #[test]
    fn fewer_lines_than_body_means_zero_offset() {
        let mut v = make_view(80, 24, 5); // body_rows=23, lines=5
        v.settle_offset();
        assert_eq!(v.scroll_offset, 0);
    }

    // --- LineBuf adapter ---

    #[test]
    fn line_buf_splits_on_newline() {
        use io::Write;
        let mut lb = LineBuf::new();
        lb.write_all(b"alpha\nbeta\n").unwrap();
        let got = lb.take_new();
        assert_eq!(got, vec![b"alpha".to_vec(), b"beta".to_vec()]);
    }

    #[test]
    fn line_buf_buffers_partial_writes() {
        use io::Write;
        let mut lb = LineBuf::new();
        lb.write_all(b"hel").unwrap();
        lb.write_all(b"lo\nwo").unwrap();
        let got = lb.take_new();
        assert_eq!(got, vec![b"hello".to_vec()]);
        lb.write_all(b"rld\n").unwrap();
        assert_eq!(lb.take_new(), vec![b"world".to_vec()]);
    }
}
