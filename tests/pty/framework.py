"""Tiny PTY test framework for the `edit` binary.

Run from the repo root:

    python3 tests/pty/framework.py                  # all tests, headless
    python3 tests/pty/framework.py --watch          # all tests, visible in terminal
    python3 tests/pty/framework.py --filter prefill # run tests whose name contains 'prefill'
    python3 tests/pty/framework.py test_typing.py   # run the listed test files

Test files are `test_*.py` in this directory. They import the framework and
register tests with `@test`:

    from framework import *

    @test
    def my_case():
        with Edit([fixture("hello.txt")]) as ed:
            pause(0.4)
            ed.send(CTRL_F)
            pause(0.4); ed.drain()
            expect(b"Find:" in ed.plain, "Find panel didn't open")

`edit` does OSC / CSI probes on startup (colors, cursor position, device
attrs). A real terminal answers; a PTY doesn't. The harness replies with
stubbed responses and — in `--watch` mode — filters the probe bytes out
of the mirrored output so the real terminal doesn't reply to them.
"""

import argparse
import fcntl
import importlib.util
import io
import os
import pty
import re
import select
import shutil
import struct
import sys
import termios
import time
import traceback


# ---- paths / env ----------------------------------------------------------

_HERE = os.path.dirname(os.path.abspath(__file__))
_REPO_ROOT = os.path.abspath(os.path.join(_HERE, "..", ".."))
FIXTURES_DIR = os.path.join(_HERE, "fixtures")
LSH_FIXTURES_DIR = os.path.join(_REPO_ROOT, "crates/lsh/tests/fixtures")
EDIT_BIN = os.environ.get(
    "EDIT_BIN",
    os.path.join(_REPO_ROOT, "target", "release", "edit"),
)


def fixture(name: str) -> str:
    """Return absolute path to a file in `tests/pty/fixtures/`."""
    return os.path.join(FIXTURES_DIR, name)


def lsh_fixture(name: str) -> str:
    """Return absolute path to a file in the lsh golden fixture tree.

    Path is `<lang>/<file>`, e.g. `go/kitchen_sink.go`. These fixtures are
    the source of truth for highlighter content; PTY tests that need a
    realistic source file should reference them rather than duplicating
    content.
    """
    return os.path.join(LSH_FIXTURES_DIR, name)


# ---- ANSI regex -----------------------------------------------------------

_ANSI_RE = re.compile(rb"\x1b\[[0-9;?]*[a-zA-Z]|\x1b\][^\x07]*\x07")

# Edit-emitted sequences that trigger real-terminal replies. We strip them
# from the mirrored stream under --watch so the host terminal doesn't reply
# into the shell's stdin after the harness exits.
_PROBE_RE = re.compile(
    rb"\x1b\]4;[^\x07]*\?[^\x07]*\x07"   # OSC 4 palette query
    rb"|\x1b\]1[01];\?\x07"              # OSC 10 / 11 fg/bg query
    rb"|\x1b\[6n"                        # CPR
    rb"|\x1b\[>?c"                        # primary/secondary Device Attributes
)


# ---- runtime state (tweaked by CLI) ---------------------------------------

WATCH = False
PACE = 1.0

# Base settle time after a `send()` — gives `edit` time to process the input
# and redraw before we read. Scaled by PACE.
SEND_SETTLE = 0.01
DRAIN_TIMEOUT = 0.01

_ORIG_STDOUT = sys.stdout
_ORIG_STDERR = sys.stderr


def pause(seconds: float) -> None:
    """Sleep, scaled by `--pace` (default 2x under `--watch`, else 1x)."""
    time.sleep(seconds * PACE)


_FG_RE = re.compile(rb"\x1b\[38;2;(\d+);(\d+);(\d+)m")


def distinct_fg_colors(raw: bytes) -> set:
    """Set of (R,G,B) foreground colors seen in a raw output buffer.

    Used by highlighting tests — plain text uses one fg, a working
    syntax highlighter produces several.
    """
    return set(_FG_RE.findall(raw))


