//! Draw: pure consumers that paint into a framebuffer.
//!
//! See `meanderings/anim_refactor.md` for the design. All textarea
//! paint has moved out of `TextBuffer::render` (which is now deleted)
//! and lives here: `textarea_lines` (pass-1 text blit), `textarea_overlays`
//! (cursor block, line highlight, selection, gutter, scrollbar,
//! minimap), and `line_move_trail`.
//!
//! Functions in this module must not read time, `Tui`, `TextBuffer`,
//! or any animator state. Their inputs are everything they need.

use gutter::GutterMark;

use crate::buffer::TextareaLayout;
use crate::buffer::{MINIMAP_SOURCE_ROWS_PER_CELL, MinimapCell, RowBand};
use crate::framebuffer::{Framebuffer, IndexedColor};
use crate::helpers::{CoordType, Point, Rect};

/// Pass-2 paint for a single textarea's [`TextareaLayout`]:
/// for each row, commit the text via `replace_text` and apply the
/// per-row blends (dim margin, selection rect, shadow matches,
/// whitespace visualisers, control chars).
///
/// Returns the accumulated selection-rects vec so the caller can
/// feed it into [`textarea_overlays`] for the post-lsh fg force
/// pass.
///
/// `replace_text` writes only glyphs (no fg/bg), so per-row blends
/// after `replace_text` are equivalent to running them before --
/// per-row paints are independent across rows.
pub fn textarea_lines(
    fb: &mut Framebuffer,
    layout: &TextareaLayout,
    dest_left: CoordType,
    dest_right: CoordType,
    margin_width: CoordType,
    focused: bool,
) -> Vec<Rect> {
    let mut selection_rects: Vec<Rect> = Vec::new();
    let shadow_bg = fb.indexed_alpha(IndexedColor::Foreground, 1, 2);
    let line_number_width = (margin_width.max(3) - 3) as CoordType;
    for line in &layout.lines {
        fb.replace_text(line.fb_y, dest_left, dest_right, &line.text);
        if line.dim_wrapped_margin {
            dim_wrapped_margin(fb, dest_left, line.fb_y, line_number_width);
        }
        if let Some(rect) = line.selection_rect {
            selection_rect(fb, rect, focused, &mut selection_rects);
        }
        for &rect in &line.shadow_match_rects {
            shadow_match_rect(fb, rect, shadow_bg, &mut selection_rects);
        }
        for &rect in &line.whitespace_visualizers {
            whitespace_visualizer(fb, rect);
        }
        for &rect in &line.control_chars {
            control_char_highlight(fb, rect);
        }
        // lsh markup -- per-row, already clipped to the row's text
        // extent by `TextBuffer::build_markup_row`. Painted here
        // (rather than in a separate pass after `textarea_lines`) so
        // wrapped rows can't get attrs smeared onto trailing blanks.
        for &(rect, color) in &line.markup_fg_rects {
            fb.blend_fg(rect, fb.indexed(color));
        }
        for &(rect, attr) in &line.markup_attr_rects {
            fb.replace_attr(rect, crate::framebuffer::Attributes::All, attr);
        }
    }
    selection_rects
}

/// Inputs to [`textarea_overlays`] -- the post-paint overlay pass
/// for a single textarea. Bundles the five separate paint calls
/// (selection fg force, margin tint, gutter marks, ruler, cursor
/// block) into one shape so the caller doesn't have to remember the
/// order or thread shared geometry through each.
///
/// Passed to [`textarea_overlays`] after the per-row text blit.
/// Carries every post-text paint input the overlay fn needs.
pub struct TextareaOverlayOpts<'a> {
    /// Full destination rect (incl. margin).
    pub dest: Rect,
    /// Viewport origin (scroll offset).
    pub origin: Point,
    /// Margin / gutter column width.
    pub margin_width: CoordType,
    /// Ruler column in document-visual coords; <=0 disables.
    pub ruler_column: CoordType,
    /// Per-line selection rects collected during the layout loop.
    pub selection_rects: &'a [Rect],
    /// Per-line gutter marks collected during the layout loop.
    pub gutter_marks: &'a [(CoordType, GutterMark)],
    /// Whether the textarea has keyboard focus -- gates the cursor
    /// block + line highlight.
    pub focused: bool,
    /// Visual cursor position (animated or buffer-authoritative).
    pub cursor_visual: Point,
    /// Word-wrap column (`<= 0` if word-wrap is off).
    pub word_wrap_column: CoordType,
    /// Whether overtype mode is on (drives terminal cursor shape).
    pub overtype: bool,
    /// Whether the cursor row should show the line-highlight band
    /// (caller pre-computes `line_highlight_enabled && no selection`).
    pub line_highlight: bool,
    /// Whether to paint the fixed 80/120 column guides.
    pub column_guides: bool,
}

