//! Minimap: low-res preview rail showing document shape.
//!
//! Unicode mode renders 2 cells wide per minimap row. Each row covers
//! 4 source lines x 4 horizontal "dot-col" buckets across the document's max
//! line width; each (source-row, dot-col) sub-cell is "inky" iff any
//! non-whitespace char falls in it. Packed into 2 braille glyphs per row.
//!
//! Ascii / `--ascii-only` mode falls back to 1 cell wide with a 5-level
//! density ramp keyed by how many of the 4 source rows have any non-ws.
//!
//! V1 caveats:
//! - Wide CJK chars are approximated as 1 visual col (under-counts mass).
//! - Tabs expand to `tab_size` cols of whitespace -> non-inky -> indent
//!   staircase is implicit (negative space), matching vscode minimap parity.
//! - Per-line dominant lsh colour sampling is a separate task; `fg` stays
//!   `None` until that lands.

use edit::buffer::{MINIMAP_SOURCE_ROWS_PER_CELL, MinimapCell};
use edit::glyphs;

pub const MAX_MINIMAP_BYTES: usize = 5 * 1024 * 1024;
const SOURCE_ROWS_PER_CELL: u32 = MINIMAP_SOURCE_ROWS_PER_CELL;

#[derive(Default)]
pub struct MinimapState {
    pub cells: Vec<MinimapCell>,
    pub content_rows: u32,
    pub max_visual_width: u32,
    suppressed: bool,
}

impl MinimapState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_suppressed(&self) -> bool {
        self.suppressed
    }

    /// Recompute from scratch. `bytes` is the whole document, `tab_size` the
    /// editor's tab width, `target_width` the intended cell width (1 or 2).
    pub fn rebuild(&mut self, bytes: &[u8], tab_size: u32, target_width: u8) {
        self.cells.clear();
        if bytes.len() > MAX_MINIMAP_BYTES {
            self.suppressed = true;
            self.content_rows = 0;
            self.max_visual_width = 0;
            return;
        }
        self.suppressed = false;
        let lines = split_lines(bytes);
        self.content_rows = lines.len() as u32;
        if self.content_rows == 0 {
            return;
        }

        let tab_size = tab_size.max(1);
        self.max_visual_width = lines.iter().map(|l| visual_width(l, tab_size)).max().unwrap_or(0);

        let ascii = glyphs::ascii_only();
        let n_cells: u8 = target_width.clamp(1, 2);
        // Unicode cells pack 2 dot-cols each; ascii cells use one horizontal
        // bucket per cell (one density level per half of the document width).
        let n_buckets: u32 = if ascii { n_cells as u32 } else { (n_cells as u32) * 2 };
        let bucket_cols = self.max_visual_width.max(1).div_ceil(n_buckets);

        let n_rows = self.content_rows.div_ceil(SOURCE_ROWS_PER_CELL);
        self.cells.reserve(n_rows as usize);
        for mr in 0..n_rows {
            let src_start = mr * SOURCE_ROWS_PER_CELL;
            self.cells.push(compute_cell(&lines, src_start, tab_size, bucket_cols, n_cells, ascii));
        }
    }
}

fn compute_cell(
    lines: &[&[u8]],
    src_start: u32,
    tab_size: u32,
    bucket_cols: u32,
    n_cells: u8,
    ascii: bool,
) -> MinimapCell {
    let mut cell = MinimapCell { glyphs: [' '; 2], width: 0, fg: None };
    if ascii {
        // One density level per horizontal bucket per cell. Level = number of
        // source rows (out of 4) with any inky char in that bucket.
        let n_buckets = n_cells as u32;
        let mut levels = [0u8; 2];
        for dr in 0..SOURCE_ROWS_PER_CELL {
            let li = (src_start + dr) as usize;
            if li >= lines.len() {
                break;
            }
            let inky = inky_buckets(lines[li], tab_size, bucket_cols, n_buckets);
            for b in 0..n_buckets {
                if inky & (1 << b) != 0 {
                    levels[b as usize] += 1;
                }
            }
        }
        for (i, &level) in levels.iter().enumerate().take(n_cells as usize) {
            cell.glyphs[i] = glyphs::minimap_ascii(level);
        }
        cell.width = n_cells;
    } else {
        let n_dot_cols = (n_cells as u32) * 2;
        let mut masks = [0u8; 2];
        for dr in 0..SOURCE_ROWS_PER_CELL {
            let li = (src_start + dr) as usize;
            if li >= lines.len() {
                break;
            }
            let inky = inky_buckets(lines[li], tab_size, bucket_cols, n_dot_cols);
            for global_dc in 0..n_dot_cols {
                if inky & (1 << global_dc) != 0 {
                    let cell_idx = (global_dc / 2) as usize;
                    let dc = global_dc % 2;
                    masks[cell_idx] |= 1 << braille_bit(dc, dr);
                }
            }
        }
        cell.glyphs[0] = glyphs::minimap_braille(masks[0]);
        if n_cells >= 2 {
            cell.glyphs[1] = glyphs::minimap_braille(masks[1]);
        }
        cell.width = n_cells;
    }
    cell
}

