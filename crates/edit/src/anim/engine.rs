//! Animator: pure perturbation over time.
//!
//! See `meanderings/anim_refactor.md` for the target shape -- the real
//! `animate(prev, target, t, dt)` lands in stage 4 once `Physics`
//! exists and per-feature anim state has been displaced off
//! `TextareaContent` / `Tui`. Today this module holds the per-feature
//! advance fns (cursor, scroll) plus the shared interpolation
//! primitives they sit on top of.

use std::time::Instant;

use crate::buffer::LineMoveEvent;
use crate::helpers::{CoordType, Point};

/// Trail-flash state for an alt+up/down line move. Tracks the moved
/// block's post-move position and start time; alpha fades over
/// [`super::LINE_MOVE_DURATION_SECS`].
///
/// Coordinates are visual y (document-space rows after word-wrap).
#[derive(Clone, Copy)]
pub struct LineMoveAnim {
    pub to_y: CoordType,
    pub height: CoordType,
    pub started_at: Instant,
}

/// On buffer-edit detection, snap the scroll / cursor lerps to their
/// target positions. Buffer edits (typing, paste, alt+up/down
/// line-move, indent, etc.) move text under the cursor; if the lerps
/// keep chasing, the cursor visibly slides *through* the moved
/// content. Snapping keeps it glued.
///
/// `last_gen` is updated to `current_gen` on every call so the
/// caller never sees a stale generation. Returns whether an edit was
/// detected this frame.
pub fn snap_on_buffer_edit(
    scroll_visual: &mut (f32, f32),
    cursor_visual: &mut Option<(f32, f32)>,
    last_gen: &mut u32,
    current_gen: u32,
    scroll_target: Point,
    cursor_target: Point,
) -> bool {
    let edited = current_gen != *last_gen;
    *last_gen = current_gen;
    if edited {
        *scroll_visual = (scroll_target.x as f32, scroll_target.y as f32);
        *cursor_visual = Some((cursor_target.x as f32, cursor_target.y as f32));
    }
    edited
}

/// Per-frame dt in seconds, with idle-gap capping. When the editor
/// has been idle for seconds, a naive `now - prev` would feed the
/// lerps a huge step and they would snap to target in one frame
/// (invisible motion). Capping at one frame keeps the first step
/// small so animation runs visibly across multiple frames driven by
/// the read-timeout cadence. `prev = None` is the first-frame seed
/// (no real dt yet) and returns one frame's worth.
pub fn frame_dt_secs(prev: Option<Instant>, now: Instant) -> f32 {
    match prev {
        Some(prev) => (now - prev).as_secs_f32().min(super::MAX_DT_SECS),
        None => 0.016,
    }
}

/// Seed the line-move trail-flash slot from a freshly-fired event.
/// When `ev` is `Some` and animations are enabled, installs a fresh
/// [`LineMoveAnim`] starting at `now`. Existing slot contents are
/// overwritten -- a new line-move always replaces the old trail. No
/// effect when `ev` is `None` or animations are off.
pub fn seed_line_move_trail(
    slot: &mut Option<LineMoveAnim>,
    ev: Option<LineMoveEvent>,
    now: Instant,
) {
    if let Some(ev) = ev
        && !crate::glyphs::no_animations()
    {
        *slot =
            Some(LineMoveAnim { to_y: ev.to_visual_y, height: ev.visual_height, started_at: now });
    }
}

/// Advance an in-flight line-move trail flash. Returns the
/// normalised `[0, 1)` progress if the flash is still visible, or
/// `None` once the duration has elapsed (also clears the slot so the
/// caller doesn't need to). `None` is also returned immediately when
/// the slot is empty.
pub fn advance_line_move_trail(state: &mut Option<LineMoveAnim>, now: Instant) -> Option<f32> {
    let anim = (*state)?;
    let elapsed = now.duration_since(anim.started_at).as_secs_f32();
    if elapsed >= super::LINE_MOVE_DURATION_SECS {
        *state = None;
        return None;
    }
    Some(elapsed / super::LINE_MOVE_DURATION_SECS)
}

/// Clip rect produced by a floater-open animation. The caller applies
/// this to its node subtree -- `Bottom(y)` shrinks the visible region
/// to `[outer.top, y]` (slide-down dropdowns), `Band(top, bottom)`
/// narrows to `[top, bottom]` (scale-in modals).
#[derive(Clone, Copy)]
pub enum FloaterClip {
    Bottom(CoordType),
    Band(CoordType, CoordType),
}

