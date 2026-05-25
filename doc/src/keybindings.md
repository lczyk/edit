# Keybindings

User-editable config file controls a subset of `edit`'s shortcuts. The rest (dialog-internal keys: Return / Escape / Arrows / Backspace) are hardcoded.

## Config file location

`<config_dir>/keybindings.toml`. Auto-created on first run from a platform-specific embedded default.

- `$XDG_CONFIG_HOME/edit/keybindings.toml`
- Fallback: `~/.config/edit/keybindings.toml`

Same path on Linux and macOS. Embedded defaults live at `crates/edit/src/bin/edit/keybindings.macos.toml` and `crates/edit/src/bin/edit/keybindings.linux.toml` respectively.

## What's bindable

The `Action` enum in `crates/edit/src/bin/edit/keybindings.rs` lists every bindable command. Roughly:

- Menubar items: `exit`, `save`, `undo`, `redo`, `cut`, `copy`, `paste`, `select_all`, `find`, `replace`, `go_to_line`, `toggle_word_wrap`, `open_about`, `focus_menubar`, `focus_statusbar`.
- Editor commands: `move_line_up` / `_down`, `delete_line`, `toggle_line_comment`, `delete_to_line_start` / `_end`.
- Cursor motion (mostly relevant on macOS where Cmd is the natural modifier): `small_jump_up` / `_down` (+ `_select` variants), `line_start` / `_end` (+ `_select`).

Anything not in the `Action` enum is hardcoded -- typically because it's wired directly into the textarea key dispatch (`tui.rs`) for things like `Cmd+Z` undo, `Cmd+C` copy, etc. Those use a `KBMOD_PRIMARY` constant that resolves to `Cmd` on macOS and `Ctrl` elsewhere, so they follow platform convention without needing per-key config.

## Chord syntax

```toml
undo            = "Cmd+Z"
redo            = "Cmd+Shift+Z"
focus_menubar   = "F10"
select_all      = "Cmd+A"
focus_statusbar = ""           # unbound
```

- **Modifier names:** `Ctrl`, `Alt`, `Shift`, `Cmd` (alias `Super`).
- **Key names:** letter `A`-`Z`, digit `0`-`9`, `Up`/`Down`/`Left`/`Right`, `Home`/`End`/`PageUp`/`PageDown`, `Insert`/`Delete`, `Tab`/`Back`/`Return`/`Escape`/`Space`, `F1`..`F24`, `Numpad0`..`Numpad9`.
- **Empty string** (`""`) leaves an action unbound. This is the linux default for several macOS-only chords (e.g. `delete_to_line_start`, `line_start`).
- **One chord per action.** No multi-bind list. If you want both `Ctrl+Z` and `Cmd+Z` for undo, you'll have to pick one.

## Cmd modifier on macOS

`Cmd` is recognised via the [kitty keyboard protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/). `edit` pushes flag 1 (`CSI > 1 u`) on startup so terminals that support the protocol encode `Cmd+anything` as a CSI-u sequence the parser can decode.

The chord still has to **reach** `edit` -- terminals frequently claim popular Cmd chords for native actions (copy, find, ...) before forwarding. See [Terminal Keyboard](./terminal-keyboard.md) for the diagnosis pattern and [Alacritty](./alacritty.md) for a worked config.

## Reloading

`edit` reads `keybindings.toml` once at startup. To pick up changes, restart the editor.
