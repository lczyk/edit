//! Physics: post-layout, anim-free, full frame IR.
//!
//! See `meanderings/anim_refactor.md` for the target shape. Today
//! this module only defines the **per-textarea** slice of physics --
//! a flat record of every quantity the painter needs to draw the
//! textarea at its current target state (no interpolation, no
//! sidechannels). The animator perturbs fields on this record over
//! time; nil animator returns it untouched.
//!
//! The frame-wide `Physics` containing all `NodeDraw` variants lands
//! in stage 1 once `TextBuffer::render` has been split into pure
//! `layout` + `paint`.

use crate::buffer::{LineMoveEvent, MinimapCell, RowBand};
use crate::helpers::{CoordType, Point, Rect};

/// Cell width the textarea reserves for the minimap rail. Authority
/// lives in the document's pre-built minimap cells; this just reads
/// back what was built. Returns 0 for single-line widgets (no
/// minimap) and for buffers w/out minimap cells.
pub fn textarea_minimap_width(single_line: bool, minimap_cells: &[MinimapCell]) -> CoordType {
    if single_line {
        return 0;
    }
    minimap_cells.first().map(|c| c.width as CoordType).unwrap_or(0)
}

/// Width of the dedicated scrollbar column. Mutually exclusive with
/// the minimap -- when the rail is visible it absorbs the
/// navigation role. Returns 0 for single-line widgets (no
/// scrollbar) and when the minimap is present.
pub fn textarea_scrollbar_width(single_line: bool, minimap_w: CoordType) -> CoordType {
    if single_line || minimap_w > 0 {
        return 0;
    }
    1
}

/// Per-textarea physics: everything a textarea paint needs, with no
/// animation state mixed in.
///
/// Coordinates are at the **target** state -- i.e. what the painter
/// would produce iff the animator were nil. The animator wiggles
/// `scroll_offset`, `cursor_visual`, and visibility of
/// `line_move_band` based on elapsed time + cached per-feature
/// animator state.
///
/// Today this struct documents intent; it's not yet wired in. Fields
/// marked as **pulled from** indicate the existing pub API path the
/// builder will use. Fields marked as **needs split** require the
/// stage-1 `TextBuffer::render -> TextBuffer::layout` extraction
/// before they can be populated.
pub struct TextareaPhysics<'a> {
    /// Framebuffer rect this textarea paints into. Already adjusted
    /// for minimap / scrollbar widths.
    pub dest: Rect,
    /// Target viewport scroll offset (post-layout, pre-animation).
    /// pulled from `TextareaContent.scroll_offset`.
    pub scroll_offset: Point,
    /// Target visual cursor position (in document-visual coords).
    /// pulled from `TextBuffer::cursor_visual_pos()`.
    pub cursor_visual: Point,
    /// Visible line count -- used by the scrollbar to size the thumb.
    /// pulled from `TextBuffer::visual_line_count()`.
    pub visual_line_count: CoordType,
    /// Minimap rail cells, if the textarea has a minimap.
    /// pulled from `TextBuffer::minimap_cells()`.
    pub minimap_cells: &'a [MinimapCell],
    /// Total content rows the minimap was built from.
    /// pulled from `TextBuffer::minimap_content_rows()`.
    pub minimap_content_rows: u32,
    /// Per-row band shape for an in-flight line-move trail.
    /// pulled from `TextBuffer::line_move_bands()`.
    pub line_move_bands: &'a [RowBand],
    /// Most recently observed line-move event, if any. The animator
    /// uses this to decide whether to paint a trail flash this frame
    /// (and at what alpha).
    ///
    /// Today this is drained via `TextBuffer::take_pending_line_move`
    /// -- in stage 4 it becomes non-draining so physics can read it
    /// every frame until the animator decides the flash is done.
    pub line_move: Option<LineMoveEvent>,
    /// Whether the textarea currently has keyboard focus.
    /// pulled from `TextareaContent.has_focus`.
    pub focus: bool,
    // Fields below require the `TextBuffer::render` split before
    // they can be populated. Placeholder docs only.
    //
    // pub visual_lines: &'a [VisualLine<'a>],   // needs split
    // pub selection_geom: Option<SelectionGeom>, // needs split
    // pub gutter: GutterDraw<'a>,               // needs split
}