/// Advance the open-animation for a single floater node. Stores the
/// node's first-seen `Instant` in `opened_at_map` keyed by its id, and
/// returns the clip rect to apply this frame. `None` means the
/// animation has finished (or animations are disabled wholesale) and
/// the floater should render at full size.
///
/// `slide_down` and `scale_in` are mutually exclusive node attributes;
/// when both are false this returns `None` immediately. `outer_top` /
/// `outer_bottom` are the floater's full (unclipped) vertical extent.
pub fn advance_floater_open(
    opened_at_map: &mut std::collections::HashMap<u64, Instant>,
    node_id: u64,
    outer_top: CoordType,
    outer_bottom: CoordType,
    slide_down: bool,
    scale_in: bool,
    now: Instant,
) -> Option<FloaterClip> {
    if crate::glyphs::no_animations() {
        return None;
    }
    if !slide_down && !scale_in {
        return None;
    }

    let opened_at = *opened_at_map.entry(node_id).or_insert(now);
    let elapsed = (now - opened_at).as_secs_f32();
    let duration =
        if scale_in { super::SCALE_IN_DURATION_SECS } else { super::SLIDE_DOWN_DURATION_SECS };
    if elapsed >= duration {
        return None;
    }

    let progress = (elapsed / duration).clamp(0.0, 1.0);
    let full_h = outer_bottom - outer_top;
    let visible = ((full_h as f32) * progress).round() as CoordType;
    if scale_in {
        let centre = (outer_top + outer_bottom) / 2;
        let half = visible.max(1) / 2;
        let top = centre - half;
        let bottom = centre + (visible.max(1) - half);
        Some(FloaterClip::Band(top, bottom))
    } else {
        let bottom = outer_top + visible.max(0);
        Some(FloaterClip::Bottom(bottom))
    }
}

/// Per-frame exponential-lerp alpha for a given time constant.
/// Saturates to 1.0 once `dt` exceeds ~6 tau (effectively done) so we don't
/// pay the cost of `exp()` for the no-op tail.
#[inline]
pub fn lerp_alpha(dt_secs: f32, tau_secs: f32) -> f32 {
    if dt_secs >= tau_secs * 6.0 { 1.0 } else { 1.0 - (-dt_secs / tau_secs).exp() }
}

/// Ease-out cubic curve applied on top of `lerp_alpha`. Front-loads the
/// motion: more distance closed in the first frames, less in the tail.
/// Visually reads as "snappy" without changing the time constant.
/// `eased = 1 - (1 - alpha)^3`.
#[inline]
pub fn ease_out_cubic(alpha: f32) -> f32 {
    let inv = 1.0 - alpha;
    1.0 - inv * inv * inv
}

/// Lerps the animated scroll offset toward `target` using a per-axis
/// exponential time-constant and snaps within 0.5 cells. Returns the
/// rounded integer offset to feed the renderer for this frame.
///
/// `visual` is the carried-over animator state: the fractional
/// position chasing `target`. `target` is today's authoritative
/// scroll offset.
///
/// Animates regardless of jump size: a PageDown / Goto-Line / search
/// jump across a long doc still slides, which actually helps the
/// user keep their orientation after a big move. The exponential
/// curve completes in ~6 tau (~360ms at the default tau), so even a
/// 5000-line jump is over quickly.
pub fn advance_scroll(visual: &mut (f32, f32), target: Point, dt_secs: f32) -> Point {
    if crate::glyphs::no_animations() {
        *visual = (target.x as f32, target.y as f32);
        return target;
    }

    let target_x = target.x as f32;
    let target_y = target.y as f32;

    let alpha = lerp_alpha(dt_secs, super::SCROLL_TAU_SECS);
    visual.0 += (target_x - visual.0) * alpha;
    visual.1 += (target_y - visual.1) * alpha;

    if (target_x - visual.0).abs() < 0.5 {
        visual.0 = target_x;
    }
    if (target_y - visual.1).abs() < 0.5 {
        visual.1 = target_y;
    }

    Point { x: visual.0.round() as CoordType, y: visual.1.round() as CoordType }
}

/// Same lerp shape as `advance_scroll`, but for the visible cursor
/// position. The buffer's logical cursor moves instantly; only the
/// rendered glyph + line highlight follow the animated point.
/// Returns the rounded integer position to feed back into the buffer
/// as a render override.
///
/// `visual = None` means "not yet initialised" -- snap to target on
/// the first call so the cursor doesn't slide in from `(0, 0)`.
///
/// Animates regardless of jump size for the same orientation reason
/// as `advance_scroll`.
pub fn advance_cursor(visual: &mut Option<(f32, f32)>, target: Point, dt_secs: f32) -> Point {
    if crate::glyphs::no_animations() {
        *visual = Some((target.x as f32, target.y as f32));
        return target;
    }

    let target_x = target.x as f32;
    let target_y = target.y as f32;

    let prev = match *visual {
        Some(v) => v,
        None => {
            *visual = Some((target_x, target_y));
            return target;
        }
    };

    let alpha = ease_out_cubic(lerp_alpha(dt_secs, super::CURSOR_TAU_SECS));
    let mut next = (prev.0 + (target_x - prev.0) * alpha, prev.1 + (target_y - prev.1) * alpha);
    if (target_x - next.0).abs() < 0.5 {
        next.0 = target_x;
    }
    if (target_y - next.1).abs() < 0.5 {
        next.1 = target_y;
    }
    *visual = Some(next);

    Point { x: next.0.round() as CoordType, y: next.1.round() as CoordType }
}
