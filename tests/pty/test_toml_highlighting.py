"""TOML syntax highlighting smoke test."""

from framework import FG_COMMENT, FG_KEYWORD_OTHER, Edit, expect, fixture, test


@test
def toml_highlighting():
    with Edit([fixture("highlighting.toml")]) as ed:
        expect(b"# Single-line comment" in ed.plain, "missing '# Single-line comment'")
        expect(FG_COMMENT + b"# Single-line comment" in ed.buf,
               "'# Single-line comment' not rendered in comment (green) color")
        expect(FG_KEYWORD_OTHER + b"[owner]" in ed.buf,
               "'[owner]' section header not rendered in header (blue) color")
