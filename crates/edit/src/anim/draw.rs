//! Draw: pure consumers that paint into a framebuffer.
//!
//! See `meanderings/anim_refactor.md` for the target shape. Today this
//! module holds the leaf draw helpers that already took a clean
//! `(inputs, &mut Framebuffer)` shape -- they belong here regardless
//! of how much of the pipeline has been carved into a `Physics` IR
//! yet. As more of the paint stage moves out of `TextBuffer::render`
//! and `Tui::render_node`, it lands here.
//!
//! Functions in this module must not read time, `Tui`, `TextBuffer`,
//! or any animator state. Their inputs are everything they need.

use ::gutter::GutterMark;

use crate::buffer::{MINIMAP_SOURCE_ROWS_PER_CELL, MinimapCell, RowBand};
use crate::framebuffer::{Framebuffer, IndexedColor};
use crate::helpers::{CoordType, Point, Rect};

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

/// Forces a uniform foreground colour over a collected list of
/// selection rects. Called after the lsh highlight pass so syntax
/// tokens inside the selection don't show through with their syntax
/// colour -- they get the contrast-friendly selection fg instead.
pub fn selection_force_fg(fb: &mut Framebuffer, rects: &[Rect], fg: crate::oklab::StraightRgba) {
    for rect in rects {
        fb.blend_fg(*rect, fg);
    }
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
