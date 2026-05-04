"""End-to-end smoke for syntax highlighting.

Per-language token coverage lives in the lsh golden suite at
`crates/lsh/tests/golden.rs`. This single PTY test only verifies that
the editor's render path actually applies highlighting: opens a markdown
fixture and checks that multiple foreground colours appear on screen.
"""

from framework import (
    FG_KEYWORD_OTHER,
    Edit,
    distinct_fg_colors,
    expect,
    fixture,
    test,
)


@test
def highlighting_renders():
    with Edit([fixture("smoke.md")]) as ed:
        expect(b"# heading" in ed.plain, "missing heading text")
        expect(FG_KEYWORD_OTHER + b"# heading" in ed.buf,
               "heading not rendered with heading colour")
        colours = distinct_fg_colors(ed.buf)
        expect(len(colours) >= 3,
               f"expected >=3 distinct fg colours, got {len(colours)}")
