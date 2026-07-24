
# AGENTS.md

Project-level guidance for AI coding assistants working on this codebase.

User-facing reference (terminal interop, keybinding syntax, debug-log format) lives in the knowledge base under [`doc/`](doc/) -- build with `make docs-serve`. This file stays focused on what the agent needs to know to act correctly in the repo.

## Context

Private fork of Microsoft's `edit` terminal editor, trimmed down for personal use. Not published, not packaged, no upstream contributions. Ignore anything on the internet that frames this as a Microsoft/MSDOS product.

## Scope and platform

- **Targets:** Linux and macOS. Windows support has been removed -- do not reintroduce `#[cfg(windows)]`, `windows-sys`, `winresource`, drive pickers, `\\` path handling, or `EDIT_CFG_*` Windows SONAMEs.
- **Single-file, single-buffer.** Invoke as `edit FILE`. No path -> print help and exit. More than one path -> error. No `New`/`Open`/`Close` menu items, no file picker tree, no fuzzy cross-document navigation, no stdin-redirect scratch buffer. `Save As` exists only as a plain path-input modal (renames in place).
- **Language:** English only. The `i18n/` directory, `localization` module, `LocId` enum, and `loc()` function have all been deleted. Use plain string literals. Do not add `gettext`-style indirection.
- **No crates.io publish, no distro packaging.** No `categories`, no `repository` URL, no package-maintainer notes, no install scripts, no snap/desktop files.
- **No benchmarks, no fuzzing in-tree.** The `benches/`, `fuzz/`, and `editing-traces/` dirs are gone. Don't add `criterion`, `libfuzzer-sys`, or similar.

## Build and test

Use the [Makefile](Makefile) -- do not invoke `cargo` directly in routine work. Run `make help` to list targets. Common ones:

- `make build` -- release build.
- `make check` / `make clippy` / `make test` -- individual checks.
- `make fmt` / `make fmt-check` -- formatting.
- `make verify` -- full pre-commit gate (fmt-check + clippy + test). Run this before reporting a task as done.
- `make docs-serve` / `make docs-build` -- knowledge base under [`doc/`](doc/) (mdBook).

ICU is loaded via `dlopen` at runtime. If missing, Search/Replace degrades gracefully. See [README.md](README.md) for `EDIT_CFG_ICU*` env vars.

## Cmd modifier and terminal interop

`kbmod::CMD` exists alongside `CTRL`/`ALT`/`SHIFT` and maps to Super in the [kitty keyboard protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/). The editor pushes flag 1 on startup (`CSI > 1 u` in `edit::term::setup`) and pops on exit (`CSI < u` from `edit::term::RestoreModes`). Textarea standard chords (Cut/Copy/Paste/Undo/Redo/SelectAll) pick the platform primary modifier via `KBMOD_PRIMARY` (Cmd on macOS, Ctrl elsewhere); word-nav-on-backspace/delete uses `KBMOD_FOR_WORD_NAV` (Alt on macOS, Ctrl elsewhere).

User-facing detail (terminal compat, alacritty `option_as_alt`, `Cmd+C` swallowing, etc.) lives in the knowledge base: [doc/src/terminal-keyboard.md](doc/src/terminal-keyboard.md) and [doc/src/alacritty.md](doc/src/alacritty.md). When a "this chord doesn't work" report lands, point there before changing code.

## Keybindings

Config: `<config_dir>/keybindings.toml`, auto-created on first run from [keybindings.macos.toml](crates/edit/src/bin/edit/keybindings.macos.toml) / [keybindings.linux.toml](crates/edit/src/bin/edit/keybindings.linux.toml). The `Action` enum in [crates/edit/src/bin/edit/keybindings.rs](crates/edit/src/bin/edit/keybindings.rs) lists what's bindable -- menubar items and a few editor commands. Dialog-internal keys (Return/Escape/Arrows/Backspace) stay hardcoded.

Full chord syntax + bindable-action reference: [doc/src/keybindings.md](doc/src/keybindings.md).

## Dev input log (`--logfile`)

Debug builds accept `--logfile=PATH` and append a JSONL stream of `Input` events + post-frame `TextBuffer` snapshots. Implementation in [crates/edit/src/bin/edit/devlog.rs](crates/edit/src/bin/edit/devlog.rs), gated by `#[cfg(debug_assertions)]`. When the user pastes excerpts, treat `cursor`, `selection`, `dirty`, `lines` as ground truth for what actually happened.

Record shape + field reference: [doc/src/dev-input-log.md](doc/src/dev-input-log.md).

## Architecture