// Unicode 8-dot braille bit positions (offset from U+2800):
//   col 0          col 1
//   bit 0 (dot 1)  bit 3 (dot 4)   <- row 0
//   bit 1 (dot 2)  bit 4 (dot 5)   <- row 1
//   bit 2 (dot 3)  bit 5 (dot 6)   <- row 2
//   bit 6 (dot 7)  bit 7 (dot 8)   <- row 3
fn braille_bit(dot_col: u32, dot_row: u32) -> u32 {
    match (dot_col, dot_row) {
        (0, 0) => 0,
        (0, 1) => 1,
        (0, 2) => 2,
        (0, 3) => 6,
        (1, 0) => 3,
        (1, 1) => 4,
        (1, 2) => 5,
        (1, 3) => 7,
        _ => unreachable!(),
    }
}

fn split_lines(bytes: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::with_capacity(bytes.iter().filter(|&&b| b == b'\n').count() + 1);
    let mut start = 0;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            let end = if i > start && bytes[i - 1] == b'\r' { i - 1 } else { i };
            out.push(&bytes[start..end]);
            start = i + 1;
        }
    }
    if start < bytes.len() {
        out.push(&bytes[start..]);
    }
    out
}

fn visual_width(line: &[u8], tab_size: u32) -> u32 {
    let mut w: u32 = 0;
    for &b in line {
        if b == b'\t' {
            w += tab_size - (w % tab_size);
        } else if b == b'\r' {
            // ignore
        } else if b < 0x20 {
            // other ascii control: 0 width
        } else if b < 0x80 || (b & 0xc0) != 0x80 {
            // ascii printable or utf-8 lead byte: count as 1 col
            w += 1;
        }
    }
    w
}

