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

use lsh_defs::{ASSEMBLY, CHARSETS, STRINGS};
use super::follow::{
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

// disable auto-wrap mode (`\x1b[?7l`): a body line wider than the terminal would
// otherwise wrap onto the next row and corrupt the body rows below until
// the next full redraw. with auto-wrap off the terminal hard-truncates at
// the right edge -- our own width budget already does the same logically,
// but this is cheap belt-and-braces.
const WRAP_OFF: &str = "\x1b[?7l";
const WRAP_ON: &str = "\x1b[?7h";

// NOTE: we deliberately do NOT enable mouse tracking. it would let us catch
// scroll-wheel events as Up/Down, but it also means the terminal stops
// passing mouse events to its own selection layer -- so the user can't
// select text with the mouse, can't middle-click-paste, etc. modern
// terminals translate scroll wheel into arrow keys when in alt-screen via
// the alternate-scroll feature, and our parser handles arrow keys, so we
// get scroll for free w/out breaking selection.

/// hard cap on buffered lines. the live-tail buffer would grow without
/// bound on a busy log; once we exceed this, drop the oldest 10% in one
/// shot. 200k lines * a few hundred bytes per line is ~50-100 MiB, which
/// is generous for an interactive viewer.
const LINE_CAP: usize = 200_000;
const LINE_CAP_DROP: usize = LINE_CAP / 10;

/// scroll-animation time constant (seconds). matches edit's `SCROLL_TAU_SECS`
/// in `crates/edit/src/tui.rs`. snappy enough to feel responsive, slow enough
/// to read as "smooth" rather than "snap".
const SCROLL_TAU_SECS: f32 = 0.060;

/// snap target if the visual is within this many cells -- avoids the
/// long exponential tail where movement is < 0.5 pixels per frame.
const SCROLL_SNAP_EPSILON: f32 = 0.5;

/// frame interval while a scroll animation is in flight (~60fps). in steady
/// state the loop only wakes for ticks (poll_interval) or the 1Hz heartbeat;
/// during animation we bump the wake rate so frames are actually visible.
const ANIM_FRAME_MS: u64 = 16;

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
    /// Ctrl/Cmd-modified Up. Scrolls by [`LARGE_JUMP_LINES`] -- matches
    /// edit's `Ctrl+Alt+Up` small-jump cadence.
    LargeUp,
    /// Ctrl/Cmd-modified Down. Mirror of [`Key::LargeUp`].
    LargeDown,
    PgUp,
    PgDn,
    Home, // jump to top + leave tail mode
    End,  // enter tail mode
    Redraw,
    Reload,
    /// resize sequence: \x1b[8;H;Wt -- gives us the new (cols, rows).
    Resize(u16, u16),
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyOutcome {
    Quit,
    Changed,
    Noop,
}

/// Lines scrolled per Ctrl/Cmd+Up/Down. Mirrors edit's `SMALL_JUMP_LINES`.
pub const LARGE_JUMP_LINES: usize = 3;

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
            b'r' => {
                out.push(Key::Reload);
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
        b'A' => arrow_key(params, Key::Up, Key::LargeUp),
        b'B' => arrow_key(params, Key::Down, Key::LargeDown),
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

/// CSI arrow with optional modifier. Returns `large` iff modifier carries
/// Ctrl (bit 4) or Meta/Cmd (bit 8). xterm encodes modifiers as
/// `(bitmask + 1)`. Accepts the common forms:
///   - `\x1b[<n>;<mod>A` -- xterm standard (params = "1;5"); modifier is
///     the second param.
///   - `\x1b[<mod>A` -- compact form some terminals emit when only the
///     modifier is present (params = "5"); a single param >= 2 is treated
///     as a modifier (param "1" alone is the no-op default and falls back
///     to plain).
/// Plain arrows (no modifier, shift-only) return `plain`.
fn arrow_key(params: &[u8], plain: Key, large: Key) -> Key {
    let s = match std::str::from_utf8(params) {
        Ok(s) => s,
        Err(_) => return plain,
    };
    let parts: Vec<&str> = s.split(';').collect();
    let m: u32 = match parts.as_slice() {
        [_, b] => b.parse().unwrap_or(0),
        [a] if !a.is_empty() => {
            let n: u32 = a.parse().unwrap_or(0);
            if n >= 2 { n } else { 0 }
        }
        _ => 0,
    };
    if m == 0 {
        return plain;
    }
    let bits = m.saturating_sub(1);
    if bits & (4 | 8) != 0 { large } else { plain }
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

/// view-side state: the buffered line bodies (highlighted bytes only -- no
/// gutter prefix, no trailing newline), the current scroll offset, and the
/// tail-mode flag. terminal-agnostic; tested directly.
///
/// the gutter prefix is composed at render time using the *current* `Gutter`
/// snapshot rather than baked into each stored body. this is what lets
/// gutter recomputes (after appends, rewrites, etc.) actually update the
/// marks for already-on-screen lines.
pub struct View {
    pub lines: Vec<Vec<u8>>,
    /// logical (target) scroll offset -- where the viewport WILL be once
    /// the animation settles.
    pub scroll_offset: usize,
    /// animated current scroll position. lerps toward `scroll_offset` per
    /// frame; rendering uses `scroll_offset_visual.round()`. matches edit's
    /// `scroll_offset_visual` shape in `tui.rs`.
    pub scroll_offset_visual: f32,
    pub tail_mode: bool,
    pub width: u16,
    pub height: u16,
    /// 1-indexed line number of `lines[0]`. usually `1`, but bumps forward
    /// when the line cap evicts old entries so per-line `line_no` arithmetic
    /// stays consistent with the file's actual line numbers.
    pub starting_line_no: usize,
}

impl View {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            lines: Vec::new(),
            scroll_offset: 0,
            scroll_offset_visual: 0.0,
            tail_mode: true,
            width,
            height,
            starting_line_no: 1,
        }
    }

    /// 1-indexed line number for the `i`th body in `self.lines`.
    pub fn line_no_of(&self, i: usize) -> usize {
        self.starting_line_no + i
    }

    /// integer offset to feed the renderer this frame. uses the animated
    /// visual position rounded to the nearest cell. clamped to a valid range
    /// so a stale visual position (after lines were dropped, etc.) can't
    /// produce an out-of-bounds slice.
    pub fn render_offset(&self) -> usize {
        let max = self.max_offset();
        let raw = self.scroll_offset_visual.round();
        if raw <= 0.0 {
            0
        } else if (raw as usize) > max {
            max
        } else {
            raw as usize
        }
    }

    /// true when the visual position has reached the target -- the loop
    /// uses this to decide whether to schedule another animation frame.
    pub fn animation_settled(&self) -> bool {
        (self.scroll_offset as f32 - self.scroll_offset_visual).abs() < SCROLL_SNAP_EPSILON
    }

    /// step the visual position toward the target over `dt_secs`. uses the
    /// same `1 - exp(-dt/tau)` lerp as edit's scroll animation. unlike edit
    /// we deliberately omit the "snap on big jumps" guard -- the user
    /// explicitly wants g / G (full-document jumps) to also animate.
    pub fn advance_animation(&mut self, dt_secs: f32) {
        let target = self.scroll_offset as f32;
        let alpha = lerp_alpha(dt_secs, SCROLL_TAU_SECS);
        self.scroll_offset_visual += (target - self.scroll_offset_visual) * alpha;
        if (target - self.scroll_offset_visual).abs() < SCROLL_SNAP_EPSILON {
            self.scroll_offset_visual = target;
        }
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
    /// always called before rendering. does NOT touch `scroll_offset_visual`
    /// -- the loop calls `tail_pin_snap` separately when new content arrives
    /// while in tail mode (to snap the visual w/out an animation), and key
    /// gestures like G leave the visual alone so the lerp can play.
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

    /// snap visual to target (no animation). called by the loop after new
    /// content arrives while tail_mode is on -- watching a busy log we don't
    /// want to play a 360ms animation per appended chunk; that'd be motion
    /// sickness. key-driven scroll changes (j/k/g/G/PgUp/PgDn) do animate.
    pub fn tail_pin_snap(&mut self) {
        self.settle_offset();
        self.scroll_offset_visual = self.scroll_offset as f32;
    }

    /// apply a key. returns whether the loop should quit, redraw, or noop.
    pub fn apply_key(&mut self, key: Key) -> KeyOutcome {
        if matches!(key, Key::Quit) {
            return KeyOutcome::Quit;
        }
        let before = (self.scroll_offset, self.tail_mode, self.width, self.height);
        match key {
            Key::Quit => unreachable!(),
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
            Key::LargeUp => {
                self.scroll_offset = self.scroll_offset.saturating_sub(LARGE_JUMP_LINES);
                self.tail_mode = false;
            }
            Key::LargeDown => {
                self.scroll_offset = self.scroll_offset.saturating_add(LARGE_JUMP_LINES);
                self.tail_mode = false;
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
            // ctrl-l forces a repaint even when nothing changed.
            Key::Redraw => return KeyOutcome::Changed,
            // Reload is handled by the snapshot loop before it reaches
            // apply_key; the arm only exists so the match stays exhaustive.
            Key::Reload => {}
            Key::Other => {}
        }
        let after = (self.scroll_offset, self.tail_mode, self.width, self.height);
        if after != before { KeyOutcome::Changed } else { KeyOutcome::Noop }
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
            // shift our notion of the first stored line forward so render
            // composes the right line numbers for the survivors.
            self.starting_line_no += to_drop;
            // and drag the animated visual position with us so a mid-flight
            // animation doesn't snap or jitter when oldest lines are evicted.
            self.scroll_offset_visual = (self.scroll_offset_visual - to_drop as f32).max(0.0);
        }
    }

    /// reset for rotation/truncation.
    pub fn reset_lines(&mut self) {
        self.lines.clear();
        self.scroll_offset = 0;
        self.scroll_offset_visual = 0.0;
        self.tail_mode = true;
        self.starting_line_no = 1;
    }
}

/// per-frame exponential-lerp alpha for a given time constant. matches edit's
/// `lerp_alpha` -- saturates to 1.0 once `dt` exceeds ~6 tau (effectively done)
/// so we don't pay the cost of `exp()` for the no-op tail.
#[inline]
fn lerp_alpha(dt_secs: f32, tau_secs: f32) -> f32 {
    if dt_secs >= tau_secs * 6.0 { 1.0 } else { 1.0 - (-dt_secs / tau_secs).exp() }
}

// --- redraw ---------------------------------------------------------------

/// render a full frame to a string buffer. composes the gutter prefix
/// (when `gutter` is `Some`) at draw time using the current snapshot --
/// this is what makes mark updates visible on already-stored body lines
/// without re-rendering them.
///
/// `pub` so the integration tests in `crates/eat/tests/` can drive the
/// loop body directly w/out spinning up a real terminal.
pub fn render_frame(
    view: &mut View,
    path_label: &str,
    last_update: &str,
    interval_ms: u128,
    gutter: Option<&super::gutter_view::Gutter>,
    use_color: bool,
) -> String {
    view.settle_offset();
    let mut buf = String::with_capacity(8 * 1024);
    // home the cursor; per-row clear_eol does the actual erasing. avoids the
    // full-screen flicker that \x1b[2J would cause on every tick.
    cursor_to(&mut buf, 1, 1);
    // use the animated visual offset for the slice -- when no animation is
    // in flight it equals view.scroll_offset (settle_offset just set it).
    let render_off = view.render_offset();

    // header
    buf.push_str(DIM);
    let header = format!(
        "{path_label} @ {last_update}  ({interval_ms}ms, j/k g/G PgUp/PgDn scroll, q exit)"
    );
    push_truncated(&mut buf, &header, view.width as usize);
    buf.push_str(RESET);
    clear_eol(&mut buf);

    // body
    let body_rows = view.body_rows();
    let start = render_off;
    let end = (start + body_rows).min(view.lines.len());
    // gutter prefix takes `width + 3` columns (number col + space + sep + space)
    // when present; the body has whatever's left. previously we passed the full
    // terminal width to push_truncated_ansi which let `prefix + body` overflow
    // and wrap onto the next row, painting over the line below.
    let prefix_width = gutter.map(|g| g.width + 3).unwrap_or(0);
    let body_width = (view.width as usize).saturating_sub(prefix_width);
    for (i, line) in view.lines[start..end].iter().enumerate() {
        cursor_to(&mut buf, 2 + i as u16, 1);
        // erase the whole row first -- body bytes may contain `\t`, which the
        // terminal handles by advancing the cursor w/out painting skipped cells.
        // clearing post-content would leave those skipped cells holding stale
        // pixels from the previous frame (visible as bg bleed after scroll).
        clear_eol(&mut buf);

        // gutter prefix, if any. composed against the *current* gutter and
        // the file-level line number, NOT a value baked in at write time.
        if let Some(g) = gutter {
            let line_no = view.line_no_of(start + i);
            let mut pbuf = Vec::with_capacity(32);
            let _ = super::gutter_view::write_prefix(
                &mut pbuf,
                line_no,
                g.width,
                g.mark(line_no),
                use_color,
            );
            buf.push_str(&String::from_utf8_lossy(&pbuf));
        }

        // body bytes (highlighted; no prefix; no trailing newline).
        match std::str::from_utf8(line) {
            Ok(s) => push_truncated_ansi(&mut buf, s, body_width),
            Err(_) => buf.push_str(&String::from_utf8_lossy(line)),
        }
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
///
/// `pub` so the integration tests can build their own loop fixtures.
pub struct LineBuf {
    pub lines: Vec<Vec<u8>>,
    /// in-progress partial -- `tick` writes a line then writes `\n`; we close
    /// the current entry on `\n` and start a fresh one.
    cur: Vec<u8>,
}

impl LineBuf {
    pub fn new() -> Self {
        Self { lines: Vec::new(), cur: Vec::new() }
    }
    pub fn take_new(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.lines)
    }
}

impl Default for LineBuf {
    fn default() -> Self {
        Self::new()
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

// --- snapshot driver -----------------------------------------------------

/// run the snapshot tui pager. reads the file into a [`TextBuffer`] (marked
/// read-only) and mounts edit's tui via [`crate::mount::mount`]. textarea
/// handles cursor, scroll, selection natively. `q` exits.
///
/// v1 scope dropped (still TODO(lczyk)):
/// - disk-change delta indicator in the header. needs a periodic wakeup
///   in [`crate::mount::mount`] (current loop blocks on stdin only). part
///   of phase B.3.
/// - `--color=never` override. edit's tui has no plain-mode toggle yet.
/// follow-tui (`eat -f`) retains its own independent path for now;
/// migrating it is phase-C work.
pub fn run_snapshot(
    path: PathBuf,
    lang: Option<&'static Language>,
    show_numbers: bool,
    _use_color: bool,
) -> io::Result<()> {
    use std::ops::ControlFlow;

    use crate::buffer::TextBuffer;
    use crate::helpers::{CoordType, Size};
    use crate::input::vk;
    use crate::mount::{self, MountOpts};

    let buf = TextBuffer::new_rc(false)
        .map_err(|e| io::Error::other(format!("text buffer: {e:?}")))?;
    {
        let mut b = buf.borrow_mut();
        let mut f = std::fs::File::open(&path)?;
        b.read_file(&mut f).map_err(|e| io::Error::other(format!("read: {e:?}")))?;
        b.set_language(lang);
        b.set_margin_enabled(show_numbers);
        b.set_read_only(true);
    }

    let path_label = path.display().to_string();
    let mut header = snapshot_header(&path_label, Instant::now());

    let _deinit = tty::init();
    tty::switch_modes()?;

    mount::mount(MountOpts::default(), |ctx| -> ControlFlow<()> {
        // global shortcuts: checked before the textarea sees the event so
        // it doesn't swallow Q / R as literal letters.
        if let Some(k) = ctx.keyboard_input() {
            match k {
                vk::Q => {
                    ctx.set_input_consumed();
                    return ControlFlow::Break(());
                }
                vk::R => {
                    ctx.set_input_consumed();
                    if let Ok(mut f) = std::fs::File::open(&path) {
                        let mut b = buf.borrow_mut();
                        b.set_read_only(false);
                        let _ = b.read_file(&mut f);
                        b.set_margin_enabled(show_numbers);
                        b.set_read_only(true);
                    }
                    header = snapshot_header(&path_label, Instant::now());
                    ctx.needs_rerender();
                }
                _ => {}
            }
        }

        let size = ctx.size();
        ctx.label("snapshot-header", &header);

        ctx.textarea("snapshot-body", buf.clone());
        ctx.inherit_focus();
        let body_h = (size.height - 1).max(1) as CoordType;
        ctx.attr_intrinsic_size(Size { width: 0, height: body_h });

        ControlFlow::Continue(())
    })
}

fn snapshot_header(path_label: &str, at: Instant) -> String {
    format!(
        "{path_label} @ {}  (q exit, r reload, arrows / PgUp / PgDn scroll)",
        format_clock(at),
    )
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

    tty::write_stdout(&format!("{ALT_SCREEN_ENTER}{CURSOR_HIDE}{CLEAR_SCREEN}{WRAP_OFF}"));
    let cleanup_screen = || {
        tty::write_stdout(&format!("{WRAP_ON}{CURSOR_SHOW}{ALT_SCREEN_LEAVE}"));
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
    let color_map = super::theme::color_map();
    let entrypoint = lang.map(|l| l.entrypoint).unwrap_or(0);
    let mut runtime = lang.map(|l| Runtime::new(&ASSEMBLY, &STRINGS, &CHARSETS, l.entrypoint));

    let mut state = FollowState::new(DEFAULT_MISS_BUDGET);
    let mut view = View::new(80, 24); // updated on first sigwinch read
    let mut sink = LineBuf::new();
    let path_label = path.display().to_string();

    // gutter: re-read the file on each content-change tick so newly-arrived
    // lines pick up correct marks; cheap (file already in disk cache, capped
    // at MAX_DIFF_BYTES). only when -n is set.
    let mut gutter: Option<super::gutter_view::Gutter> = if show_numbers {
        std::fs::read(path).ok().map(|bytes| {
            super::gutter_view::Gutter::compute(path, &bytes, super::follow::FOLLOW_NUM_WIDTH)
        })
    } else {
        None
    };

    let mut last_tick = Instant::now() - poll_interval; // ensures first iteration ticks
    let mut last_anim_step = Instant::now();
    let arena_main = Arena::new(64 * 1024)?;
    let mut first_paint_done = false;

    let anim_frame = Duration::from_millis(ANIM_FRAME_MS);

    loop {
        // 1. tick if it's time
        let now = Instant::now();
        let mut body_changed = false;
        let mut header_changed = false;
        if now.duration_since(last_tick) >= poll_interval {
            let outcome = tick(
                &mut state,
                src,
                runtime.as_mut(),
                entrypoint,
                &color_map,
                None,
                use_color,
                &mut sink,
            )?;
            match outcome {
                TickOutcome::Reset(n) => {
                    view.reset_lines();
                    view.extend_lines(&sink.take_new());
                    if show_numbers {
                        gutter = std::fs::read(path).ok().map(|bytes| {
                            super::gutter_view::Gutter::compute(
                                path,
                                &bytes,
                                super::follow::FOLLOW_NUM_WIDTH,
                            )
                        });
                    }
                    if n > 0 && view.tail_mode {
                        view.tail_pin_snap();
                    }
                    body_changed = true;
                }
                TickOutcome::Wrote(n) => {
                    view.extend_lines(&sink.take_new());
                    if n > 0 {
                        if show_numbers {
                            gutter = std::fs::read(path).ok().map(|bytes| {
                                super::gutter_view::Gutter::compute(
                                    path,
                                    &bytes,
                                    super::follow::FOLLOW_NUM_WIDTH,
                                )
                            });
                        }
                        if view.tail_mode {
                            view.tail_pin_snap();
                        }
                        body_changed = true;
                    }
                }
                TickOutcome::Idle => {
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
            header_changed = true;
        }

        // mid-flight scroll animation: step it forward and request a redraw.
        let now2 = Instant::now();
        if !view.animation_settled() {
            let dt = now2.duration_since(last_anim_step).as_secs_f32();
            view.advance_animation(dt);
            body_changed = true;
        }
        last_anim_step = now2;

        if body_changed {
            if !first_paint_done {
                view.scroll_offset_visual = view.scroll_offset as f32;
                first_paint_done = true;
            }
            redraw(&mut view, &path_label, poll_interval, gutter.as_ref(), use_color);
        } else if header_changed {
            redraw_header_only(&view, &path_label, poll_interval);
        }

        let next_tick_in = poll_interval.saturating_sub(Instant::now().duration_since(last_tick));
        let mut wait = next_tick_in.max(Duration::from_millis(1));
        if !view.animation_settled() {
            wait = wait.min(anim_frame);
        }
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
                let was_settled = view.animation_settled();
                let mut should_redraw = false;
                let mut quit = false;
                for k in parse_keys(s.as_bytes()) {
                    match view.apply_key(k) {
                        KeyOutcome::Quit => {
                            quit = true;
                            break;
                        }
                        KeyOutcome::Changed => should_redraw = true,
                        KeyOutcome::Noop => {}
                    }
                }
                if quit {
                    return Ok(());
                }
                if was_settled && !view.animation_settled() {
                    last_anim_step = Instant::now();
                }
                if should_redraw {
                    redraw(&mut view, &path_label, poll_interval, gutter.as_ref(), use_color);
                    // see snapshot loop for rationale (must run after redraw).
                    if !view.animation_settled() && was_settled {
                        last_anim_step = Instant::now();
                    }
                    first_paint_done = true;
                }
            }
        }
    }
}

fn redraw(
    view: &mut View,
    path_label: &str,
    poll_interval: Duration,
    gutter: Option<&super::gutter_view::Gutter>,
    use_color: bool,
) {
    let now_str = format_clock(Instant::now());
    let frame =
        render_frame(view, path_label, &now_str, poll_interval.as_millis(), gutter, use_color);
    tty::write_stdout(&frame);
}

fn redraw_header_only(view: &View, path_label: &str, poll_interval: Duration) {
    let now_str = format_clock(Instant::now());
    let header = format!(
        "{path_label} @ {now_str}  ({}ms, j/k g/G PgUp/PgDn scroll, q exit)",
        poll_interval.as_millis()
    );
    let mut buf = String::with_capacity(header.len() + 64);
    cursor_to(&mut buf, 1, 1);
    buf.push_str(DIM);
    push_truncated(&mut buf, &header, view.width as usize);
    buf.push_str(RESET);
    clear_eol(&mut buf);
    tty::write_stdout(&buf);
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
    fn modified_arrow_keys() {
        // xterm modifier encoding: param2 = bitmask + 1.
        // Ctrl (bit 4) -> mod 5; Cmd/Meta (bit 8) -> mod 9.
        assert_eq!(keys(b"\x1b[1;5A"), vec![Key::LargeUp]); // Ctrl+Up
        assert_eq!(keys(b"\x1b[1;5B"), vec![Key::LargeDown]); // Ctrl+Down
        assert_eq!(keys(b"\x1b[1;9A"), vec![Key::LargeUp]); // Cmd/Meta+Up
        assert_eq!(keys(b"\x1b[1;9B"), vec![Key::LargeDown]); // Cmd/Meta+Down
        // Ctrl+Shift (4|1 +1 = 6) still counts as Ctrl-modified.
        assert_eq!(keys(b"\x1b[1;6A"), vec![Key::LargeUp]);
        // Shift-only (1 +1 = 2) stays as plain arrow.
        assert_eq!(keys(b"\x1b[1;2A"), vec![Key::Up]);
        // Compact form (no leading "1;") -- some terminals emit Ctrl+Up
        // as `\x1b[5A` instead of `\x1b[1;5A`.
        assert_eq!(keys(b"\x1b[5A"), vec![Key::LargeUp]);
        assert_eq!(keys(b"\x1b[5B"), vec![Key::LargeDown]);
        assert_eq!(keys(b"\x1b[9A"), vec![Key::LargeUp]);
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
    fn quit_returns_quit_outcome() {
        let mut v = make_view(80, 24, 10);
        assert_eq!(v.apply_key(Key::Quit), KeyOutcome::Quit);
    }

    #[test]
    fn down_at_bottom_is_noop() {
        let mut v = make_view(80, 24, 100);
        v.apply_key(Key::End);
        v.settle_offset();
        assert_eq!(v.apply_key(Key::Down), KeyOutcome::Noop);
    }

    #[test]
    fn up_at_top_is_noop() {
        let mut v = make_view(80, 24, 100);
        v.apply_key(Key::Home);
        assert_eq!(v.apply_key(Key::Up), KeyOutcome::Noop);
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

    // --- gutter is composed at draw time, not baked into bodies ---

    #[test]
    fn line_no_of_tracks_starting_offset() {
        let mut v = View::new(80, 24);
        v.lines = vec![b"x".to_vec(); 5];
        assert_eq!(v.line_no_of(0), 1);
        assert_eq!(v.line_no_of(4), 5);
        v.starting_line_no = 7;
        assert_eq!(v.line_no_of(0), 7);
        assert_eq!(v.line_no_of(4), 11);
    }

    #[test]
    fn extend_lines_capped_advances_starting_line_no_on_drop() {
        let mut v = View::new(80, 24);
        let pre: Vec<Vec<u8>> = (0..20).map(|i| format!("a{i}").into_bytes()).collect();
        v.extend_lines_capped(&pre, 100, 10);
        assert_eq!(v.starting_line_no, 1);
        let more: Vec<Vec<u8>> = (0..0).map(|_| Vec::new()).collect();
        v.extend_lines_capped(&more, 15, 4);
        // dropped 5 -> starting_line_no shifts to 6.
        assert_eq!(v.starting_line_no, 6);
        assert_eq!(v.line_no_of(0), 6);
        assert_eq!(v.line_no_of(v.lines.len() - 1), 20);
    }

    #[test]
    fn reset_lines_resets_starting_line_no() {
        let mut v = View::new(80, 24);
        v.starting_line_no = 50;
        v.lines = vec![b"x".to_vec(); 3];
        v.reset_lines();
        assert_eq!(v.starting_line_no, 1);
    }

    #[test]
    fn render_frame_composes_gutter_at_draw_time() {
        // the bug fix: same body bytes, two different Gutter snapshots ->
        // the rendered frames should differ in the gutter region. proves we
        // compose at draw time rather than baking the prefix in.
        use crate::eat::gutter_view::Gutter;
        use gutter::GutterMark;

        let mut v = View::new(80, 4); // body_rows = 3
        v.lines = vec![b"alpha".to_vec(), b"beta".to_vec(), b"gamma".to_vec()];

        let g_clean =
            Gutter { width: 2, marks: vec![GutterMark::None, GutterMark::None, GutterMark::None] };
        let g_dirty = Gutter {
            width: 2,
            marks: vec![GutterMark::None, GutterMark::Added, GutterMark::Modified],
        };

        let frame_clean = render_frame(&mut v, "p", "00:00:00", 100, Some(&g_clean), false);
        let frame_dirty = render_frame(&mut v, "p", "00:00:00", 100, Some(&g_dirty), false);

        // bodies are unchanged across the two frames -- only the gutter cues
        // (no-color: `+` for Added, `~` for Modified, `|` for None) shift.
        assert!(frame_dirty.contains("+"));
        assert!(frame_dirty.contains("~"));
        assert!(!frame_clean.contains("+"));
        assert!(!frame_clean.contains("~"));
        assert!(frame_clean.contains("alpha"));
        assert!(frame_dirty.contains("alpha"));
    }

    #[test]
    fn render_frame_uses_correct_line_numbers_after_drop() {
        // when oldest lines are evicted, surviving bodies should still get
        // their original (file-level) line numbers in the gutter.
        use crate::eat::gutter_view::Gutter;
        use gutter::GutterMark;

        let mut v = View::new(80, 6); // body_rows = 5
        v.lines = (0..10).map(|i| format!("L{i}").into_bytes()).collect();
        v.starting_line_no = 3; // simulate having dropped 2 oldest
        // snap the animated visual to the tail-mode target so we render
        // from the bottom (production calls `tail_pin_snap` after content
        // arrives in tail mode for the same reason).
        v.tail_pin_snap();

        let g = Gutter { width: 2, marks: vec![GutterMark::None; 20] };
        let frame = render_frame(&mut v, "p", "00:00:00", 100, Some(&g), false);

        // tail mode pins to bottom (offset 5..10). these correspond to file
        // line numbers starting_line_no + 5 .. starting_line_no + 9 = 8..12.
        // we should see the right-aligned numbers 8..12 in the frame.
        for n in 8..=12 {
            let expected = format!("{n:>2} | ");
            assert!(
                frame.contains(&expected),
                "expected {expected:?} in frame, full frame:\n{frame:?}"
            );
        }
    }

    // --- smooth scroll ---

    #[test]
    fn lerp_alpha_saturates_past_six_tau() {
        // long-frame fast-path: should return exactly 1.0 (no exp() call).
        assert_eq!(lerp_alpha(1.0, 0.060), 1.0);
        // mid-range: 1 tau ~= 63% closed.
        let a = lerp_alpha(0.060, 0.060);
        assert!((a - (1.0 - (-1.0_f32).exp())).abs() < 1e-6);
    }

    #[test]
    fn advance_animation_lerps_toward_target() {
        let mut v = View::new(80, 24);
        v.scroll_offset = 100;
        v.scroll_offset_visual = 0.0;
        // step a single ~16ms frame -- alpha = 1 - exp(-0.016/0.060) ~= 0.234.
        // visual should be ~23.4 after one step.
        v.advance_animation(0.016);
        let approx = 100.0 * (1.0 - (-0.016_f32 / 0.060).exp());
        assert!((v.scroll_offset_visual - approx).abs() < 0.01);
    }

    #[test]
    fn advance_animation_snaps_within_epsilon() {
        let mut v = View::new(80, 24);
        v.scroll_offset = 50;
        v.scroll_offset_visual = 49.7; // within SCROLL_SNAP_EPSILON of target
        v.advance_animation(0.016);
        // should hard-snap to 50 rather than asymptote.
        assert_eq!(v.scroll_offset_visual, 50.0);
    }

    #[test]
    fn animation_settled_when_visual_at_target() {
        let mut v = View::new(80, 24);
        v.scroll_offset = 5;
        v.scroll_offset_visual = 5.0;
        assert!(v.animation_settled());
        v.scroll_offset_visual = 4.7; // still within epsilon
        assert!(v.animation_settled());
        v.scroll_offset_visual = 3.0;
        assert!(!v.animation_settled());
    }

    #[test]
    fn render_offset_uses_visual_not_target() {
        let mut v = View::new(80, 24);
        v.lines = (0..50).map(|i| format!("{i}").into_bytes()).collect();
        v.scroll_offset = 30; // target
        v.scroll_offset_visual = 12.6; // mid-animation
        // rounded to 13 -- the renderer should slice from there, not 30.
        assert_eq!(v.render_offset(), 13);
    }

    #[test]
    fn render_offset_clamps_into_valid_range() {
        let mut v = View::new(80, 5); // body_rows = 4
        v.lines = (0..10).map(|i| format!("{i}").into_bytes()).collect();
        v.scroll_offset = 0;
        // visual past the end (e.g. lines were dropped after the tick).
        v.scroll_offset_visual = 99.0;
        // max_offset = 10 - 4 = 6.
        assert_eq!(v.render_offset(), 6);
    }

    #[test]
    fn tail_pin_snap_sets_both_offsets() {
        let mut v = View::new(80, 5); // body_rows = 4
        v.lines = (0..20).map(|i| format!("{i}").into_bytes()).collect();
        v.tail_mode = true;
        v.scroll_offset_visual = 0.0;
        v.tail_pin_snap();
        // max_offset = 16; both target and visual snap there.
        assert_eq!(v.scroll_offset, 16);
        assert_eq!(v.scroll_offset_visual, 16.0);
        assert!(v.animation_settled());
    }

    #[test]
    fn key_scroll_leaves_visual_for_animation_to_catch_up() {
        // proves the production split: apply_key only nudges the target,
        // never the visual. the loop's per-frame `advance_animation` is
        // what actually moves the rendered offset.
        let mut v = View::new(80, 6); // body_rows = 5
        v.lines = (0..200).map(|i| format!("{i}").into_bytes()).collect();
        v.tail_pin_snap();
        let v_before = v.scroll_offset_visual;
        let target_before = v.scroll_offset;

        v.apply_key(Key::Home); // jump to top
        // target moves immediately, visual does not.
        assert_eq!(v.scroll_offset, 0);
        assert_eq!(v.scroll_offset_visual, v_before);
        assert_ne!(v.scroll_offset, target_before);
    }

    #[test]
    fn extend_lines_capped_drags_visual_offset_with_drop() {
        let mut v = View::new(80, 24);
        let pre: Vec<Vec<u8>> = (0..20).map(|i| format!("a{i}").into_bytes()).collect();
        v.extend_lines_capped(&pre, 100, 10);
        v.scroll_offset = 12;
        v.scroll_offset_visual = 12.0;
        v.extend_lines_capped(&[], 15, 4);
        // dropped 5; both offsets shift back by 5 to keep the same window.
        assert_eq!(v.scroll_offset, 7);
        assert_eq!(v.scroll_offset_visual, 7.0);
    }

    #[test]
    fn render_frame_no_gutter_when_none() {
        // no gutter means no prefix at all -- bodies render bare.
        let mut v = View::new(80, 4);
        v.lines = vec![b"alpha".to_vec(), b"beta".to_vec()];
        let frame = render_frame(&mut v, "p", "00:00:00", 100, None, false);
        assert!(frame.contains("alpha"));
        assert!(frame.contains("beta"));
        assert!(!frame.contains("|"));
    }

    #[test]
    fn render_frame_body_is_truncated_to_terminal_width_minus_gutter() {
        // regression: a body wider than `view.width - prefix_width` used to
        // wrap onto the next row (terminal auto-wrap), painting over the next
        // body line. now the body is hard-truncated by us so total visible
        // chars per line stay <= view.width.
        use crate::eat::gutter_view::Gutter;
        use gutter::GutterMark;

        let mut v = View::new(20, 4); // body_rows = 3, terminal cols = 20
        // 50 chars of body -- well over the budget.
        let big = vec![b'x'; 50];
        v.lines = vec![big.clone(), big.clone(), big.clone()];

        // gutter: width=2 -> prefix takes "{n:>2} | " = 5 cols.
        // body budget = 20 - 5 = 15. each rendered body should contain at
        // most 15 'x' chars before any escape sequence.
        let g = Gutter { width: 2, marks: vec![GutterMark::None; 3] };
        let frame = render_frame(&mut v, "p", "00:00:00", 100, Some(&g), false);

        // every 'xxxxxxxx...' run in the frame must be <= 15 long.
        let max_run = frame.split(|c: char| c != 'x').map(|s| s.len()).max().unwrap_or(0);
        assert!(max_run <= 15, "found a run of {max_run} 'x's, body should cap at 15");
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
