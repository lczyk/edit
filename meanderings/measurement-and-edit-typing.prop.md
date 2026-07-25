---
status: open
date: 2026-07-25
description: four structural changes considered around the measurement/edit core -- two rejected with evidence, two scoped and deferred
---

# measurement and edit typing -- four proposals, two rejected

Written after a session that fixed a run of bugs in the cursor-measurement and
undo paths. Each of these came up as "the structural change that would stop
this class of bug". Two do not survive contact; two are real but larger than
the fixes they would generalise. Recorded so the next person does not
re-derive them from scratch.

## rejected: a `RowStart` newtype for the measurement seed

**The idea.** `MeasurementConfig::with_cursor` accepts any `Cursor`, but a
resumed measurement only reproduces the layout when seeded from a row or line
start. Make that a type: a `RowStart` producible only by `goto_line_start` and
the row walk, required by the measurements whose answer must match the layout.

**Why not.**

- The contract as stated is false. `measurement.rs`'s own passing tests resume
  from deliberately non-row-start positions and expect the documented
  `goto_visual` behaviour. The type would have to be bypassed there.
- The highest-traffic site -- ordinary rightward movement and typing -- would
  need an unchecked escape hatch to stay fast, which is precisely where the
  bug was. A type nobody can use at the one place it matters is decoration.
- The bug it was meant to prevent is now prevented directly: the three
  resumption sites fall back to a line-start walk when the row holds an
  outstanding wrap opportunity, which is the actual precondition. It is
  cheaper, and it is checkable at runtime by the existing drift check.

If the resumption ever needs to be exact rather than avoided, the work is to
carry the opportunity's position and the previous cluster's line-break
properties in `Cursor` -- see the note in `canonicalize_wrap_boundary`. That is
a hot-path struct growing by roughly its own size again, and every explicit
`Cursor` literal in the measurement tests changing with it.

## rejected: making the three silent-absorb sites return errors

**The idea.** `GapBuffer::allocate_gap` clamps out-of-range offsets,
`LineBuffer::replace_text` drops out-of-range rows, and `gutter`'s `set_mark`
drops out-of-range indices. Each turns a caller's arithmetic slip into
slightly-wrong output. Make them say so.

**Why not, site by site.**

- `allocate_gap`'s clamps are load-bearing API, not slack. `copy_from` spells
  "replace to the end" as `off..usize::MAX` and `copy_into` spells "append" as
  `usize::MAX..usize::MAX`. A bounds check cannot tell a slip from the
  sentinel. This was tried, fired immediately, and was reverted -- the comment
  at the site records it. Fixing this properly means the typed-edit proposal
  below, not a check.
- `replace_text` already carries a sanity check. Promoting it to a hard assert
  or a `Result` would turn one mis-clipped render row into a panic under the
  `sanity` feature, which CI now runs the PTY suite against -- a cosmetic
  glitch becoming a failed build.
- `set_mark`'s per-call check would be strictly redundant with the aggregate
  coverage check at the end of `marks_from_ops`, which already catches the same
  condition once instead of once per line.

## deferred: typed gap-buffer edits

**The idea.** Replace `allocate_gap(off, len, delete)` and its sentinel ranges
with an enum -- `Insert { at, text }`, `Delete { range }`, `ReplaceToEnd
{ from }`, `Append` -- so both intents are explicit, a bounds check becomes
possible, and an undo entry becomes a natural mirror of the edit.

**Why it is bigger than it looks.** `undo.rs`'s reinsert loop drives
`allocate_gap` directly for the `&mut [u8]` gap it returns, writes into it by
hand, and commits a partial length. It also carries the OOM-truncation check.
Either the enum coexists with raw gap access -- in which case the sentinel path
survives beside it and nothing is really typed -- or that loop is restructured
at the same time. That loop was the site of a line-ending corruption bug fixed
this session, so it deserves its own change with its own tests rather than
riding along.

## deferred: deriving the caret from the layout

**The idea.** `Cursor` is both resumable measurement state and a paint
position. That dual role produced a caret that disagreed with the text, the
collapse that fixed it, and the regression the collapse caused.
`buffer/layout.rs` already produces an owned `TextareaLayout` per frame; the
caret should be a value it returns.

**What blocks it.** The caret is needed *before* `layout()` runs, because it
seeds the animation lerp whose output `layout()` then consumes to pin the
selection edge. So it cannot simply be read back out afterwards without
restructuring that loop. The row loop also only spans the visible viewport, so
a derived caret is `Option<Point>` and needs a tested off-screen fallback for
animation continuity while scrolling.

Two things that look like they would fall out of it, but do not:
`canonicalize_wrap_boundary` has a second caller in the drift check and cannot
be deleted, and the paint-time bump in `cursor_block` covers a case the
collapse deliberately leaves alone (a line whose length is exactly the wrap
column).

The cheap part has been taken: `build_textarea_physics` now reads
`caret_visual_pos()` rather than the navigation cursor, so the frame-wide IR
that `anim_refactor.md` descoped will not reintroduce the bug when someone
wires it up.

## open: the visual-line stats drift underneath

Fixing the resumption bug uncovered a third one in the same family, quieter and
intermittent: `stats_visual_lines_drift` (stored 5, recomputed 6) fires on
about one run in four of the long-word PTY test, when typing inside a word wide
enough to hard-wrap. The buffer's cached visual line count ends up a row short.

It is not a regression from the guards. Without them the cursor bug fires first
and the test aborts before reaching this state, so the earlier trips were
masking it -- measured both ways: 10 cursor trips and no stats trips without
the guards, no cursor trips and intermittent stats trips with them.

One hypothesis tested and rejected. `edit_begin` computes
`line_height_in_rows` as `next_line.visual_pos.y - safe_start.visual_pos.y`
where `next_line` is measured from the mid-row `cursor` and the subtrahend
comes from `safe_start` -- two bases. Making both start at `safe_start` did not
fix it and made the trip slightly more frequent, so the mismatch is not the
cause even though it still reads wrong.

Where to look next: `edit_end` has two arms for updating `visual_lines`, an
incremental delta when `deleted_count < info.distance_next_line_start` and a
full remeasure otherwise. Instrumenting both against a from-scratch walk, per
keystroke, would say which one drifts and on which input. The intermittency
tracks input batching -- the test sends keys with `settle=0` -- so the state
that triggers it depends on how many graphemes arrive in one frame.

CI's strict-sanity PTY run is marked `continue-on-error` until this is fixed:
it still reports, so a second unrelated trip would be visible, but it does not
gate on a known-flaky check.