/// `n_buckets`-bit mask: bit `i` set iff bucket `i` of this line contains any
/// non-whitespace char. Bucket index = visual_col / `bucket_cols`, clamped to
/// `n_buckets - 1`.
fn inky_buckets(line: &[u8], tab_size: u32, bucket_cols: u32, n_buckets: u32) -> u32 {
    let bucket_cols = bucket_cols.max(1);
    let max_bucket = n_buckets.saturating_sub(1);
    let mut mask: u32 = 0;
    let mut col: u32 = 0;
    for &b in line {
        if b == b'\t' {
            col += tab_size - (col % tab_size);
        } else if b == b'\r' {
            // ignore
        } else if b == b' ' {
            col += 1;
        } else if b < 0x20 {
            // skip
        } else if b < 0x80 || (b & 0xc0) != 0x80 {
            let bucket = (col / bucket_cols).min(max_bucket);
            mask |= 1 << bucket;
            col += 1;
        }
    }
    mask
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialise: `ascii_only()` is a process-wide atomic. Without this lock,
    // tests in different modes step on each other under cargo's parallel runner.
    static MODE_LOCK: Mutex<()> = Mutex::new(());

    fn unicode_mode() {
        edit::glyphs::set_ascii_only(false);
    }

    fn ascii_mode() {
        edit::glyphs::set_ascii_only(true);
    }

    #[test]
    fn empty_buffer_no_cells() {
        let _g = MODE_LOCK.lock().unwrap();
        unicode_mode();
        let mut m = MinimapState::new();
        m.rebuild(b"", 4, 2);
        assert_eq!(m.content_rows, 0);
        assert!(m.cells.is_empty());
    }

    #[test]
    fn one_line_one_cell_unicode() {
        let _g = MODE_LOCK.lock().unwrap();
        unicode_mode();
        let mut m = MinimapState::new();
        m.rebuild(b"abcd", 4, 2);
        assert_eq!(m.content_rows, 1);
        assert_eq!(m.cells.len(), 1);
        assert_eq!(m.cells[0].width, 2);
        // 4 chars over 4 buckets = bucket_cols=1, every dot-col inky on row 0.
        // dot-row 0 sets bit 0 (col 0) + bit 3 (col 1) per cell -> mask = 9.
        assert_eq!(m.cells[0].glyphs[0], glyphs::minimap_braille(0b0000_1001));
        assert_eq!(m.cells[0].glyphs[1], glyphs::minimap_braille(0b0000_1001));
    }

    #[test]
    fn ascii_mode_density_ramp_narrow() {
        let _g = MODE_LOCK.lock().unwrap();
        ascii_mode();
        let mut m = MinimapState::new();
        m.rebuild(b"a\nb\nc\nd", 4, 1);
        assert_eq!(m.cells.len(), 1);
        assert_eq!(m.cells[0].width, 1);
        assert_eq!(m.cells[0].glyphs[0], glyphs::minimap_ascii(4));
        unicode_mode();
    }

    #[test]
    fn ascii_mode_two_cells_split_horizontally() {
        // Two lines: left cell inky on row 0, right cell inky on row 1.
        // max_w = 5, n_buckets = 2, bucket_cols = ceil(5/2) = 3.
        // line "ab"  : col 0,1 -> bucket 0 (left)
        // line "    e" (4 spaces + e): col 4 -> bucket 1 (right)
        let _g = MODE_LOCK.lock().unwrap();
        ascii_mode();
        let mut m = MinimapState::new();
        m.rebuild(b"ab\n    e", 4, 2);
        assert_eq!(m.cells.len(), 1);
        assert_eq!(m.cells[0].width, 2);
        assert_eq!(m.cells[0].glyphs[0], glyphs::minimap_ascii(1));
        assert_eq!(m.cells[0].glyphs[1], glyphs::minimap_ascii(1));
        unicode_mode();
    }

    #[test]
    fn ascii_mode_blank_lines_drop_level() {
        let _g = MODE_LOCK.lock().unwrap();
        ascii_mode();
        let mut m = MinimapState::new();
        m.rebuild(b"a\n\nb\n", 4, 1);
        assert_eq!(m.cells.len(), 1);
        assert_eq!(m.cells[0].glyphs[0], glyphs::minimap_ascii(2));
        unicode_mode();
    }

    #[test]
    fn oversized_buffer_suppressed() {
        let _g = MODE_LOCK.lock().unwrap();
        unicode_mode();
        let mut m = MinimapState::new();
        let big = vec![b'x'; MAX_MINIMAP_BYTES + 1];
        m.rebuild(&big, 4, 2);
        assert!(m.is_suppressed());
        assert!(m.cells.is_empty());
    }

    #[test]
    fn whitespace_only_line_not_inky() {
        let _g = MODE_LOCK.lock().unwrap();
        ascii_mode();
        let mut m = MinimapState::new();
        m.rebuild(b"   \n\t\t\n   \n   ", 4, 1);
        assert_eq!(m.cells.len(), 1);
        assert_eq!(m.cells[0].glyphs[0], glyphs::minimap_ascii(0));
        unicode_mode();
    }

    #[test]
    fn tabs_expand_then_non_inky() {
        // "\tx" with tab_size=4: col 0..4 ws, col 4 inky.
        // max_w = 5, bucket_cols = ceil(5/4) = 2. inky col 4 -> bucket 2 (cell 1, dc 0).
        let _g = MODE_LOCK.lock().unwrap();
        unicode_mode();
        let mut m = MinimapState::new();
        m.rebuild(b"\tx", 4, 2);
        assert_eq!(m.max_visual_width, 5);
        assert_eq!(m.cells[0].glyphs[0], glyphs::minimap_braille(0));
        assert_eq!(m.cells[0].glyphs[1], glyphs::minimap_braille(0b0000_0001));
    }

    #[test]
    fn multi_row_buffer() {
        let _g = MODE_LOCK.lock().unwrap();
        unicode_mode();
        let mut m = MinimapState::new();
        m.rebuild(b"a\nb\nc\nd\ne", 4, 2);
        assert_eq!(m.content_rows, 5);
        assert_eq!(m.cells.len(), 2);
    }
}
