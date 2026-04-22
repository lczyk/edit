# AGENTS.md

Project-level guidance for AI coding assistants working on this codebase.

## Context

Private fork of Microsoft's `edit` terminal editor, trimmed down for personal use. Not published, not packaged, no upstream contributions. Ignore anything on the internet that frames this as a Microsoft/MSDOS product.

## Scope and platform

- **Targets:** Linux and macOS. Windows support has been removed — do not reintroduce `#[cfg(windows)]`, `windows-sys`, `winresource`, drive pickers, `\\` path handling, or `EDIT_CFG_*` Windows SONAMEs.
- **Single-file, single-buffer.** Invoke as `edit FILE`. No path → print help and exit. More than one path → error. No `New`/`Open`/`Close` menu items, no file picker tree, no fuzzy cross-document navigation, no stdin-redirect scratch buffer. `Save As` exists only as a plain path-input modal (renames in place).
- **Language:** English only. The `i18n/` directory, `localization` module, `LocId` enum, and `loc()` function have all been deleted. Use plain string literals. Do not add `gettext`-style indirection.
- **No crates.io publish, no distro packaging.** No `categories`, no `repository` URL, no package-maintainer notes, no install scripts, no snap/desktop files.
- **No benchmarks, no fuzzing in-tree.** The `benches/`, `fuzz/`, and `editing-traces/` dirs are gone. Don't add `criterion`, `libfuzzer-sys`, or similar.

## Build and test

Use the [Makefile](Makefile) — do not invoke `cargo` directly in routine work. Run `make help` to list targets. Common ones:

- `make build` / `make build-nightly` — release builds.
- `make check` / `make clippy` / `make test` — individual checks.
- `make fmt` / `make fmt-check` — formatting.
- `make verify` — full pre-commit gate (fmt-check + clippy + test). Run this before reporting a task as done.

ICU is loaded via `dlopen` at runtime. If missing, Search/Replace degrades gracefully. See [README.md](README.md) for `EDIT_CFG_ICU*` env vars.

## Cmd / Super modifier

`kbmod::CMD` is available alongside `CTRL`/`ALT`/`SHIFT` and maps to the Super modifier in the [kitty keyboard protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/). The editor pushes flag 1 (disambiguate escape codes) on startup via `CSI > 1 u` in `setup_terminal` and pops on exit via `CSI < u`.

Reaches the editor only when (a) the terminal supports the protocol and (b) the terminal forwards Cmd rather than binding it at the window level. Known-good: Ghostty, kitty, WezTerm, Alacritty ≥ 0.14. Any built-in terminal shortcut (Cmd+Q, Cmd+C, …) must be cleared in the terminal's config before that chord reaches the editor.

## Keybindings

Config file: `<config_dir>/keybindings.toml`. Auto-created on first run from a platform-specific embedded default: [keybindings.macos.toml](crates/edit/src/bin/edit/keybindings.macos.toml) or [keybindings.linux.toml](crates/edit/src/bin/edit/keybindings.linux.toml). Location: `$XDG_CONFIG_HOME/edit/keybindings.toml` (fallback `~/.config/edit/keybindings.toml`) on both platforms.

Configurable actions (21) in [crates/edit/src/bin/edit/keybindings.rs](crates/edit/src/bin/edit/keybindings.rs) `Action` enum — menubar items only. Dialog-internal keys (Return/Escape/Arrows/Backspace) stay hardcoded.

Chord syntax: `"Ctrl+Shift+F"`, `"Cmd+P"`, `"F10"`, `"A"`, `""` (unbound). Modifier names: `Ctrl`, `Alt`, `Shift`, `Cmd` (alias `Super`). Key names: letter A-Z, digit 0-9, Up/Down/Left/Right, Home/End/PageUp/PageDown, Insert/Delete, Tab/Back/Return/Escape/Space, F1..F24, Numpad0..Numpad9.

## Dev input log (`--logfile`)

