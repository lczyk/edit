//! Line-level Myers diff. Operates on byte slices split into lines and
//! produces a coalesced run-length op stream.

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum LineOp {
    Equal(u32),
    Insert(u32),
    Delete(u32),
}

/// Cap on Myers' D. Beyond this, diff bails and returns `None` -- caller
/// treats that as "too divergent, suppress marks".
const MAX_D: i32 = 5000;

pub fn split_lines(bytes: &[u8]) -> Vec<&[u8]> {
    if bytes.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut start = 0;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            out.push(&bytes[start..i]);
            start = i + 1;
        }
    }
    if start < bytes.len() {
        out.push(&bytes[start..]);
    }
    out
}

pub fn diff(a: &[&[u8]], b: &[&[u8]]) -> Option<Vec<LineOp>> {
    let n = a.len() as i32;
    let m = b.len() as i32;
    if n == 0 && m == 0 {
        return Some(Vec::new());
    }
    let max = (n + m).min(MAX_D);
    let offset = max.max(1);
    let v_len = (2 * offset + 1) as usize;
    let mut v = vec![0i32; v_len];
    let mut trace: Vec<Vec<i32>> = Vec::new();

    let mut found = false;
    'outer: for d in 0..=max {
        trace.push(v.clone());
        let mut k = -d;
        while k <= d {
            let idx_lo = (k - 1 + offset) as usize;
            let idx_hi = (k + 1 + offset) as usize;
            let mut x = if k == -d || (k != d && v[idx_lo] < v[idx_hi]) {
                v[idx_hi]
            } else {
                v[idx_lo] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[(k + offset) as usize] = x;
            if x >= n && y >= m {
                found = true;
                break 'outer;
            }
            k += 2;
        }
    }

    if !found {
        return None;
    }

    // Backtrack through the saved V snapshots.
    let mut ops: Vec<LineOp> = Vec::new();
    let mut x = n;
    let mut y = m;
    for d in (0..trace.len()).rev() {
        let prev = &trace[d];
        let d = d as i32;
        let k = x - y;
        let idx_lo = (k - 1 + offset) as usize;
        let idx_hi = (k + 1 + offset) as usize;
        let prev_k = if k == -d || (k != d && prev[idx_lo] < prev[idx_hi]) { k + 1 } else { k - 1 };
        let prev_x = prev[(prev_k + offset) as usize];
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            push_op(&mut ops, LineOp::Equal(1));
            x -= 1;
            y -= 1;
        }
        if d > 0 {
            if x == prev_x {
                push_op(&mut ops, LineOp::Insert(1));
                y -= 1;
            } else {
                push_op(&mut ops, LineOp::Delete(1));
                x -= 1;
            }
        }
    }

    ops.reverse();
    // After reversing, coalesce again -- the per-op pushes coalesce within
    // each D-step but reversal can leave neighbouring runs of the same kind.
    let mut coalesced: Vec<LineOp> = Vec::with_capacity(ops.len());
    for op in ops {
        push_op(&mut coalesced, op);
    }
    Some(coalesced)
}

fn push_op(out: &mut Vec<LineOp>, op: LineOp) {
    match (out.last_mut(), op) {
        (Some(LineOp::Equal(c)), LineOp::Equal(n)) => *c += n,
        (Some(LineOp::Insert(c)), LineOp::Insert(n)) => *c += n,
        (Some(LineOp::Delete(c)), LineOp::Delete(n)) => *c += n,
        _ => out.push(op),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<&[u8]> {
        split_lines(s.as_bytes())
    }

    #[test]
    fn empty_vs_empty() {
        assert_eq!(diff(&[], &[]), Some(vec![]));
    }

    #[test]
    fn pure_equal() {
        let a = lines("a\nb\nc");
        let b = lines("a\nb\nc");
        assert_eq!(diff(&a, &b), Some(vec![LineOp::Equal(3)]));
    }

    #[test]
    fn pure_insert() {
        let a = lines("");
        let b = lines("a\nb\n");
        assert_eq!(diff(&a, &b), Some(vec![LineOp::Insert(2)]));
    }

    #[test]
    fn pure_delete() {
        let a = lines("a\nb\n");
        let b = lines("");
        assert_eq!(diff(&a, &b), Some(vec![LineOp::Delete(2)]));
    }

    #[test]
    fn modify_middle_line() {
        let a = lines("a\nb\nc");
        let b = lines("a\nB\nc");
        let ops = diff(&a, &b).unwrap();
        // del b, ins B (or ins B, del b -- both valid; check shape)
        assert_eq!(ops.first(), Some(&LineOp::Equal(1)));
        assert_eq!(ops.last(), Some(&LineOp::Equal(1)));
        let middle: Vec<_> = ops[1..ops.len() - 1].to_vec();
        assert!(middle.contains(&LineOp::Delete(1)));
        assert!(middle.contains(&LineOp::Insert(1)));
    }

    #[test]
    fn append_at_end() {
        let a = lines("a\n");
        let b = lines("a\nb\n");
        assert_eq!(diff(&a, &b), Some(vec![LineOp::Equal(1), LineOp::Insert(1)]));
    }

    #[test]
    fn delete_at_end() {
        let a = lines("a\nb\n");
        let b = lines("a\n");
        assert_eq!(diff(&a, &b), Some(vec![LineOp::Equal(1), LineOp::Delete(1)]));
    }

    #[test]
    fn no_trailing_newline_handling() {
        // Last "line" without trailing \n still counts as a line.
        assert_eq!(split_lines(b"a"), vec![b"a" as &[u8]]);
        assert_eq!(split_lines(b"a\n"), vec![b"a" as &[u8]]);
        assert_eq!(split_lines(b"a\nb"), vec![b"a" as &[u8], b"b" as &[u8]]);
    }

    #[test]
    fn wildly_divergent_input_bails_instead_of_grinding() {
        // Myers is O(ND); past MAX_D the diff gives up and the caller falls
        // back to "no marks" rather than wait. Nothing else exercised that
        // branch -- and a regression there shows up as a hang, not a wrong
        // answer.
        let a: String = (0..6000).map(|i| format!("a{i}\n")).collect();
        let b: String = (0..6000).map(|i| format!("b{i}\n")).collect();
        let al = split_lines(a.as_bytes());
        let bl = split_lines(b.as_bytes());
        assert_eq!(diff(&al, &bl), None);
    }

    #[test]
    fn a_long_but_similar_file_still_diffs() {
        // The bail is on divergence, not size: same length, one changed line.
        let a: String = (0..6000).map(|i| format!("line {i}\n")).collect();
        let b = a.replace("line 3000\n", "changed\n");
        let al = split_lines(a.as_bytes());
        let bl = split_lines(b.as_bytes());
        let ops = diff(&al, &bl).expect("one changed line is well within MAX_D");
        assert_eq!(
            ops.iter().filter(|op| !matches!(op, LineOp::Equal(_))).count(),
            2,
            "expected one delete and one insert, got {ops:?}"
        );
    }
}
