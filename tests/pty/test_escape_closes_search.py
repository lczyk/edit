"""Escape closes the Find panel."""

from framework import CTRL_F, ESC, Edit, expect, fixture, test


@test
def escape_hides_find_panel():
    with Edit([fixture("hello.txt")]) as ed:
        ed.send(CTRL_F)
        expect(b"Find:" in ed.plain, "Find panel didn't open")

        mark = ed.mark()
        ed.send(ESC)
        after = ed.plain_since(mark)
        expect(b"Find:" not in after,
               f"'Find:' still visible after ESC: {after[:300]!r}")
