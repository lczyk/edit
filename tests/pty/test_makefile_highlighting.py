"""Makefile syntax highlighting smoke test."""

from framework import FG_COMMENT, Edit, expect, fixture, test


@test
def makefile_highlighting():
    with Edit([fixture("Makefile")]) as ed:
        expect(b"# Build configuration" in ed.plain, "missing '# Build configuration'")
        expect(FG_COMMENT + b"# Build configuration" in ed.buf,
               "'# Build configuration' not rendered in comment (green) color")
