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

## Cmd chords need no config at all

Alacritty encodes `Command` as Super in the kitty protocol on its own. Every `Cmd+<key>` that Alacritty does not claim for itself already arrives as a genuine Cmd chord -- `Cmd+Z`, `Cmd+A`, `Cmd+S`, `Cmd+L`, `Cmd+G`, `Cmd+/`, `Cmd+Shift+K` and the rest need no bindings at all.

So the only chords worth configuring are the handful Alacritty claims: `Cmd+C`, `Cmd+F`, `Cmd+B`, `Cmd+K`, `Cmd+N`, `Cmd+W`, `Cmd+Q`. Anything else you add is at best redundant -- and a binding that rewrites the chord into different bytes will actively break it.

In particular, do **not** rewrite a Cmd chord into a legacy control byte. `edit` binds Find to `Cmd+F` on macOS, and a `0x06` byte cannot express Super, so a `chars`-based `Ctrl+F` rewrite loses the chord entirely.

## Forwarding `Cmd+F`

Alacritty's default `Cmd+F` opens the built-in `SearchForward`. Forward it the way `Cmd+C` is forwarded below -- and for the same reason, register both halves, or the terminal's own search stops working in the shell:

```toml
# Cmd+F in alt-screen mode (TUI apps like `edit`) -> forward to program.
[[keyboard.bindings]]
key = "F"
mods = "Command"
mode = "Alt"
action = "ReceiveChar"

# Cmd+F in normal-screen mode (shell) -> Alacritty's own search.
[[keyboard.bindings]]
key = "F"
mods = "Command"
mode = "~Alt"
action = "SearchForward"
```

## `Cmd+Q` and `Cmd+W` belong to the terminal

Both quit Alacritty and neither reaches `edit`. That is why the shipped macOS bindings leave Exit on `Ctrl+Q`: bound to `Cmd+Q` it would never fire, and the terminal would exit with the document still open and no save-changes prompt.

`Cmd+W` carries the same hazard and has no editor-side fix -- it closes the window out from under whatever is running.

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
mode = "Alt"
action = "ReceiveChar"

[[keyboard.bindings]]
key = "F"
mods = "Command"
mode = "~Alt"
action = "SearchForward"

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
