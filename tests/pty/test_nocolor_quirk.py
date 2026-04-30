"""--quirks=nocolor suppresses all SGR colour output.

We assert no `\\x1b[38;...m` (FG) or `\\x1b[48;...m` (BG) sequences appear
in the raw output buffer. Default behaviour (without the quirk) does emit
them, so a control test confirms the colour pipeline is otherwise live.
"""

import re

from framework import Edit, expect, fixture, test


_FG_OR_BG_RE = re.compile(rb"\x1b\[(?:38|48);[0-9;]+m")


@test
def nocolor_quirk_emits_no_color_sgr():
    with Edit(["--quirks=nocolor", fixture("highlighting.go")]) as ed:
        match = _FG_OR_BG_RE.search(ed.buf)
        expect(match is None,
               f"found colour SGR under --quirks=nocolor: {match.group(0) if match else None!r}")


@test
def default_emits_color_sgr():
    with Edit([fixture("highlighting.go")]) as ed:
        expect(_FG_OR_BG_RE.search(ed.buf) is not None,
               "expected colour SGR without --quirks=nocolor")