Debug builds accept `--logfile=PATH`. Each `Input` event (keyboard, mouse, resize, paste, text) is written to PATH as JSONL alongside a snapshot of the active [`TextBuffer`](crates/edit/src/buffer/mod.rs) *after* the frame is processed. Meant for the "I pressed X, expected Y" feedback loop — not for crash debugging.

Record shape:

```json
{"ts_ms":123,"input":{"kind":"key","key":"Ctrl+S"},"buffer":{"cursor":[5,3],"visual":[5,3],"offset":78,"preferred_col":5,"selection":null,"dirty":true,"lines":42}}
```

Fields: `cursor` = logical `[x, y]` (grapheme col, line). `visual` = laid-out `[x, y]` (affected by word wrap and tabs). `offset` = byte offset in the buffer. `preferred_col` = sticky visual column carried across vertical motion so short lines don't lose x. `selection` = `[[x0,y0],[x1,y1]]` or `null`.

Implementation: [crates/edit/src/bin/edit/devlog.rs](crates/edit/src/bin/edit/devlog.rs). Gated behind `#[cfg(debug_assertions)]` so release builds carry zero cost. When you paste log excerpts here, I'll read the `cursor`, `selection`, `dirty`, `lines` fields as ground truth for what actually happened.

## Architecture

- **Text buffer ([crates/edit/src/buffer/](crates/edit/src/buffer/))** does not track line breaks. Only the current cursor position is kept; navigation seeks `O(n)` through the document. Every other perf decision flows from this:
  - [crates/edit/src/simd/](crates/edit/src/simd/) — `memchr2` line-break scanners (>100 GB/s).
  - [crates/edit/src/unicode/](crates/edit/src/unicode/) — `Utf8Chars` iterator (4 GB/s, transparently inserts U+FFFD) and `MeasurementConfig` grapheme/width measurement (600 MB/s).
  - Without word-wrap, `memchr2` drives all line navigation — 1 GB files feel like 1 MB.
- **[crates/edit/src/framebuffer.rs](crates/edit/src/framebuffer.rs)** — video-game-style framebuffer. UI draws into a buffer; diff against the previous frame is sent to the terminal.
- **[crates/edit/src/tui.rs](crates/edit/src/tui.rs)** — immediate-mode UI. Read its module doc.
- **[crates/edit/src/vt.rs](crates/edit/src/vt.rs)** — VT parser.
- **[crates/edit/src/sys/](crates/edit/src/sys/)** — platform abstractions (unix only).
- **[crates/edit/src/bin/edit/](crates/edit/src/bin/edit/)** — the binary. ~90% UI and business logic, plus `setup_terminal` in [main.rs](crates/edit/src/bin/edit/main.rs).

Terminal issues: check `vt.rs`, `sys/unix.rs`, and `setup_terminal` first.

## Crates

- `edit` — main binary and library.
- `lsh` — syntax-highlighting compiler and runtime. Language definitions in [crates/lsh/definitions/](crates/lsh/definitions/). See [crates/lsh/README.md](crates/lsh/README.md).
- `lsh-bin` — CLI for debugging LSH output.
- `stdext` — shared utilities (arena allocator, collections, SIMD helpers, sys shims).
- `unicode-gen` — codegen for Unicode LUTs (only needed to regenerate tables; tables are checked in).

## Code conventions

- **Binary size matters.** Don't introduce dependencies lightly. Check whether stdlib or existing helpers already cover the use case.
- **[rustfmt.toml](rustfmt.toml):** `style_edition = "2024"`, `use_small_heuristics = "Max"`, `group_imports = "StdExternalCrate"`, `imports_granularity = "Module"`. Run `cargo fmt` before committing.
- **Clippy:** `--deny warnings` is the CI bar.
- **No comments explaining what well-named code already says.** Only comment hidden constraints, workarounds, or subtle invariants.
- **Rust edition:** 2024, MSRV `1.93`.

## Things to avoid

- Reintroducing Windows support, localization, or packaging surface.
- Adding features, dependencies, or abstractions beyond what the task requires.
- Mocking in tests where the real thing is cheap.
- Committing without running `make verify`.
