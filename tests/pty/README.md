# PTY tests for `edit`

Small test framework that drives the `edit` binary through a pseudo-terminal
and asserts against its rendered output.

## Running

Build first:

```sh
cargo build --release
```

Run everything:

```sh
python3 tests/pty/framework.py
```

Watch the editor live while tests run:

```sh
python3 tests/pty/framework.py --watch
```

`--watch` mirrors child-PTY output to your real terminal, sizes the PTY to
your current window, filters the startup probe sequences (so the terminal
doesn't reply to them and leak `rgb:…` bytes onto your shell afterwards), and
slows scripted `pause()` calls 3× so you can see what's happening. Tune pace
with `--pace 5`.

Run a subset:

```sh
python3 tests/pty/framework.py --filter prefill
python3 tests/pty/framework.py tests/pty/test_typing.py
```

Point at a debug build:

```sh
EDIT_BIN=target/debug/edit python3 tests/pty/framework.py
```

## Layout

- `framework.py` — library + CLI entrypoint. Contains `Edit`, key constants,
  `expect`, `pause`, `fixture`, the `@test` registry, and the discovery runner.
- `fixtures/` — sample input files.
- `test_*.py` — each defines `@test`-decorated functions.

## Writing a test

```python
# tests/pty/test_example.py
from framework import CTRL_F, Edit, SHIFT_RIGHT, expect, fixture, test


@test
def ctrl_f_prefills_selection():
    with Edit([fixture("hello.txt")]) as ed:
        ed.send(SHIFT_RIGHT * 5)   # select 5 chars
        ed.send(CTRL_F)            # open Find
        frame = ed.last_plain_frame(b"Find:")
        expect(b"Find: hello" in frame, f"got: {frame[:120]!r}")
```

`ed.send()` writes the bytes, lets `edit` settle, and drains the resulting
output — so most tests need no explicit `pause` or `drain` calls. Override
per call: `ed.send(data, settle=0.3, drain=False)`.

Key pieces:

- `Edit(argv, cols=?, rows=?)` — spawns the editor. Use as a context manager.
- `ed.send(bytes, settle=?, drain=?)` — write + settle + drain.
- `ed.drain(timeout=?)` / `ed.plain` — manual read + ANSI-stripped buffer.
- `ed.last_plain_frame(anchor)` — stripped view from the last occurrence of
  `anchor` onward; use for assertions on the most recent frame.
- `ed.mark()` / `ed.plain_since(mark)` — assert only on bytes received after
  a known point.
- `expect(cond, msg)` — fail the test if `cond` is falsy.
- `pause(s)` — `time.sleep(s * PACE)`; rarely needed.
- `fixture("name")` — absolute path to a file in `fixtures/`.
- Key constants: `CTRL_A…CTRL_Z`, `ESC`, `ENTER`, `BACKSPACE`, `TAB`, `F10`,
  arrows (`LEFT`/`RIGHT`/`UP`/`DOWN`), `HOME`/`END`, and the `SHIFT_*` variants.

## Output

Each test reports `PASS`/`FAIL` (colorized on TTY), its wall-clock duration,
and total runtime at the end:

```
PASS ( 0.46s)  test_checkbox_ascii::no_unicode_checkboxes_in_search_panel
...
8/8 passed in 5.99s
```

Set `NO_COLOR=1` to disable color.

## Caveats

- Timings use plain `time.sleep` under the hood. If CI is slow, bump pauses.
- `framework.py` answers only the startup probes that actually block `edit`.
  If you add new probes to the editor, extend `Edit._settle()`.
- `close()` sends Ctrl+Q then `n` to decline the "save changes" dialog;
  keybindings for Exit are assumed to be Ctrl+Q on both platforms.
