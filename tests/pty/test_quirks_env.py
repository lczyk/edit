"""EDIT_QUIRKS env var layers in before --quirks; only one --quirks allowed."""

import re

from framework import Edit, expect, fixture, lsh_fixture, test


_SGR_RE = re.compile(rb"\x1b\[[\d;]*m")
_FG_OR_BG_RE = re.compile(rb"\x1b\[(?:38|48);[0-9;]+m")
BOX_V = b"\xe2\x94\x82"


@test
def env_quirks_nocolor_suppresses_sgr():
    with Edit([lsh_fixture("go/kitchen_sink.go")], env={"EDIT_QUIRKS": "nocolor"}) as ed:
        match = _SGR_RE.search(ed.buf)
        expect(match is None,
               f"EDIT_QUIRKS=nocolor leaked SGR: {match.group(0) if match else None!r}")


@test
def cli_negate_overrides_env_quirk():
    # EDIT_QUIRKS=ascii would strip box drawing; --quirks=-ascii must
    # negate that and let box drawing render.
    with Edit(
        ["--quirks=-ascii", fixture("hello.txt")],
        env={"EDIT_QUIRKS": "ascii"},
    ) as ed:
        expect(BOX_V in ed.buf,
               "expected box drawing after --quirks=-ascii negated EDIT_QUIRKS=ascii")


@test
def env_quirks_unknown_token_errors():
    with Edit([fixture("hello.txt")], env={"EDIT_QUIRKS": "bogus"}) as ed:
        expect(b"EDIT_QUIRKS contains unknown quirk" in ed.plain,
               f"missing EDIT_QUIRKS error message: {ed.plain[:300]!r}")


@test
def double_quirks_flag_errors():
    with Edit(
        ["--quirks=ascii", "--quirks=nocolor", fixture("hello.txt")],
    ) as ed:
        expect(b"--quirks may only be passed once" in ed.plain,
               f"missing duplicate-quirks error: {ed.plain[:300]!r}")
