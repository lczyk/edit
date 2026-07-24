"""Ctrl+A selects everything; the resulting multi-line selection does not
prefill the Find needle."""

from framework import FIND, SELECT_ALL, Edit, expect, fixture, test


@test
def select_all_then_find_leaves_needle_empty():
    with Edit([fixture("hello.txt")]) as ed:
        ed.send(SELECT_ALL)
        ed.send(FIND)
        frame = ed.last_plain_frame(b"Find:")
        expect(b"Find: hello" not in frame,
               f"multi-line selection leaked into Find: {frame[:200]!r}")
        expect(b"Find:  " in frame,
               f"Find field should be empty; got: {frame[:200]!r}")