# Truecolor SGR prefixes for the default palette, keyed by the HighlightKind
# in `crates/edit/src/buffer/mod.rs`. Values match `DEFAULT_THEME` /
# `colormap.toml`.
FG_COMMENT         = b"\x1b[38;2;63;174;58m"    # Green              — Comment
FG_STRING          = b"\x1b[38;2;255;62;48m"    # BrightRed          — String
FG_METHOD          = b"\x1b[38;2;255;201;68m"   # BrightYellow       — Method
FG_VARIABLE        = b"\x1b[38;2;0;225;240m"    # BrightCyan         — Variable
FG_KEYWORD_OTHER   = b"\x1b[38;2;47;106;255m"   # BrightBlue         — keyword.other / constant.language / meta.header / markup.heading / markup.list / markup.changed
FG_NUMERIC         = b"\x1b[38;2;88;234;81m"    # BrightGreen        — constant.numeric / markup.inserted
FG_KEYWORD_CONTROL = b"\x1b[38;2;252;116;255m"  # BrightMagenta      — keyword.control


# ---- ANSI color ------------------------------------------------------------

def _color_enabled() -> bool:
    if os.environ.get("NO_COLOR"):
        return False
    try:
        return _ORIG_STDOUT.isatty()
    except Exception:
        return False


def _c(s: str, code: str) -> str:
    if not _color_enabled():
        return s
    return f"\x1b[{code}m{s}\x1b[0m"


def _green(s): return _c(s, "32")
def _red(s):   return _c(s, "31")
def _dim(s):   return _c(s, "2")
def _bold(s):  return _c(s, "1")


# ---- Edit PTY wrapper -----------------------------------------------------

def _host_term_size(default=(80, 24)):
    try:
        sz = shutil.get_terminal_size(fallback=default)
        return sz.columns, sz.lines
    except Exception:
        return default


def _set_winsize(fd: int, cols: int, rows: int) -> None:
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))


