"""XML syntax highlighting smoke test."""

from framework import FG_COMMENT, Edit, expect, fixture, test


@test
def xml_highlighting():
    with Edit([fixture("highlighting.xml")]) as ed:
        expect(b"<?xml" in ed.plain, "missing '<?xml'")
        expect(FG_COMMENT + b"<!-- Root comment" in ed.buf,
               "'<!-- Root comment' not rendered in comment (green) color")
