//! Gutter rendering for `eat -n`. Same shape as `edit`'s margin: right-aligned
//! line number, then a vertical separator that gets recoloured (or replaced
//! with an ascii cue) per `GutterMark`. Marks come from `gutter::gutter_diff`
//! against a `git show HEAD:<rel>` baseline; missing baseline -> no marks,
//! plain numbers + dim separator.

use std::io::{self, Write};
use std::path::Path;

use gutter::GutterMark;
use gutter::gutter_diff::{self, BaselineState};

const SEP_UNICODE: &str = "\u{2502}"; // U+2502 BOX DRAWINGS LIGHT VERTICAL
const ANSI_DIM_GRAY: &str = "\x1b[38;5;240m";
const ANSI_GREEN: &str = "\x1b[32m";
const ANSI_YELLOW: &str = "\x1b[33m";
const ANSI_RED: &str = "\x1b[31m";
const ANSI_MAGENTA: &str = "\x1b[35m";
const ANSI_RESET: &str = "\x1b[m";

/// Per-line gutter context. Built once per file (or after each follow-tick
/// content change) by `compute`, then handed to `write_prefix` for each
/// emitted line.
pub struct Gutter {
    pub width: usize, // line-number column width (digits)
    pub marks: Vec<GutterMark>,
}

impl Gutter {
    /// Construct a Gutter from a path's full byte contents. If the file is
    /// inside a git repo and `git show HEAD:<rel>` succeeds, marks are
    /// computed; otherwise they're all `None`. `min_width` lets the caller
    /// reserve space for future appends (used by follow mode).
    pub fn compute(path: &Path, current: &[u8], min_width: usize) -> Self {
        let lines = count_lines(current);
        let baseline = BaselineState::load(path);
        let marks = match baseline.bytes {
            Some(b) if current.len() <= gutter_diff::MAX_DIFF_BYTES => {
                gutter_diff::compute_marks(&b, current, lines as u32)
            }
            _ => vec![GutterMark::None; lines],
        };
        let width = digit_width(lines).max(min_width);
        Self { width, marks }
    }

    /// Get the mark for a 1-indexed line number; `None` for out-of-range.
    pub fn mark(&self, line_no: usize) -> GutterMark {
        if line_no == 0 {
            return GutterMark::None;
        }
        self.marks.get(line_no - 1).copied().unwrap_or(GutterMark::None)
    }
}

/// Write the gutter prefix `{number} {sep} ` for `line_no`. `width` is the
/// digit-column width; `mark` recolours / replaces the separator.
///
/// Color/no-color split:
///   - color on: separator is U+2502 recoloured per mark (default = dim gray,
///     Added = green, Modified = yellow, Conflict = magenta, DeletedAbove/Below
///     = red with the glyph swapped to U+25B4 / U+25BE).
///   - color off: separator is replaced with a distinct ascii char per mark
///     (`|` for None; `+`/`~`/`^`/`v`/`!` for the rest), so the cue survives
///     in non-tty pipes.
pub fn write_prefix(
    writer: &mut dyn Write,
    line_no: usize,
    width: usize,
    mark: GutterMark,
    use_color: bool,
) -> io::Result<()> {
    let (sep, color) = sep_and_color(mark, use_color);
    if use_color {
        write!(writer, "{ANSI_DIM_GRAY}{:>width$}{ANSI_RESET} {color}{sep}{ANSI_RESET} ", line_no)
    } else {
        write!(writer, "{:>width$} {sep} ", line_no)
    }
}

fn sep_and_color(mark: GutterMark, use_color: bool) -> (&'static str, &'static str) {
    if use_color {
        match mark {
            GutterMark::None => (SEP_UNICODE, ANSI_DIM_GRAY),
            GutterMark::Added => (SEP_UNICODE, ANSI_GREEN),
            GutterMark::Modified => (SEP_UNICODE, ANSI_YELLOW),
            GutterMark::DeletedAbove => ("\u{25B4}", ANSI_RED), // small upward triangle
            GutterMark::DeletedBelow => ("\u{25BE}", ANSI_RED), // small downward triangle
            GutterMark::Conflict => (SEP_UNICODE, ANSI_MAGENTA),
        }
    } else {
        let g = match mark {
            GutterMark::None => "|",
            GutterMark::Added => "+",
            GutterMark::Modified => "~",
            GutterMark::DeletedAbove => "^",
            GutterMark::DeletedBelow => "v",
            GutterMark::Conflict => "!",
        };
        (g, "")
    }
}

