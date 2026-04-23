"""YAML syntax highlighting smoke test."""

from framework import FG_COMMENT, Edit, expect, fixture, test


@test
def yaml_highlighting():
    with Edit([fixture("highlighting.yml")]) as ed:
        expect(b"# This is a comment" in ed.plain, "missing '# This is a comment'")
        expect(FG_COMMENT + b"# This is a comment" in ed.buf,
               "'# This is a comment' not rendered in comment (green) color")
