"""LSH definition language syntax highlighting smoke test."""

from framework import FG_COMMENT, Edit, expect, fixture, test


@test
def lsh_highlighting():
    with Edit([fixture("highlighting.lsh")]) as ed:
        expect(b"// LSH definition example" in ed.plain,
               "missing '// LSH definition example'")
        expect(FG_COMMENT + b"// LSH definition example" in ed.buf,
               "'// LSH definition example' not rendered in comment (green) color")