/// Width of the largest line number's decimal representation, with a
/// minimum of 1 for the empty case.
fn digit_width(n: usize) -> usize {
    if n < 10 { 1 } else { (n as f64).log10().floor() as usize + 1 }
}

/// Count `\n`-terminated lines, treating a trailing partial line as a
/// full line (matches `linediff::split_lines`).
fn count_lines(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 0;
    }
    let mut n = 0;
    let mut last_was_nl = false;
    for &b in bytes {
        if b == b'\n' {
            n += 1;
            last_was_nl = true;
        } else {
            last_was_nl = false;
        }
    }
    if !last_was_nl {
        n += 1;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(line_no: usize, width: usize, mark: GutterMark, use_color: bool) -> String {
        let mut buf = Vec::new();
        write_prefix(&mut buf, line_no, width, mark, use_color).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn no_color_format_matches_edit_shape() {
        // {n:>width$} {sep} \space
        assert_eq!(render(1, 3, GutterMark::None, false), "  1 | ");
        assert_eq!(render(42, 3, GutterMark::None, false), " 42 | ");
        assert_eq!(render(7, 1, GutterMark::None, false), "7 | ");
    }

    #[test]
    fn no_color_marks_use_distinct_ascii() {
        assert_eq!(render(1, 1, GutterMark::Added, false), "1 + ");
        assert_eq!(render(1, 1, GutterMark::Modified, false), "1 ~ ");
        assert_eq!(render(1, 1, GutterMark::DeletedAbove, false), "1 ^ ");
        assert_eq!(render(1, 1, GutterMark::DeletedBelow, false), "1 v ");
        assert_eq!(render(1, 1, GutterMark::Conflict, false), "1 ! ");
    }

    #[test]
    fn colored_separator_recolours_per_mark() {
        let none = render(1, 1, GutterMark::None, true);
        let added = render(1, 1, GutterMark::Added, true);
        // both contain the same separator char, but with different colour escapes.
        assert!(none.contains("\u{2502}"));
        assert!(added.contains("\u{2502}"));
        assert!(added.contains("\x1b[32m"));
        assert!(none.contains("\x1b[38;5;240m"));
    }

    #[test]
    fn colored_deleted_uses_arrow_glyph() {
        let above = render(1, 1, GutterMark::DeletedAbove, true);
        let below = render(1, 1, GutterMark::DeletedBelow, true);
        assert!(above.contains("\u{25B4}"));
        assert!(below.contains("\u{25BE}"));
        assert!(above.contains("\x1b[31m"));
    }

    #[test]
    fn count_lines_handles_no_trailing_newline() {
        assert_eq!(count_lines(b""), 0);
        assert_eq!(count_lines(b"a"), 1);
        assert_eq!(count_lines(b"a\n"), 1);
        assert_eq!(count_lines(b"a\nb"), 2);
        assert_eq!(count_lines(b"a\nb\n"), 2);
    }

    #[test]
    fn digit_width_matches_log10_floor_plus_one() {
        assert_eq!(digit_width(0), 1);
        assert_eq!(digit_width(1), 1);
        assert_eq!(digit_width(9), 1);
        assert_eq!(digit_width(10), 2);
        assert_eq!(digit_width(99), 2);
        assert_eq!(digit_width(100), 3);
    }

    #[test]
    fn gutter_mark_lookup_is_one_indexed() {
        let g = Gutter {
            width: 1,
            marks: vec![GutterMark::Added, GutterMark::Modified, GutterMark::None],
        };
        assert_eq!(g.mark(0), GutterMark::None); // out of range
        assert_eq!(g.mark(1), GutterMark::Added);
        assert_eq!(g.mark(2), GutterMark::Modified);
        assert_eq!(g.mark(4), GutterMark::None); // beyond
    }
}
