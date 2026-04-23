"""PowerShell syntax highlighting smoke test."""

from framework import FG_COMMENT, Edit, expect, fixture, test


@test
def powershell_highlighting():
    with Edit([fixture("highlighting.ps1")]) as ed:
        expect(b"# Single-line comment" in ed.plain, "missing '# Single-line comment'")
        expect(FG_COMMENT + b"# Single-line comment" in ed.buf,
               "'# Single-line comment' not rendered in comment (green) color")
