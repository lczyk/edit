"""Search panel checkboxes render as ASCII, not Unicode glyphs."""

from framework import FIND, Edit, expect, fixture, test

UNICODE_BOXES = ["☐", "☑", "\U0001F5F9"]  # ☐, ☑, 🗹


@test
def no_unicode_checkboxes_in_search_panel():
    with Edit([fixture("hello.txt")]) as ed:
        ed.send(FIND)
        text = ed.plain.decode("utf-8", "replace")
        for glyph in UNICODE_BOXES:
            expect(glyph not in text, f"unicode checkbox {glyph!r} leaked into output")
        expect("[ Match Case]" in text, "ASCII '[ Match Case]' not found")
        expect("[ Whole Word]" in text, "ASCII '[ Whole Word]' not found")
        expect("[ Use Regex]" in text, "ASCII '[ Use Regex]' not found")
