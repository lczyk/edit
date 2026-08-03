"""The caret stays on the last line when moving back off the end of it.

A file that ends without a newline leaves the last line's wrap opportunity
open. Moving the cursor left off the end of it used to report a wrap the
terminal width cannot produce, painting the caret a row below the document.
"""

import re

from framework import LEFT, Edit, expect, fixture, primary, test

# The frame ends with the caret's absolute position, then the cursor shape
# and the show-cursor sequence.
CARET = re.compile(rb"\x1b\[(\d+);(\d+)H(?=\x1b\[\d* q)")


def caret_row(ed):
    found = CARET.findall(ed.buf)
    expect(bool(found), f"no caret position in output; got: {ed.buf[-200:]!r}")
    return int(found[-1][0])


@test
def left_off_the_end_of_a_file_without_a_trailing_newline_keeps_the_row():
    with Edit([fixture("no_trailing_newline.txt")], cols=60, rows=10) as ed:
        ed.send(primary("g", shift=True))  # jump to the end of the document
        before = caret_row(ed)

        ed.send(LEFT)
        after = caret_row(ed)

        expect(after == before, f"caret left the last line: row {before} -> {after}")
