// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Line-granular diff used by diff-mode editing.
//!
//! Myers' O(ND) algorithm over line hashes, with a `d` cap beyond which we
//! bail to a whole-file replace. CRLF is normalized for comparison; byte
//! ranges refer to the original input (terminators included).

use std::ops::Range;

use edit::hash::hash;

/// A single line from the source slice.
#[derive(Clone, Debug)]
pub struct Line<'a> {
    /// Byte range in the source, including any trailing `\r` / `\n`.
    pub range: Range<usize>,
    /// Line hash after stripping the trailing newline (and preceding `\r`).
    pub hash: u64,
    /// Line bytes without the trailing newline. Kept for equality checks
    /// in the presence of hash collisions and for consumers that need the text.
    pub bytes: &'a [u8],
}

/// A coalesced diff operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LineOp {
    Equal { baseline: usize, current: usize, count: usize },
    Delete { baseline: usize, count: usize },
    Insert { current: usize, count: usize },
}

/// Maximum `d` (edit distance) we're willing to explore. Beyond this we
/// bail to a whole-file replace. Tuned to keep worst-case work bounded for
/// files up to the diff-mode size cap (~5 MB).
const MYERS_MAX_D: usize = 100_000;

/// Splits `src` into lines. The returned `Line::range` values cover the
/// entire input with no gaps, each ending just past its `\n` terminator
/// (or at the end of input for a final unterminated line).
pub fn split_lines(src: &[u8]) -> Vec<Line<'_>> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < src.len() {
        if src[i] == b'\n' {
            let end = i + 1;
            let content_end = if i > start && src[i - 1] == b'\r' { i - 1 } else { i };
            let bytes = &src[start..content_end];
            out.push(Line { range: start..end, hash: hash(0, bytes), bytes });
            start = end;
            i = end;
        } else {
            i += 1;
        }
    }
    if start < src.len() {
        let bytes = &src[start..];
        out.push(Line { range: start..src.len(), hash: hash(0, bytes), bytes });
    }
    out
}

/// Computes a line-level diff from `baseline` to `current`. Operations are
/// emitted in source order and coalesced (no two adjacent ops share a kind).
pub fn diff(baseline: &[Line<'_>], current: &[Line<'_>]) -> Vec<LineOp> {
    let n = baseline.len();
    let m = current.len();

    if n == 0 && m == 0 {
        return Vec::new();
    }
    if n == 0 {
        return vec![LineOp::Insert { current: 0, count: m }];
    }
    if m == 0 {
        return vec![LineOp::Delete { baseline: 0, count: n }];
    }

    match myers_trace(baseline, current) {
        Some(trace) => coalesce(myers_backtrack(&trace, baseline, current)),
        None => {
            vec![LineOp::Delete { baseline: 0, count: n }, LineOp::Insert { current: 0, count: m }]
        }
    }
}

fn lines_equal(a: &Line<'_>, b: &Line<'_>) -> bool {
    a.hash == b.hash && a.bytes == b.bytes
}

fn myers_trace(a: &[Line<'_>], b: &[Line<'_>]) -> Option<Vec<Vec<i32>>> {
    let n = a.len() as i32;
    let m = b.len() as i32;
    let max = (n + m) as usize;
    let cap = MYERS_MAX_D.min(max);

    let offset = cap as i32;
    let v_len = 2 * cap + 1;
    // All zeros is a safe initialization: the only cell actually read before
    // any write is V[1] (= v[offset + 1]) at d = 0, k = 0, and it must be 0.
    // Other cells are only read after being written thanks to the k-parity
    // invariant of the algorithm.
    let mut v = vec![0i32; v_len];
    let mut trace = Vec::with_capacity(cap + 1);

    for d in 0..=cap as i32 {
        for k in (-d..=d).step_by(2) {
            let from_down =
                k == -d || (k != d && v[(k - 1 + offset) as usize] < v[(k + 1 + offset) as usize]);
            let mut x = if from_down {
                v[(k + 1 + offset) as usize]
            } else {
                v[(k - 1 + offset) as usize] + 1
            };
            let mut y = x - k;
            while x < n && y < m && lines_equal(&a[x as usize], &b[y as usize]) {
                x += 1;
                y += 1;
            }
            v[(k + offset) as usize] = x;
            if x >= n && y >= m {
                trace.push(v.clone());
                return Some(trace);
            }
        }
        trace.push(v.clone());
    }
    None
}

