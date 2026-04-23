# Diff-mode editing

Status: **implemented behind `Ctrl+Shift+D`. This doc is a recreation spec —
sufficient detail to rebuild the feature from scratch if reverted.**

## Goal

When editing a file that is tracked by git, let the user toggle a "diff view"
that renders the file with baseline-deleted lines interleaved in place as
read-only stripes. The user can select and copy from deleted stripes, but any
edit that would modify them is rejected. Saving writes only the editable
content. As the user edits, the diff re-computes (debounced 150 ms) so the
stripes stay aligned with the current edit state.

Baseline = `HEAD:<path>` with fallback to the index (`:<path>`) for
staged-but-uncommitted files.

## Concept

A single `TextBuffer` holds the interleaved view:

- **Keep** lines come from `current` (unchanged vs. baseline).
- **Add** lines come from `current` (new vs. baseline). Tinted green.
- **Del** lines come from `baseline`. Tinted red. Byte ranges installed as
  *locked ranges* that reject mutating edits.

Saving filters out Del lines. Re-diffing extracts editable bytes, runs a
fresh diff against the cached baseline, and rebuilds the view.

Trade-offs chosen (all confirmed by user in original design pass):

- Baseline: HEAD, fall back to index.
- Rediff cadence: live, debounced 150 ms.
- Stripes: always visible when in diff mode; no collapse.
- Undo history preserved across rediff rebuilds. Cleared on enter/exit.
- Dirty flag preserved across enter/exit/rediff (the file on disk hasn't
  changed just because the view has).

## File layout

- [crates/edit/src/bin/edit/linediff.rs](../crates/edit/src/bin/edit/linediff.rs)
  — Myers O(ND) line diff.
- [crates/edit/src/bin/edit/git.rs](../crates/edit/src/bin/edit/git.rs)
  — thin `git` subprocess wrapper (no libgit2).
- [crates/edit/src/bin/edit/diff_mode.rs](../crates/edit/src/bin/edit/diff_mode.rs)
  — orchestrator: view construction, extract_real, cursor remap.
- [crates/edit/src/buffer/mod.rs](../crates/edit/src/buffer/mod.rs)
  — locked-range gating, `refresh_view_content`, line decorations.
- [crates/edit/src/bin/edit/documents.rs](../crates/edit/src/bin/edit/documents.rs)
  — `Document::{enter_diff_mode, exit_diff_mode, rediff, save}` wiring.
- [crates/edit/src/bin/edit/keybindings.rs](../crates/edit/src/bin/edit/keybindings.rs)
  — `Action::ToggleDiffMode`.
- [crates/edit/src/bin/edit/draw_menubar.rs](../crates/edit/src/bin/edit/draw_menubar.rs)
  — "Diff Mode" checkbox under View.
- [crates/edit/src/bin/edit/draw_statusbar.rs](../crates/edit/src/bin/edit/draw_statusbar.rs)
  — `DIFF +A/-D` badge.
- [crates/edit/src/bin/edit/main.rs](../crates/edit/src/bin/edit/main.rs)
  — shortcut handler, rebuild-on-idle in main loop.

## Phases (recreation order)

### Phase 1 — `linediff.rs`

Myers O(ND) diff over wyhash-hashed lines.

- `split_lines(&[u8]) -> Vec<Line<'_>>` where `Line { range, hash, bytes }`.
  Ranges cover the entire input with no gaps. CRLF is normalized for hashing
  (strip trailing `\r` before hashing and equality) but preserved in the
  stored byte range.
- `diff(&[Line], &[Line]) -> Vec<LineOp>` where `LineOp` is one of:
  - `Equal { baseline, current, count }`
  - `Delete { baseline, count }`
  - `Insert { current, count }`
- `MYERS_MAX_D = 100_000`. Beyond that, bail to a whole-file delete+insert.
- Use `edit::hash::hash` (wyhash); do **not** add `similar` or any other
  diff crate. Binary size matters.

Key implementation notes:

- `myers_trace` uses a single persistent `V` array, snapshotting after each
  `d`. Initialize all-zeros (relies on `V[1] = 0` read at `d = 0`).
