---
status: open
date: 2026-05-23
description: bring eat's follow_tui under edit's tui machinery so the workspace has one TUI; keep two output pipelines (framebuffer vs ansi-stream) since they serve different targets
---

# unification plan: one tui for eat + edit

## update 2026-05-25

phase A landed:
- `edit::term` (hoisted terminal setup + probe, restore guard)
- `edit::mount` (thin external mount api: `mount(opts, draw_fn)`)
- `buffer::IoError: Debug` (ergonomic fix for external callers)

dep-direction question resolved by **absorbing eat into edit**: the
`crates/eat/` workspace crate is gone, its code lives at
`crates/edit/src/eat/` as `edit::eat` module. The `eat` binary is now
just a `make install`-time symlink to `edit`; argv0 dispatch in
`bin/edit/main.rs` routes `eat` -> `edit::eat::main()`. No cycle, no
inversion, no extra crate.

phases B/C/D below now describe intra-crate refactors, much easier than
the original cross-crate plan.

## thesis

eat and edit are already aligned at the syntactic-data layer -- shared
crates own the lsh definitions, language detection, simd primitives, the
`Highlighter` wrapper, and gutter rendering. divergence below that layer
splits into two categories:

1. **output-target divergence -- by design.** edit paints a framebuffer
   for an interactive alt-screen tty; eat writes raw ansi escapes to a
   `Writer` for pipes / pagers / non-tty stdout. these are different
   targets and cannot collapse -- `eat foo.go | grep` literally cannot
   accept a framebuffer.
2. **TUI divergence -- accidental.** eat's `follow_tui.rs` (~2000 lines)
   implements its own alt-screen viewer: vt parser, key dispatch,
   cursor positioning, ansi composition, scroll state. edit's
   `tui.rs` does all of these too, on the heavier framebuffer-centric
   model w/ the new physics/animator/draw split. two tuis, one
   workspace -- the alignment work worth doing.

target end state:

