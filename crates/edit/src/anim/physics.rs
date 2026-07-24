//! Physics: post-layout, anim-free per-textarea IR.
//!
//! See `meanderings/anim_refactor.md` for the design. This module
//! defines the **per-textarea** slice of physics -- a flat record of
//! every quantity the painter needs to draw the textarea at its
//! current target state (no interpolation, no sidechannels). The
//! animator perturbs fields on this record over time; nil animator
//! returns it untouched.
//!
//! The frame-wide `Physics` with `NodeDraw` variants (Box, Text,
//! Floater, etc.) from the original target shape was descoped -- the
//! `render_node` tree walk in `Tui` still dispatches to per-node
//! paint paths directly. The per-textarea slice was the most
//! valuable cut: it pulled sidechannels out of `TextBuffer` and
//! made paint pure.
//!
//! The per-row layout IR this record embeds ([`TextareaLayout`]) is
//! owned by `crate::buffer`, since `TextBuffer::layout` is what
//! produces it. Everything here depends downward on `buffer`; nothing
//! in `buffer` depends back on `anim`.

use crate::buffer::{LineMoveEvent, MinimapCell, RowBand, TextareaLayout};
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
/// `scroll_offset`, `cursor_visual`, and visibility of the
/// `line_move` flash based on elapsed time + cached per-feature
/// animator state. The post-layout per-row data (text + decoration
/// rects, gutter marks, ranges) lives in the embedded
/// [`TextareaLayout`].
pub struct TextareaPhysics<'a> {
    /// Framebuffer rect this textarea paints into. Already adjusted
    /// for minimap / scrollbar widths.
    pub dest: Rect,
    /// Target viewport scroll offset (post-layout, pre-animation).
    pub scroll_offset: Point,
    /// Target visual cursor position (in document-visual coords).
    pub cursor_visual: Point,
    /// Visible line count -- used by the scrollbar to size the thumb.
    pub visual_line_count: CoordType,
    /// Minimap rail cells, if the textarea has a minimap.
    pub minimap_cells: &'a [MinimapCell],
    /// Total content rows the minimap was built from.
    pub minimap_content_rows: u32,
    /// Per-row band shape for an in-flight line-move trail.
    pub line_move_bands: &'a [RowBand],
    /// Most recently observed line-move event paired w/ its
    /// monotonic generation. The animator carries the last-seen gen
    /// and treats a higher gen as a fresh event worth starting a
    /// trail flash for. `(None, 0)` means no move has happened yet
    /// on the underlying buffer.
    pub line_move: (Option<LineMoveEvent>, u32),
    /// Whether the textarea currently has keyboard focus.
    pub focus: bool,
    /// Per-row layout output (text + decoration rects) produced by
    /// [`crate::buffer::TextBuffer::layout`]. `None` for textareas
    /// whose dest rect is empty.
    pub layout: Option<TextareaLayout>,
}

/// Build a [`TextareaPhysics`] from the live `TextBuffer` +
/// surrounding context. Runs `TextBuffer::layout()` to populate the
/// post-layout per-row outputs as part of physics build; the
/// resulting `TextareaPhysics` carries everything
/// `textarea_lines` / `textarea_overlays` need to render this textarea at its
/// target state.
pub fn build_textarea_physics<'a>(
    tb: &'a mut crate::buffer::TextBuffer,
    scroll_offset: Point,
    dest: Rect,
    cursor_override: Option<Point>,
    focus: bool,
) -> TextareaPhysics<'a> {
    let layout = tb.layout(scroll_offset, dest, cursor_override);
    TextareaPhysics {
        dest,
        scroll_offset,
        cursor_visual: tb.cursor_visual_pos(),
        visual_line_count: tb.visual_line_count(),
        minimap_cells: tb.minimap_cells(),
        minimap_content_rows: tb.minimap_content_rows(),
        line_move_bands: tb.line_move_bands(),
        line_move: tb.peek_pending_line_move(),
        focus,
        layout,
    }
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
        let mut tb = crate::buffer::TextBuffer::new(true).unwrap();
        tb.set_width(80);
        let dest = Rect { left: 0, top: 0, right: 80, bottom: 24 };
        let cursor_pos = tb.cursor_visual_pos();
        let line_count = tb.visual_line_count();
        let phys = build_textarea_physics(&mut tb, Point { x: 0, y: 0 }, dest, None, true);
        assert_eq!(phys.dest, dest);
        assert_eq!(phys.scroll_offset, Point { x: 0, y: 0 });
        assert_eq!(phys.cursor_visual, cursor_pos);
        assert_eq!(phys.visual_line_count, line_count);
        assert!(phys.line_move.0.is_none());
        assert_eq!(phys.line_move.1, 0);
        assert!(phys.focus);
        assert!(phys.layout.is_some(), "non-empty dest produces a layout");
    }
}