/// Runs the post-paint overlay pass for one textarea: the five
/// paints that need to run **after** the per-line body text +
/// inline-paint loop has finished. Order matters and is encoded
/// here:
///
/// 1. [`selection_force_fg`] -- overrides lsh syntax fg on
///    selection rects with Black so tokens stay readable.
/// 2. [`margin_tint`] -- dims the gutter column.
/// 3. [`gutter_marks`] -- paints per-line marks; must come after
///    `margin_tint` so marks aren't dimmed.
/// 4. [`ruler`] -- paints the column-ruler band.
/// 5. [`cursor_block`] -- terminal cursor + optional line
///    highlight; only if `focused`.
pub fn textarea_overlays(fb: &mut Framebuffer, opts: TextareaOverlayOpts) {
    let sel_fg = fb.indexed(IndexedColor::Black);
    selection_force_fg(fb, opts.selection_rects, sel_fg);
    margin_tint(fb, opts.dest.left, opts.dest.top, opts.margin_width, opts.dest.bottom);
    gutter_marks(fb, opts.dest.left, opts.margin_width, opts.gutter_marks);
    ruler(
        fb,
        opts.dest.left + opts.margin_width,
        opts.dest.top,
        opts.dest.right,
        opts.dest.bottom,
        opts.origin.x,
        opts.ruler_column,
    );
    if opts.column_guides {
        column_guides(
            fb,
            opts.dest.left + opts.margin_width,
            opts.dest.top,
            opts.dest.right,
            opts.dest.bottom,
            opts.origin.x,
        );
    }
    if opts.focused {
        cursor_block(
            fb,
            opts.dest,
            opts.origin,
            opts.margin_width,
            opts.cursor_visual,
            opts.word_wrap_column,
            opts.overtype,
            opts.line_highlight,
        );
    }
}

/// Maps a viewport scroll offset to the [top, bottom) row range on the rail.
/// Band height is held constant across scroll positions (computed once from
/// viewport / content ratio) so the highlight doesn't visibly shrink/grow as
/// integer-division rounding shifts. At the extremes the band is shifted in
/// instead of clipped, preserving its size.
fn minimap_band_range(
    track_top: CoordType,
    track_h: CoordType,
    scroll_offset: CoordType,
    viewport_h: CoordType,
    content_rows: u32,
) -> (CoordType, CoordType) {
    let rail_h = track_h as i64;
    let cr = content_rows as i64;
    if rail_h <= 0 || cr <= 0 || viewport_h <= 0 {
        return (track_top, track_top);
    }
    // Map currently-visible source rows to the cells that contain them, then
    // to rail rows. Band shrinks toward the last cell as the viewport
    // scrolls past the end (fewer source rows still on screen). When the
    // viewport is fully past content, pin to the last cell.
    let rows_per_cell = MINIMAP_SOURCE_ROWS_PER_CELL as i64;
    let n_cells = (cr + rows_per_cell - 1) / rows_per_cell;

    let scroll = scroll_offset.max(0) as i64;
    let view_start = scroll.min(cr);
    // Bottom of band is derived from the full viewport extent (not clipped
    // to `cr`), so the band keeps its viewport-shaped height and slides
    // into the blank rail rows past the last cell when scrolled past end.
    let view_end_unclipped = scroll + viewport_h as i64;
    let cell_start = view_start / rows_per_cell;
    let cell_end = (view_end_unclipped + rows_per_cell - 1) / rows_per_cell;

    let map_cell = |c: i64| -> i64 { if rail_h >= n_cells { c } else { c * rail_h / n_cells } };
    let band_top = map_cell(cell_start);
    let band_bottom = map_cell(cell_end).max(band_top + 1).min(rail_h);
    (track_top + band_top as CoordType, track_top + band_bottom as CoordType)
}

