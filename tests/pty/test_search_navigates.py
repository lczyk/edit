"""Typing into Find + Enter keeps hits visible across successive matches."""

from framework import ENTER, FIND, Edit, expect, fixture, test


@test
def needle_reflected_and_enter_navigates():
    with Edit([fixture("hello.txt")]) as ed:
        ed.send(FIND)
        ed.send(b"foo")
        expect(b"Find: foo" in ed.plain, "needle 'foo' not reflected in Find panel")

        ed.send(ENTER)
        expect(b"foo" in ed.plain, "'foo' not in rendered frame after first Enter")

        ed.send(ENTER)
        expect(b"foo" in ed.plain, "'foo' disappeared after second Enter")
