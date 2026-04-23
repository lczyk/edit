"""COMMIT_EDITMSG (git commit message) syntax highlighting smoke test."""

from framework import FG_COMMENT, Edit, expect, fixture, test


@test
def git_commit_highlighting():
    with Edit([fixture("COMMIT_EDITMSG")]) as ed:
        expect(b"# Please enter" in ed.plain, "missing '# Please enter'")
        expect(FG_COMMENT + b"# Please enter" in ed.buf,
               "'# Please enter' line not rendered in comment (green) color")