/// Paints the minimap rail in `track`. Each minimap cell occupies one rail row;
/// the cell's `glyphs` are written left-aligned and any per-cell `fg` is blended
/// over them. The current viewport window is overlaid as a dim background band.
pub fn minimap_rail(
    fb: &mut Framebuffer,
    track: Rect,
    cells: &[MinimapCell],
    content_rows: u32,
    scroll_offset: CoordType,
    viewport_height: CoordType,
) {
    if track.is_empty() || cells.is_empty() || content_rows == 0 {
        return;
    }

    let rail_h = track.height() as i64;
    let n_cells = cells.len() as i64;

    // Map minimap cell index -> rail y. If rail is taller than the cell list,
    // each cell gets one row; otherwise scale.
    let cell_to_y = |idx: i64| -> CoordType {
        let y = if rail_h >= n_cells { idx } else { idx * rail_h / n_cells };
        track.top + y as CoordType
    };

    let mut buf = [0u8; 8];
    for (i, cell) in cells.iter().enumerate() {
        if cell.width == 0 {
            continue;
        }
        let y = cell_to_y(i as i64);
        if y >= track.bottom {
            break;
        }
        let mut s = String::new();
        for g in &cell.glyphs[..cell.width as usize] {
            s.push_str(g.encode_utf8(&mut buf));
        }
        fb.replace_text(y, track.left, track.right, &s);
        if let Some(fg) = cell.fg {
            let row = Rect {
                left: track.left,
                top: y,
                right: (track.left + cell.width as CoordType).min(track.right),
                bottom: y + 1,
            };
            fb.blend_fg(row, fb.indexed(fg));
        }
    }

    // Viewport-window overlay: bright band over the rail rows that map to
    // the current scroll slice. Doubles as the draggable thumb when the
    // minimap absorbs the scrollbar.
    let (band_top, band_bottom) =
        minimap_band_range(track.top, track.height(), scroll_offset, viewport_height, content_rows);
    let band = Rect { left: track.left, top: band_top, right: track.right, bottom: band_bottom };
    if !band.is_empty() {
        // Solid (alpha=1) bg + contrasting fg so the highlighted slice stands
        // out unambiguously, even on terminals where alpha-blended overlays
        // come through faint. `--no-color` strips SGR colours, so fall back
        // to overwriting the band glyphs with a solid marker -- loses the
        // per-row density inside the band but the band itself stays visible.
        if crate::glyphs::no_color() {
            // Density ramp tops out at `#` -- pick a heavier glyph so the
            // band reads as distinctly different.
            let marker: &str = if crate::glyphs::ascii_only() { "@" } else { "\u{2588}" };
            let mut buf = String::with_capacity(band.width() as usize * marker.len());
            for _ in 0..band.width() {
                buf.push_str(marker);
            }
            for y in band.top..band.bottom {
                fb.replace_text(y, band.left, band.right, &buf);
            }
        } else {
            fb.blend_bg(band, fb.indexed(IndexedColor::BrightWhite));
            fb.blend_fg(band, fb.indexed(IndexedColor::Black));
        }
    }
}

/// Dims the foreground of a whitespace visualizer cell (the
/// `central-dot` for spaces, the `rightward-arrow` for tabs) so it
/// reads as a hint rather than as content.
pub fn whitespace_visualizer(fb: &mut Framebuffer, rect: Rect) {
    fb.blend_fg(rect, fb.indexed_alpha(IndexedColor::Foreground, 1, 2));
}