- `myers_backtrack` walks snapshots in reverse, emitting `Equal` / `Delete` /
  `Insert` steps.
- `coalesce` merges consecutive same-kind steps into counted ops.
- Trailing-newline-only difference collapses to `Equal` (normalization strips
  terminators before hashing).

Tests: equal, empty-both, pure-insert, pure-delete, insert-middle,
delete-middle, replace-middle, CRLF==LF, trailing-newline collapsed,
whole-replace, multi-block, index-validity walkthrough. 16 tests.

### Phase 2 — `git.rs`

Subprocess wrapper. No libgit2.

- `GitInfo { repo_root: PathBuf, rel_path: String }`. `rel_path` uses forward
  slashes (git form).
- `locate(path) -> io::Result<GitInfo>` runs `git rev-parse --show-toplevel`
  from `path.parent()`. **Canonicalize both `path` and the returned
  top-level** — on macOS `git` returns `/private/var/...` while tempdirs and
  symlinks give `/var/...`; `strip_prefix` fails otherwise. Fall back to a
  lexical `normalize` if canonicalize fails (path may not exist yet).
- `is_tracked(&info) -> io::Result<bool>` via
  `git ls-files --error-unmatch -- <rel>`.
- `read_baseline(&info) -> io::Result<Vec<u8>>`: try
  `git show HEAD:<rel>` first, fall back to `git show :<rel>` (index
  stage 0).

Tests use real `git` subprocesses against temp repos (skip gracefully if
`git` not on PATH). `git init -b main`, set author env vars, commit, dirty,
assert. Covers: outside-repo errors, locate returns relpath, tracked after
add, baseline returns HEAD content, baseline falls back to index for
new-file-in-index, untracked-in-empty-repo errors. 6 tests.

### Phase 3 — `TextBuffer` locked ranges

In [buffer/mod.rs](../crates/edit/src/buffer/mod.rs):

**New fields:**

```rust
locked_ranges: Vec<Range<usize>>,  // sorted, non-overlapping
edit_rejected: u32,                 // counter for UI flash
line_decorations: Vec<LineDecoration>,
```

**New public API:**

- `locked_ranges(&self) -> &[Range<usize>]`
- `clear_locked_ranges(&mut self)`
- `set_locked_content(&mut self, text, ranges)` — wholesale replace, clears
  undo, preserves dirty flag.
- `refresh_view_content(&mut self, text, ranges)` — wholesale replace,
  preserves undo and dirty. Used by rediff.
- `edit_rejected_count(&self) -> u32`
- `set_line_decorations(Vec<LineDecoration>)`, `clear_line_decorations()`
- `pub enum LineDecoration { None, Added, Deleted }`

**Gating:**

- `locked_intersects(range) -> bool` — binary search. Intersection is
  non-empty overlap; an empty range at a boundary is allowed.
- `reject_if_locked(range) -> bool` — intersects + bumps `edit_rejected`.
- Gates added at: `write()` (covers `write_canon`, `write_raw`, `paste`,
  `find_and_replace`, `find_and_replace_all`, indent-change-via-write),
  `delete()`, `extract_selection()` when delete=true (covers `cut` and
  `move_selected_lines`), `normalize_newlines()`. Each computes the
  replace-range it is about to mutate; overtype conservatively widens by
  `text.len()` past the cursor.
- **Never** mid-edit abort — pre-check at the entry point so edits are
  atomic.

**Shift bookkeeping:**

- `shift_locked(at, delta)` — any range with `start >= at` shifts by
  `delta`. Called from inside `edit_write` (post-insert) and `edit_delete`
  (post-delete). Correctness rests on the pre-check: no allowed edit can
  straddle a locked range.

**Critical `set_locked_content` / `refresh_view_content` detail:**

The shared impl replaces buffer contents via `buffer.clear() + buffer.replace(0..0, text)`. It must **recount `stats.logical_lines`** (newlines
+ 1) — bypassing the edit pipeline leaves stats stale and trips a
debug assertion in `set_cursor_internal`.

For history/dirty preservation:

