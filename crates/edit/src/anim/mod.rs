//! Animation infrastructure.
//!
//! Architecture (see `meanderings/anim_refactor.md`): split the
//! editor's rendering pipeline into three concerns.
//!
//! 1. **physics** -- post-layout, anim-free per-textarea IR. Everything
//!    the painter needs to draw a textarea at its target state. If the
//!    animator is nil, drawing physics directly produces a valid frame.
//!    No sidechannels.
//! 2. **engine** -- pure perturbation over time. Per-feature advance
//!    fns + state types + `animate` / `animate_nil` wrappers.
//!    `no_animations()` dispatch lives at the Tui level.
//! 3. **draw** -- pure consumer. `textarea_lines`, `textarea_overlays`,
//!    `line_move_trail`, and helpers. No reads from `Tui` /
//!    `TextBuffer` / time.
//!
//! Timing constants live here; submodules house the rest.

pub mod draw;
pub mod engine;
pub mod physics;

use std::time::Duration;

/// Exponential-lerp time constant for the cursor block. Snappy --
/// cursor must feel responsive. `alpha = 1 - exp(-dt / TAU)` per
/// frame. Larger = slower / more visible motion.
pub const CURSOR_TAU_SECS: f32 = 0.060;

/// Exponential-lerp time constant for the viewport scroll offset.
/// Same target feel as the cursor; visible but not laggy on fast
/// PageDown / wheel bursts.
pub const SCROLL_TAU_SECS: f32 = 0.060;

/// One-shot open animation duration for slide-down dropdowns.
pub const SLIDE_DOWN_DURATION_SECS: f32 = 0.080;

/// One-shot open animation duration for scale-in modals.
pub const SCALE_IN_DURATION_SECS: f32 = 0.150;

/// Duration of the line-move (Alt+Up/Down) trail flash. Painted at
/// the new position and faded out -- no motion, just a transient
/// highlight that points the eye at where the line landed.
pub const LINE_MOVE_DURATION_SECS: f32 = 0.150;

/// Wakeup interval the main loop is asked to honour while any
/// animation is still in flight (~60 fps).
pub const FRAME_INTERVAL: Duration = Duration::from_millis(16);

/// Cap on the per-frame `dt` fed into the lerp. Without this, the
/// first frame after a long idle (no input for seconds) sees a
/// huge `dt`, the lerp jumps the entire distance in one step, and
/// the animation is invisible.
pub const MAX_DT_SECS: f32 = 0.020;
