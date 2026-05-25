---
status: done
date: 2026-05-23
description: bring eat's follow_tui under edit's tui machinery so the workspace has one TUI; keep two output pipelines (framebuffer vs ansi-stream) since they serve different targets
---

# unification plan: one tui for eat + edit

## update 2026-05-25 (session log)

### phase A -- landed

- `edit::term::setup` + `RestoreModes` (hoisted alt-screen mode switch,
  OSC 4/10/11 palette probe, ambiguous-width probe, kitty kbd proto
  push). out of `bin/edit/main.rs::setup_terminal`.
- `edit::mount::mount(opts, draw_fn) -> io::Result<()>` -- thin external
  mount api. owns `Tui::new` + `term::setup` + input/render loop +
  alt-screen restore. callback-driven exit via `ControlFlow::Break`.
  caller pre-inits arena + sys.
- `MountOpts::tick_interval: Option<Duration>` -- caps the read_stdin
  timeout so the draw callback fires at least that often even with no
  user input. lets callers do periodic refresh (clock, disk poll).
  added in phase B.3.
- `buffer::IoError: Debug` -- ergonomic fix; external callers can
  `.expect()` / `?` cleanly.
- compile-only `tests/mount_api.rs` pins the public surface.

### dep direction -- resolved by absorption

`crates/eat/` workspace crate gone; sources moved to
`crates/edit/src/eat/` as `edit::eat` module. the standalone `eat`
binary inside the edit crate also went away. `make install` already
created the `eat -> edit` symlink; argv0 dispatch in `bin/edit/main.rs`
routes `eat` -> `edit::eat::main()`. no cycle, no inversion, no extra
crate. phases B/C/D below are now intra-crate refactors.

### phase B -- snapshot view, landed

`eat <file>` (tty + single file) renders through `edit::mount::mount`
+ a read-only `TextBuffer`. textarea handles cursor / scroll /
selection / mouse-wheel natively. deleted ~340 lines of bespoke
snapshot driver (`render_snapshot_header`, `run_snapshot_loop`,
`redraw_snapshot`, `YELLOW`, the old `View`-based redraw path).

features wired:
- `-n` line numbers via `TextBuffer::set_margin_enabled`.
- `r` reload (re-reads file, refreshes captured-at, clears delta flag).
- `q` exit (consumed before textarea sees Q).
- disk-change `[modified on disk]` flag via 2s `tick_interval` poll.

still dropped (TODO(lczyk) in `run_snapshot` doc):
- `--color=never` plain-mode toggle. edit's tui has no plain-mode
  switch yet; defer until needed.

### phase C.1 -- mount-based follow spike, landed

`follow_tui::run_follow_mount` mounts the textarea via `edit::mount` and
re-reads the file into a `TextBuffer` whenever `stat_fingerprint` shows
the file changed. routing: `EAT_FOLLOW_USE_MOUNT=1` opts the tty pager
into the mount path; default still goes through bespoke `follow_tui::run`
so the old + new can run side-by-side until C.2/C.3 close the gap.

what landed (beyond the original recc):
- **funnel-style follow / paused model.** mirror funnel's `display_offset`
  as `pause_offset: CoordType` (lines above tail; 0 == follow). every
  key + mouse-wheel delta we either send or observe is shadowed onto
  `pause_offset`, clamped to `[0, visual_line_count - body_h]` so it
  stays in lockstep with the textarea's own clamping. transitions
  through 0 re-enter follow automatically. matches funnel's "scroll up
  to pause, scroll back down to resume" UX.
- **keybindings + bar match the bespoke driver.** Up/Dn, j/k, g/G, Home,
  End, PgUp, PgDn, q/esc. header is
  `<path> [following|paused] @ <clock>  (<poll>ms Up/Dn g/G PgUp/PgDn scroll, q)`.
- **mouse wheel works.** new `Context::scroll_delta() -> Point` exposes
  the per-frame wheel delta to mount callers; we read it before the
  textarea consumes it (textarea applies scroll to its own offset, our
  pause_offset shadow updates in parallel).
- **no cursor.** textarea is mounted **without** `inherit_focus()` so
  the focused-only `cursor_block` path stays inert -- eat is a viewer,
  not an editor. mouse-wheel scroll still flows through (handled
  pre-focus-check in `textarea_handle_input`).
- **tail-snap via `cursor_move_to_logical(Point::MAX) + make_cursor_visible`.**
  The naive `request_scroll_delta_y(visual_line_count)` route only
  reaches `visual_line_count - 1` after the clamp in
  `textarea_adjust_scroll_offset` (last line at the **top** of the
  viewport). going via the cursor pulls in
  `textarea_make_cursor_visible`'s `scroll_y = cursor_y - viewport_h + 1`
  which is the correct "last line pinned to the bottom edge".