/// Per-visual-line layout output produced by pass 1 of
/// [`crate::buffer::TextBuffer::render`] (the eventual
/// `TextBuffer::layout()`). One per row of the visible viewport.
///
/// Owned data only: `text` is `String`, rect collections are
/// `Vec<Rect>`. This lets pass 1's output cross arena boundaries
/// (per-iter scratch arenas can die before pass 2 paints from this
/// struct). Cost is ~80 bytes of text copy per visible row, a few
/// KB per render at common viewport sizes -- not a hot path.
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
}

/// Build a [`TextareaPhysics`] from the live `TextBuffer` +
/// surrounding context. Today this only populates the fields the
/// existing pub API can supply -- viewport + cursor + scrollbar +
/// minimap geometry. The fields blocked on the stage-1 layout
/// carve (`visual_lines`, `selection_geom`, `gutter`) are still
/// commented out on the struct; they get filled in once
/// `TextBuffer::layout()` exists.
///
/// `line_move` is `None` today because the existing path drains
/// the event into the animator's slot inline. In stage 4 the
/// buffer keeps the event non-draining and physics reads it
/// directly.
pub fn build_textarea_physics<'a>(
    tb: &'a crate::buffer::TextBuffer,
    scroll_offset: Point,
    dest: Rect,
    focus: bool,
) -> TextareaPhysics<'a> {
    TextareaPhysics {
        dest,
        scroll_offset,
        cursor_visual: tb.cursor_visual_pos(),
        visual_line_count: tb.visual_line_count(),
        minimap_cells: tb.minimap_cells(),
        minimap_content_rows: tb.minimap_content_rows(),
        line_move_bands: tb.line_move_bands(),
        line_move: None,
        focus,
    }
}

/// Full layout output of [`crate::buffer::TextBuffer::layout`] --
/// pass 1 of the eventual `render(physics)` flow. Carries every
/// piece of data pass 2 paint needs: one [`VisualLine`] per visible
/// row, the per-row gutter marks vec, the visual-x extent, and the
/// scratch state pass 2 forwards into `render_apply_highlights` +
/// `textarea_overlays`.
pub struct TextareaLayout {
    /// One per visible row.
    pub lines: Vec<VisualLine>,
    /// Per-row gutter marks to paint after the margin tint.
    pub gutter_marks: Vec<(CoordType, gutter::GutterMark)>,
    /// Max visual-x reached across all visible rows. Reported back
    /// out via `RenderResult` so the textarea can update its
    /// horizontal scroll cap.
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
}

/// Selection geometry covering the whole visible viewport, in
/// document-visual coords. Produced by the eventual `layout()`.
/// The `active_edge_x` is the visual x of the cursor-anchored end
/// of the selection -- the animator displaces this to keep the
/// trailing edge glued to the animated cursor without disturbing
/// the static end.
pub struct SelectionGeom {
    pub beg: Point,
    pub end: Point,
    /// `true` iff the cursor sits at the `end` endpoint (vs `beg`).
    pub active_is_end: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimap_width_zero_for_single_line() {
        assert_eq!(textarea_minimap_width(true, &[]), 0);
    }

    #[test]
    fn minimap_width_zero_when_no_cells() {
        assert_eq!(textarea_minimap_width(false, &[]), 0);
    }

    #[test]
    fn scrollbar_width_zero_for_single_line() {
        assert_eq!(textarea_scrollbar_width(true, 0), 0);
    }

    #[test]
    fn scrollbar_width_zero_when_minimap_present() {
        assert_eq!(textarea_scrollbar_width(false, 4), 0);
    }

    #[test]
    fn scrollbar_width_one_when_alone() {
        assert_eq!(textarea_scrollbar_width(false, 0), 1);
    }

    #[test]
    fn build_textarea_physics_smoke() {
        // Smoke test: a freshly-built TextBuffer w/ default state
        // gives us a physics record whose simple fields match what
        // the pub API exposes.
        let tb = crate::buffer::TextBuffer::new(true).unwrap();
        let dest = Rect { left: 0, top: 0, right: 80, bottom: 24 };
        let phys = build_textarea_physics(&tb, Point { x: 0, y: 0 }, dest, true);
        assert_eq!(phys.dest, dest);
        assert_eq!(phys.scroll_offset, Point { x: 0, y: 0 });
        assert_eq!(phys.cursor_visual, tb.cursor_visual_pos());
        assert_eq!(phys.visual_line_count, tb.visual_line_count());
        assert!(phys.line_move.is_none());
        assert!(phys.focus);
    }
}
