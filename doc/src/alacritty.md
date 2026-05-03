# Alacritty

Tested with Alacritty >= 0.14 on macOS. Config lives at `~/.config/alacritty/alacritty.toml`.

## `option_as_alt`

Required for `Option+letter` and `Option+Backspace` to carry the Alt modifier. Without it, Option behaves as the macOS native option key (composes accents etc.) and Alt-modified chords never reach `edit`.

```toml
[window]
option_as_alt = "Both"
```

Values:

- `"None"` (default) -- Option behaves as native macOS option key. Alt-modified chords lost.
- `"OnlyLeft"` / `"OnlyRight"` -- only the named side acts as Alt; the other keeps native behaviour. Useful if you still want `Option+E` -> `é` etc. on one side.
- `"Both"` -- both Option keys act as Alt. Loses native composed-char input entirely.

## Forwarding `Cmd+F`

Alacritty's default `Cmd+F` opens the built-in `SearchForward`. To forward to the running program (so `edit`'s `Cmd+F` Find chord fires), send `Ctrl+F` (`0x06`) on the PTY:

```toml
[[keyboard.bindings]]
key = "F"
mods = "Command"
chars = ""
```

This sends a literal `Ctrl+F` byte. Works because `edit`'s textarea historically also accepted `Ctrl+F` -- though the canonical chord is now `Cmd+F` via the kitty protocol on macOS, the legacy byte still routes correctly through the menubar Find action.

Alternative (cleaner): use `action = "ReceiveChar"` to let the chord pass through as a kitty-encoded keypress, same pattern as `Cmd+C` below. Either works.

## Forwarding `Cmd+C` (split per screen mode)

The trap: registering a user binding for `Cmd+C` shadows Alacritty's default `Copy` action **everywhere**, even if the binding is `mode`-gated. So if you only register the alt-screen forwarding rule, normal-screen `Cmd+C` (copy terminal selection in your shell) silently breaks.

Fix: register **both** halves explicitly. One for alt-screen (forward to TUI app), one for normal-screen (re-register the default `Copy`).

```toml
# Cmd+C in alt-screen mode (TUI apps like `edit`) -> forward to program.
[[keyboard.bindings]]
key = "C"
mods = "Command"
mode = "Alt"
action = "ReceiveChar"

# Cmd+C in normal-screen mode (shell) -> default Copy.
[[keyboard.bindings]]
key = "C"
mods = "Command"
mode = "~Alt"
action = "Copy"
```

`mode = "Alt"` matches when the alt-screen buffer is active (most TUI apps including `edit`). `mode = "~Alt"` is the inverse.

## Other useful bits

- **Reload config** without restart: `Ctrl+Shift+R` (Alacritty default).
- **Inspect what the terminal sends**: build `edit` in debug mode and run with `--logfile=/tmp/edit.log` -- each `Input` event lands in JSONL. If a chord does nothing in `edit`, check whether it's logged at all (-> terminal swallowed it) or logged with the wrong modifiers (-> parser issue).
- **Creating a new window** (`Cmd+N`) is in the user's existing config as an example of explicit Cmd binding that doesn't conflict with `edit`.

## Worked example: full config block

A minimal addition that gets `edit` working with `Cmd+C`, `Cmd+F`, and `Option+Backspace`:

```toml
[window]
option_as_alt = "Both"

[[keyboard.bindings]]
key = "F"
mods = "Command"
chars = ""

[[keyboard.bindings]]
key = "C"
mods = "Command"
mode = "Alt"
action = "ReceiveChar"

[[keyboard.bindings]]
key = "C"
mods = "Command"
mode = "~Alt"
action = "Copy"
```

Restart or reload Alacritty after editing.
