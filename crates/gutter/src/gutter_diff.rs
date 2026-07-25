//! Orchestrates the per-line gutter marks: holds the cached baseline blob
//! and computes a `Vec<GutterMark>` from a fresh diff against the current
//! buffer contents.

use std::path::Path;

use crate::GutterMark;
use crate::git;
use crate::linediff::{self, LineOp};

pub const MAX_DIFF_BYTES: usize = 5 * 1024 * 1024;

#[derive(Debug)]
pub struct BaselineState {
    pub bytes: Option<Vec<u8>>,
}

impl BaselineState {
    pub fn load(path: &Path) -> Self {
        let Ok(info) = git::locate(path) else {
            return Self { bytes: None };
        };
        let Ok(bytes) = git::read_baseline(&info) else {
            return Self { bytes: None };
        };
        if bytes.len() > MAX_DIFF_BYTES || is_binary(&bytes) {
            return Self { bytes: None };
        }
        Self { bytes: Some(bytes) }
    }
}

fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8 * 1024).any(|&b| b == 0)
}

pub fn compute_marks(baseline: &[u8], current: &[u8], current_lines: u32) -> Vec<GutterMark> {
    let a = linediff::split_lines(baseline);
    let b = linediff::split_lines(current);
    let Some(ops) = linediff::diff(&a, &b) else {
        return vec![GutterMark::None; current_lines as usize];
    };
    marks_from_ops(&ops, current_lines)
}

fn marks_from_ops(ops: &[LineOp], current_lines: u32) -> Vec<GutterMark> {
    let mut marks = vec![GutterMark::None; current_lines as usize];
    let mut y: u32 = 0;
    let mut i = 0;
    while i < ops.len() {
        match ops[i] {
            LineOp::Equal(n) => {
                y += n;
                i += 1;
            }
            LineOp::Insert(n) => {
                for k in 0..n {
                    set_mark(&mut marks, y + k, GutterMark::Added);
                }
                y += n;
                i += 1;
            }
            LineOp::Delete(n) => {
                // Look ahead: a Delete adjacent to an Insert collapses into
                // Modified for the overlapping prefix.
                if let Some(LineOp::Insert(m)) = ops.get(i + 1).copied() {
                    let overlap = n.min(m);
                    for k in 0..overlap {
                        set_mark(&mut marks, y + k, GutterMark::Modified);
                    }
                    if m > n {
                        for k in n..m {
                            set_mark(&mut marks, y + k, GutterMark::Added);
                        }
                    } else if n > m {
                        // Surplus delete attaches to the next current line
                        // (or previous, at EOF).
                        let attach_to = y + m;
                        if (attach_to as usize) < marks.len() {
                            set_mark(&mut marks, attach_to, GutterMark::DeletedAbove);
                        } else if attach_to > 0 {
                            set_mark(&mut marks, attach_to - 1, GutterMark::DeletedBelow);
                        }
                    }
                    y += m;
                    i += 2;
                } else {
                    let attach_to = y;
                    if (attach_to as usize) < marks.len() {
                        set_mark(&mut marks, attach_to, GutterMark::DeletedAbove);
                    } else if attach_to > 0 {
                        set_mark(&mut marks, attach_to - 1, GutterMark::DeletedBelow);
                    }
                    i += 1;
                }
            }
        }
    }
    // `y` walks the current side of the diff. Overshooting `current_lines`
    // means `set_mark` dropped marks on the floor and the margin is quietly
    // wrong for the rest of the file rather than obviously broken.
    //
    // Undershooting is normal and not checked: a buffer ending in a newline
    // shows a final empty line that `split_lines` does not produce, so the
    // caller legitimately asks for one more mark than the diff describes.
    stdext::sanity_check!(
        gutter_marks_cover_the_buffer,
        y <= current_lines,
        "op stream covers {y} lines, caller said {current_lines}"
    );

    marks
}

