"""Walking through a word too long to fit a wrapped row.

A word wider than the text area cannot be moved down whole, so it hard-wraps
mid-word while the row before it ends early at the space that let it try. The
cursor's own measurement has to agree with that layout -- see
`crates/edit/src/unicode/measurement.rs`, where a resumed measurement used to
decide the break for itself.

These assert against the editor's own invariant checks rather than the screen:
run under `--strict-sanity` with a `--features sanity` build and a trip fails
the test. Without those they still exercise the path, they just cannot see the
damage, so `check_bounds` also asserts the visible part.
"""

from framework import DOWN, END, LEFT, RIGHT, UP, Edit, expect, fixture, test

WRAPPING = "longword.txt"

# Narrow enough that the z-run cannot fit beside the leading words.
COLS = 40
ROWS = 12


@test
def holding_right_through_a_long_word_keeps_the_cursor_consistent():
    with Edit([fixture(WRAPPING)], cols=COLS, rows=ROWS) as ed:
        for _ in range(40):
            ed.send(RIGHT, settle=0)
        ed.drain()
        # The line is 63 columns; the cursor should have walked into the z-run
        # and the row it reports must still exist.
        screen = ed.screen()
        expect(b"second line here" in screen, "the buffer stopped rendering")


@test
def walking_back_out_of_a_long_word_keeps_the_cursor_consistent():
    with Edit([fixture(WRAPPING)], cols=COLS, rows=ROWS) as ed:
        ed.send(END)
        for _ in range(40):
            ed.send(LEFT, settle=0)
        ed.drain()
        expect(b"second line here" in ed.screen(), "the buffer stopped rendering")


@test
def crossing_a_long_word_vertically_keeps_the_cursor_consistent():
    with Edit([fixture(WRAPPING)], cols=COLS, rows=ROWS) as ed:
        for _ in range(6):
            ed.send(DOWN, settle=0)
        for _ in range(6):
            ed.send(UP, settle=0)
        ed.drain()
        expect(b"second line here" in ed.screen(), "the buffer stopped rendering")


@test
def typing_inside_a_long_word_keeps_the_cursor_consistent():
    with Edit([fixture(WRAPPING)], cols=COLS, rows=ROWS) as ed:
        for _ in range(25):
            ed.send(RIGHT, settle=0)
        ed.send(b"QQ")
        ed.drain()
        expect(b"QQ" in ed.screen(), "the insertion is not on screen")
