"""Document actions must not fire while a text field owns the keyboard.

handle_global_shortcuts runs at the top of the frame, before any widget is
declared, and used to consume every chord unconditionally. So Cmd+Shift+K
deleted a line from the document while the user was typing in the Find box,
and the macOS cursor-motion chords edited the document from inside every input
field in the program.
"""

import sys

from framework import ENTER, ESC, FIND, Edit, expect, fixture, test

_MAC = sys.platform == "darwin"


def _chord(letter, shift=False):
    """The platform's chord for a document action, per the shipped defaults."""
    from framework import csi_u

    if _MAC:
        return csi_u(ord(letter), shift=shift, cmd=True)
    return csi_u(ord(letter), shift=shift, ctrl=True)


DELETE_LINE = _chord("k", shift=True)   # delete_line: Cmd+Shift+K / Ctrl+Shift+K
GO_TO_LINE = _chord("l")                # go_to_line: a window action, must still work


@test
def delete_line_does_not_reach_the_document_from_the_find_field():
    with Edit([fixture("hello.txt")], cols=60, rows=12) as ed:
        ed.send(FIND)
        expect(b"Find:" in ed.screen(), "the Find panel did not open")

        ed.send(DELETE_LINE)
        expect(b"hello world" in ed.screen(), "delete_line reached the document")


@test
def delete_line_still_works_when_the_document_has_focus():
    # The other half: the guard must not have simply disabled the action.
    with Edit([fixture("hello.txt")], cols=60, rows=12) as ed:
        expect(b"hello world" in ed.screen(), "fixture did not load")
        ed.send(DELETE_LINE)
        expect(b"hello world" not in ed.screen(), "delete_line stopped working")


# Deliberately not tested here: that closing the panel re-enables the actions.
# Focus takes a frame to settle back onto the document, and `ed.screen()` forces
# a resize to get a full repaint, which with a panel open puts window-report
# sequences into the stream that survive ANSI stripping -- so the assertion ends
# up measuring the harness rather than the editor. The guard asks "is a field
# focused" rather than "is the document focused" precisely so that the settling
# frame cannot swallow a keystroke.


@test
def window_actions_still_work_from_a_field():
    # Exit/Save/Find/Replace/GoToLine act on the window, not the buffer, and
    # deliberately stay reachable from anywhere.
    with Edit([fixture("hello.txt")], cols=60, rows=12) as ed:
        ed.send(FIND)
        ed.send(GO_TO_LINE)
        screen = ed.screen()
        expect(b"hello world" in screen, "go_to_line damaged the document")
        # The go-to-line modal is a dialog with its own input field.
        expect(b":" in screen, "the go-to-line modal did not open")
        ed.send(ESC)
        ed.send(ENTER, drain=False)
