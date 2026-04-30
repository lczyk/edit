---
status: open
date: 2026-04-30
description: vscode-style per-line gutter marks for git changes (added/modified/deleted vs HEAD)
---

# gutter diff -- plan

vscode-parity "dirty diff" gutter glyphs. when a file is open inside a git repo, paint a single-character mark in the line-number column for each line that differs from `HEAD:<path>` (or `:<path>` if untracked-but-staged). green for added, yellow for modified, red wedge for deleted-above. lightweight, read-only, debounced. passive overlay -- no buffer mutation, no save-path changes.

## scope

- v1: HEAD baseline only. fall back to index for new-files-staged. silent noop for everything else (no repo, untracked, binary, too big, no `git`).
- v1: marks are colour-only on the existing line-number column; no extra gutter column. no glyph for "deleted-above" in v1 -- replace with recoloured separator `│`. revisit if it reads poorly.
- v1: always-on when applicable. no toggle. menubar checkbox + setting can follow.

## semantics

per-line state, computed by a Myers diff of current buffer vs baseline:

```rust
enum GutterMark { None, Added, Modified, DeletedAbove, DeletedBelow }
```

mapping from a per-line diff op stream (`Equal` / `Insert` / `Delete`):

- `Equal`           -> `None` for each current-side line.
- `Insert`          -> `Added` for each current-side line.
- `Delete`          -> attach `DeletedAbove` to the *next* current line, or `DeletedBelow` to the *previous* current line if at EOF.
- `Insert` adjacent to a `Delete` of equal-ish run length -> `Modified` for the overlapping lines, `Added` for the surplus. (vscode collapses same-hunk del+ins into "Modified" -- match that.)

decision: "modified" detection runs as a post-pass over the op stream. when a `Delete(n)` is immediately followed by an `Insert(m)`, the first `min(n,m)` current-side lines flip from `Added` to `Modified`; the surplus keeps its kind; if `n > m` the leftover delete attaches as `DeletedAbove`/`Below` to the surrounding line.

## colours / glyphs

terminal constraint: we cannot draw thin vertical bars between cells. choose between two render strategies and ship strategy A first:

**strategy A (chosen):** recolour the existing `│` separator that already lives in the margin (line-numbers column, see `buffer/mod.rs` render: the `"{:1$} │ "` format string). that `│` glyph becomes the gutter bar.

- `Added`        -> bright green `│`
- `Modified`     -> bright yellow `│`  (vscode uses blue; yellow reads better against our default dim-grey margin)
- `DeletedAbove` -> bright red `▴` replacing the `│` (upward wedge)
- `DeletedBelow` -> bright red `▾` replacing the `│`
- `None`         -> default `│` (existing 0x7f7f7f7f tint)

deleted-above and deleted-below can co-occur on the same line. when both, prefer `DeletedAbove` (match vscode).

**strategy B (deferred):** add a dedicated 1-cell gutter column between line numbers and `│`. rejected for v1: requires widening `margin_width` across the layout pipeline. revisit once a real second column is needed.

## baseline source

new module `crates/edit/src/bin/edit/git.rs`, subprocess wrapper:

- `git::locate(path) -> io::Result<GitInfo { repo_root, rel_path }>` via `git rev-parse --show-toplevel`. canonicalise both sides (macos `/var` vs `/private/var`).
- `git::read_baseline(&info) -> io::Result<Vec<u8>>`: `git show HEAD:<rel>`, fall back to `git show :<rel>`.
- no libgit2. binary missing -> graceful noop.

caching: per-document, lazy. `Document::baseline: Option<BaselineState>`:

```rust
struct BaselineState {
    bytes: Vec<u8>,           // None on tracked-but-binary or oversize
    head_oid: Option<String>, // for staleness check (see below)
    loaded_at: Instant,
}
```

invalidation:

- on `Document::save` success: refresh `head_oid` (still `HEAD`, but the working-tree path may have just diverged -- nothing to do here, the baseline is unchanged by *our* save).
- on idle (>= 30 s since last check, only when buffer is at rest): re-run `git rev-parse HEAD` and re-read baseline if oid changed. catches external commits / branch switches without a heavy filesystem watcher.
- on first failure (e.g. git binary disappears): mark `disabled` and stop retrying for this document.