class Edit:
    """Spawns `edit` on a pseudo-terminal. Use as a context manager.

    Call `send()` to inject input, `drain()` to read pending output, inspect
    `plain` (ANSI-stripped whole buffer) or `last_plain_frame(anchor)` to
    assert against the most recent frame.
    """

    def __init__(self, argv=None, cols=None, rows=None):
        argv = argv or []
        if cols is None or rows is None:
            host_cols, host_rows = _host_term_size()
            cols = cols or host_cols
            rows = rows or host_rows

        self.buf = b""
        self.cols = cols
        self.rows = rows
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.environ["TERM"] = "xterm-256color"
            os.environ["LINES"] = str(rows)
            os.environ["COLUMNS"] = str(cols)
            os.execv(EDIT_BIN, [EDIT_BIN] + argv)
        _set_winsize(self.fd, cols, rows)
        # Disable ECHO on the pty line discipline. `edit` puts the terminal
        # into raw mode anyway; after it exits the kernel may restore cooked
        # mode and echo whatever we write (e.g. the `n` we send to dismiss
        # the save-changes dialog) back onto the master — which would show up
        # as a rogue `n` prefix in --watch output.
        try:
            attrs = termios.tcgetattr(self.fd)
            attrs[3] &= ~termios.ECHO  # lflag
            termios.tcsetattr(self.fd, termios.TCSANOW, attrs)
        except termios.error:
            pass
        self._settle()

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, tb):
        self.close()
        return False

    # -- low-level

    def _read(self, timeout=0.3) -> bool:
        r, _, _ = select.select([self.fd], [], [], timeout)
        if not r:
            return False
        try:
            chunk = os.read(self.fd, 8192)
        except OSError:
            return False
        if not chunk:
            return False
        self.buf += chunk
        if WATCH:
            try:
                _ORIG_STDOUT.buffer.write(_PROBE_RE.sub(b"", chunk))
                _ORIG_STDOUT.buffer.flush()
            except (BrokenPipeError, OSError):
                pass
        return True

    def _settle(self) -> None:
        # Drain the startup probe writes.
        self._read(0.25)
        while self._read(0.05):
            pass
        # Stubbed replies so edit's startup loop exits.
        os.write(self.fd, b"\x1b[1;2R")      # CPR reply
        os.write(self.fd, b"\x1b[?62;c")     # DA reply
        # Read until the opening frame finishes painting.
        self._read(0.25)
        while self._read(0.08):
            pass

    # -- public

    def send(self, data: bytes, settle: float = None, drain: bool = True) -> None:
        """Write `data`, let `edit` react, then drain its output.

        Tests rarely need explicit `pause()`/`drain()` anymore. Pass
        `settle=0` or `drain=False` if you want finer control.
        """
        os.write(self.fd, data)
        pause(SEND_SETTLE if settle is None else settle)
        if drain:
            self.drain(DRAIN_TIMEOUT)

    def drain(self, timeout: float = DRAIN_TIMEOUT) -> None:
        while self._read(timeout):
            pass

    @property
    def plain(self) -> bytes:
        return _ANSI_RE.sub(b"", self.buf)

    def last_plain_frame(self, anchor: bytes) -> bytes:
        idx = self.buf.rfind(anchor)
        if idx < 0:
            return b""
        return _ANSI_RE.sub(b"", self.buf[idx:])

    def plain_since(self, mark: int) -> bytes:
        """ANSI-stripped view of bytes received after position `mark`."""
        return _ANSI_RE.sub(b"", self.buf[mark:])

    def mark(self) -> int:
        """Opaque position in the raw byte stream; pass to `plain_since`."""
        return len(self.buf)

    def close(self) -> None:
        try:
            os.write(self.fd, CTRL_Q)
        except OSError:
            pass
        # If the buffer was clean, edit exits immediately and the `n` below
        # is both unnecessary and risks being echoed back. Wait briefly for
        # the child to exit; only dismiss the dialog if it's still running.
        if not self._wait_exit(0.1):
            try:
                os.write(self.fd, b"n")
            except OSError:
                pass
            self._wait_exit(0.1)
        self.drain(0.1)
        try:
            os.close(self.fd)
        except OSError:
            pass

    def _wait_exit(self, timeout: float) -> bool:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                pid, _ = os.waitpid(self.pid, os.WNOHANG)
            except ChildProcessError:
                return True
            if pid:
                return True
            time.sleep(0.01)
        return False


# ---- assertions -----------------------------------------------------------

class ExpectError(AssertionError):
    pass


def expect(cond: bool, msg: str) -> None:
    """Raise on failure. The framework reports a single PASS/FAIL per test."""
    if not cond:
        raise ExpectError(msg)


# ---- test registry --------------------------------------------------------

_TESTS: list = []  # list of (display_name, fn)


def test(fn):
    """Register `fn` as a test. Name = module:function."""
    modname = os.path.splitext(os.path.basename(fn.__code__.co_filename))[0]
    _TESTS.append((f"{modname}::{fn.__name__}", fn))
    return fn


# ---- keys -----------------------------------------------------------------

CTRL_A = b"\x01"
CTRL_C = b"\x03"
CTRL_F = b"\x06"
CTRL_H = b"\x08"
CTRL_Q = b"\x11"
CTRL_V = b"\x16"
CTRL_X = b"\x18"
CTRL_Y = b"\x19"
CTRL_Z = b"\x1a"
ESC = b"\x1b"
ENTER = b"\r"
BACKSPACE = b"\x7f"
TAB = b"\t"
F10 = b"\x1b[21~"
RIGHT = b"\x1b[C"
LEFT = b"\x1b[D"
UP = b"\x1b[A"
DOWN = b"\x1b[B"
HOME = b"\x1b[H"
END = b"\x1b[F"
SHIFT_RIGHT = b"\x1b[1;2C"
SHIFT_LEFT = b"\x1b[1;2D"
SHIFT_DOWN = b"\x1b[1;2B"
SHIFT_UP = b"\x1b[1;2A"
SHIFT_HOME = b"\x1b[1;2H"
SHIFT_END = b"\x1b[1;2F"


