"""Python syntax highlighting smoke test."""

from framework import FG_COMMENT, Edit, expect, fixture, test


@test
def python_highlighting():
    with Edit([fixture("highlighting.py")]) as ed:
        expect(b"# Comments" in ed.plain, "missing '# Comments'")
        expect(FG_COMMENT + b"# Comments" in ed.buf,
               "'# Comments' not rendered in comment (green) color")
