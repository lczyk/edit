"""Markdown syntax highlighting smoke test.

Detailed token-level coverage lives in the lsh golden suite at
`crates/lsh/tests/golden.rs`. This file just verifies that markdown
highlighting is wired through the editor's render path at all.
"""

from framework import FG_KEYWORD_OTHER, Edit, expect, fixture, test


@test
def markdown_highlighting():
    with Edit([fixture("highlighting.md")]) as ed:
        expect(b"# H1" in ed.plain, "missing '# H1'")
        expect(FG_KEYWORD_OTHER + b"# H1" in ed.buf,
               "'# H1' heading not rendered in heading (blue) color")
