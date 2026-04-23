"""JavaScript syntax highlighting smoke test."""

from framework import FG_COMMENT, Edit, expect, fixture, test


@test
def javascript_highlighting():
    with Edit([fixture("highlighting.js")]) as ed:
        expect(b"// Comments" in ed.plain, "missing '// Comments'")
        expect(FG_COMMENT + b"// Comments" in ed.buf,
               "'// Comments' not rendered in comment (green) color")
