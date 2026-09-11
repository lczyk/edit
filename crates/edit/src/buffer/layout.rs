//! Layout IR: the output of [`TextBuffer::layout`], consumed by the
//! paint layer.
//!
//! These types live here rather than next to the painter because
//! [`TextBuffer::layout`] is what produces them -- the producer owns the
//! shape. `crate::anim` is a downstream consumer and depends on `buffer`,
//! never the reverse.
//!
//! Everything is owned data (`String`, `Vec<Rect>`) rather than borrows,
//! so pass 1's output can outlive the per-iteration scratch arena that
//! built it and still be painted in pass 2. Cost is ~80 bytes of text
//! copy per visible row -- a few KB per render at common viewport sizes,
//! not a hot path.
//!
//! [`TextBuffer::layout`]: super::TextBuffer::layout

use crate::framebuffer::{Attributes, IndexedColor};
use crate::helpers::{CoordType, Point, Rect};

/// Per-visual-line layout output. One per row of the visible viewport.
pub struct VisualLine {
    /// Framebuffer y coordinate this line writes into.
    pub fb_y: CoordType,
    /// Body text including the margin prefix (line numbers + box
    /// glyph separator + space) and the visual-line content. Pushed
    /// into the framebuffer via `replace_text`.
    pub text: String,
    /// Whether the line's margin column should be dimmed (wrapped
    /// continuation row that doesn't show a real line number).
    pub dim_wrapped_margin: bool,
    /// Selection rect on this line, if the selection covers any of
    /// it. `selection_force_fg` re-applies the fg colour after lsh.
    pub selection_rect: Option<Rect>,
    /// Shadow-match rects on this line (literal occurrences of the
    /// selected text). Usually empty.
    pub shadow_match_rects: Vec<Rect>,
    /// Per-cell rects for whitespace visualisers (central-dot for
    /// spaces, rightward-arrow for tabs).
    pub whitespace_visualizers: Vec<Rect>,
    /// Per-cell rects for control-character visualisers (U+2400-
    /// range pictures inserted for unprintable bytes).
    pub control_chars: Vec<Rect>,
    /// Per-row markup fg rects (lsh syntax colours), clipped to this
    /// visual row's text extent. Computed during layout so wrapped
    /// continuation rows don't paint past their actual end.
    pub markup_fg_rects: Vec<(Rect, IndexedColor)>,
    /// Per-row markup attribute rects (bold / italic / underline /
    /// strikethrough), clipped the same way as `markup_fg_rects`.
    pub markup_attr_rects: Vec<(Rect, Attributes)>,
}

/// Full layout output -- the first pass of the textarea render flow.
/// Carries every piece of data the second paint pass needs: one
/// [`VisualLine`] per visible row, the per-row gutter marks, the
/// visual-x extent, and the scratch state pass 2 forwards into
/// `render_apply_highlights` + `textarea_overlays`.
pub struct TextareaLayout {
    /// One per visible row.
    pub lines: Vec<VisualLine>,
    /// Per-row gutter marks to paint after the margin tint.
    pub gutter_marks: Vec<(CoordType, gutter::GutterMark)>,
    /// Width of the widest visible row, measured past the viewport's right
    /// edge so it describes the content rather than the current scroll
    /// offset. The textarea bounds horizontal scrolling with it.
    /// `CoordType::MAX` when a row ran past the measurement limit.
    pub visual_pos_x_max: CoordType,
    /// Visual cursor position the paint pass uses for the cursor
    /// block + line highlight (animated or buffer-authoritative).
    pub cursor_visual_render: Point,
    /// Whether the selection is empty after the active-edge pin.
    /// Drives the line-highlight gate (line highlight shows only
    /// when there's no selection).
    pub selection_empty: bool,
    /// Logical y range to scan for syntax highlights, derived from
    /// the running cursor at the top of pass 1 and the cursor at
    /// the end of the visible region.
    pub highlight_logical_y_range: std::ops::Range<CoordType>,
    /// Cursor at the start of the first visible row -- used as the
    /// seed for the next render's cursor walk, and as the start
    /// cursor for the syntax-highlight scan in
    /// [`TextBuffer::render_apply_highlights`]. The caller writes this
    /// back into `TextBuffer::cursor_for_rendering` before invoking the
    /// lsh pass. `None` when the visible area is empty.
    ///
    /// [`TextBuffer::render_apply_highlights`]: super::TextBuffer::render_apply_highlights
    pub start_cursor: Option<crate::unicode::Cursor>,
}