```rust
let was_dirty = self.is_dirty();
let saved_history = (!clear_history).then(|| (
    mem::take(&mut self.undo_stack),
    mem::take(&mut self.redo_stack),
    self.last_history_type,
));
// ... buffer.clear + buffer.replace + stats recount ...
self.recalc_after_content_swap();  // clears undo + marks clean
if let Some((undo, redo, last_type)) = saved_history {
    self.undo_stack = undo;
    self.redo_stack = redo;
    self.last_history_type = last_type;
}
if was_dirty {
    self.last_save_generation = self.last_save_generation.wrapping_sub(1);
}
```

`recalc_after_content_swap` should also clear `locked_ranges` and
`line_decorations`; the caller re-installs as needed.

Tests (14): intersects basic + boundary, write rejected inside locked,
write before/at-start/at-end shifts correctly, delete across locked
rejected, delete outside shifts, set_locked_content, clear, normalize
rejected when locked, dirty preserved across `set_locked_content`, undo +
dirty preserved across `refresh_view_content`, line decorations set/clear.

### Phase 4 — render tint

In [buffer/mod.rs render()](../crates/edit/src/buffer/mod.rs), inside the
per-visual-line loop, **before** the selection pass, look up
`line_decorations[cursor_beg.logical_pos.y]`. If `Added` or `Deleted`, blend
a line-wide background:

```rust
let tint_color = match decoration {
    LineDecoration::Added => IndexedColor::BrightGreen,
    LineDecoration::Deleted => IndexedColor::BrightRed,
    LineDecoration::None => unreachable!(),
};
fb.blend_bg(rect, fb.indexed_alpha(tint_color, 2, 5));
```

`rect` spans `origin.x..origin.x + text_width` at the row's `top`.
The **2/5 alpha** is tuned: 1/4 was too subtle; 1/2 overwhelms foreground.

Placement before selection means a selection overlay stays readable on top
of diff colors.

Gutter prefix (`+`/`-`/` `) was considered and skipped — requires widening
`margin_width` by one column across the whole layout system. Tint alone is
sufficient visual signal.

### Phase 5 — `diff_mode.rs` orchestrator

```rust
pub struct DiffState { pub baseline: Vec<u8> }
pub struct BuiltView {
    pub bytes: Vec<u8>,
    pub locked_ranges: Vec<Range<usize>>,
    pub decorations: Vec<LineDecoration>,
}
```

**`DiffState::build_view(current) -> BuiltView`:**

1. `ensure_terminated` both sides (append `\n` if missing). This is a
   conscious trade: we save with a trailing newline even if input didn't
   have one, which is already the editor's default via
   `set_insert_final_newline`. The alternative — tracking synthetic
   terminators through the whole pipeline — is not worth the complexity.
2. `split_lines` both sides.
3. `diff` to get ops.
4. Walk ops, emitting bytes:
   - `Equal` / `Insert`: push current-line bytes; decoration `None` / `Added`.
   - `Delete`: push baseline-line bytes; decoration `Deleted`; extend the
     current locked-range stripe.
5. Each `Delete` op produces one contiguous locked range (adjacent Del
   lines merge naturally because they're emitted in one block).

**`extract_real(&TextBuffer) -> Vec<u8>`:**

Concatenate buffer bytes outside locked ranges. Uses `buf.read_forward` in
chunks. This is the save-path content and the input to the next rediff.

**Cursor mapping:**

- `view_to_real_off(view_off, &locked) -> usize`: walk locked ranges;
  if `view_off` falls inside one, anchor to the range's start (so cursors
  inside stripes snap to "just before" on rebuild).
- `real_to_view_off(real_off, &locked, view_len) -> usize`: inverse.
  Clamped to `view_len`.

**Constants:**

```rust
pub const MAX_DIFF_BYTES: usize = 5 * 1024 * 1024;
pub const REDIFF_DEBOUNCE_MS: u64 = 150;
```

Tests (12): pure-equal, deleted-stripe, inserted-line, adjacent-deletes
merge, extract_real basic, extract_real after user edit,
view_to_real_off basic, real_to_view_off basic, cursor roundtrips across
rebuild, ensure_terminated no-op cases, trailing-newline doesn't trigger
hunk, rebuild via refresh preserves undo of edit.

### Phase 6 — wiring

