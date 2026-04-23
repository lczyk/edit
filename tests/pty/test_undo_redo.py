"""Ctrl+Z undoes, Ctrl+Y redoes the last edit."""

from framework import CTRL_Y, CTRL_Z, END, Edit, expect, fixture, test


@test
def undo_removes_insertion_and_redo_restores_it():
    with Edit([fixture("hello.txt")]) as ed:
        ed.send(END)
        ed.send(b"ZZZ")
        expect(b"helloZZZ" in ed.plain, "inserted text not visible before undo")

        mark = ed.mark()
        ed.send(CTRL_Z)
        after_undo = ed.plain_since(mark)
        expect(b"helloZZZ" not in after_undo,
               f"ZZZ still present after undo: {after_undo[-200:]!r}")
        expect(b"hello world hello again hello" in after_undo,
               f"original line missing after undo: {after_undo[-200:]!r}")

        mark = ed.mark()
        ed.send(CTRL_Y)
        after_redo = ed.plain_since(mark)
        expect(b"helloZZZ" in after_redo,
               f"ZZZ did not come back after redo: {after_redo[-200:]!r}")
