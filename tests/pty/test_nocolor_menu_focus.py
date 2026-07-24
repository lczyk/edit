"""Under --quirks=-color the focused menu item gets a `<...>` marker.

With colour gone, the existing bg-tint focus highlight is invisible, so
the renderer overwrites the focused rect's first/last cell with `<` `>`.
"""

from framework import F10, Edit, expect, fixture, pause, test


@test
def nocolor_menubar_focus_shows_marker():
    with Edit(["--quirks=-color", fixture("hello.txt")]) as ed:
        ed.send(F10)
        pause(0.2)
        ed.drain()
        # Focused menu = the first one. The marker overwrites the focused
        # rect's outer cells, so it hugs the label with no padding inside.
        expect(b"<file>" in ed.plain,
               f"expected '<file>' focus marker, got: {ed.plain[:300]!r}")