oversize / binary: `MAX_DIFF_BYTES = 5 MiB`, null-byte sniff in first 8 KiB. exceeds either -> baseline stays `None`, all marks suppressed.

## per-line state storage

new field on `TextBuffer`:

```rust
gutter_marks: Vec<GutterMark>,  // indexed by logical line y
```

api:

```rust
pub fn set_gutter_marks(&mut self, marks: Vec<GutterMark>);
pub fn clear_gutter_marks(&mut self);
pub fn gutter_mark(&self, y: CoordType) -> GutterMark;  // out-of-range -> None
```

shifting on edit: don't bother. marks are recomputed wholesale on rediff; between cycles they may briefly point at the wrong logical line. that's fine -- the debounce window is short, and a shifted mark is much less distracting than vscode's identical UX (which has the same delay).

`clear_gutter_marks` from `recalc_after_content_swap`.

## recompute cadence

debounced, in the main loop:

```rust
const GUTTER_REDIFF_DEBOUNCE_MS: u64 = 300;
```

ambient signal -- no need to repaint on every keystroke. pseudocode in main loop after input:

```text
state.document.gutter_mark_dirty_if_changed();
if state.document.gutter_should_rebuild() {
    state.document.refresh_gutter();
}
```

`refresh_gutter`:

1. ensure `BaselineState` loaded (lazy first call). if disabled / no baseline -> `clear_gutter_marks` and return.
2. read current bytes via `read_forward` chunks (cap at `MAX_DIFF_BYTES`; over -> clear and disable for this document until next save).
3. line-level Myers diff (`baseline` vs `current`).
4. walk ops, fold del+ins into modified, build `Vec<GutterMark>` of length `logical_lines`.
5. `set_gutter_marks(marks)`.

cost: 5 MiB cap, Myers `MAX_D` fallback to bail on pathological inputs. realistic files diff in single-digit ms.

## render integration

in `buffer/mod.rs render()`, inside the per-visual-line loop, after the margin string is written but before the `blend_fg` of the existing dim-grey foreground (line ~1951 of current `render`). lookup `self.gutter_marks.get(cursor_beg.logical_pos.y).copied().unwrap_or(None)`. if non-`None`:

- locate the `│` cell within the margin: it's at column `destination.left + margin_width - 2` (the format is `"{n} │ "` -- two chars after the `│`).
- for `DeletedAbove`/`Below`: overwrite that cell with `▴` / `▾` via a small `fb.replace_text` (or equivalent low-level write -- check what's available; if the framebuffer doesn't expose a single-cell rewrite helper yet, write a bounded utility next to `blend_fg`).
- for all marks (including `Added`/`Modified` which keep the `│`): blend the target colour onto that single-cell rect with full alpha, *after* the global margin tint. order matters: the tint pass currently dims the whole margin; we want the marker cell at full saturation.

colours via `IndexedColor::BrightGreen` / `BrightYellow` / `BrightRed`.

