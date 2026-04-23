"""Windows batch: no .lsh definition — smoke test only (content visible)."""

from framework import Edit, expect, fixture, test


@test
def batch_highlighting():
    with Edit([fixture("highlighting.bat")]) as ed:
        expect(b"@echo off" in ed.plain, "missing '@echo off'")
        expect(b"REM" in ed.plain, "missing 'REM'")
