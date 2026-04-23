"""git-rebase-todo syntax highlighting smoke test."""

from framework import FG_KEYWORD_CONTROL, Edit, expect, fixture, test


@test
def git_rebase_highlighting():
    with Edit([fixture("git-rebase-todo")]) as ed:
        expect(b"pick" in ed.plain, "missing 'pick'")
        expect(FG_KEYWORD_CONTROL + b"pick" in ed.buf,
               "'pick' keyword not rendered in control (magenta) color")
