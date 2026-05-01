"""Under --quirks=nocolor the exit dialog's focused button gets a marker.

After making the buffer dirty and pressing Ctrl+Q, the "Unsaved Changes"
modal opens with [Save] / [Don't Save] / [Cancel] buttons. The focused
one (Save by default) is shown as `<Save>` instead of `[Save]` so the
focus is visible without colour.
"""

from framework import CTRL_Q, Edit, expect, fixture, pause, test


@test
def nocolor_exit_dialog_focus_shows_marker():
    with Edit(["--quirks=nocolor", fixture("hello.txt")]) as ed:
        # Type something to make the buffer dirty.
        ed.send(b"x")
        pause(0.2)
        # Open the unsaved-changes dialog.
        ed.send(CTRL_Q)
        pause(0.3)
        ed.drain()
        expect(b"<Save>" in ed.plain,
               f"expected '<Save>' focused button marker, got: {ed.plain[:600]!r}")
        # The other two stay plain-bracketed.
        expect(b"[Don't Save]" in ed.plain,
               f"expected '[Don't Save]' unfocused, got: {ed.plain[:600]!r}")
        expect(b"[Cancel]" in ed.plain,
               f"expected '[Cancel]' unfocused, got: {ed.plain[:600]!r}")
