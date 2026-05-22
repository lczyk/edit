//! Animation infrastructure.
//!
//! Target architecture (see `meanderings/anim_refactor.md`): split the
//! editor's rendering pipeline into three pure stages.
//!
//! 1. **physics** -- post-layout, anim-free, full frame IR. Everything
//!    needed to draw a frame; if the animator is nil, drawing physics
//!    directly produces a valid frame. No sidechannels.
//! 2. **animator** -- pure perturbation over time. Consumes target
//!    physics + previous anim state; emits displayed physics + next
//!    anim state. Only wiggles fields physics already carries.
//! 3. **draw** -- pure consumer. `(physics, &mut framebuffer) -> ()`.
//!    No reads from `Tui` / `TextBuffer` / time.
//!
//! Today this module only exposes timing knobs. Submodules
//! [`physics`], [`engine`], and [`draw`] are stubs that get filled in
//! as the refactor progresses.

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
