"""--quirks=nocolor suppresses every SGR sequence (colour + attributes).

The quirk emits no `\\x1b[...m` sequences at all -- not fg/bg colours, not
bold/italic/underline/strikethrough. Cursor positioning (`CUP`) and other
non-SGR control sequences are still required for the editor to function.
"""

import re

from framework import Edit, expect, fixture, test


_SGR_RE = re.compile(rb"\x1b\[[\d;]*m")
_FG_OR_BG_RE = re.compile(rb"\x1b\[(?:38|48);[0-9;]+m")


@test
def nocolor_quirk_emits_no_sgr():
    with Edit(["--quirks=nocolor", fixture("highlighting.go")]) as ed:
        match = _SGR_RE.search(ed.buf)
        expect(match is None,
               f"found SGR under --quirks=nocolor: {match.group(0) if match else None!r}")


@test
def default_emits_color_sgr():
    with Edit([fixture("highlighting.go")]) as ed:
        expect(_FG_OR_BG_RE.search(ed.buf) is not None,
               "expected colour SGR without --quirks=nocolor")
