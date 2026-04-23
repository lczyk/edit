"""Rust syntax highlighting smoke test."""

from framework import FG_COMMENT, Edit, expect, fixture, test


@test
def rust_highlighting():
    with Edit([fixture("highlighting.rs")]) as ed:
        expect(b"// Line comment" in ed.plain, "missing '// Line comment'")
        expect(FG_COMMENT + b"// Line comment" in ed.buf,
               "'// Line comment' not rendered in comment (green) color")
