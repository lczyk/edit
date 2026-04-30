"""Dockerfile syntax highlighting smoke test."""

from framework import FG_COMMENT, Edit, expect, fixture, test


@test
def dockerfile_highlighting():
    with Edit([fixture("Dockerfile")]) as ed:
        expect(b"# Build stage" in ed.plain, "missing '# Build stage'")
        expect(FG_COMMENT + b"# Build stage" in ed.buf,
               "'# Build stage' not rendered in comment (green) color")