/// Highlights a single C0 / C1 control character glyph (the
/// `U+2400`-range pictures inserted for unprintable bytes) with a
/// yellow bg + contrasted fg.
pub fn control_char_highlight(fb: &mut Framebuffer, rect: Rect) {
    let bg = fb.indexed(IndexedColor::Yellow);
    let fg = fb.contrasted(bg);
    fb.blend_bg(rect, bg);
    fb.blend_fg(rect, fg);
}

/// Paints a single selection-rect with the menubar-derived bg
/// colour + Black fg. Appends `rect` to `out` so the post-lsh pass
/// can re-apply the fg after syntax tokens would have overwritten
/// it (see [`selection_force_fg`]).
///
/// The bg colour matches the menubar fg colour (the colour `file`,
/// `edit` etc. are drawn in), which is the contrasted of the
/// menubar bg = Background oklab BrightBlue/2. When the textarea is
/// unfocused, the selection bg is dimmed by half toward the
/// background colour for the muted-look unfocused-selection cue.
pub fn selection_rect(fb: &mut Framebuffer, rect: Rect, focused: bool, out: &mut Vec<Rect>) {
    let menubar_bg = fb.indexed(IndexedColor::Background).oklab_blend(fb.indexed_alpha(
        IndexedColor::BrightBlue,
        1,
        2,
    ));
    let mut bg = fb.contrasted(menubar_bg);
    if !focused {
        bg = bg.oklab_blend(fb.indexed_alpha(IndexedColor::Background, 1, 2));
    }
    let fg = fb.indexed(IndexedColor::Black);
    fb.blend_bg(rect, bg);
    fb.blend_fg(rect, fg);
    out.push(rect);
}

/// Paints a shadow-match rect (a literal-match of the current
/// selection, elsewhere on the visible line) with the given bg.
/// Appends to `out` so the post-lsh fg force pass picks it up too.
pub fn shadow_match_rect(
    fb: &mut Framebuffer,
    rect: Rect,
    bg: crate::oklab::StraightRgba,
    out: &mut Vec<Rect>,
) {
    fb.blend_bg(rect, bg);
    out.push(rect);
}

/// Forces a uniform foreground colour over a collected list of
/// selection rects. Called after the lsh highlight pass so syntax
/// tokens inside the selection don't show through with their syntax
/// colour -- they get the contrast-friendly selection fg instead.
pub fn selection_force_fg(fb: &mut Framebuffer, rects: &[Rect], fg: crate::oklab::StraightRgba) {
    for rect in rects {
        fb.blend_fg(*rect, fg);
    }
}

/// Dims the indicator-dot margin of a wrapped (continuation) line by
/// blending the line-number column toward the background colour. Run
/// inline per visual row for rows whose gutter shows the wrap-marker
/// dots instead of a real line number.
pub fn dim_wrapped_margin(fb: &mut Framebuffer, left: CoordType, top: CoordType, width: CoordType) {
    fb.blend_fg(
        Rect { left, top, right: left + width, bottom: top + 1 },
        fb.indexed_alpha(IndexedColor::Background, 1, 2),
    );
}

/// Tints the textarea's margin column with a dimmed fg. Run after
/// the per-line gutter writes so the line numbers + box separator
/// inherit the dim, then run **before** the gutter-mark paint so
/// marks aren't dimmed.
pub fn margin_tint(
    fb: &mut Framebuffer,
    margin_left: CoordType,
    margin_top: CoordType,
    margin_width: CoordType,
    margin_bottom: CoordType,
) {
    if margin_width <= 0 {
        return;
    }
    let margin = Rect {
        left: margin_left,
        top: margin_top,
        right: margin_left + margin_width,
        bottom: margin_bottom,
    };
    fb.blend_fg(margin, crate::oklab::StraightRgba::from_le(0x7f7f7f7f));
}

