# Terminal Keyboard

`edit` runs inside a host terminal. Every keystroke you make first hits the terminal, which decides whether to:

1. **Act on it itself** (e.g. `Cmd+C` -> copy terminal selection to OS clipboard).
2. **Translate it** (e.g. `Option+letter` -> macOS-native composed character).
3. **Forward it** to the running app as a byte sequence on the PTY.

Only chords in category 3 reach `edit`. Categories 1 and 2 are invisible to it. That's the source of almost every "this shortcut doesn't work" report.

## How `edit` decodes keys

`edit` understands two encodings on the wire:

- **Legacy ASCII / VT escapes** -- single bytes (`\x7f` for Backspace), `ESC <letter>` for Alt-modified chords, `CSI ... ~` for special keys.
- **Kitty keyboard protocol** -- a CSI-u encoding that disambiguates modifier combinations the legacy form can't express (`Cmd+letter`, `Alt+special-key`, etc.). On startup `edit` pushes flag 1 (`CSI > 1 u` in `setup_terminal`) and pops it on exit (`CSI < u`).

The kitty protocol is what makes `Cmd+anything` reachable at all -- legacy ASCII has no way to encode the Cmd / Super modifier. Terminals that don't support it (Terminal.app, iTerm2 without explicit opt-in) cap you at what legacy escapes can carry.

Known-good terminals (kitty proto support): Ghostty, kitty, WezTerm, Alacritty >= 0.14.

## Symptoms and what they mean

The diagnostic shortcut is **asymmetry between sibling chords**:

- `Cmd+X` works in `edit` but `Cmd+C` doesn't -> terminal claimed `Cmd+C` for native copy. Forward it.
- `Option+Delete` (= `Option+Fn+Backspace`) does word-delete but `Option+Backspace` deletes one char -> Option key isn't producing the Alt modifier for the regular Backspace path. Either the terminal needs `option_as_alt` enabled, or it sends the legacy `ESC <key>` form for some keys and a kitty CSI for others. (Both: `edit` decodes both pathways; the terminal's `option_as_alt` setting controls whether Option produces Alt at all.)
- A chord does nothing whatsoever -> either the terminal is silently swallowing it (some shortcuts have no default action but are still "claimed"), or `edit`'s parser doesn't decode that exact sequence. The dev input log (`--logfile`, debug builds) prints the raw `Input` event so you can tell which.

## Common offenders on macOS

| Chord | Default terminal action | Fix |
|---|---|---|
| `Cmd+C` | copy terminal selection to OS clipboard | forward in alt-screen, keep default in normal screen |
| `Cmd+V` | paste OS clipboard as bracketed paste | usually fine -- `edit` handles bracketed paste as a paste event |
| `Cmd+F` | terminal search | forward to running program |
| `Cmd+Q` | quit terminal | leave alone (you want this) |
| `Cmd+N` / `Cmd+T` | new window / tab | leave alone |
| `Option+Backspace` | depends on `option_as_alt` | enable `option_as_alt` so Option carries Alt modifier |

## Per-terminal config

- [Alacritty](./alacritty.md) -- worked example with snippets.
- For other terminals (Ghostty, kitty, WezTerm) the equivalent is `unbind` / `discard_event` / config override of the relevant key. Check the terminal's docs.

## Hard cases

- **System clipboard sync**: `edit`'s `tb.copy()` writes to an internal clipboard, not the macOS clipboard. Forwarding `Cmd+C` to `edit` lets `edit`'s copy fire, but the result lands in the in-process clipboard only. To round-trip with the OS clipboard, `edit` would need to emit OSC 52 on copy. Not currently implemented.
- **Bracketed-paste path is fine**: when the terminal intercepts `Cmd+V` and sends the OS clipboard as bracketed paste, `edit` handles it via `Input::Paste` -- the synthesised key event uses `KBMOD_PRIMARY | vk::V` so the textarea's paste arm fires.
