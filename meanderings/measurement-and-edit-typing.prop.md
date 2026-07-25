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

## done: typed gap-buffer edits

`allocate_gap(off, len, delete)` with `off..usize::MAX` for "replace to the
end" and `usize::MAX..usize::MAX` for "append" is now an `Edit` enum plus
`apply`, which can and does check the range. `replace` stays as the permissive
door, matching `WriteableDocument::replace`'s clamping contract, and
`reserve_gap` keeps the two callers that fill space incrementally.

Deferred here on the grounds that undo's reinsert loop would have to be
restructured at the same time. That turned out not to be true: fixing the
line-ending bug had already reduced that loop to a plain insert, so it took
the typed API unchanged and got a better OOM check out of it.

## blocked: deriving the caret from the layout

**The idea.** `Cursor` is both resumable measurement state and a paint
position. That dual role produced a caret that disagreed with the text, the
collapse that fixed it, and the regression the collapse caused.
`buffer/layout.rs` already produces an owned `TextareaLayout` per frame; the
caret should be a value it returns.

**Why it is blocked, precisely.** It is circular as the code stands.
`tui/textarea.rs` reads `caret_visual_pos()` to get the animation's *target*,
advances the lerp, and passes the *animated* result into `tb.layout(...)`,
which uses it to pin the selection's active edge. So the caret has to exist
before layout runs. Deriving it from layout instead needs either two layout
passes per frame or the animator/draw separation that `anim_refactor.md`
describes and descoped -- an animator that perturbs a finished frame IR rather
than feeding into layout. That is the remaining stage of that refactor, not a
change that can be made here.

**What has been taken instead.** Every place that painted the raw navigation
cursor as the caret now uses `caret_visual_pos()`: the production path, the
`build_textarea_physics` IR that the descoped stage would build on, and
`render`'s fallback for callers passing no override. So the duplication that
made this worth doing is gone even though the structural fix is not, and there
is no outstanding defect behind it.

Two things that look like they would fall out of the structural version, but do
not: `canonicalize_wrap_boundary` has a second caller in the drift check and
cannot be deleted, and the paint-time bump in `cursor_block` covers a case the
collapse deliberately leaves alone (a line whose length is exactly the wrap
column).

## fixed: the visual-line stats drift underneath

Fixing the resumption bug uncovered two more in the same family, and they were
cancelling each other out often enough to look like one intermittent fault.
Both had the same shape as everything else here: an absolute quantity computed
by relative arithmetic from a seed that was allowed to be mid-row.

`goto_line_start` derives its result's `visual_pos.y` by offsetting whatever
seed it is handed. A cursor inside a word too wide for a row carries the
row-break ambiguity, so any count derived from it inherits the error.

- `edit_begin` measured the next line's row from the mid-row `cursor` while
  subtracting a base taken from `safe_start` -- two seeds, one of them
  ambiguous. Left `stats.visual_lines` a row **under**.
- `reflow` recomputed the absolute count starting from `self.cursor`. A row
  **over**.

Both now start from an unambiguous seed: `safe_start` for the delta, the
document start for the absolute count, which is also what the drift check
itself walks.

Worth recording how nearly this was missed. The first hypothesis -- the
mismatched bases in `edit_begin` -- was correct, and I rejected it on the
evidence of PTY run counts, 2 failures in 5 against 4 in 6. That is noise, not
a measurement. It only became tractable with a deterministic unit-level repro,
which showed the drift on every run once the cursor was inside the word, and
then showed the second fault the moment the first was fixed: the sign of the
error flipped from under to over.
