"""Properties/INI syntax highlighting smoke test."""

from framework import FG_COMMENT, Edit, expect, fixture, test


@test
def properties_highlighting():
    with Edit([fixture("highlighting.conf")]) as ed:
        expect(b"# General Settings" in ed.plain, "missing '# General Settings'")
        expect(FG_COMMENT + b"# General Settings" in ed.buf,
               "'# General Settings' comment not rendered in comment (green) color")
