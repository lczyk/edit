"""Cmd+/ / Ctrl+/ toggles line comments. Undo reverts."""

from framework import TOGGLE_COMMENT, UNDO, Edit, expect, lsh_fixture, test


def _frame_after(ed):
    # Anchor on the line-1 gutter marker so we read only the latest frame's
    # rendered content for line 1 onward.
    return ed.last_plain_frame(b" 1 \xe2\x94\x82")


@test
def keybinding_toggles_line_comment_and_undo_reverts():
    with Edit([lsh_fixture("go/kitchen_sink.go")]) as ed:
        ed.send(TOGGLE_COMMENT)
        frame = _frame_after(ed)
        expect(b"// Line comment" not in frame,
               f"comment still present after toggle: {frame[:300]!r}")
        expect(b"Line comment" in frame,
               f"line content missing: {frame[:300]!r}")

        ed.send(UNDO)
        frame = _frame_after(ed)
        expect(b"// Line comment" in frame,
               f"undo did not restore comment: {frame[:300]!r}")


@test
def double_keyboard_toggle_round_trips():
    with Edit([lsh_fixture("go/kitchen_sink.go")]) as ed:
        ed.send(TOGGLE_COMMENT)
        frame = _frame_after(ed)
        expect(b"// Line comment" not in frame,
               f"first toggle did not strip: {frame[:300]!r}")
        ed.send(TOGGLE_COMMENT)
        frame = _frame_after(ed)
        expect(b"// Line comment" in frame,
               f"second toggle did not re-comment: {frame[:300]!r}")


@test
def two_toggles_undo_individually():
    """Regression: each toggle should land in its own undo entry, not coalesce
    with the prior toggle (or any prior same-type edit) into one entry."""
    with Edit([lsh_fixture("go/kitchen_sink.go")]) as ed:
        ed.send(TOGGLE_COMMENT)
        ed.send(TOGGLE_COMMENT)
        frame = _frame_after(ed)
        expect(b"// Line comment" in frame,
               f"second toggle did not re-comment: {frame[:300]!r}")
        # First undo reverts the second (re-comment) -> uncommented.
        ed.send(UNDO)
        frame = _frame_after(ed)
        expect(b"// Line comment" not in frame,
               f"first undo did not strip second toggle: {frame[:300]!r}")
        # Second undo reverts the first (uncomment) -> re-commented.
        ed.send(UNDO)
        frame = _frame_after(ed)
        expect(b"// Line comment" in frame,
               f"second undo did not restore comment: {frame[:300]!r}")