- **Text buffer ([crates/edit/src/buffer/](crates/edit/src/buffer/))** does not track line breaks. Only the current cursor position is kept; navigation seeks `O(n)` through the document. Every other perf decision flows from this:
  - [crates/edit/src/simd/](crates/edit/src/simd/) -- `memchr2` line-break scanners (>100 GB/s).
  - [crates/edit/src/unicode/](crates/edit/src/unicode/) -- `Utf8Chars` iterator (4 GB/s, transparently inserts U+FFFD) and `MeasurementConfig` grapheme/width measurement (600 MB/s).
  - Without word-wrap, `memchr2` drives all line navigation -- 1 GB files feel like 1 MB.
- **[crates/edit/src/framebuffer.rs](crates/edit/src/framebuffer.rs)** -- video-game-style framebuffer. UI draws into a buffer; diff against the previous frame is sent to the terminal.
- **[crates/edit/src/tui.rs](crates/edit/src/tui.rs)** -- immediate-mode UI. Read its module doc.
- **[crates/edit/src/vt.rs](crates/edit/src/vt.rs)** -- VT parser.
- **[crates/edit/src/sys/](crates/edit/src/sys/)** -- platform abstractions (unix only): terminal i/o (raw mode, sigwinch resize injection, polling stdin reader, `write_stdout`) plus the fs + ICU helpers.
- **[crates/edit/src/term.rs](crates/edit/src/term.rs)** -- alt-screen mode switch, OSC 4/10/11 palette probe, ambiguous-width probe, kitty kbd proto push; `RestoreModes` is the inverse-on-drop guard. Used by `bin/edit/main.rs` and by `edit::mount`.
- **[crates/edit/src/mount.rs](crates/edit/src/mount.rs)** -- thin external mount api for the tui: `mount(opts, draw_fn)` owns `Tui::new` + `term::setup` + the input/render loop + alt-screen restore. Used by the `eat` persona's snapshot view; not used by `bin/edit/main.rs` (which has its own richer loop).
- **[crates/edit/src/eat/](crates/edit/src/eat/)** -- the `eat` persona's cli + render glue (snapshot tui via `mount`, follow tui via its own bespoke driver pending phase C). Reachable via argv0 dispatch in `bin/edit/main.rs` (`name == "eat"` or `--eat`); the `eat` binary is a `make install`-time symlink to `edit`, not a separate cargo target.
- **[crates/edit/src/bin/edit/](crates/edit/src/bin/edit/)** -- the binary. ~90% UI and business logic.

Terminal issues: check `vt.rs`, `sys/unix.rs`, and `edit::term::setup` first.

## Crates

- `edit` -- main binary and library. Includes `edit::eat` (busybox-style multicall: when invoked as `eat` via symlink, or with `--eat`, acts as a `bat`-like syntax-highlighting cat).
- `lsh` -- syntax-highlighting compiler and runtime. Language definitions in [crates/lsh/definitions/](crates/lsh/definitions/). See [crates/lsh/README.md](crates/lsh/README.md).
- `lsh-bin` -- CLI for debugging LSH output.
- `lsh-defs` -- bundled lsh language defs codegen + detection helpers + ansi-16 colourmap. Shared by `edit` and `edit::eat`.
- `gutter` -- per-line gutter mark computation + render (git-diff overlays).
- `stdext` -- shared utilities (arena allocator, collections, SIMD helpers, sys shims).
- `unicode-gen` -- codegen for Unicode LUTs (only needed to regenerate tables; tables are checked in).

## Meanderings

Design notes, proposals, and comparisons live in [meanderings/](meanderings/) as `*.prop.md` files with YAML frontmatter (`status`, `date`, `description`). Run [meanderings/index.sh](meanderings/index.sh) for a status-grouped index. See [meanderings/README.md](meanderings/README.md) for the format. Files prefixed `_` are no longer active (implemented or shelved).

## Code conventions

- **Binary size matters.** Don't introduce dependencies lightly. Check whether stdlib or existing helpers already cover the use case.
- **[rustfmt.toml](rustfmt.toml):** stable rustfmt only -- `style_edition = "2024"`, `use_small_heuristics = "Max"`, `newline_style = "Unix"`, `use_field_init_shorthand = true`. Run `cargo fmt` before committing.
- **Clippy:** `--deny warnings` is the CI bar.
- **No comments explaining what well-named code already says.** Only comment hidden constraints, workarounds, or subtle invariants.
- **Rust edition:** 2024, MSRV `1.90` (see `rust-version` in [Cargo.toml](Cargo.toml)).

## Things to avoid

- Reintroducing Windows support, localization, or packaging surface.
- Adding features, dependencies, or abstractions beyond what the task requires.
- Mocking in tests where the real thing is cheap.
- Committing without running `make verify`.