**`Action::ToggleDiffMode`** in
[keybindings.rs](../crates/edit/src/bin/edit/keybindings.rs). Bump
`ACTION_COUNT` from 24 to 25. Default chord `Ctrl+Shift+D` in both
[keybindings.macos.toml](../crates/edit/src/bin/edit/keybindings.macos.toml)
and [keybindings.linux.toml](../crates/edit/src/bin/edit/keybindings.linux.toml).

**`Document::diff: Option<DiffMode>`** where:

```rust
pub struct DiffMode {
    pub state: DiffState,
    pub dirty_generation: u32,
    pub last_rebuilt_generation: u32,
    pub dirty_since: Option<Instant>,
    pub hunk_adds: u32,
    pub hunk_dels: u32,
}
```

**`Document::enter_diff_mode()`:**

1. Refuse if already in diff mode or `read_only`.
2. `git::locate(self.path)`.
3. `git::is_tracked(&info)` — error "file is not tracked by git".
4. `git::read_baseline(&info)`.
5. Size cap check against `MAX_DIFF_BYTES`.
6. Binary sniff via `looks_binary` (null byte in first 8 KB) — refuse.
7. Read current bytes from buffer (`read_forward` loop), size cap check.
8. `state.build_view(&current)`, install via `set_locked_content` +
   `set_line_decorations`.
9. Count adds/dels for the status badge.

**`Document::exit_diff_mode()`:** `extract_real`, reinstall via
`set_locked_content` with empty ranges, clear decorations, drop `self.diff`.

**`Document::rediff()`:**

1. Snapshot cursor view offset; compute `cursor_real = view_to_real_off(...)`.
2. `extract_real` for the current editable bytes.
3. `state.build_view(&current)`.
4. Install via `refresh_view_content` (preserves undo + dirty) +
   `set_line_decorations`.
5. `cursor_move_to_offset(real_to_view_off(cursor_real, new_locked,
   text_length))`.
6. Update `last_rebuilt_generation`, clear `dirty_since`, refresh hunk
   counts.

**`Document::diff_mark_dirty_if_changed()`** compares
`buffer.generation()` to `last_rebuilt_generation`; sets `dirty_since` on
first observed change.

**`Document::diff_should_rebuild()`** checks
`dirty_since.elapsed() >= REDIFF_DEBOUNCE_MS`.

**`Document::save()`** — when `self.diff.is_some()`, write
`extract_real(&buf)` and call `mark_as_clean` manually. Otherwise unchanged.

**Main loop** ([main.rs](../crates/edit/src/bin/edit/main.rs)) — between
the input-processing block and the settling loop:

```rust
state.document.diff_mark_dirty_if_changed();
if state.document.diff_should_rebuild() {
    state.document.rediff();
}
```

**`handle_global_shortcuts`** — new branch for
`Action::ToggleDiffMode`, calling a new `toggle_diff_mode(ctx, state)` that
enters or exits and surfaces errors via `error_log_add`.

**Menubar** — add a checkbox under View:

```rust
if ctx.menubar_menu_checkbox(
    "Diff Mode", 'D',
    keybindings::chord(Action::ToggleDiffMode),
    state.document.diff.is_some(),
) {
    crate::toggle_diff_mode(ctx, state);
    ctx.needs_rerender();
}
```

Note the borrow dance: don't hold `tb` across the toggle call.

**Statusbar** — when `doc.diff.is_some()`, render `DIFF +N/-M` from
`hunk_adds` / `hunk_dels`.

### Phase 7 — hardening (already merged with earlier phases in this doc)

Items covered elsewhere in the phases above:

- CRLF normalization (phase 1).
- Binary sniff and 5 MB size cap (phase 6).
- Trailing-newline collapse (phases 1 and 5).
- Rename (`git mv`) handled by HEAD→index fallback (phase 2).
- Read-only file rejected at entry (phase 6).

The critical phase-7 fix is the **undo + dirty preservation across
rediff** — see the `replace_view_content` design in phase 3.

External baseline refresh (user commits from another shell): no dedicated
action. User exits and re-enters diff mode (two presses of `Ctrl+Shift+D`);
`enter_diff_mode` re-reads the baseline.