- **one tui codebase (edit's).** eat mounts edit's tui machinery for its
  alt-screen modes (snapshot view, follow view). eat's bespoke vt parser
  / scroll machinery / ansi composition (alt-screen path only) goes
  away.
- **two render pipelines, kept distinct on purpose.**
    - alt-screen / tty viewer -- framebuffer (edit's tui), used by both
      interactive edit and eat's alt-screen modes.
    - pipe / non-tty stdout -- ansi-stream (eat's existing
      `write_highlighted_line`), used by eat's non-tty paths only.

## current state

shared infra (already done):
- [crates/lsh/](../crates/lsh) -- pure highlighter library.
- [crates/lsh-defs/](../crates/lsh-defs) -- bundled defs codegen,
  `detect::*`, `Highlighter`, `ReadableDocument`.
- [crates/stdext/](../crates/stdext) -- arena, `simd::{lines_fwd,
  lines_bwd, memchr2, memset}`, `CoordType`, glob, etc.
- [crates/gutter/](../crates/gutter) -- gutter computation + render.

divergent TUIs:
- [crates/edit/src/tui.rs](../crates/edit/src/tui.rs) -- ~4500 lines,
  framebuffer-centric. `Tui::render(arena)` walks a tree of containers
  and dispatches to per-node paint paths in `anim::draw`. handles input
  via `crate::input`, vt parsing via `crate::vt`. mounts arbitrary node
  trees -- menubar, statusbar, modals, textareas.
- [crates/eat/src/follow_tui.rs](../crates/eat/src/follow_tui.rs) --
  ~2000 lines, ansi-string-centric. `run_snapshot` + `run` drive their
  own alt-screen loops. compose `String` buffers w/ escape sequences,
  write to stdout. own vt parser (`parse_keys`, `parse_csi`, ...) and
  own scroll/viewport state.

interaction model:
- edit invoked as `eat` (via symlink or `edit --eat`) short-circuits to
  `eat::main()` -- the editor tui never spins up. completely separate
  control flow.

## phased plan

four phases, each landable independently. phase A is the load-bearing
spike; B/C/D are progressively bolder applications of what A unlocks.

### phase A -- minimal mount surface in edit's tui (~1-2 days)

goal: prove edit's `Tui` can be driven from an external caller for a
single-textarea, read-only, no-chrome layout.

substeps:
1. **read edit's tui public api.** identify what's needed to construct a
   `Tui`, push a node tree, dispatch keys, drive the render loop. confirm
   which dependencies (`Settings`, `Colormap`, `KeyBindings`, the panic
   hook, the scratch arena) are load-bearing vs incidental.
2. **expose a mount pattern.** likely a new `pub fn` in edit's lib
   (e.g. `tui::mount_minimal(buffer: TextBuffer, opts: MinimalOpts) ->
   impl Iterator<...>` or a builder) that constructs a `Tui` with the
   minimum scaffolding to host one textarea. read-only flag forced on.
   no menubar / no statusbar / no modals (or stubs).
3. **smoke-test from a new toy bin in `crates/lsh-bin/` or a `tests/`
   harness.** opens a file, mounts via the new pattern, prints the
   first rendered frame. confirms the api is enough to drive a frame.
4. **stop-criterion:** a 30-line caller outside `edit/src/bin/edit/`
   produces a syntax-highlighted, scrollable, alt-screen frame using
   edit's tui machinery.

risks:
- edit's `Tui` may have hidden state tied to the editor's `App`, the
  filesystem watch loop, or settings load that's awkward to stub.
  mitigation: identify the friction at step 1, hoist as needed.
- the panic hook + scratch arena are global state set up in
  `edit::main`. mounting from a different binary needs the same setup.
  hoist to a `edit::sys::init_for_tui()` helper.

### phase B -- eat snapshot view migrates (~2-3 days)

goal: `eat foo.go` (tty + single file) renders through edit's tui via
the phase-A mount pattern. delete the snapshot-side ansi composition.

substeps:
1. **swap `run_snapshot`'s body.** read file into a `TextBuffer`
   (already exists from edit), mount via phase A, drive the render
   loop until user presses `q` / `esc`. handle the language override
   via the buffer's existing `language` field.
2. **delete dead code.** `redraw_snapshot`, `render_snapshot_header`,
   the cursor-positioning helpers used only by snapshot. anything else
   that's snapshot-only.
3. **non-tty + `--paging=never` paths unchanged.** they stay on
   `write_highlighted_line` (ansi-stream pipeline). only the alt-screen
   path migrates.
4. **theme story:** edit's framebuffer pipeline uses its rgb colormap.
   eat's snapshot view, once on framebuffer, inherits this. user-loaded
   colormaps work for `eat` too. non-tty `eat` keeps ansi-16 via the
   existing theme (separate target).
5. **stop-criterion:** golden snapshot tests still pass for non-tty
   path; alt-screen `eat foo.go` produces visually equivalent output to
   today's snapshot view, w/ edit's framebuffer driving the paint.

risks:
- read-only buffer behaviour under edit's tui: cursor / selection / find
  must still work, edits must no-op. the read-only flag exists today
  but is exercised only when a file is non-writable. need to confirm it
  behaves the same when set explicitly.
- key bindings: edit's default bindings vs eat's snapshot bindings
  (`q` to quit, `space`/`b` to page). need a way to override the
  binding set when mounting. probably a `MinimalOpts.keybindings` field.

### phase C -- eat follow mode migrates (~3 days)

goal: `eat -f foo.log` runs through edit's tui via the phase-A mount.
streaming appends feed a growing `TextBuffer`; the viewer follows the
tail by default, breaks-to-stay when user scrolls up.

substeps:
1. **streaming buffer feed.** new helper (in eat's `follow.rs` or
   edit's buffer) that takes append-bytes from `FileSource` /
   `FollowSource` and pushes them into the `TextBuffer`. invalidate the
   highlighter cache from the modified line.
2. **tail-mode cursor behaviour.** anim or buffer flag: cursor pinned
   to last line when the user hasn't scrolled, breaks free when user
   moves it up. edit's existing scroll/cursor anim handles the smooth
   tail naturally.
3. **edit gains `--follow` flag.** mounts the same pattern, so
   `edit --follow foo.log` and `eat -f foo.log` end up identical
   internally. flag plumbed through edit's hand-rolled arg parser.
4. **delete eat's vt parser, key dispatch, scroll state, ansi
   composition for follow.** at this point `follow_tui.rs` should be
   close to empty -- only the cli-shape glue left.
5. **stop-criterion:** existing follow tests (`crates/eat/tests/
   follow_loop.rs`, the 10-test alt-screen suite) still pass against
   the new pipeline. live `eat -f` on a growing log behaves the same.

risks:
- buffer-append performance: edit's `TextBuffer` is rope-shaped and
  designed for interactive edits, not append-only growth. unknown
  whether `tail -f` on a fast-growing log keeps up. mitigation:
  benchmark early; if needed, add an append-optimised fast path.
- the highlighter cache invariants: cache is keyed by line; mid-line
  appends mean the in-progress line's spans change. existing cache
  invalidation rules should cover this but worth a fresh look.

### phase D -- one tui codebase (~1 day, cleanup)

goal: `crates/eat/src/follow_tui.rs` deleted. eat is:
- cli + arg parsing,
- non-tty `write_highlighted_line` ansi-stream path,
- thin glue around edit's tui for alt-screen modes (snapshot + follow).

actions:
1. **delete `follow_tui.rs`.** any remaining surface either moves to
   `edit::tui` (if reusable) or to `eat::lib` (if cli-shaped).
2. **eat's `Cargo.toml` gains a `edit` dep** (build-only / runtime,
   confirm direction is acceptable). currently `edit` depends on
   `eat` for the multicall dispatch; we may want to invert this or
   accept the cycle (cargo allows it as long as it's
   build-only vs runtime, or sliced via features). risk to monitor.
3. **update [meanderings/eat.prop.md](eat.prop.md)** -- mark the
   snapshot/follow sections as superseded by this plan.
4. **stop-criterion:** workspace builds w/ no `follow_tui` module; all
   tests green; `eat foo.go` and `edit --eat foo.go` behave identically;
   `eat -f foo.log` and `edit --follow foo.log` behave identically.

## what stays separate

- **ansi-stream rendering** for non-tty `eat`. `crates/eat/src/lib.rs::
  write_highlighted_line` is the right tool for `eat foo.go | less` /
  `eat foo.go > out.ansi` / `git diff | eat`. cannot share with
  framebuffer.
- **edit's full editor chrome** (menubar, statusbar, find/replace,
  modals). only the textarea-rendering / input / scroll bits get reused
  by eat. eat's alt-screen modes mount a stripped tui, not the editor.
- **two theme paths.** rgb (framebuffer) and ansi-16 (ansi-stream)
  serve different targets. same `HighlightKind::default_color` table
  drives both, but the mapping to output bytes differs by target.

## what's not in scope

- **rgb -> truecolor ansi escapes for the ansi-stream path.** would let
  eat's pipe output use the rgb theme. defer -- ansi-16 respects user
  terminal palettes, which is what users expect from a cat-shape tool.
  see [eat.prop.md](eat.prop.md) "theme (v1)".
- **edit growing a `--print` mode** (non-tty stdout-flush of a buffer).
  conceivable but not driven by current needs. would consume the same
  `write_highlighted_line` from eat. revisit if asked.
- **lsh-bin gaining a tui mode.** orthogonal; lsh-bin is a debug tool.

## sequencing recc

three commits, each phase:
1. `feat(edit): expose minimal tui mount pattern` (phase A)
2. `refactor(eat): snapshot view consumes edit's tui` (phase B)
3. `refactor(eat): follow view consumes edit's tui; edit gains --follow` (phase C, +D)

each lands independently; b/c rely on a but a is useful standalone (any
future caller wanting a one-textarea tui benefits).

before phase A: optionally land `refactor(lsh-defs): share ansi-16
colourmap` (the "step 3c" tidy-up). small, independent, closes loose
ends from the lsh-defs extraction work. ~30 min.

## open questions

- **dependency direction.** edit currently depends on eat (multicall
  dispatch). if eat starts depending on edit (for the mount pattern),
  we have a cycle. options:
    1. invert -- edit's `--eat` calls into a shared `eat-lib`, eat-bin
       is the only thing that depends on edit for tui mount.
    2. split mount-pattern into its own crate (`edit-tui-embed`?). both
       eat and edit depend on it.
    3. accept the cycle if it's runtime-only (cargo permits).
  needs deciding before phase A wraps.
- **panic hook + arena init.** edit's `main` sets these up. mounting
  from eat means eat does the same setup, or both call a shared
  `init_for_tui()` helper. probably the latter.
- **settings + colormap loading.** edit reads user TOML for these. when
  eat mounts a minimal tui, does it load the same user settings? likely
  yes (consistency between `edit foo.go` and `edit --eat foo.go`); but
  worth confirming the loading code is independent of the editor `App`.
- **key binding set.** edit's default keymap is for editing (write /
  cut / paste). eat's snapshot/follow modes want view-only keys (`q`,
  `space`, `b`, `g`, `G`). mount pattern needs a key-set override.
- **node tree shape for a "just one textarea" mount.** does the tree
  walker happily render a tree of one node? or does it expect chrome
  scaffolding (header / footer rows)? probably fine but verify in
  phase A.

## decisions (to be confirmed)

- **one tui in the workspace, edit's.** confirmed in conversation
  2026-05-23.
- **two output pipelines kept (framebuffer + ansi-stream).** different
  targets, can't merge.
- **eat retains its cli surface + non-tty path.** only the alt-screen
  internals migrate.
- **edit grows a `--follow` flag** as the natural fallout of phase C.
- **edit's existing rgb colormap drives `eat`'s alt-screen modes** once
  migrated. ansi-16 stays for non-tty `eat`.