/// Paints per-line gutter marks (added / modified / deleted-above /
/// deleted-below) in the textarea margin. Replays the marks **after**
/// the global margin tint, so the mark colours aren't dimmed by the
/// tint pass that comes before this in the textarea paint flow.
///
/// `margin_left` is the textarea's destination.left; `margin_width`
/// is the full margin column width. The mark sits one column left of
/// the box-vertical separator at `margin_left + margin_width - 2`.
///
/// When `--no-color` is in effect, Added and Modified marks would be
/// invisible (they normally just recolour the box-vertical); this fn falls
/// back to distinct glyphs so the cue survives.
pub fn gutter_marks(
    fb: &mut Framebuffer,
    margin_left: CoordType,
    margin_width: CoordType,
    marks: &[(CoordType, GutterMark)],
) {
    if margin_width < 2 || marks.is_empty() {
        return;
    }
    let mark_x = margin_left + margin_width - 2;
    let no_color = crate::glyphs::no_color();
    for (y, mark) in marks {
        let cell = Rect { left: mark_x, top: *y, right: mark_x + 1, bottom: *y + 1 };
        let (fg, glyph) = match mark {
            GutterMark::Added => {
                (fb.indexed(IndexedColor::BrightGreen), if no_color { Some("+") } else { None })
            }
            GutterMark::Modified => {
                (fb.indexed(IndexedColor::BrightYellow), if no_color { Some("~") } else { None })
            }
            GutterMark::DeletedAbove => {
                (fb.indexed(IndexedColor::BrightRed), Some(crate::glyphs::gutter_deleted_above()))
            }
            GutterMark::DeletedBelow => {
                (fb.indexed(IndexedColor::BrightRed), Some(crate::glyphs::gutter_deleted_below()))
            }
            GutterMark::None => continue,
        };
        if let Some(g) = glyph {
            fb.replace_text(*y, mark_x, mark_x + 1, g);
        }
        fb.blend_fg(cell, fg);
    }
}

/// Paints the column ruler (a faint vertical band over the column
/// past which lines wrap or are flagged as long). No-op if
/// `ruler_column` is non-positive or the ruler falls outside the
/// visible text area.
///
/// `text_left` is the textarea's destination.left + margin_width.
pub fn ruler(
    fb: &mut Framebuffer,
    text_left: CoordType,
    text_top: CoordType,
    text_right: CoordType,
    text_bottom: CoordType,
    scroll_offset_x: CoordType,
    ruler_column: CoordType,
) {
    if ruler_column <= 0 {
        return;
    }
    let left = text_left + (ruler_column - scroll_offset_x).max(0);
    if left >= text_right {
        return;
    }
    fb.blend_bg(
        Rect { left, top: text_top, right: text_right, bottom: text_bottom },
        fb.indexed_alpha(IndexedColor::BrightRed, 1, 4),
    );
}

/// Fixed columns for [`column_guides`]. Hardcoded -- the common 80/120
/// width markers. Toggle via `TextBuffer::set_column_guides_enabled`.
const COLUMN_GUIDE_COLS: [CoordType; 2] = [80, 120];

/// Paints faint 1-column vertical guides at [`COLUMN_GUIDE_COLS`].
/// Gutter-grey (`0x7f7f7f`) at low alpha so they read as a thin hint,
/// not a band. Each guide is clipped if scrolled off-screen-left or
/// past the right edge.
///
/// `text_left` is the textarea's destination.left + margin_width.
pub fn column_guides(
    fb: &mut Framebuffer,
    text_left: CoordType,
    text_top: CoordType,
    text_right: CoordType,
    text_bottom: CoordType,
    scroll_offset_x: CoordType,
) {
    for col in COLUMN_GUIDE_COLS {
        let left = text_left + col - scroll_offset_x;
        if left < text_left || left >= text_right {
            continue;
        }
        fb.blend_bg(
            Rect { left, top: text_top, right: left + 1, bottom: text_bottom },
            crate::oklab::StraightRgba::from_le(0x307f7f7f),
        );
    }
}