fn set_mark(marks: &mut [GutterMark], idx: u32, mark: GutterMark) {
    if let Some(slot) = marks.get_mut(idx as usize) {
        // DeletedAbove wins over DeletedBelow if both land on the same line
        // (matches vscode); other kinds just overwrite.
        if matches!(mark, GutterMark::DeletedBelow) && matches!(*slot, GutterMark::DeletedAbove) {
            return;
        }
        *slot = mark;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marks(baseline: &str, current: &str) -> Vec<GutterMark> {
        let lines = linediff::split_lines(current.as_bytes()).len() as u32;
        compute_marks(baseline.as_bytes(), current.as_bytes(), lines)
    }

    #[test]
    fn equal_means_none() {
        assert_eq!(marks("a\nb\nc", "a\nb\nc"), vec![GutterMark::None; 3]);
    }

    #[test]
    fn pure_add() {
        assert_eq!(
            marks("", "a\nb\nc"),
            vec![GutterMark::Added, GutterMark::Added, GutterMark::Added]
        );
    }

    #[test]
    fn pure_delete_attaches_at_eof() {
        // baseline 3 lines, current empty -> DeletedAbove on... wait,
        // current has 0 lines so there's nowhere to attach. Marks vec is
        // empty.
        assert_eq!(marks("a\nb\nc", ""), Vec::<GutterMark>::new());
    }

    #[test]
    fn delete_at_end_attaches_below_last() {
        let m = marks("a\nb\nc", "a\nb");
        assert_eq!(m, vec![GutterMark::None, GutterMark::DeletedBelow]);
    }

    #[test]
    fn delete_at_top_attaches_above_first() {
        let m = marks("a\nb\nc", "b\nc");
        assert_eq!(m, vec![GutterMark::DeletedAbove, GutterMark::None]);
    }

    #[test]
    fn modify_one_line_is_modified() {
        let m = marks("a\nb\nc", "a\nB\nc");
        assert_eq!(m, vec![GutterMark::None, GutterMark::Modified, GutterMark::None]);
    }

    #[test]
    fn del2_ins3_is_two_modified_one_added() {
        // baseline: a b c d, current: a X Y Z d
        // delete b,c (2) + insert X,Y,Z (3) -> 2 modified + 1 added
        let m = marks("a\nb\nc\nd", "a\nX\nY\nZ\nd");
        assert_eq!(
            m,
            vec![
                GutterMark::None,
                GutterMark::Modified,
                GutterMark::Modified,
                GutterMark::Added,
                GutterMark::None,
            ]
        );
    }

    #[test]
    fn too_divergent_yields_no_marks_at_the_right_length() {
        // `diff` bails past MAX_D; the fallback must still hand back one mark
        // per current line, or the caller indexes into a short vec.
        let baseline: String = (0..6000).map(|i| format!("a{i}\n")).collect();
        let current: String = (0..6000).map(|i| format!("b{i}\n")).collect();
        let lines = linediff::split_lines(current.as_bytes()).len() as u32;
        let got = compute_marks(baseline.as_bytes(), current.as_bytes(), lines);
        assert_eq!(got.len(), lines as usize);
        assert!(got.iter().all(|m| *m == GutterMark::None), "expected marks suppressed");
    }

    #[test]
    fn binary_content_is_recognised() {
        // A NUL anywhere in the first 8 KiB suppresses the baseline, so a
        // binary file does not get a line diff run over it.
        assert!(is_binary(b"\x7fELF\0\0\0"));
        assert!(is_binary(&[b'a'; 4096].iter().copied().chain([0]).collect::<Vec<_>>()));
        assert!(!is_binary(b"plain text\nwith lines\n"));
        assert!(!is_binary(b""));
        // Past the sampled window it reads as text, by design.
        let mut late = vec![b'a'; 8 * 1024];
        late.push(0);
        assert!(!is_binary(&late));
    }

    #[test]
    fn a_mark_is_produced_for_every_current_line() {
        // The op walk indexes marks by line, and `set_mark` drops anything out
        // of range. Whatever the shape of the change, the length has to match
        // and the marks have to be in range for the caller to trust either.
        let cases: &[(&str, &str)] = &[
            ("", ""),
            ("a\n", ""),
            ("", "a\n"),
            ("a\nb\nc\n", "a\nb\nc\n"),
            ("a\nb\nc\n", "c\nb\na\n"),
            ("a\nb\nc\nd\ne\n", "a\nX\ne\n"),
            ("a\n", "a\nb\nc\nd\ne\n"),
            ("one\ntwo", "one\ntwo\nthree"),
        ];
        for (baseline, current) in cases {
            let lines = linediff::split_lines(current.as_bytes()).len() as u32;
            let got = compute_marks(baseline.as_bytes(), current.as_bytes(), lines);
            assert_eq!(got.len(), lines as usize, "{baseline:?} -> {current:?}");
        }
    }

    #[cfg(feature = "sanity")]
    #[test]
    fn the_coverage_check_stays_quiet_on_real_diffs() {
        use stdext::sanity::capture;

        let ((), msgs) = capture::trips(|| {
            for (baseline, current) in [
                ("", "a\nb\n"),
                ("a\nb\nc\n", "a\nB\nc\n"),
                ("a\nb\nc\nd\ne\n", "a\nX\ne\n"),
                ("a\nb\nc\n", ""),
            ] {
                let lines = linediff::split_lines(current.as_bytes()).len() as u32;
                _ = compute_marks(baseline.as_bytes(), current.as_bytes(), lines);
            }
        });
        assert!(msgs.is_empty(), "{msgs:?}");
    }

    #[cfg(feature = "sanity")]
    #[test]
    fn a_line_count_short_of_the_diff_trips_the_coverage_check() {
        use stdext::sanity::capture;

        // What the check is for: the op stream describes more lines than the
        // caller left room for, so marks past the end are discarded.
        let ((), msgs) = capture::trips(|| {
            _ = compute_marks(b"", b"a\nb\nc\n", 1);
        });
        assert!(capture::fired(&msgs, "gutter_marks_cover_the_buffer"), "{msgs:?}");
    }

    #[cfg(feature = "sanity")]
    #[test]
    fn a_trailing_newline_asking_for_one_extra_mark_is_fine() {
        use stdext::sanity::capture;

        // The editor counts the empty line after a final newline, which
        // `split_lines` does not produce. Asking for that extra mark is normal
        // and must stay quiet -- this fired on four PTY tests when the check
        // demanded exact equality.
        let ((), msgs) = capture::trips(|| {
            _ = compute_marks(b"a\nb\nc\n", b"a\nb\nc\n", 4);
        });
        assert!(msgs.is_empty(), "{msgs:?}");
    }

    #[test]
    fn del3_ins1_is_one_modified_one_deleted_above() {
        // baseline: a b c d e, current: a X e
        // delete b,c,d (3) + insert X (1) -> 1 modified + 2-line surplus delete attaches above 'e'
        let m = marks("a\nb\nc\nd\ne", "a\nX\ne");
        assert_eq!(m, vec![GutterMark::None, GutterMark::Modified, GutterMark::DeletedAbove]);
    }
}