# ---- discovery + runner ---------------------------------------------------

def _discover(paths: list) -> None:
    """Import each path so its `@test`-decorated functions register."""
    # Make sibling modules importable (so `from framework import *` works).
    if _HERE not in sys.path:
        sys.path.insert(0, _HERE)

    for p in paths:
        mod_name = "_pty_" + os.path.splitext(os.path.basename(p))[0]
        spec = importlib.util.spec_from_file_location(mod_name, p)
        if spec is None or spec.loader is None:
            raise RuntimeError(f"cannot load {p}")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)


def _collect_default_paths() -> list:
    return sorted(
        os.path.join(_HERE, f)
        for f in os.listdir(_HERE)
        if f.startswith("test_") and f.endswith(".py")
    )


def _run_one(name, fn) -> tuple:
    """Return (ok: bool, msg: str). In --watch, script prints are buffered."""
    buf_out = io.StringIO() if WATCH else None
    buf_err = io.StringIO() if WATCH else None
    if WATCH:
        sys.stdout = buf_out
        sys.stderr = buf_err
    try:
        fn()
        return True, ""
    except ExpectError as e:
        return False, str(e)
    except Exception:
        return False, traceback.format_exc().rstrip()
    finally:
        if WATCH:
            sys.stdout = _ORIG_STDOUT
            sys.stderr = _ORIG_STDERR
            out = buf_out.getvalue() if buf_out else ""
            err = buf_err.getvalue() if buf_err else ""
            if out:
                _ORIG_STDOUT.write(out)
                _ORIG_STDOUT.flush()
            if err:
                _ORIG_STDERR.write(err)
                _ORIG_STDERR.flush()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--watch", action="store_true",
                        help="Mirror PTY output to the real terminal and slow pacing.")
    parser.add_argument("--pace", type=float, default=None,
                        help="Multiplier on pause(s) calls (default 1.0, 3.0 with --watch).")
    parser.add_argument("--filter", default=None,
                        help="Run tests whose registered name contains this substring.")
    parser.add_argument("paths", nargs="*",
                        help="Test files to run (default: all test_*.py in this dir).")
    args = parser.parse_args()

    global WATCH, PACE
    WATCH = args.watch
    PACE = args.pace if args.pace is not None else (2.0 if args.watch else 1.0)

    paths = args.paths or _collect_default_paths()
    # Resolve relative paths.
    paths = [p if os.path.isabs(p) else os.path.abspath(p) for p in paths]
    _discover(paths)

    tests = _TESTS
    if args.filter:
        tests = [t for t in tests if args.filter in t[0]]

    if not tests:
        print("no tests matched.")
        sys.exit(0)

    passed = 0
    failures = []
    total_dt = 0.0
    for name, fn in tests:
        t0 = time.perf_counter()
        ok, msg = _run_one(name, fn)
        dt = time.perf_counter() - t0
        total_dt += dt
        dt_s = _dim(f"({dt:5.2f}s)")
        if ok:
            passed += 1
            print(f"{_green('PASS')} {dt_s}  {name}")
        else:
            failures.append((name, msg))
            print(f"{_red('FAIL')} {dt_s}  {name}")
            for line in msg.splitlines():
                print(f"        {line}")

    total = passed + len(failures)
    summary = f"{passed}/{total} passed"
    if failures:
        summary = f"{_red(summary)}, {len(failures)} failed"
    else:
        summary = _green(summary)
    print(f"\n{summary} in {total_dt:.2f}s")
    sys.exit(0 if not failures else 1)


if __name__ == "__main__":
    # Re-enter as the importable `framework` module so `@test` decorators in
    # test files (which do `from framework import test`) register into the
    # same _TESTS list the runner reads. Running as `__main__` would give
    # them a second, empty copy.
    sys.path.insert(0, _HERE)
    import framework as _self  # noqa: E402
    _self.main()
