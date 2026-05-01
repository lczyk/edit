"""Under --quirks=nocolor the focused menu item gets a `<…>` marker.

With colour gone, the existing bg-tint focus highlight is invisible, so
the renderer overwrites the focused rect's first/last cell with `<` `>`.
"""

from framework import F10, Edit, expect, fixture, pause, test


@test
def nocolor_menubar_focus_shows_marker():
    with Edit(["--quirks=nocolor", fixture("hello.txt")]) as ed:
        ed.send(F10)
        pause(0.2)
        ed.drain()
        # Focused menu = first one (File). Padding leaves a space inside
        # the markers.
        expect(b"< File " in ed.plain or b"<File" in ed.plain,
               f"expected '< File ' focus marker, got: {ed.plain[:300]!r}")