## Edge cases and why they work

- **Edit re-adds a deleted line.** Next rediff collapses the stripe
  naturally. Cursor anchored by real-offset survives the collapse.
- **Edit at the start of a locked stripe.** Allowed — insertion pushes the
  stripe forward. `shift_locked` updates the range's start and end.
- **Edit at the end of a locked stripe.** Allowed — insertion is after the
  stripe. `shift_locked` leaves the range alone (its `start` is not `>=`
  the edit offset in a way that matters; actually `start < at` so no
  shift). Range stays put, new bytes go after.
- **Selection crossing a stripe + delete/backspace.** `reject_if_locked`
  sees non-empty intersection; whole op rejected atomically.
- **Selection crossing a stripe + copy.** Goes through `extract_selection(delete=false)` which has no gate. Copy works, deleted-stripe bytes land
  in the clipboard — useful.
- **Rediff during active typing.** Debounce (150 ms) prevents firing
  between keystrokes. Single-threaded main loop: no race with user input.
- **Empty file / empty baseline.** `linediff::diff` handles both empty
  cases at the top. `build_view` emits an all-`Add` or all-`Del`
  decoration.
- **File whose last line has no newline.** `ensure_terminated` normalizes
  both sides for comparison. Save ends up with a final newline (matches the
  editor's default anyway).
- **`git` not on PATH.** `locate` returns an `io::Error`; `enter_diff_mode`
  surfaces it to the error log.
- **Huge baseline.** `MAX_DIFF_BYTES` check in `enter_diff_mode`. Myers has
  `MYERS_MAX_D = 100_000` worst-case cap that falls back to whole-replace.

## Not in scope (v1)

- Collapsible / foldable deleted stripes.
- Gutter `+`/`-` prefix characters (tint alone is sufficient).
- Picking the baseline revision interactively (always HEAD→index).
- Per-hunk stage/unstage or revert actions.
- Diff against working-tree version of another branch.
- Merge-conflict three-way diff.
- Word-level diff within a changed line.

## Test plan summary

82 total tests green at phase 7:

- 16 linediff tests.
- 6 git tests (skip if `git` absent).
- 14 TextBuffer locked-range + decoration tests.
- 12 diff_mode tests (build_view, extract_real, cursor mapping, undo
  survival across rebuild).
- Balance: existing buffer/tui/etc. tests.

`make test` is the gate (skip `make fmt-check` / `make clippy` if you see
pre-existing drift unrelated to this feature — the lsh codegen trips
`clippy::large_const_arrays` on HEAD; stable rustfmt doesn't know about
nightly-only rustfmt.toml settings).

## Recreation checklist

If you revert and later want to rebuild, hit the phases in order. Each
phase compiles and tests on its own; no phase leaves the tree broken.

1. `linediff.rs` + 16 tests.
2. `git.rs` + 6 tests.
3. Locked ranges + decorations + `set_locked_content` /
   `refresh_view_content` on TextBuffer + 14 tests.
4. Render tint (2/5 alpha, before selection pass).
5. `diff_mode.rs` orchestrator + 12 tests.
6. Wiring: `Action::ToggleDiffMode`, Document::{enter,exit,rediff,save},
   main-loop rebuild check, menubar checkbox, statusbar badge.
7. Tint visibility bump and undo+dirty preservation fix (integrated in
   phase 3's `replace_view_content` if rebuilding now).

Biggest landmines encountered the first time:

- Myers V-array initialization (must be all zeros; V[1] = 0 is the only
  read before any write).
- macOS `/var` vs `/private/var` symlink — canonicalize both sides in
  `git::locate`.
- `copy_from_str` assumes line count doesn't change; use
  `set_locked_content` instead for setup, or recount stats manually.
- `recalc_after_content_swap` wipes undo, dirty, cursor, and selection.
  For rediff we must snapshot+restore undo/redo/last_history_type and
  adjust `last_save_generation` to re-read as dirty.
- `gen` is a reserved keyword in Rust 2024 — name the local
  `current_gen`.
- `clippy::single_range_in_vec_init` fires on test helpers with a
  `&[x..y]` literal; local `#[allow]` on the test mod.