wrapped lines: only the first visual line of a wrapped logical line gets the mark (that's the only one with the line number anyway; subsequent visual lines have the `... │` template and the existing dim-fg blend).

selection / cursor highlight already happens after this -- they'll cover the marker cell when the cursor sits there. acceptable: vscode behaves the same.

## degradation

| condition                          | behaviour                                |
| ---------------------------------- | ---------------------------------------- |
| file outside any git repo          | `locate` fails -> disable, no marks      |
| `git` not on PATH                  | `locate` fails -> disable, no marks      |
| file untracked, no staged blob     | `read_baseline` fails -> disable         |
| file binary or > 5 MiB             | suppress, marks cleared                  |
| baseline read transiently fails    | retry on next save / 30 s idle           |
| read-only buffer                   | still computed and shown (read-only is orthogonal to "differs from HEAD") |
| repo with detached HEAD / no HEAD  | HEAD lookup fails; index fallback used if any |

never block the input loop on git. v1 runs the subprocess synchronously inside `refresh_gutter`. if that proves laggy on slow filesystems, move baseline reads to a background thread in v2 -- the diff step itself is fast enough to stay on the main thread.

## files to touch

- `crates/edit/src/bin/edit/git.rs` -- new. subprocess wrapper: `locate`, `read_baseline`, `head_oid`.
- `crates/edit/src/bin/edit/linediff.rs` -- new. line-level Myers diff producing an `Equal`/`Insert`/`Delete` op stream.
- `crates/edit/src/bin/edit/gutter_diff.rs` -- new. orchestrator: `BaselineState`, `compute_marks(baseline, current) -> Vec<GutterMark>`, modified-line folding.
- `crates/edit/src/buffer/mod.rs` -- add `gutter_marks` field, getters / setters, render-time draw of the per-line marker cell. `enum GutterMark` lives here.
- `crates/edit/src/bin/edit/documents.rs` -- `Document::baseline`, `Document::gutter_*` methods, post-save baseline refresh hook.
- `crates/edit/src/bin/edit/main.rs` -- main-loop debounce check between input and render.
- `crates/edit/src/bin/edit/state.rs` -- maybe nothing; gutter state lives on the document.
- `crates/edit/src/bin/edit/settings.rs` -- optional toggle `gutter_diff: bool` (default true) for users who don't want it.

no key binding, no menubar entry in v1. (could add `View > Gutter Diff` checkbox later -- one-line edit in `draw_menubar.rs`.)

## tests

- `linediff`: equal / pure-insert / pure-delete / mixed / empty-vs-empty / empty-vs-content / pathological-bail. ~10 tests.
- `git`: locate / read_baseline / fallback / outside-repo (skip if no `git`). 4-6 tests.
- `gutter_diff::compute_marks`: pure-equal -> all `None`; pure-add -> all `Added`; pure-delete-at-eof -> last current line is `DeletedBelow`; del+ins of same length -> `Modified`; del(2)+ins(3) -> 2 `Modified` + 1 `Added`; del(3)+ins(1) -> 1 `Modified` + 1 `DeletedAbove` on the next line; deleted-at-bof -> first line `DeletedAbove`. 8 tests.
- buffer: `set_gutter_marks` clears via `recalc_after_content_swap`; out-of-range `gutter_mark()` returns `None`. 2 tests.
- end-to-end (pty harness, optional): open a file, `git commit`, edit a line, wait 350 ms, assert the rendered margin contains a `│` cell with the modified colour. probably skip in v1 -- terminal-colour assertions are flaky.

## trade-offs / open questions

- **single-character glyph instead of a thin bar.** unavoidable in a tui. hijacking the existing `│` separator is the cleanest path; the alternative (extra column) is a layout-system change for a 1-bit signal.
- **modified vs added detection.** the del+ins-fold heuristic matches vscode but produces surprising results when a single line is edited drastically (myers may emit it as `Insert`+`Delete` non-adjacent if surrounding context shifts). acceptable; vscode has the same artefact.
- **debounce length.** 300 ms feels right for ambient marks. start there and tune.
- **baseline staleness across external commits.** the 30 s idle check is cheap (`git rev-parse HEAD` is sub-ms) but adds a subprocess every 30 s per document. could instead refresh on focus-in / window-activate events when those exist. v1: idle check is fine.
- **performance on large repos with lfs / sparse checkouts.** `git show` always works regardless of sparse status; lfs-pointer baselines diff sensibly against working-tree binaries (binary sniff catches this and suppresses).
- **untracked files.** silent noop. could mark every line as `Added` (a la vscode for new files) -- defer; needs a clear UX call.
- **submodules.** `git rev-parse --show-toplevel` returns the submodule root, and `git show HEAD:<rel>` works inside the submodule. expected to just work; no special-casing.

## not in scope (v1)

- per-hunk stage / unstage / revert.
- click-to-show-hunk popup.
- baseline picker (always HEAD->index).
- async baseline / off-thread diff.
- diff against a non-HEAD ref or against another branch.
- file-watcher-driven baseline refresh (idle poll only).
- markers on minimap / scrollbar (we have no minimap).
