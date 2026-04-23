"""HTML syntax highlighting smoke test (handled by the XML definition)."""

from framework import FG_COMMENT, Edit, expect, fixture, test


@test
def html_highlighting():
    with Edit([fixture("highlighting.html")]) as ed:
        expect(b"<!DOCTYPE html>" in ed.plain, "missing '<!DOCTYPE html>'")
        expect(FG_COMMENT + b"<!-- Comment" in ed.buf,
               "'<!-- Comment' not rendered in comment (green) color")
