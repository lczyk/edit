// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Orchestrates diff-mode editing.
//!
//! Builds a view of the current file interleaved with baseline-deleted
//! lines (stripes), preserving the rest as editable. Deleted stripes are
//! installed as [`TextBuffer`] locked ranges and tinted via line
//! decorations. Saving and re-diffing extract the editable content by
//! skipping the locked byte ranges.

use std::ops::Range;

use edit::buffer::{LineDecoration, TextBuffer};

use crate::linediff::{self, LineOp};

/// Hard cap on the size of either side fed to the diff algorithm. Files
/// above this are refused entry to diff mode. Myers is O(ND) memory; with
/// [`linediff::MYERS_MAX_D`] we keep worst-case behaviour bounded.
pub const MAX_DIFF_BYTES: usize = 5 * 1024 * 1024;

/// Minimum idle gap between re-diff passes, in milliseconds.
pub const REDIFF_DEBOUNCE_MS: u64 = 150;

pub struct DiffState {
    pub baseline: Vec<u8>,
}

pub struct BuiltView {
    pub bytes: Vec<u8>,
    pub locked_ranges: Vec<Range<usize>>,
    pub decorations: Vec<LineDecoration>,
}

impl DiffState {
    pub fn new(baseline: Vec<u8>) -> Self {
        Self { baseline }
    }

    /// Builds a view of `current` against the baseline. The view interleaves
    /// baseline-deleted lines as locked stripes.
    pub fn build_view(&self, current: &[u8]) -> BuiltView {
        let baseline = ensure_terminated(&self.baseline);
        let current = ensure_terminated(current);

        let a = linediff::split_lines(&baseline);
        let b = linediff::split_lines(&current);
        let ops = linediff::diff(&a, &b);

        let mut bytes: Vec<u8> = Vec::with_capacity(baseline.len() + current.len());
        let mut locked: Vec<Range<usize>> = Vec::new();
        let mut decorations: Vec<LineDecoration> = Vec::new();

        for op in ops {
            match op {
                LineOp::Equal { current: c, count, .. } => {
                    for i in 0..count {
                        let line = &b[c + i];
                        bytes.extend_from_slice(&current[line.range.clone()]);
                        decorations.push(LineDecoration::None);
                    }
                }
                LineOp::Insert { current: c, count } => {
                    for i in 0..count {
                        let line = &b[c + i];
                        bytes.extend_from_slice(&current[line.range.clone()]);
                        decorations.push(LineDecoration::Added);
                    }
                }
                LineOp::Delete { baseline: bi, count } => {
                    let stripe_start = bytes.len();
                    for i in 0..count {
                        let line = &a[bi + i];
                        bytes.extend_from_slice(&baseline[line.range.clone()]);
                        decorations.push(LineDecoration::Deleted);
                    }
                    let stripe_end = bytes.len();
                    if stripe_end > stripe_start {
                        locked.push(stripe_start..stripe_end);
                    }
                }
            }
        }

        BuiltView { bytes, locked_ranges: locked, decorations }
    }
}

/// Collects the editable bytes from `buf` by concatenating everything
/// outside of its locked ranges. Equivalent to "skip the deleted stripes".
pub fn extract_real(buf: &TextBuffer) -> Vec<u8> {
    let total = buf.text_length();
    let locked = buf.locked_ranges();
    let mut out = Vec::with_capacity(total);
    let mut cursor = 0;
    for r in locked {
        append_range(&mut out, buf, cursor..r.start);
        cursor = r.end;
    }
    append_range(&mut out, buf, cursor..total);
    out
}

fn append_range(out: &mut Vec<u8>, buf: &TextBuffer, range: Range<usize>) {
    let end = range.end.min(buf.text_length());
    let mut off = range.start.min(end);
    while off < end {
        let chunk = buf.read_forward(off);
        if chunk.is_empty() {
            break;
        }
        let take = chunk.len().min(end - off);
        out.extend_from_slice(&chunk[..take]);
        off += take;
    }
}

