from framework import Edit, expect, fixture, test

POINTER = b"\x1b]22;pointer\x07"
TEXT = b"\x1b]22;text\x07"
DEFAULT = b"\x1b]22;default\x07"


def motion(x: int, y: int) -> bytes:
    """Bare mouse motion (no button held), 0-based cell coordinates."""
    return b"\x1b[<35;%d;%dM" % (x + 1, y + 1)


@test
def hovering_a_menu_repaints_it_and_shows_a_pointer():
    with Edit([fixture("hello.txt")], cols=80, rows=24) as ed:
        mark = ed.mark()
        ed.send(motion(2, 0))
        expect(POINTER in ed.buf[mark:], f"no pointer shape: {ed.buf[mark:][:200]!r}")
        expect(b"file" in ed.plain_since(mark), "menu not repainted on hover")

        mark = ed.mark()
        ed.send(motion(10, 5))
        expect(TEXT in ed.buf[mark:], f"no text shape: {ed.buf[mark:][:200]!r}")
        expect(b"file" in ed.plain_since(mark), "menu not repainted when hover left")


@test
def the_pointer_shape_is_only_sent_when_it_changes():
    with Edit([fixture("hello.txt")], cols=80, rows=24) as ed:
        ed.send(motion(10, 5))
        mark = ed.mark()
        ed.send(motion(11, 5))
        expect(TEXT not in ed.buf[mark:], "text shape resent while still over the text")


@test
def exiting_restores_the_default_pointer():
    ed = Edit([fixture("hello.txt")], cols=80, rows=24)
    ed.send(motion(2, 0))
    mark = ed.mark()
    ed.close()
    expect(DEFAULT in ed.buf[mark:], "default pointer not restored on exit")
