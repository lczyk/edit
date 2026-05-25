//! Alt-screen tuis for `eat`, both mounted through `edit::mount`:
//!
//! - **snapshot view** (`run_snapshot`) -- `eat <file>` on a tty.
//!   Read-only `TextBuffer` + edit's textarea (cursor / scroll /
//!   selection / mouse-wheel native). A 2s `tick_interval` drives the
//!   `[modified on disk]` flag; `r` reloads, `q` exits.
//! - **follow view** (`run_follow_mount`) -- `eat -f <file>`.
//!   Funnel-modelled drain into the same `TextBuffer`: stat each tick,
//!   on growth append only the new bytes via `write_raw`; on rotation
//!   reload via `read_file`. Funnel-style `pause_offset` tracks
//!   follow/paused; wheel-up or Up/PgUp/g/Home pauses, End/G or
//!   wheel-down to the bottom resumes follow. `q/esc` exit.
//!
//! Both views go through `edit::mount::mount`; the bespoke alt-screen
//! driver (own vt parser, own viewport state, own line ring) is gone
//! as of phase C.5.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use lsh::runtime::Language;

// --- snapshot driver -----------------------------------------------------

/// Run the snapshot tui pager. Reads the file into a [`TextBuffer`]
/// (marked read-only) and mounts edit's tui via [`crate::mount::mount`].
/// Textarea handles cursor, scroll, selection natively. `q` exits.
///
/// Scope dropped (TODO(lczyk)):
/// - `--color=never` override. edit's tui has no plain-mode toggle yet.
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

    let buf =
        TextBuffer::new_rc(false).map_err(|e| io::Error::other(format!("text buffer: {e:?}")))?;
    {
        let mut b = buf.borrow_mut();
        let mut f = std::fs::File::open(&path)?;
        b.read_file(&mut f).map_err(|e| io::Error::other(format!("read: {e:?}")))?;
        b.set_language(lang);
        b.set_margin_enabled(show_numbers);
        b.set_read_only(true);
    }

    let path_label = path.display().to_string();
    let mut captured_stat = stat_fingerprint(&path);
    let mut file_changed = false;
    let mut last_disk_check = Instant::now();
    let mut header = snapshot_header(&path_label, Instant::now(), file_changed);
    let disk_check_interval = Duration::from_secs(2);

    let _deinit = tty::init();
    tty::switch_modes()?;

    let opts = MountOpts { tick_interval: Some(disk_check_interval), ..Default::default() };
    mount::mount(opts, |ctx| -> ControlFlow<()> {
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
                    captured_stat = stat_fingerprint(&path);
                    file_changed = false;
                    last_disk_check = Instant::now();
                    header = snapshot_header(&path_label, Instant::now(), file_changed);
                    ctx.needs_rerender();
                }
                _ => {}
            }
        }

        // disk-change poll. fires at most every `disk_check_interval`;
        // mount's tick_interval keeps the loop awake to hit this branch
        // even with no user input.
        let now = Instant::now();
        if now.duration_since(last_disk_check) >= disk_check_interval {
            last_disk_check = now;
            let now_stat = stat_fingerprint(&path);
            let changed = match (&captured_stat, &now_stat) {
                (Some(a), Some(b)) => a != b,
                (Some(_), None) => true,
                _ => false,
            };
            if changed != file_changed {
                file_changed = changed;
                header = snapshot_header(&path_label, Instant::now(), file_changed);
                ctx.needs_rerender();
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

fn snapshot_header(path_label: &str, at: Instant, file_changed: bool) -> String {
    let delta = if file_changed { "  [modified on disk]" } else { "" };
    format!(
        "{path_label} @ {}{delta}  (q exit, r reload, arrows / PgUp / PgDn scroll)",
        format_clock(at),
    )
}

/// Mount-based follow view -- the default `eat -f` tty pager and the
/// only path exercised by `edit --follow`.
///
/// Drain loop is modelled on funnel's: stat the file each tick (cheap),
/// branch on (rotated || size unchanged || grown), read only the new
/// bytes from `last_size..current_size`, append via
/// `TextBuffer::write_raw` so the rope grows in place and the
/// highlighter cache only invalidates from the modified line down.
/// Rotation (inode change or size shrink) falls back to a full reload
/// via `read_file`.
///
/// Funnel-style follow/paused: `pause_offset` mirrors funnel's
/// `display_offset` (lines above the live bottom). Transitions through
/// 0 re-enter follow. Wheel-up enters pause; wheel-down toward the
/// bottom resumes. Key bindings: Up/Dn, j/k, g/G, Home, End, PgUp,
/// PgDn, q/esc.
///
/// The textarea is mounted **without** focus. Unfocused textareas:
/// - do not paint the terminal cursor (eat is a viewer, not an editor),
/// - still accept mouse-wheel scroll (handled before the focus check),
/// - ignore keyboard input -- we translate keys to
///   `request_scroll_delta_y` calls in this callback instead.
pub fn run_follow_mount(
    path: PathBuf,
    lang: Option<&'static Language>,
    show_numbers: bool,
    _use_color: bool,
    poll_interval: Duration,
) -> io::Result<()> {
    use std::io::{Read as _, Seek as _};
    use std::ops::ControlFlow;

    use crate::buffer::TextBuffer;
    use crate::helpers::{CoordType, Point, Size};
    use crate::input::{kbmod, vk};
    use crate::mount::{self, MountOpts};

    // tail-snap helper. `request_scroll_delta_y(visual_line_count)`
    // lands at `visual_line_count - 1` after `textarea_adjust_scroll_offset`'s
    // clamp -- last line at the **top** of the viewport, not the
    // bottom. `cursor_move + make_cursor_visible` instead pipes through
    // `textarea_make_cursor_visible`, which sets
    // `scroll_y = cursor_y - viewport_height + 1` -- last line at the
    // bottom edge, which is what a tail view wants.
    fn snap_to_tail(b: &mut TextBuffer) {
        b.cursor_move_to_logical(Point::MAX);
        let x = b.cursor_visual_pos().x;
        b.set_preferred_column(x);
        b.make_cursor_visible();
    }

    // initial load: full read via `read_file` so encoding / line-ending
    // detection runs once.
    let buf =
        TextBuffer::new_rc(false).map_err(|e| io::Error::other(format!("text buffer: {e:?}")))?;
    let (mut last_size, mut last_inode) = {
        let mut b = buf.borrow_mut();
        let mut f = std::fs::File::open(&path)?;
        b.read_file(&mut f).map_err(|e| io::Error::other(format!("read: {e:?}")))?;
        b.set_language(lang);
        b.set_margin_enabled(show_numbers);
        b.set_read_only(true);
        snap_to_tail(&mut b);
        let m = std::fs::metadata(&path)?;
        (m.len(), inode_of(&m))
    };

    let path_label = path.display().to_string();

    // funnel-style follow/paused state. See module-level docs for the
    // pause_offset mirror semantics; body_h is recomputed per frame and
    // pause_offset is clamped against `visual_line_count - body_h` so
    // wheel-up past the top doesn't accumulate phantom offset that the
    // textarea won't honour.
    let mut following = true;
    let mut pause_offset: CoordType = 0;

    let mut wheel_accel = WheelAccel::default();

    let _deinit = tty::init();
    tty::switch_modes()?;

    // disable scroll/cursor animation. with a 250ms (or even 100ms)
    // poll the textarea's ~60ms scroll lerp lands between ticks --
    // viewport sits still then snaps, reading as "jerky". Snapping
    // instantly per tick gives the smooth funnel-style cadence.
    // Restored on scope exit so the editor's animations are unaffected.
    let prev_no_anim = crate::glyphs::no_animations();
    crate::glyphs::set_no_animations(true);
    let _restore_anim = scopeguard_no_anim(prev_no_anim);

    // tick at ~30fps regardless of the user's poll_interval. stat is
    // cheap; reload still gated on stat change. Decoupling the wake
    // rate from the reload rate is what makes a fast-growing log read
    // as smooth scrolling rather than 4Hz chunk-jumps. honour the
    // user's `-f <interval>` only as a floor (don't wake faster than
    // requested if they explicitly want less frequent polling).
    let tick = poll_interval.min(Duration::from_millis(33));
    let opts = MountOpts { tick_interval: Some(tick), ..Default::default() };
    mount::mount(opts, |ctx| -> ControlFlow<()> {
        let body_h = (ctx.size().height - 1).max(1) as CoordType;
        let max_offset = (buf.borrow().visual_line_count() - body_h).max(0);

        // helper: apply a scroll delta and update pause_offset / follow
        // state in lockstep. `delta` is the same value passed to the
        // textarea: positive = scroll down (toward bottom).
        let mut apply_scroll = |delta: CoordType| {
            // pause_offset moves inversely to scroll_offset.y.
            pause_offset = (pause_offset - delta).clamp(0, max_offset);
            following = pause_offset == 0;
        };

        // wheel-input is applied by the textarea on its own, but we
        // still need to mirror its effect on pause_offset so the
        // follow/paused state stays in sync.
        let raw_wheel = ctx.scroll_delta().y;
        if raw_wheel != 0 {
            // ramp ride: fast spinning bumps the wheel-line multiplier
            // (funnel WheelAccel ported verbatim). only used to update
            // our shadow -- textarea has already applied raw_wheel to
            // its own scroll_offset.
            let dir = if raw_wheel < 0 { ScrollDir::Up } else { ScrollDir::Down };
            let factor = wheel_accel.lines(Instant::now(), dir) as CoordType;
            // the textarea only knows the raw delta; we apply the same
            // raw delta to pause_offset so they stay in sync. accel is
            // ignored on the textarea side b/c we don't have a way to
            // boost its scroll from out here. revisit if wheel feel
            // becomes a problem.
            let _ = factor;
            apply_scroll(raw_wheel);
        }

        if let Some(k) = ctx.keyboard_input() {
            // strip modifiers for the plain-key matches; SHIFT+g is
            // handled separately so it can map to End-equivalent.
            let bare = k.key();
            let shifted = k.modifiers_contains(kbmod::SHIFT);

            // q / esc exit. textarea is unfocused so it won't handle
            // anything itself; translate the rest into scroll-delta
            // requests on the buffer and mirror the effect on
            // pause_offset. j/k aliased to Down/Up, g to Home, G
            // (shift+g) to End -- matches the legacy follow-tui keys.
            if bare == vk::Q || bare == vk::ESCAPE {
                ctx.set_input_consumed();
                return ControlFlow::Break(());
            } else if bare == vk::UP || (bare == vk::K && !shifted) {
                buf.borrow_mut().request_scroll_delta_y(-1);
                apply_scroll(-1);
                ctx.set_input_consumed();
            } else if bare == vk::DOWN || (bare == vk::J && !shifted) {
                buf.borrow_mut().request_scroll_delta_y(1);
                apply_scroll(1);
                ctx.set_input_consumed();
            } else if bare == vk::PRIOR {
                let d = (body_h - 1).max(1);
                buf.borrow_mut().request_scroll_delta_y(-d);
                apply_scroll(-d);
                ctx.set_input_consumed();
            } else if bare == vk::NEXT {
                let d = (body_h - 1).max(1);
                buf.borrow_mut().request_scroll_delta_y(d);
                apply_scroll(d);
                ctx.set_input_consumed();
            } else if bare == vk::HOME || (bare == vk::G && !shifted) {
                let n = buf.borrow().visual_line_count();
                buf.borrow_mut().request_scroll_delta_y(-n);
                apply_scroll(-n);
                ctx.set_input_consumed();
            } else if bare == vk::END || (bare == vk::G && shifted) {
                snap_to_tail(&mut buf.borrow_mut());
                pause_offset = 0;
                following = true;
                ctx.set_input_consumed();
            }
        }

        // drain: stat, branch on (rotated, idle, grown), feed
        // append-bytes through TextBuffer::write_raw. mirror of funnel's
        // `drain` minus the per-line emit (we feed the buffer in one
        // chunk; the textarea handles wrapping/highlighting at paint
        // time).
        let mut grew = false;
        if let Ok(meta) = std::fs::metadata(&path) {
            let cur_size = meta.len();
            let cur_inode = inode_of(&meta);
            let rotated = cur_inode != last_inode || cur_size < last_size;
            if rotated {
                // full reload on rotation: clears the buffer, resets
                // the highlighter cache, then re-snaps to whatever the
                // file currently has.
                if let Ok(mut f) = std::fs::File::open(&path) {
                    let mut b = buf.borrow_mut();
                    b.set_read_only(false);
                    let _ = b.read_file(&mut f);
                    b.set_margin_enabled(show_numbers);
                    b.set_read_only(true);
                }
                last_size = cur_size;
                last_inode = cur_inode;
                grew = true;
            } else if cur_size > last_size {
                // grew: read [last_size..cur_size] and append.
                let mut chunk = Vec::with_capacity((cur_size - last_size) as usize);
                if let Ok(mut f) = std::fs::File::open(&path)
                    && f.seek(std::io::SeekFrom::Start(last_size)).is_ok()
                    && f.read_to_end(&mut chunk).is_ok()
                {
                    let mut b = buf.borrow_mut();
                    b.set_read_only(false);
                    // cursor at end so write_raw appends. invalidates
                    // the highlighter cache from this line down --
                    // cheap when appending near EOF.
                    b.cursor_move_to_logical(Point::MAX);
                    b.write_raw(&chunk);
                    b.set_read_only(true);
                    last_size = cur_size;
                    grew = true;
                }
            }
        }
        if grew {
            if following {
                snap_to_tail(&mut buf.borrow_mut());
            }
            // paused branch is a no-op: scroll_offset.y is a top-line
            // index that stays put as the buffer grows, so already-
            // visible content stays anchored.
            ctx.needs_rerender();
        }

        let counts = HeaderCounts { pause_offset, body_h, total: buf.borrow().visual_line_count() };
        let header =
            follow_mount_header(&path_label, Instant::now(), following, poll_interval, counts);

        let size = ctx.size();
        ctx.label("follow-header", &header);

        // NB: no `inherit_focus()` -- unfocused textarea suppresses the
        // terminal cursor. mouse-wheel scroll still works (handled
        // pre-focus-check in textarea_handle_input).
        ctx.textarea("follow-body", buf.clone());
        let body_h = (size.height - 1).max(1) as CoordType;
        ctx.attr_intrinsic_size(Size { width: 0, height: body_h });

        ControlFlow::Continue(())
    })
}

#[cfg(unix)]
fn inode_of(meta: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt as _;
    meta.ino()
}

#[cfg(not(unix))]
fn inode_of(_meta: &std::fs::Metadata) -> u64 {
    0
}

/// Mouse-wheel acceleration ported from funnel. Slow spins return 1
/// line; sustained fast spinning ramps to 2. Streak counts up on
/// FAST_THRESHOLD ticks, decays per medium tick, resets on direction
/// flip or after RESET_MS of quiet. Kept as a struct (and exported
/// internally as `ScrollDir`) so the C.3 work that adds wheel-boost on
/// the textarea side has somewhere to plug in.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
enum ScrollDir {
    Up,
    Down,
}

#[derive(Default)]
struct WheelAccel {
    last_tick: Option<Instant>,
    last_dir: Option<ScrollDir>,
    fast_streak: u32,
}

const WHEEL_FAST_THRESHOLD_MS: u128 = 60;
const WHEEL_RESET_MS: u128 = 250;
const WHEEL_STREAK_MAX: u32 = 12;

impl WheelAccel {
    fn lines(&mut self, now: Instant, dir: ScrollDir) -> usize {
        let dt = self.last_tick.map(|t| now.duration_since(t).as_millis()).unwrap_or(u128::MAX);
        let dir_flipped = self.last_dir.is_some_and(|d| d != dir);
        self.last_tick = Some(now);
        self.last_dir = Some(dir);
        if dir_flipped || dt > WHEEL_RESET_MS {
            self.fast_streak = 0;
        } else if dt < WHEEL_FAST_THRESHOLD_MS {
            self.fast_streak = (self.fast_streak + 1).min(WHEEL_STREAK_MAX);
        } else {
            self.fast_streak = self.fast_streak.saturating_sub(1);
        }
        match self.fast_streak {
            0..=3 => 1,
            _ => 2,
        }
    }
}

struct HeaderCounts {
    pause_offset: crate::helpers::CoordType,
    body_h: crate::helpers::CoordType,
    total: crate::helpers::CoordType,
}

/// RAII guard restoring the global `no_animations` flag on drop. Used
/// by `run_follow_mount` so animations are disabled only for the
/// lifetime of the follow view.
struct NoAnimRestore(bool);
impl Drop for NoAnimRestore {
    fn drop(&mut self) {
        crate::glyphs::set_no_animations(self.0);
    }
}
fn scopeguard_no_anim(prev: bool) -> NoAnimRestore {
    NoAnimRestore(prev)
}

fn follow_mount_header(
    path_label: &str,
    at: Instant,
    following: bool,
    poll: Duration,
    counts: HeaderCounts,
) -> String {
    let poll_ms = poll.as_millis();
    let mode = if following {
        "following".to_string()
    } else {
        // pause_offset == lines below viewport's bottom edge. above ==
        // top-line index, i.e. total - body_h - pause_offset (clamp 0).
        let below = counts.pause_offset.max(0);
        let above = (counts.total - counts.body_h - counts.pause_offset).max(0);
        format!("paused: {below} below | {above} above")
    };
    format!(
        "{path_label} [{mode}] @ {}  ({poll_ms}ms Up/Dn g/G PgUp/PgDn scroll, q)",
        format_clock(at),
    )
}

/// Snapshot of file metadata used to detect on-disk changes between polls.
/// `None` on platforms without unix metadata or when the file is gone.
struct SnapshotStat {
    size: u64,
    ino: u64,
    mtime_ns: i128,
}

impl PartialEq for SnapshotStat {
    fn eq(&self, other: &Self) -> bool {
        self.size == other.size && self.ino == other.ino && self.mtime_ns == other.mtime_ns
    }
}

fn stat_fingerprint(path: &Path) -> Option<SnapshotStat> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let meta = std::fs::metadata(path).ok()?;
        Some(SnapshotStat {
            size: meta.size(),
            ino: meta.ino(),
            mtime_ns: meta.mtime() as i128 * 1_000_000_000 + meta.mtime_nsec() as i128,
        })
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
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
