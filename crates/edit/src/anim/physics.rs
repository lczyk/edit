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
