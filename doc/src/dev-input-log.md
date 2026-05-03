<!-- cspell:ignore keybinding -->

# Dev Input Log (`--logfile`)

Debug-build feature for the "I pressed X, expected Y" feedback loop. Not for crash debugging -- there's no panic capture, just an event stream.

## Usage

```sh
make build  # release build won't include this -- you need a debug build
cargo build  # plain debug build
./target/debug/edit FILE --logfile=/tmp/edit.log
```

`--logfile=PATH` is gated behind `#[cfg(debug_assertions)]` so release builds carry zero overhead. Implementation in `crates/edit/src/bin/edit/devlog.rs`.

## Record format

JSONL -- one record per `Input` event (keyboard, mouse, resize, paste, text). Each record bundles the raw input with a snapshot of the active `TextBuffer` taken **after** the frame is processed, so you see the state the input produced.

```json
{"ts_ms":123,"input":{"kind":"key","key":"Ctrl+S"},"buffer":{"cursor":[5,3],"visual":[5,3],"offset":78,"preferred_col":5,"selection":null,"dirty":true,"lines":42}}
```

## Field reference

`input` -- the raw event. Common kinds:

| `kind` | extra fields | meaning |
|---|---|---|
| `key` | `key: "Ctrl+S"` | a decoded keyboard chord |
| `mouse` | position, button, scroll | mouse event |
| `paste` | `bytes: <len>` | bracketed paste |
| `text` | `text: "..."` | UTF-8 text input |
| `resize` | rows, cols | terminal resize |

`buffer` -- post-frame snapshot:

| field | meaning |
|---|---|
| `cursor` | logical `[x, y]` -- grapheme column, line. Independent of word wrap. |
| `visual` | laid-out `[x, y]` -- affected by word wrap and tab expansion. |
| `offset` | byte offset into the buffer. |
| `preferred_col` | sticky visual column, carried across vertical motion so short lines don't lose x. |
| `selection` | `[[x0, y0], [x1, y1]]` of the selection's logical anchors, or `null` when no selection. |
| `dirty` | whether the buffer has unsaved changes. |
| `lines` | logical line count. |

## Use cases

- **Reproducing a bug.** Capture a session, then replay the events into a test or share the log so the symptom is concrete.
- **Pasting in a chat.** Excerpts from the log are unambiguous ground truth -- `cursor`, `selection`, `dirty`, `lines` describe what actually happened, separate from what you intended.
- **Watching keybinding decoding.** When a chord doesn't fire, the log tells you whether it reached the parser at all (and with what modifiers). Pairs naturally with [Terminal Keyboard](./terminal-keyboard.md) diagnostics.
