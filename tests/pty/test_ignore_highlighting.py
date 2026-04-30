"""gitignore syntax highlighting smoke test."""

from framework import FG_COMMENT, Edit, expect, fixture, test


@test
def ignore_highlighting():
    with Edit([fixture(".gitignore")]) as ed:
        expect(b"# Build artefacts" in ed.plain, "missing '# Build artefacts'")
        expect(FG_COMMENT + b"# Build artefacts" in ed.buf,
               "'# Build artefacts' not rendered in comment (green) color")
