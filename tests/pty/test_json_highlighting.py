"""JSON/JSONC syntax highlighting smoke test."""

from framework import FG_COMMENT, Edit, expect, fixture, test


@test
def json_highlighting():
    with Edit([fixture("highlighting.json")]) as ed:
        expect(b'"Hello, world!"' in ed.plain, "missing '\"Hello, world!\"'")
        expect(FG_COMMENT + b"// Object" in ed.buf,
               "'// Object' JSONC comment not rendered in comment (green) color")
