"""Unified diff syntax highlighting smoke test."""

from framework import FG_KEYWORD_OTHER, Edit, expect, fixture, test


@test
def diff_highlighting():
    with Edit([fixture("highlighting.diff")]) as ed:
        expect(b"diff --git" in ed.plain, "missing 'diff --git'")
        expect(FG_KEYWORD_OTHER + b"diff --git" in ed.buf,
               "'diff --git' header not rendered in header (blue) color")
