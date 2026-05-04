"""Markdown syntax highlighting smoke test."""

from framework import FG_KEYWORD_OTHER, Edit, expect, fixture, test


@test
def markdown_highlighting():
    with Edit([fixture("highlighting.md")]) as ed:
        expect(b"# H1" in ed.plain, "missing '# H1'")
        expect(FG_KEYWORD_OTHER + b"# H1" in ed.buf,
               "'# H1' heading not rendered in heading (blue) color")


@test
def markdown_fenced_code_block_nested_highlighting():
    # ```python fences should highlight their inner content via the python
    # sublanguage (e.g. `def` as a control keyword).
    with Edit([fixture("highlighting_md_fenced.md")]) as ed:
        expect(b"def greet" in ed.plain, "missing python sample")
        expect(FG_KEYWORD_OTHER + b"def" in ed.buf,
               "python `def` not highlighted inside ```python fence")


@test
def markdown_indented_code_block_stays_plain():
    # 4-space (or tab) indent without a list marker stays a code block:
    # inline emphasis should not be applied.
    with Edit([fixture("highlighting_md_indented_code.md")]) as ed:
        for marker in (b"xcodeblock4x", b"xcodeblocktabx"):
            expect(marker in ed.plain, f"missing {marker!r}")
            idx = ed.buf.find(marker)
            window = ed.buf[max(0, idx - 32):idx + len(marker) + 8]
            expect(b"\x1b[1m" not in window,
                   f"bold leaked into indented code block at {marker!r}")


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
