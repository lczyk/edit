"""Horizontal scroll in the eat viewers survives vertical movement.

The follow view re-anchors on the tail whenever content arrives, and
End does the same in both views. Neither may reset the columns the
reader scrolled to: a tail snap only moves the vertical offset.
"""

import os
import tempfile

from framework import (
    Edit,
    expect,
    pause,
    test,
)


def _numbered_lines(n):
    # Column 24 (three Right presses at 8 columns each) lands inside the
    # per-line tag, so a row that still shows its tag is one whose
    # horizontal offset was kept.
    return ["line %03d " % i + ("L%02d-" % i) * 30 for i in range(n)]


def _write(path, lines):
    with open(path, "w") as f:
        f.write("\n".join(lines) + "\n")


@test
def follow_view_keeps_columns_when_the_tail_grows():
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "grow.txt")
        _write(path, _numbered_lines(30))
        env = {"EAT_FOLLOW_INTERVAL_MS": "200"}
        with Edit(["--eat", "--color", "never", "--wrap", "never", "-f", path],
                  cols=40, rows=8, env=env) as ed:
            for _ in range(3):
                ed.send(b"l")
            expect(b"L29-L29" in ed.plain, "not scrolled right before the append")

            mark = ed.mark()
            with open(path, "a") as f:
                f.write("line NEW " + "NEW-" * 30 + "\n")
            pause(1.0)
            ed.drain()
            frame = ed.plain_since(mark)
            expect(b"NEW-NEW" in frame, "tail did not snap to the new line")
            expect(b"line NEW" not in frame, "tail snap reset the horizontal offset")
            ed.send(b"q")


@test
def end_keeps_columns_in_both_views():
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "long.txt")
        _write(path, _numbered_lines(30))
        for argv in (["--eat", "--color", "never", "--wrap", "never", path],
                     ["--eat", "--color", "never", "--wrap", "never", "-f", path]):
            with Edit(argv, cols=40, rows=8) as ed:
                for _ in range(3):
                    ed.send(b"l")
                ed.send(b"g")
                expect(b"L00-L00" in ed.plain, "not scrolled right at the top")
                mark = ed.mark()
                ed.send(b"G")
                frame = ed.plain_since(mark)
                expect(b"L29-L29" in frame, "End did not reach the tail")
                expect(b"line 029" not in frame, "End reset the horizontal offset")
                ed.send(b"q")
