"""Markdown syntax highlighting smoke test."""

from framework import FG_KEYWORD_OTHER, Edit, expect, fixture, test


@test
def markdown_highlighting():
    with Edit([fixture("highlighting.md")]) as ed:
        expect(b"# H1" in ed.plain, "missing '# H1'")
        expect(FG_KEYWORD_OTHER + b"# H1" in ed.buf,
               "'# H1' heading not rendered in heading (blue) color")


@test
def markdown_indented_list_not_code_block():
    # Regression: lines indented by 4+ spaces used to be unconditionally
    # treated as code blocks, breaking inline emphasis on nested list items.
    # CommonMark allows nested list continuations at deeper indents.
    with Edit([fixture("highlighting_md_indented_list.md")]) as ed:
        for marker in (b"xboldindent4x", b"xboldindent8x"):
            expect(marker in ed.plain, f"missing {marker!r}")
            idx = ed.buf.find(marker)
            window = ed.buf[max(0, idx - 32):idx]
            expect(b"\x1b[1m" in window,
                   f"bold not applied to {marker!r} — indented list "
                   f"item rendered as code block (regression)")
