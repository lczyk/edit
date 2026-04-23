"""Ctrl+F prefills the search needle with the current single-line selection."""

from framework import CTRL_F, Edit, SHIFT_DOWN, SHIFT_RIGHT, expect, fixture, test


@test
def prefill_single_line_selection():
    with Edit([fixture("hello.txt")]) as ed:
        ed.send(SHIFT_RIGHT * 5)
        ed.send(CTRL_F)
        frame = ed.last_plain_frame(b"Find:")
        expect(b"Find: hello" in frame, f"got: {frame[:120]!r}")


@test
def no_prefill_on_multi_line_selection():
    with Edit([fixture("hello.txt")]) as ed:
        ed.send(SHIFT_DOWN + SHIFT_RIGHT * 3)
        ed.send(CTRL_F)
        frame = ed.last_plain_frame(b"Find:")
        expect(b"Find: hello" not in frame, f"needle leaked: {frame[:120]!r}")
        expect(b"Find:  " in frame, f"expected empty field; got: {frame[:120]!r}")