/// Converts a view-buffer byte offset into a real-content byte offset. If
/// the offset falls inside a locked range it anchors to the byte just
/// before the range (so cursors in stripes snap to the prior editable
/// position after rebuild).
pub fn view_to_real_off(view_off: usize, locked: &[Range<usize>]) -> usize {
    let mut real = 0;
    let mut last_end = 0;
    for r in locked {
        if view_off <= r.start {
            return real + (view_off - last_end);
        }
        if view_off < r.end {
            return real + (r.start - last_end);
        }
        real += r.start - last_end;
        last_end = r.end;
    }
    real + view_off.saturating_sub(last_end)
}

/// Converts a real-content byte offset into a view-buffer byte offset,
/// skipping over locked ranges. Clamped to `view_len`.
pub fn real_to_view_off(real_off: usize, locked: &[Range<usize>], view_len: usize) -> usize {
    let mut consumed = 0;
    let mut view_cursor = 0;
    for r in locked {
        let free_len = r.start - view_cursor;
        if consumed + free_len >= real_off {
            return view_cursor + (real_off - consumed);
        }
        consumed += free_len;
        view_cursor = r.end;
    }
    (view_cursor + (real_off - consumed)).min(view_len)
}

fn ensure_terminated(bytes: &[u8]) -> Vec<u8> {
    if bytes.is_empty() || bytes.ends_with(b"\n") {
        return bytes.to_vec();
    }
    let mut v = Vec::with_capacity(bytes.len() + 1);
    v.extend_from_slice(bytes);
    v.push(b'\n');
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install(buf: &mut TextBuffer, view: &BuiltView) {
        buf.set_locked_content(&view.bytes, view.locked_ranges.clone());
        buf.set_line_decorations(view.decorations.clone());
    }

    fn new_buffer() -> TextBuffer {
        TextBuffer::new(true).unwrap()
    }

    #[test]
    fn pure_equal_produces_no_locks() {
        let state = DiffState::new(b"a\nb\nc\n".to_vec());
        let view = state.build_view(b"a\nb\nc\n");
        assert_eq!(view.bytes, b"a\nb\nc\n");
        assert!(view.locked_ranges.is_empty());
        assert_eq!(view.decorations, vec![LineDecoration::None; 3]);
    }

    #[test]
    fn deleted_line_becomes_locked_stripe() {
        let state = DiffState::new(b"a\nb\nc\n".to_vec());
        let view = state.build_view(b"a\nc\n");
        // View interleaves the deleted `b\n` between the kept `a\n` and `c\n`.
        assert_eq!(view.bytes, b"a\nb\nc\n");
        assert_eq!(view.locked_ranges, vec![2..4]);
        assert_eq!(
            view.decorations,
            vec![LineDecoration::None, LineDecoration::Deleted, LineDecoration::None]
        );
    }

    #[test]
    fn inserted_line_is_marked_added() {
        let state = DiffState::new(b"a\nc\n".to_vec());
        let view = state.build_view(b"a\nb\nc\n");
        assert_eq!(view.bytes, b"a\nb\nc\n");
        assert!(view.locked_ranges.is_empty());
        assert_eq!(
            view.decorations,
            vec![LineDecoration::None, LineDecoration::Added, LineDecoration::None]
        );
    }

    #[test]
    fn adjacent_deletes_merge_into_single_stripe() {
        let state = DiffState::new(b"a\nb\nc\nd\n".to_vec());
        let view = state.build_view(b"a\nd\n");
        assert_eq!(view.bytes, b"a\nb\nc\nd\n");
        assert_eq!(view.locked_ranges, vec![2..6]);
    }

    #[test]
    fn extract_real_skips_locked_stripes() {
        let state = DiffState::new(b"a\nb\nc\n".to_vec());
        let view = state.build_view(b"a\nc\n");
        let mut buf = new_buffer();
        install(&mut buf, &view);
        assert_eq!(extract_real(&buf), b"a\nc\n");
    }

    #[test]
    fn extract_real_after_user_edit_in_kept_region() {
        let state = DiffState::new(b"one\ntwo\nthree\n".to_vec());
        let view = state.build_view(b"one\nthree\n");
        let mut buf = new_buffer();
        install(&mut buf, &view);
        // Simulate a user insert in the kept region right after "one\n" but
        // before the deleted stripe. Byte 4 = start of "two\n" (locked).
        buf.cursor_move_to_offset(4);
        // Insert at exactly the locked-stripe start: allowed, shifts the
        // stripe forward.
        buf.write_canon(b"EXTRA\n");
        assert_eq!(extract_real(&buf), b"one\nEXTRA\nthree\n");
    }

    #[test]
    fn view_to_real_off_basic() {
        // View: a\n[LOCKED b\n]c\n. Locked = 2..4.
        let locked = [2..4];
        assert_eq!(view_to_real_off(0, &locked), 0);
        assert_eq!(view_to_real_off(2, &locked), 2); // at locked start -> 2 real
        assert_eq!(view_to_real_off(3, &locked), 2); // inside locked -> anchor to start
        assert_eq!(view_to_real_off(4, &locked), 2); // just past locked
        assert_eq!(view_to_real_off(6, &locked), 4);
    }

    #[test]
    fn real_to_view_off_basic() {
        let locked = [2..4];
        assert_eq!(real_to_view_off(0, &locked, 6), 0);
        assert_eq!(real_to_view_off(2, &locked, 6), 2); // still at boundary, prefer before-stripe
        assert_eq!(real_to_view_off(3, &locked, 6), 5); // real byte 3 = first byte after stripe
        assert_eq!(real_to_view_off(4, &locked, 6), 6);
    }

    #[test]
    fn cursor_roundtrips_across_rebuild() {
        let state = DiffState::new(b"one\ntwo\nthree\nfour\n".to_vec());
        let view = state.build_view(b"one\nthree\nfour\n");
        let mut buf = new_buffer();
        install(&mut buf, &view);

        // Place cursor at "three" start in the view (after kept "one\n" and
        // the locked "two\n" stripe).
        buf.cursor_move_to_offset(8);
        let cursor_view = buf.cursor_offset();
        let cursor_real = view_to_real_off(cursor_view, buf.locked_ranges());
        assert_eq!(cursor_real, 4); // start of "three" in real

        // Simulate a rebuild that collapses the delete (edit re-adds "two").
        let state2 = DiffState::new(b"one\ntwo\nthree\nfour\n".to_vec());
        let view2 = state2.build_view(b"one\ntwo\nthree\nfour\n");
        install(&mut buf, &view2);

        let new_view_off = real_to_view_off(cursor_real, buf.locked_ranges(), buf.text_length());
        // In the collapsed view, real offset 4 maps back to view offset 4.
        assert_eq!(new_view_off, 4);
    }

    #[test]
    fn ensure_terminated_does_nothing_if_already_terminated() {
        assert_eq!(ensure_terminated(b""), b"");
        assert_eq!(ensure_terminated(b"a\n"), b"a\n");
        assert_eq!(ensure_terminated(b"a"), b"a\n");
    }

    #[test]
    fn rebuild_via_refresh_preserves_undo_of_edit() {
        // Simulate the rediff path: build an initial view, install it, type a
        // character, then rebuild with `refresh_view_content` and verify the
        // edit can still be undone.
        let state = DiffState::new(b"a\nb\nc\n".to_vec());
        let view = state.build_view(b"a\nc\n");
        let mut buf = new_buffer();
        install(&mut buf, &view);

        // Cursor at 0; insert 'X'.
        buf.cursor_move_to_offset(0);
        buf.write_canon(b"X");
        assert_eq!(extract_real(&buf), b"Xa\nc\n");

        // Rediff: rebuild the view from the new editable content.
        let current = extract_real(&buf);
        let view2 = state.build_view(&current);
        buf.refresh_view_content(&view2.bytes, view2.locked_ranges.clone());
        buf.set_line_decorations(view2.decorations);

        // Undo should revert the inserted 'X'.
        buf.undo();
        assert_eq!(extract_real(&buf), b"a\nc\n");
    }

    #[test]
    fn trailing_newline_in_baseline_only_does_not_trigger_hunk() {
        // Ensures we don't emit a spurious Deleted stripe for a final-newline
        // difference — linediff already collapses this, but verify here.
        let state = DiffState::new(b"a\nb\n".to_vec());
        let view = state.build_view(b"a\nb");
        assert!(view.locked_ranges.is_empty());
        assert!(view.decorations.iter().all(|d| *d == LineDecoration::None));
    }
}