- **`glyphs::set_no_animations(true)` for the follow lifetime** via a
  small RAII guard. with a 250ms poll the textarea's ~60ms scroll lerp
  lands between ticks and reads as jerky "snap, settle, snap, settle";
  snapping instantly per tick gives the smooth funnel cadence. restored
  on exit so the editor's own animations are unaffected.
- **tick decoupled from poll.** `tick_interval = min(poll_interval, 33ms)`.
  stat is cheap, reload is still gated on change; decoupling the wake
  rate from the reload rate is what stops a fast-growing log from
  reading as discrete chunk-jumps at 4Hz.

bugs found + fixed during the spike (worth keeping for context next
session):
- `request_scroll_delta_y(CoordType::MAX)` wraps `scroll_offset.y += MAX`
  to a negative value before the clamp lands, producing a "snap to top,
  then snap to bottom" flicker. use `visual_line_count()` as the
  saturating-safe upper bound instead.
- the cursor-based tail snap is the **only** route that produces "last
  line at the viewport bottom" -- direct scroll_offset manipulation
  pins to the top. recorded in a comment in `snap_to_tail`.

known gaps vs the bespoke driver (still C.2+ work):
- **rope rebuild + highlighter cache wipe per stat-change.** for a
  fast-growing log (~100 lines/sec, 10k+ lines buffered) the reload
  itself outpaces the 30Hz wake cadence -- viewport trails real EOF by
  a visible margin. C.2 (incremental append via `FollowSource::tick` +
  cache invalidation from the modified line) is the fix.
- pause_offset shadow can drift from the textarea's actual scroll
  offset if the two clamps diverge (e.g. animation interactions,
  visual_line_count changes during a frame). C.3 may want a proper
  read-back, e.g. exposing scroll_offset.y on the buffer side or via a
  Context accessor keyed off prev_node_map.
- no header `[modified on disk]` / miss-budget bookkeeping. fine for a
  spike.

### phase C.2 -- incremental append, landed

`run_follow_mount`'s drain rewritten to mirror funnel's stat-first
shape: stat the file each tick (cheap), branch on
(rotated | idle | grown), and on growth read only the new bytes from
`last_size..cur_size` and append via `TextBuffer::write_raw` at
`cursor = Point::MAX`. the rope grows in place; the highlighter cache
only invalidates from the appended line down (cheap when appending
near EOF). on rotation (inode change or size shrink) we fall back to
the existing `read_file` path -- clean reset of buffer + cache +
encoding detection.

eliminated the C.1 perf cliff: ~10k-line buffers w/ a fast-growing
writer (writer at ~100 lines/s) now track real EOF in step instead
of trailing visibly behind it.

driveby:
- ported funnel's `WheelAccel` (1->2 line ramp on sustained fast
  spin). currently shadowed -- the textarea applies raw wheel deltas
  from outside our control and the ramp factor isn't propagated
  through. left in place so C.3 can plumb a scroll-boost path.
- header now shows `[paused: N below | M above]` when paused, derived
  from `pause_offset` + `body_h` + `visual_line_count`.

still gated behind `EAT_FOLLOW_USE_MOUNT=1`. ready to flip the
default for `eat -f` once we've spent more time leaning on it
manually + decided what to do about the C.3 / C.4 / C.5 follow-ups.

### phase C.4 -- edit --follow flag, reverted

Landed and then reverted in the same session. Decided that edit is
the editor and eat is the viewer, and adding a viewer-shaped flag
to edit muddied the boundary. eat -f stays the one entry to the
mount-based follow view. The mount path doesn't lose a second
caller in any meaningful sense -- it's exercised plenty via eat -f.

Originally landed:

### phase C.4 -- edit --follow flag, landed

`edit --follow [<dur>] <path>` short-circuits ahead of edit's normal
editor flow (parallel to --eat) and dispatches to
`edit::eat::run_follow_for_edit`, a thin wrapper that does language
detect + use_color + arena init then calls `follow_tui::run_follow_mount`.

picks up `edit::eat::FollowDuration::parse` for the same `30s`/`500ms`/
bare-number-as-seconds shape as `eat -f`. defaults to 250ms when no
value is given. one path required; multi-path / stdin rejected.

useful as a second caller on the mount path: it exercises the same
code as `eat -f` (under EAT_FOLLOW_USE_MOUNT=1) but without eat's
non-tty / bespoke-driver branches. shrinks the surface still riding
on the bespoke driver as we prepare to flip default routing.

### phase C.5 -- delete bespoke follow-tui driver, landed

