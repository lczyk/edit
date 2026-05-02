"""--quirks=allow-create gates new-file creation.

Default (quirk off) refuses to open a path that doesn't exist, and refuses
to save into a missing file. With the quirk on, both work. Directory
creation is never permitted regardless of the quirk.
"""

import os
import subprocess
import tempfile

from framework import EDIT_BIN, Edit, expect, fixture, test


def _run_cli(argv, timeout=5.0):
    """Run edit synchronously, return (returncode, combined-output bytes).

    Does not allocate a pty -- intended for the early-exit cases where edit
    prints a message and exits before reaching the TUI loop.
    """
    proc = subprocess.run(
        [EDIT_BIN] + argv,
        capture_output=True,
        timeout=timeout,
    )
    return proc.returncode, proc.stdout + proc.stderr


@test
def default_refuses_missing_file():
    with tempfile.TemporaryDirectory() as tmp:
        target = os.path.join(tmp, "newfile.txt")
        _rc, out = _run_cli([target])
        expect(b"refusing to create new file" in out,
               f"expected refusal message, got: {out!r}")
        expect(b"--quirks=allow-create" in out,
               f"expected quirk hint, got: {out!r}")
        expect(not os.path.exists(target),
               "edit created the file despite refusing")


@test
def allow_create_opens_missing_file():
    with tempfile.TemporaryDirectory() as tmp:
        target = os.path.join(tmp, "newfile.txt")
        with Edit(["--quirks=allow-create", target]) as ed:
            expect(b"newfile.txt" in ed.plain,
                   f"expected filename in titlebar, got: {ed.plain!r}")


@test
def existing_file_opens_without_quirk():
    # Sanity: the default mode must still open files that do exist.
    with Edit([fixture("hello.txt")]) as ed:
        expect(b"hello.txt" in ed.plain,
               "expected hello.txt to open in default mode")


@test
def mkdir_always_refused():
    # Even with the quirk on, edit must not create parent directories.
    # We use a path under a non-existent subdir; opening succeeds (quirk
    # allows it as a new file), but saving must fail because the parent
    # doesn't exist. We check by asserting the parent dir stays absent
    # after edit exits.
    with tempfile.TemporaryDirectory() as tmp:
        missing_parent = os.path.join(tmp, "does_not_exist")
        target = os.path.join(missing_parent, "newfile.txt")
        # Try open + save via the TUI. Even if save dialogs etc. don't
        # cleanly trigger here, the contract is: directory must not
        # appear. Drive a quick session that tries Ctrl+S then exits.
        with Edit(["--quirks=allow-create", target]) as ed:
            ed.send(b"hello")
            ed.send(b"\x13")  # Ctrl+S
            ed.drain(0.2)
        expect(not os.path.exists(missing_parent),
               f"edit created missing parent dir {missing_parent}")
        expect(not os.path.exists(target),
               f"edit created file under missing parent {target}")
