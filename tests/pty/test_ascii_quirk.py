"""--quirks=-unicode forces ASCII-only rendering.

Smoke test that with unicode disabled, the line-number margin uses `|`
instead of the box-drawing `│`, and a sanity check that with the default
(unicode on) the box glyph is present.
"""

from framework import Edit, expect, fixture, test


# Box-drawing vertical bar `│` is U+2502 -> UTF-8 0xE2 0x94 0x82.
BOX_V = b"\xe2\x94\x82"


@test
def ascii_quirk_replaces_margin_separator():
    with Edit(["--quirks=-unicode", fixture("hello.txt")]) as ed:
        expect(BOX_V not in ed.plain,
               "found box-drawing | in margin under --quirks=-unicode")
        # Margin still has a separator, just the ASCII pipe.
        expect(b" 1 | " in ed.plain,
               "expected ' 1 | ' margin under --quirks=-unicode")


@test
def default_uses_box_drawing_margin():
    with Edit([fixture("hello.txt")]) as ed:
        expect(BOX_V in ed.plain,
               "expected box-drawing | in margin by default (unicode on)")