#[derive(Clone, Copy)]
enum Step {
    Equal,
    Delete,
    Insert,
}

fn myers_backtrack(trace: &[Vec<i32>], a: &[Line<'_>], b: &[Line<'_>]) -> Vec<Step> {
    let n = a.len() as i32;
    let m = b.len() as i32;
    let cap = trace.len() - 1;
    let offset = (trace[0].len() / 2) as i32;

    let mut x = n;
    let mut y = m;
    let mut steps = Vec::with_capacity((n + m) as usize);

    for d in (1..=cap as i32).rev() {
        let v = &trace[d as usize];
        let k = x - y;
        let from_down =
            k == -d || (k != d && v[(k - 1 + offset) as usize] < v[(k + 1 + offset) as usize]);
        let prev_k = if from_down { k + 1 } else { k - 1 };
        let prev_x = v[(prev_k + offset) as usize];
        let prev_y = prev_x - prev_k;

        while x > prev_x && y > prev_y {
            steps.push(Step::Equal);
            x -= 1;
            y -= 1;
        }
        if d > 0 {
            if from_down {
                steps.push(Step::Insert);
                y -= 1;
            } else {
                steps.push(Step::Delete);
                x -= 1;
            }
        }
    }

    while x > 0 && y > 0 {
        steps.push(Step::Equal);
        x -= 1;
        y -= 1;
    }
    while x > 0 {
        steps.push(Step::Delete);
        x -= 1;
    }
    while y > 0 {
        steps.push(Step::Insert);
        y -= 1;
    }

    steps.reverse();
    steps
}

