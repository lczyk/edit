"""Typing characters inserts them into the buffer."""

from framework import END, Edit, expect, fixture, test


@test
def type_at_end_of_line():
    with Edit([fixture("hello.txt")]) as ed:
        ed.send(END)
        ed.send(b" XYZ")
        expect(
            b"hello world hello again hello XYZ" in ed.plain,
            f"typed text not in frame; got: {ed.plain[-300:]!r}",
        )