/// Paints the focused-textarea cursor block (terminal cursor at the
/// caret cell, optional line-highlight band across the cursor's
/// row).
///
/// `cursor_visual` is in document-visual coords (post-layout but
/// pre-screen-translation); the fn handles the wrap-column edge case
/// and translates into screen coords using `dest` + `origin` +
/// `margin_width`. `line_highlight` is the composite "should the
/// cursor row glow?" decision -- the caller checks
/// `line_highlight_enabled && no selection` before calling.
#[allow(clippy::too_many_arguments)]
pub fn cursor_block(
    fb: &mut Framebuffer,
    dest: Rect,
    origin: Point,
    margin_width: CoordType,
    cursor_visual: Point,
    word_wrap_column: CoordType,
    overtype: bool,
    line_highlight: bool,
) {
    let mut x = cursor_visual.x;
    let mut y = cursor_visual.y;

    if word_wrap_column > 0 && x >= word_wrap_column {
        // The line the cursor is on wraps exactly on the word wrap column
        // which means the cursor is invisible. We need to move it to the
        // next line.
        //
        // Sanity (C): hitting this branch means cursor.visual_pos.x landed
        // exactly on the wrap column -- the bug class from the screenshot
        // thread. Selection paint and line highlight still read the
        // un-bumped visual_pos.y, so the caret appears on a row offset
        // from where text is being inserted.
        #[cfg(feature = "sanity")]
        crate::sanity_check!(
            render_cursor_on_wrap_boundary,
            false,
            "vp={:?} wrap_col={} -- caret bumped to next row, may desync from text",
            cursor_visual,
            word_wrap_column
        );
        x = 0;
        y += 1;
    }

    // Move the cursor into screen space.
    x += dest.left - origin.x + margin_width;
    y += dest.top - origin.y;

    let cursor = Point { x, y };
    let text = Rect {
        left: dest.left + margin_width,
        top: dest.top,
        right: dest.right,
        bottom: dest.bottom,
    };

    if !text.contains(cursor) {
        return;
    }
    fb.set_cursor(cursor, overtype);

    if line_highlight {
        fb.blend_bg(
            Rect { left: dest.left, top: cursor.y, right: dest.right, bottom: cursor.y + 1 },
            crate::oklab::StraightRgba::from_le(0x7f7f7f7f),
        );
    }
}

