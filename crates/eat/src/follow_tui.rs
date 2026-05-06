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

// SGR mouse mode: ?1000h = button events, ?1006h = SGR coordinate format.
// we use SGR exclusively (no X10 fallback) because legacy x10 emits raw
// bytes that other terminals' alt-scroll converts into arrow keys -- we
// already handle arrow keys, so this is fine. wheel events come through as
// btn=64/65; our parser maps them to Up/Down.
const MOUSE_ENABLE: &str = "\x1b[?1000h\x1b[?1006h";
const MOUSE_DISABLE: &str = "\x1b[?1000l\x1b[?1006l";

/// hard cap on buffered lines. the live-tail buffer would grow without
/// bound on a busy log; once we exceed this, drop the oldest 10% in one
/// shot. 200k lines * a few hundred bytes per line is ~50-100 MiB, which
/// is generous for an interactive viewer.
const LINE_CAP: usize = 200_000;
const LINE_CAP_DROP: usize = LINE_CAP / 10;

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
///
/// the parser is deliberately conservative: any unknown escape sequence (SS3,
/// CSI with private prefix, mouse, focus reports, etc.) is consumed and
/// emitted as `Key::Other`. quit is reserved for explicit user gestures
/// (q, ctrl-c/d/q, bare ESC) so a stray terminal report can't kill the tui.
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
            // ctrl-u -> page up
            0x15 => {
                out.push(Key::PgUp);
                i += 1;
            }
            // ESC -- start of a CSI / SS3 / other escape sequence, or a
            // bare ESC at the end of the buffer = quit.
            0x1b => {
                let consumed = parse_escape(&bytes[i..], &mut out);
                if consumed == 0 {
                    // bare ESC w/ no continuation
                    out.push(Key::Quit);
                    i += 1;
                } else {
                    i += consumed;
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

/// parse one escape sequence starting at `bytes[0] == 0x1b`. returns the
/// number of bytes consumed, or 0 if `bytes` is just `\x1b` with nothing
/// following (caller decides what to do -- usually "Quit").
///
/// recognised:
///   - `\x1b[ ...`   CSI (also covers private `?` / `<` / `>` prefixes)
///   - `\x1b[M???`   X10 mouse: M + 3 raw bytes
///   - `\x1bO X`     SS3 single-letter (arrow keys in keypad mode)
///   - `\x1b]...\x07` or `...\x1b\\`  OSC (consumed, no key emitted)
///
/// unknown forms: consume `\x1b` plus the next byte as `Key::Other`. we
/// deliberately avoid treating "ESC + something" as Quit -- terminals send
/// a *lot* of unsolicited escape sequences (focus events, mouse, resize
/// reports, etc.) and a stray bare-ESC misclassification would kill the tui
/// every time the user clicks a window or moves the mouse.
fn parse_escape(bytes: &[u8], out: &mut Vec<Key>) -> usize {
    if bytes.len() < 2 {
        return 0;
    }
    match bytes[1] {
        b'[' => parse_csi(bytes, out),
        b'O' => {
            if bytes.len() < 3 {
                // truncated SS3; eat what we have, no key.
                bytes.len()
            } else {
                let key = match bytes[2] {
                    b'A' => Key::Up,
                    b'B' => Key::Down,
                    b'H' => Key::Home,
                    b'F' => Key::End,
                    _ => Key::Other,
                };
                out.push(key);
                3
            }
        }
        b']' => {
            // OSC: terminated by BEL (0x07) or ST (\x1b\\). just consume.
            let mut j = 2;
            while j < bytes.len() {
                if bytes[j] == 0x07 {
                    return j + 1;
                }
                if bytes[j] == 0x1b && bytes.get(j + 1) == Some(&b'\\') {
                    return j + 2;
                }
                j += 1;
            }
            // unterminated; consume what we have.
            bytes.len()
        }
        _ => {
            // unknown escape; eat ESC + next byte and emit Other.
            out.push(Key::Other);
            2
        }
    }
}

fn parse_csi(bytes: &[u8], out: &mut Vec<Key>) -> usize {
    debug_assert!(bytes.starts_with(b"\x1b["));
    // X10 mouse: \x1b[M + 3 raw bytes (button, col, row, each +32).
    if bytes.get(2) == Some(&b'M') {
        if bytes.len() < 6 {
            return bytes.len(); // truncated
        }
        // wheel up = button 64 (i.e. 96 = 64+32), wheel down = 65 (97).
        let key = match bytes[3] {
            96 => Key::Up,
            97 => Key::Down,
            _ => Key::Other,
        };
        out.push(key);
        return 6;
    }

    // standard CSI: optional private-marker (`?` `<` `>` `=`), digits + `;`,
    // then a single final byte in the range 0x40..=0x7e.
    let mut j = 2;
    let private = matches!(bytes.get(j), Some(b'?' | b'<' | b'>' | b'='));
    if private {
        j += 1;
    }
    let params_start = j;
    while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b';') {
        j += 1;
    }
    if j >= bytes.len() {
        return bytes.len(); // truncated
    }
    let final_byte = bytes[j];
    if !(0x40..=0x7e).contains(&final_byte) {
        // malformed; eat what we scanned and move on.
        out.push(Key::Other);
        return j + 1;
    }
    let params = &bytes[params_start..j];

    let key = if private {
        match (bytes[2], final_byte) {
            // SGR mouse: \x1b[<btn;col;rowM (press) or m (release).
            (b'<', b'M' | b'm') => parse_sgr_mouse(params, final_byte == b'M'),
            _ => Key::Other,
        }
    } else {
        classify_csi(params, final_byte)
    };
    out.push(key);
    j + 1
}

/// classify a non-private CSI sequence. params is the digits-and-semicolons
/// chunk between `\x1b[` and the final letter.
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

/// SGR mouse: params are `btn;col;row`. wheel up = 64, wheel down = 65;
/// modifier bits (4=shift, 8=meta, 16=ctrl) are ignored. clicks/motion are
/// suppressed -- only wheel events scroll the view.
fn parse_sgr_mouse(params: &[u8], _press: bool) -> Key {
    let s = match std::str::from_utf8(params) {
        Ok(s) => s,
        Err(_) => return Key::Other,
    };
    let mut it = s.split(';');
    let btn: u32 = match it.next().and_then(|p| p.parse().ok()) {
        Some(n) => n,
        None => return Key::Other,
    };
    // strip modifier bits to get the bare button id.
    let bare = btn & !(4 | 8 | 16);
    match bare {
        64 => Key::Up,   // wheel up
        65 => Key::Down, // wheel down
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
    /// applies the `LINE_CAP` cap by dropping the oldest `LINE_CAP_DROP`
    /// lines once the buffer overflows; updates `scroll_offset` so the
    /// view doesn't visually jump when oldest lines are evicted.
    pub fn extend_lines(&mut self, new: &[Vec<u8>]) {
        self.extend_lines_capped(new, LINE_CAP, LINE_CAP_DROP);
    }

    /// like `extend_lines` but with explicit cap parameters. used by tests
    /// to exercise the cap-and-drop path w/out allocating 200k entries.
    pub fn extend_lines_capped(&mut self, new: &[Vec<u8>], cap: usize, drop: usize) {
        self.lines.extend_from_slice(new);
        if self.lines.len() > cap {
            let to_drop = drop.max(self.lines.len() - cap);
            let to_drop = to_drop.min(self.lines.len());
            self.lines.drain(..to_drop);
            self.scroll_offset = self.scroll_offset.saturating_sub(to_drop);
        }
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

    tty::write_stdout(&format!("{ALT_SCREEN_ENTER}{CURSOR_HIDE}{CLEAR_SCREEN}{MOUSE_ENABLE}"));
    let cleanup_screen = || {
        tty::write_stdout(&format!("{MOUSE_DISABLE}{CURSOR_SHOW}{ALT_SCREEN_LEAVE}"));
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

    // gutter: re-read the file on each content-change tick so newly-arrived
    // lines pick up correct marks; cheap (file already in disk cache, capped
    // at MAX_DIFF_BYTES). only when -n is set.
    let mut gutter: Option<crate::gutter_view::Gutter> = if show_numbers {
        std::fs::read(path).ok().map(|bytes| {
            crate::gutter_view::Gutter::compute(path, &bytes, crate::follow::FOLLOW_NUM_WIDTH)
        })
    } else {
        None
    };

    let mut last_tick = Instant::now() - poll_interval; // ensures first iteration ticks
    let mut last_redraw = Instant::now() - Duration::from_secs(1);
    let arena_main = Arena::new(64 * 1024)?;

    // 1Hz heartbeat: even when nothing is changing, refresh once a second so
    // the "last update" timestamp keeps ticking.
    let heartbeat = Duration::from_secs(1);

    loop {
        // 1. tick if it's time
        let now = Instant::now();
        let mut want_redraw = false;
        if now.duration_since(last_tick) >= poll_interval {
            let outcome = tick(
                &mut state,
                src,
                runtime.as_mut(),
                entrypoint,
                &color_map,
                gutter.as_ref(),
                use_color,
                &mut sink,
            )?;
            match outcome {
                TickOutcome::Reset(n) => {
                    view.reset_lines();
                    view.extend_lines(&sink.take_new());
                    if show_numbers {
                        gutter = std::fs::read(path).ok().map(|bytes| {
                            crate::gutter_view::Gutter::compute(
                                path,
                                &bytes,
                                crate::follow::FOLLOW_NUM_WIDTH,
                            )
                        });
                    }
                    if n > 0 {
                        want_redraw = true;
                    }
                }
                TickOutcome::Wrote(n) => {
                    view.extend_lines(&sink.take_new());
                    if n > 0 {
                        if show_numbers {
                            gutter = std::fs::read(path).ok().map(|bytes| {
                                crate::gutter_view::Gutter::compute(
                                    path,
                                    &bytes,
                                    crate::follow::FOLLOW_NUM_WIDTH,
                                )
                            });
                        }
                        want_redraw = true;
                    }
                }
                TickOutcome::Idle => {
                    // drain any leftover sink content (shouldn't be any).
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
        }

        // heartbeat redraw so the timestamp keeps moving even if nothing else does.
        if Instant::now().duration_since(last_redraw) >= heartbeat {
            want_redraw = true;
        }

        if want_redraw {
            redraw(&mut view, &path_label, poll_interval);
            last_redraw = Instant::now();
        }

        // 2. wait for input until the next tick deadline (or heartbeat, whichever first)
        let next_tick_in = poll_interval.saturating_sub(Instant::now().duration_since(last_tick));
        let next_beat_in = heartbeat.saturating_sub(Instant::now().duration_since(last_redraw));
        let wait = next_tick_in.min(next_beat_in).max(Duration::from_millis(1));
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
                    last_redraw = Instant::now();
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

    #[test]
    fn ss3_arrows() {
        // application-keypad mode: terminals send arrow keys as ESC O X
        // instead of CSI X. used to crash the tui because the bare-ESC
        // path treated the whole thing as Quit.
        assert_eq!(keys(b"\x1bOA"), vec![Key::Up]);
        assert_eq!(keys(b"\x1bOB"), vec![Key::Down]);
        assert_eq!(keys(b"\x1bOH"), vec![Key::Home]);
        assert_eq!(keys(b"\x1bOF"), vec![Key::End]);
    }

    #[test]
    fn unknown_esc_does_not_quit() {
        // any ESC + non-bracket sequence used to map to Quit. terminals send
        // a *lot* of these (focus reports, mouse, alt-modified keys), so
        // misclassifying as Quit would close the tui every time the user
        // moused around or alt-tabbed.
        assert_eq!(keys(b"\x1ba"), vec![Key::Other]); // alt-a
        assert_eq!(keys(b"\x1b\x1b"), vec![Key::Other]); // double-escape
    }

    #[test]
    fn x10_mouse_wheel() {
        // \x1b[M then 3 raw bytes (button+32, col+32, row+32).
        // wheel up = button 64 -> byte 96; wheel down = 65 -> 97.
        assert_eq!(keys(b"\x1b[M\x60\x21\x21"), vec![Key::Up]);
        assert_eq!(keys(b"\x1b[M\x61\x21\x21"), vec![Key::Down]);
        // a regular click (button 0 -> byte 32) emits Other, not Quit.
        assert_eq!(keys(b"\x1b[M\x20\x21\x21"), vec![Key::Other]);
    }

    #[test]
    fn x10_mouse_with_low_coord_bytes_does_not_quit() {
        // the bug we hit: column/row coords that happen to be 3 (= ctrl-c)
        // would trigger a quit if the parser fell through to per-byte
        // processing. with proper consumption they're swallowed.
        // (real x10 uses +32 offset so this scenario doesn't actually
        // arise on the wire, but we assert the consumption is exact.)
        assert_eq!(keys(b"\x1b[M\x60\x03\x04").len(), 1);
        assert_eq!(keys(b"\x1b[M\x60\x03\x04")[0], Key::Up);
    }

    #[test]
    fn sgr_mouse_wheel() {
        assert_eq!(keys(b"\x1b[<64;10;5M"), vec![Key::Up]);
        assert_eq!(keys(b"\x1b[<65;10;5M"), vec![Key::Down]);
        // wheel + shift modifier (4): bare button is still 64.
        assert_eq!(keys(b"\x1b[<68;10;5M"), vec![Key::Up]);
        // ordinary press/release: not a wheel, emit Other.
        assert_eq!(keys(b"\x1b[<0;10;5M"), vec![Key::Other]);
        assert_eq!(keys(b"\x1b[<0;10;5m"), vec![Key::Other]);
    }

    #[test]
    fn private_csi_does_not_quit() {
        // focus-in / focus-out (`\x1b[I`, `\x1b[O`) and similar reports.
        // any CSI w/ a known final letter just classifies as Other.
        assert_eq!(keys(b"\x1b[I"), vec![Key::Other]);
        assert_eq!(keys(b"\x1b[O"), vec![Key::Other]);
        assert_eq!(keys(b"\x1b[?1;2c"), vec![Key::Other]); // device attrs response
    }

    #[test]
    fn osc_consumed_silently() {
        // OSC ends with BEL or ST. consumer should swallow the whole thing
        // and emit no key.
        assert_eq!(keys(b"\x1b]0;some title\x07"), vec![]);
        assert_eq!(keys(b"\x1b]0;some title\x1b\\"), vec![]);
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

    #[test]
    fn extend_lines_capped_drops_oldest_when_over_cap() {
        let mut v = View::new(80, 24);
        // pre-fill with 8 lines, cap at 10, drop 3 at a time.
        let pre: Vec<Vec<u8>> = (0..8).map(|i| format!("a{i}").into_bytes()).collect();
        v.extend_lines_capped(&pre, 10, 3);
        assert_eq!(v.lines.len(), 8); // under cap

        // add 5 more -> total 13, over cap (10) by 3, drop 3 -> 10.
        let more: Vec<Vec<u8>> = (0..5).map(|i| format!("b{i}").into_bytes()).collect();
        v.extend_lines_capped(&more, 10, 3);
        assert_eq!(v.lines.len(), 10);
        // oldest 3 (a0, a1, a2) should be gone; first remaining is a3.
        assert_eq!(v.lines[0], b"a3");
        assert_eq!(v.lines.last().unwrap(), b"b4");
    }

    #[test]
    fn extend_lines_capped_keeps_offset_in_view() {
        // user has scrolled away from tail. drops shouldn't make the
        // current visible window jump or invalidate the offset.
        let mut v = View::new(80, 6); // body_rows=5
        v.tail_mode = false;
        let pre: Vec<Vec<u8>> = (0..20).map(|i| format!("a{i}").into_bytes()).collect();
        v.extend_lines_capped(&pre, 100, 10);
        v.scroll_offset = 5; // looking at lines 5..10
        // overflow: add enough to exceed cap=15 and trigger drop=4.
        let more: Vec<Vec<u8>> = (0..0).map(|_| Vec::new()).collect(); // no add
        v.extend_lines_capped(&more, 15, 4);
        // 20 > 15: drop max(4, 20-15)=5 -> 15 remain. offset 5 - 5 = 0.
        assert_eq!(v.lines.len(), 15);
        assert_eq!(v.scroll_offset, 0);
        assert_eq!(v.lines[0], b"a5");
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