Following the routing flip + edit --follow shaking out the mount path
from a second caller, the legacy follow_tui::run driver is gone. Net
delete is ~2050 lines:

- `follow_tui.rs` shrinks from ~1700 to ~520 LOC. dropped: the run /
  run_loop / redraw / redraw_header_only loop, `View` + animation +
  ring + apply_key, `LineBuf` + `push_truncated*` + `render_frame` +
  `lerp_alpha`, the `Key`/`KeyOutcome` enums, the entire vt parser
  (`parse_keys` / `parse_escape` / `parse_csi` / `classify_csi` /
  `arrow_key` / `parse_sgr_mouse`), the ansi-string consts
  (ALT_SCREEN_* / CURSOR_* / CLEAR_SCREEN / RESET / DIM / WRAP_* /
  cursor_to / clear_eol), the SCROLL_TAU_SECS / SCROLL_SNAP_EPSILON
  / ANIM_FRAME_MS / LINE_CAP / LINE_CAP_DROP constants, and the
  in-module 47-test bespoke-key suite.
- `tests/follow_loop.rs` (440 LOC) -- entirely targeted the bespoke
  surface; the mount path has no equivalent unit tests yet (gap to
  fill: `tests/follow_mount.rs` against the drain logic).
- `eat::mod::run_follow_cli` lost the EAT_FOLLOW_USE_BESPOKE escape
  hatch; there is no bespoke path left to opt into.

Kept (still load-bearing): `format_clock` (used by both snapshot and
follow headers), `stat_fingerprint` + `SnapshotStat` (used by both
snapshot disk-change poll and the follow drain).

Phase C complete. The unification plan's "one tui codebase" goal is
satisfied for both alt-screen modes; the ansi-stream path (non-tty
eat) stays separate by design, as intended.

### phase C -- follow tui, in progress

`crates/edit/src/eat/follow_tui.rs` still holds ~1500 LOC of bespoke
follow-mode driver: `Key` enum + `parse_keys` / `parse_escape` /
`parse_csi`, `View` (scroll + buffered lines), `render_frame`,
`run` / `run_loop` / `redraw` / `redraw_header_only`, `LineBuf`.
also `format_clock`, `push_truncated*` helpers that snapshot used to
share. these all stay alive until phase C migrates `run` to mount.

recc breakdown:
- **C.1** -- minimal mount-based follow: crude periodic re-read of
  the file into a `TextBuffer`. sanity-check the architecture.
- **C.2** -- incremental append via `follow.rs::FollowSource::tick`
  feeding bytes into the existing buffer. invalidate the highlighter
  cache from the modified line.
- **C.3** -- tail-pin cursor: pinned to last line until user scrolls
  up, breaks free on user move. edit's existing scroll/cursor anim
  carries it smoothly.
- **C.4** -- edit gains `--follow` flag. plumbed through edit's
  hand-rolled arg parser; mounts the same pattern as eat -f.
- **C.5** -- delete dead code from `follow_tui.rs`: `View`, `LineBuf`,
  `parse_keys`, `run_loop`, `redraw`, `redraw_header_only`, mouse/key
  CSI helpers. follow tests adjust to the new surface.

### open / next session

- tty-verify the snapshot view by hand (`cargo run --release -- --eat
  <file>`). spike notes said cursor + selection worked; the q-handler
  + mouse-mode wiring should now mean q exits and scroll responds.
- gate phase C on whether `TextBuffer` append performance keeps up
  with a fast-growing log. rope-shaped buffer was designed for
  interactive edits; benchmark before committing to C.2 incremental
  appends. fallback path: crude re-read of the file every poll cycle
  via tick_interval (already proven by snapshot's disk poll).
- decide whether `edit --follow` should share the same `run_snapshot`
  body w/ a "follow-mode" flag, or stay a separate fn. shared seems
  cleaner once C.3 tail-pin is in.

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
- [crates/edit/src/eat/follow_tui.rs](../crates/edit/src/eat/follow_tui.rs) --
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
5. **stop-criterion:** existing follow tests (`crates/edit/tests/
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

goal: `crates/edit/src/eat/follow_tui.rs` deleted. eat is:
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
3. **update [meanderings/_eat.prop.md](_eat.prop.md)** -- mark the
   snapshot/follow sections as superseded by this plan.
4. **stop-criterion:** workspace builds w/ no `follow_tui` module; all
   tests green; `eat foo.go` and `edit --eat foo.go` behave identically;
   `eat -f foo.log` and `edit --follow foo.log` behave identically.

## what stays separate

- **ansi-stream rendering** for non-tty `eat`. `crates/edit/src/eat/mod.rs::
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
  see [eat.prop.md](_eat.prop.md) "theme (v1)".
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