/// Trail-flash overlay for the alt+up/down line-move animation. Paints a
/// per-row tinted band at the moved block's *new* position and fades the
/// alpha down to zero over the animation duration. No sliding, no glyph
/// replacement -- the text under the band is preserved for the entire
/// flash.
///
/// `to_y` is the moved block's top visual y; `height` is its row count.
/// `bands` carries one entry per visual row of the moved block and decides
/// which columns each row tints: empty rows are left alone, "full" rows
/// span the textarea width, and "range" rows narrow to the line's
/// non-whitespace columns so trailing indent / blank tails do not get
/// highlighted.
pub fn line_move_trail(
    fb: &mut Framebuffer,
    dest: Rect,
    scroll_offset: Point,
    to_y: CoordType,
    height: CoordType,
    t: f32,
    bands: &[RowBand],
) {
    // No-colour fallback: a bg-only blend without colour is invisible.
    // Bail so the text is undisturbed; --no-color users lose the cue.
    if crate::glyphs::no_color() {
        return;
    }
    if dest.is_empty() || height <= 0 || bands.is_empty() {
        return;
    }

    let screen_top = dest.top + to_y - scroll_offset.y;
    let height = height.min(bands.len() as CoordType);

    // Linear fade from `BASE_ALPHA_NUM / BASE_ALPHA_DEN` at t=0 down to 0
    // at t=1. Scaled up by 1024 so fractional alpha steps survive the
    // integer division inside `tint_bg_with_fg`. Peak alpha is 1/1 so the
    // blend can actually lerp all the way to the tint colour at flash
    // start -- with 1/2 caps the lerp lands halfway between the (dark)
    // Background and the tint, and transparent-bg cells like the empty
    // tail of a wrapped continuation row read as "stuck near black".
    const BASE_ALPHA_NUM: u32 = 1;
    const BASE_ALPHA_DEN: u32 = 1;
    let fade = (1.0 - t).clamp(0.0, 1.0);
    let alpha_num = (BASE_ALPHA_NUM as f32 * 1024.0 * fade) as u32;
    if alpha_num == 0 {
        return;
    }
    let alpha_den = BASE_ALPHA_DEN * 1024;

    let span_screen =
        |left_col: CoordType, right_col: CoordType| -> Option<(CoordType, CoordType)> {
            let l = (dest.left + left_col - scroll_offset.x).max(dest.left);
            let r = (dest.left + right_col - scroll_offset.x).min(dest.right);
            if r > l { Some((l, r)) } else { None }
        };

    for k in 0..height {
        let screen_y = screen_top + k;
        if screen_y < dest.top || screen_y >= dest.bottom {
            continue;
        }
        for &(left_col, right_col) in bands[k as usize].iter() {
            if let Some((l, r)) = span_screen(left_col, right_col) {
                let rect = Rect { left: l, top: screen_y, right: r, bottom: screen_y + 1 };
                fb.tint_bg_with_fg(rect, alpha_num, alpha_den);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::helpers::Size;

    use super::*;

    fn fb() -> Framebuffer {
        let mut fb = Framebuffer::new();
        fb.flip(Size { width: 80, height: 24 });
        fb
    }

    #[test]
    fn gutter_marks_skips_empty_list() {
        gutter_marks(&mut fb(), 0, 6, &[]);
    }

    #[test]
    fn gutter_marks_skips_narrow_margin() {
        let marks = [(0, GutterMark::Added)];
        // margin_width < 2 -- no room for the mark cell at margin-2.
        gutter_marks(&mut fb(), 0, 1, &marks);
    }

    #[test]
    fn gutter_marks_paints_each_kind() {
        let marks = [
            (0, GutterMark::Added),
            (1, GutterMark::Modified),
            (2, GutterMark::DeletedAbove),
            (3, GutterMark::DeletedBelow),
            (4, GutterMark::None),
        ];
        // Should run without panicking across all variants.
        gutter_marks(&mut fb(), 0, 6, &marks);
    }

    #[test]
    fn ruler_noop_when_column_zero() {
        ruler(&mut fb(), 0, 0, 80, 24, 0, 0);
    }

    #[test]
    fn ruler_clipped_when_past_right() {
        // ruler_column = 200, viewport scroll = 0, text_right = 80
        // -> left would be 200, falls outside text_right -- no-op.
        ruler(&mut fb(), 0, 0, 80, 24, 0, 200);
    }

    #[test]
    fn column_guides_clips_off_screen() {
        // scroll 40: col 80 -> left 40 (painted), col 120 -> left 80 (>= right, skipped).
        column_guides(&mut fb(), 0, 0, 80, 24, 40);
        // scroll 200: both guides land left of text_left -- skipped, no panic.
        column_guides(&mut fb(), 0, 0, 80, 24, 200);
    }

    #[test]
    fn margin_tint_noop_when_width_zero() {
        margin_tint(&mut fb(), 0, 0, 0, 24);
    }

    #[test]
    fn selection_force_fg_empty_rects_no_op() {
        let fg = crate::oklab::StraightRgba::from_le(0);
        selection_force_fg(&mut fb(), &[], fg);
    }

    #[test]
    fn cursor_block_skips_when_outside_text_rect() {
        let dest = Rect { left: 0, top: 0, right: 80, bottom: 24 };
        let origin = Point { x: 0, y: 0 };
        // cursor_visual far past viewport -- the screen-space mapping
        // lands outside `text` rect, so set_cursor + line_highlight
        // are skipped.
        cursor_block(&mut fb(), dest, origin, 6, Point { x: 500, y: 500 }, 0, false, true);
    }

    #[test]
    fn minimap_rail_noop_when_empty() {
        let track = Rect { left: 70, top: 0, right: 80, bottom: 24 };
        minimap_rail(&mut fb(), track, &[], 0, 0, 24);
    }

    #[test]
    fn line_move_trail_noop_when_height_zero() {
        let dest = Rect { left: 0, top: 0, right: 80, bottom: 24 };
        line_move_trail(&mut fb(), dest, Point { x: 0, y: 0 }, 0, 0, 0.5, &[]);
    }
}
