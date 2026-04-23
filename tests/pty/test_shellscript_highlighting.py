"""Shell-script syntax highlighting smoke test."""

from framework import FG_COMMENT, Edit, expect, fixture, test


@test
def shellscript_highlighting():
    with Edit([fixture("highlighting.sh")]) as ed:
        expect(b"#!/bin/bash" in ed.plain, "missing '#!/bin/bash'")
        expect(FG_COMMENT + b"#!/bin/bash" in ed.buf,
               "shebang not rendered in comment (green) color")