fn coalesce(steps: Vec<Step>) -> Vec<LineOp> {
    let mut out: Vec<LineOp> = Vec::new();
    let mut ai = 0usize;
    let mut bi = 0usize;

    for step in steps {
        match step {
            Step::Equal => {
                if let Some(LineOp::Equal { count, .. }) = out.last_mut() {
                    *count += 1;
                } else {
                    out.push(LineOp::Equal { baseline: ai, current: bi, count: 1 });
                }
                ai += 1;
                bi += 1;
            }
            Step::Delete => {
                if let Some(LineOp::Delete { count, .. }) = out.last_mut() {
                    *count += 1;
                } else {
                    out.push(LineOp::Delete { baseline: ai, count: 1 });
                }
                ai += 1;
            }
            Step::Insert => {
                if let Some(LineOp::Insert { count, .. }) = out.last_mut() {
                    *count += 1;
                } else {
                    out.push(LineOp::Insert { current: bi, count: 1 });
                }
                bi += 1;
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ops(baseline: &[u8], current: &[u8]) -> Vec<LineOp> {
        let a = split_lines(baseline);
        let b = split_lines(current);
        diff(&a, &b)
    }

    #[test]
    fn split_basic() {
        let lines = split_lines(b"a\nb\nc");
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].range, 0..2);
        assert_eq!(lines[0].bytes, b"a");
        assert_eq!(lines[1].range, 2..4);
        assert_eq!(lines[2].range, 4..5);
        assert_eq!(lines[2].bytes, b"c");
    }

    #[test]
    fn split_empty() {
        assert!(split_lines(b"").is_empty());
    }

    #[test]
    fn split_trailing_newline() {
        let lines = split_lines(b"a\nb\n");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].range, 2..4);
        assert_eq!(lines[1].bytes, b"b");
    }

    #[test]
    fn split_crlf_normalizes_hash() {
        let lf = split_lines(b"hello\nworld\n");
        let crlf = split_lines(b"hello\r\nworld\r\n");
        assert_eq!(lf[0].hash, crlf[0].hash);
        assert_eq!(lf[1].hash, crlf[1].hash);
        assert_eq!(crlf[0].range, 0..7);
        assert_eq!(crlf[0].bytes, b"hello");
    }

    #[test]
    fn diff_equal() {
        let o = ops(b"a\nb\nc\n", b"a\nb\nc\n");
        assert_eq!(o, vec![LineOp::Equal { baseline: 0, current: 0, count: 3 }]);
    }

    #[test]
    fn diff_empty_both() {
        assert!(ops(b"", b"").is_empty());
    }

    #[test]
    fn diff_pure_insert() {
        let o = ops(b"", b"a\nb\n");
        assert_eq!(o, vec![LineOp::Insert { current: 0, count: 2 }]);
    }

    #[test]
    fn diff_pure_delete() {
        let o = ops(b"a\nb\n", b"");
        assert_eq!(o, vec![LineOp::Delete { baseline: 0, count: 2 }]);
    }

    #[test]
    fn diff_insert_middle() {
        let o = ops(b"a\nc\n", b"a\nb\nc\n");
        assert_eq!(
            o,
            vec![
                LineOp::Equal { baseline: 0, current: 0, count: 1 },
                LineOp::Insert { current: 1, count: 1 },
                LineOp::Equal { baseline: 1, current: 2, count: 1 },
            ]
        );
    }

    #[test]
    fn diff_delete_middle() {
        let o = ops(b"a\nb\nc\n", b"a\nc\n");
        assert_eq!(
            o,
            vec![
                LineOp::Equal { baseline: 0, current: 0, count: 1 },
                LineOp::Delete { baseline: 1, count: 1 },
                LineOp::Equal { baseline: 2, current: 1, count: 1 },
            ]
        );
    }

    #[test]
    fn diff_replace_middle() {
        let o = ops(b"a\nb\nc\n", b"a\nB\nc\n");
        assert_eq!(
            o,
            vec![
                LineOp::Equal { baseline: 0, current: 0, count: 1 },
                LineOp::Delete { baseline: 1, count: 1 },
                LineOp::Insert { current: 1, count: 1 },
                LineOp::Equal { baseline: 2, current: 2, count: 1 },
            ]
        );
    }

    #[test]
    fn diff_crlf_vs_lf_equal() {
        let o = ops(b"a\r\nb\r\n", b"a\nb\n");
        assert_eq!(o, vec![LineOp::Equal { baseline: 0, current: 0, count: 2 }]);
    }

    #[test]
    fn diff_trailing_newline_ignored() {
        // Content matches line-for-line once trailing terminators are stripped,
        // so the diff collapses to a single Equal block.
        let o = ops(b"a\nb", b"a\nb\n");
        assert_eq!(o, vec![LineOp::Equal { baseline: 0, current: 0, count: 2 }]);
    }

    #[test]
    fn diff_whole_replace() {
        let o = ops(b"a\nb\nc\n", b"x\ny\nz\n");
        assert_eq!(
            o,
            vec![LineOp::Delete { baseline: 0, count: 3 }, LineOp::Insert { current: 0, count: 3 }]
        );
    }

    #[test]
    fn diff_multi_block() {
        let o = ops(b"a\nb\nc\nd\ne\n", b"a\nB\nc\nD\ne\n");
        assert_eq!(
            o,
            vec![
                LineOp::Equal { baseline: 0, current: 0, count: 1 },
                LineOp::Delete { baseline: 1, count: 1 },
                LineOp::Insert { current: 1, count: 1 },
                LineOp::Equal { baseline: 2, current: 2, count: 1 },
                LineOp::Delete { baseline: 3, count: 1 },
                LineOp::Insert { current: 3, count: 1 },
                LineOp::Equal { baseline: 4, current: 4, count: 1 },
            ]
        );
    }

    #[test]
    fn diff_indices_are_valid() {
        let a = split_lines(b"a\nb\nc\nd\n");
        let b = split_lines(b"a\nX\nc\nY\nd\n");
        let o = diff(&a, &b);
        let mut ai = 0;
        let mut bi = 0;
        for op in &o {
            match *op {
                LineOp::Equal { baseline, current, count } => {
                    assert_eq!(baseline, ai);
                    assert_eq!(current, bi);
                    ai += count;
                    bi += count;
                }
                LineOp::Delete { baseline, count } => {
                    assert_eq!(baseline, ai);
                    ai += count;
                }
                LineOp::Insert { current, count } => {
                    assert_eq!(current, bi);
                    bi += count;
                }
            }
        }
        assert_eq!(ai, a.len());
        assert_eq!(bi, b.len());
    }
}
