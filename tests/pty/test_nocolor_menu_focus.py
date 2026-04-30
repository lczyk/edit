"""Under --quirks=nocolor the focused menu item gets bracketed.

With colour gone, the existing bg-tint focus highlight is invisible, so
the renderer falls back to wrapping the focused rect with `[` `]`.
"""

from framework import F10, Edit, expect, fixture, pause, test


@test
def nocolor_menubar_focus_shows_brackets():
    with Edit(["--quirks=nocolor", fixture("hello.txt")]) as ed:
        ed.send(F10)
        pause(0.2)
        ed.drain()
        # Focused menu = first one (File). Look for [ and ] surrounding it.
        # Layout puts a space inside the brackets due to padding.
        expect(b"[ File " in ed.plain or b"[File" in ed.plain,
               f"expected '[ File ' bracketed focus marker, got: {ed.plain[:300]!r}")
