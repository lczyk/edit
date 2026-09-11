"""Merge-conflict markers light up in any file, plain text included.

The marker lines are recognised by the highlighter runtime ahead of any
language definition, so a .txt file with no language of its own still
shows them in the conflict colour.
"""

from framework import (
    FG_CONFLICT_MARKER,
    Edit,
    expect,
    fixture,
    test,
)


@test
def conflict_markers_render_in_plain_text():
    with Edit([fixture("conflict.txt")]) as ed:
        expect(b"<<<<<<< HEAD" in ed.plain, "missing marker text")
        expect(FG_CONFLICT_MARKER + b"<<<<<<< HEAD" in ed.buf,
               "opening marker not rendered with the conflict colour")
        expect(FG_CONFLICT_MARKER + b">>>>>>> feature" in ed.buf,
               "closing marker not rendered with the conflict colour")
