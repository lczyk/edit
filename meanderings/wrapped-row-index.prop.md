---
status: open
date: 2026-05-31
description: viewport-scoped wrapped-row index to simplify vertical cursor motion under word-wrap
---

# viewport-scoped wrapped-row index

plan for a per-frame index of visual rows that accelerates and simplifies vertical
cursor motion under word-wrap, without materialising the whole document.

## motivation

vertical cursor motion (arrows, pgup/pgdn, shift-select up/down) and the move-line
visual-band math all route through `cursor_move_to_visual_internal`, which under
word-wrap falls back to `goto_line_start` + a multi-row `measure_forward` walk with
wrap-opportunity backtracking. `measure_forward` (measurement.rs) tracks five
coordinate systems at once (offset, logical_pos, visual_pos, column, wrap_opp) and the
backtracking paths are where the visual-motion bugs have historically lived (see the
`measure_forward_monotonic` and `cursor_visual_pos_drift` sanity checks).

the wrap layout is viewport-width-dependent and changes on resize, so the document is
deliberately never materialised -- the streaming gap-buffer design is what lets the
editor scale to huge files. so we don't materialise the file. we materialise the
**viewport**: a small index of the visual rows currently on screen, rebuilt where
`layout()` already walks them.

## non-goals

- no whole-document wrapped buffer. memory + rebuild-on-edit cost is exactly what the
  streaming design avoids.
- move-line (`MoveLineUp`/`MoveLineDown`) stays a logical-line reorder in offset/logical
  space. the index only simplifies its visual-band *landing* math, not the text move.
- motion call sites do not change. they keep calling `cursor_move_to_visual`.

## integration strategy

the index sits **behind `cursor_move_to_visual_internal`** as a cache. it is a free
side-product of `layout()`:

- `layout()` (buffer/mod.rs) already walks every visible row, computing `cursor_beg`
  per row via `cursor_move_to_visual_internal`, then discards it. we capture those
  row-start cursors into a `Vec`. near-zero added cost -- the measurement already runs.
- next input's `cursor_move_to_visual_internal` consults the index: if the target row
  is in range, seed `measure_forward` from the row-start cursor and walk *only within
  that row* to resolve `target.x`. no `goto_line_start`, no cross-row backtracking.
- out of range -> fall back to the existing path.

data flow: `layout()` builds the index each frame -> next motion consults it -> resolve
within one row -> fall back when outside the indexed range.

## data structure

```rust
// buffer/mod.rs, near TextBufferStatistics
struct WrappedView {
    rows: Vec<Cursor>,            // row-start cursor per visual row, + 1 trailing end cursor
    origin_y: CoordType,          // visual y of rows[0]
    word_wrap_column: CoordType,  // tag: wrap config it was built under
    generation: u32,              // tag: buffer.generation() at build time
}
```

new `TextBuffer` field: `wrapped_view: Option<WrappedView>`, beside `cursor_for_rendering`.

storing the **full `Cursor`** per row (not just `offset`) is load-bearing: `wrap_opp`
must carry so resuming `measure_forward` mid-logical-line makes the same force-wrap /
wrap-opportunity decisions.

### validity / invalidation

valid iff `generation == buffer.generation() && word_wrap_column == self.word_wrap_column`.

- the generation tag self-invalidates on every edit (`buffer.generation()` bumps per
  edit).
- the wrap-column tag covers resize + word-wrap toggle (both change
  `self.word_wrap_column` via `reflow_internal`).
- `reflow_internal` already clears `cursor_for_rendering`; clear `wrapped_view` in the
  same place.

no explicit per-edit invalidation needed; the tags self-invalidate.

### in-range check

target row covered iff `origin_y <= target.y < origin_y + rows.len()`. index into the
vec is `target.y - origin_y` (visual_pos.y is monotonic across rows, so direct index,
no search).

## touchpoints

- `buffer/mod.rs` `TextBuffer` struct -- add `wrapped_view` field (+ init in `new`,
  clear in `reflow_internal` alongside `cursor_for_rendering = None`).
- `buffer/mod.rs` `layout()` -- capture `cursor_beg` per row into a `Vec` (the loop
  already computes each `cursor_beg`); write `self.wrapped_view` after the loop.
- `buffer/mod.rs` `cursor_move_to_visual_internal` -- add the in-range fast path: index
  = `target.y - view.origin_y`; seed `measure_forward` from `rows[index]`, short walk to
  `target.x`. else existing path.
- `buffer/mod.rs` `move_selected_lines` `visual_y_of` -- optional (phase 3): swap the
  `cursor_move_to_logical_internal` lookups for index hits when the logical line is
  in-view; keep fallback.

one internal method gains a fast path; one struct; one field; layout gains ~3 lines.

## phasing

### phase 1 -- index as pure accelerator (correctness-neutral) -- LANDED (`9259be6`)

build the view in `layout()`, consult in `cursor_move_to_visual_internal`, fall back
otherwise. behaviour identical to today, just faster + simpler in the common case. gate
behind the `sanity` feature cross-check (below) to prove equivalence before trusting it.
the bulk of the value lands here.

### phase 2 -- incremental scroll reuse (perf) -- SUBSUMED BY PHASE 1

original idea: when `layout()` runs with `origin_y` near the previous view's, reuse the
overlap instead of re-walking -- scroll down extends the tail from the last row-end
cursor; scroll up re-walks from `goto_line_start` of the new top row.

outcome: not worth building -- phase 1 already delivers it. reasoning:

- **cross-frame seed is already bounded by scroll distance.** after each layout,
  `set_cursor_for_rendering(layout.start_cursor)` caches the frame's row-0 cursor at the
  current `origin.y` (tui.rs). the next frame seeds row 0 from the closer of `self.cursor`
  / `cursor_for_rendering` to the new `origin.y` and walks only the scroll delta -- never
  the document.
- **scroll down + in-range motion is already O(1).** layout's per-row `cursor_beg` goes
  through `cursor_move_to_visual_internal`, which consults the previous frame's
  `wrapped_view`. a new `origin.y` inside the old view's range makes the row-0 seed an
  index hit; rows `1..height` each seed from the prior row-end (forward ~1 row, also an
  in-range hit). so the down/small-scroll case the proposal wanted to optimise is already
  free.
- **scroll up is an inherent walk phase 2 can't shorten.** new `origin.y` below the old
  view has no anchor above it, and you can't measure backward through a wrap. the
  proposal's "re-walk from `goto_line_start` of the new top row" is *exactly* the existing
  backward seek -- phase 2 would reimplement the same walk for no gain. the seek is
  already bounded by the scroll delta via `cursor_for_rendering`.

the only residual is reusing the per-frame `Vec<Cursor>` allocation (~`height * size_of::
<Cursor>`), which is trivial and not worth the added invalidation surface.

### phase 3 -- move-line band lookups (optional)

swap `visual_y_of` in `move_selected_lines` for index lookups. marginal -- the targets
are already nearby and cheap. do only if it reads cleaner.

## risks / edge cases

- **staleness between motions** -- two motions before a re-layout: the second sees a view
  built for the prior frame. generation + wrap tags catch edits; the in-range check
  guards pure-motion staleness (out of range -> fall back). safe by construction.
- **pgup/pgdn at the indexed edge** -- target = `cursor.y +- height`, often one row
  outside the view -> falls back to the measure path. fine: big jump, cost acceptable,
  next frame re-indexes.
- **preferred_column semantics unchanged** -- the `MAX` / `0` sentinels still resolve
  through `measure_forward`'s `calc_target_x`; the index only changes the seed, not
  target resolution.
- **multi-width / wrap-opp at row boundary** -- exactly why we store the full cursor incl
  `wrap_opp`. the sanity cross-check de-risks this.

## test plan

- **sanity cross-check (phase 1 gate):** under `#[cfg(feature = "sanity")]`, when the
  fast path fires, also run the old path and assert equal cursor -- same pattern as the
  existing `cursor_visual_pos_drift` / `measure_forward_monotonic` checks. run the golden
  + buffer suites with it on; any divergence = bug, zero behaviour risk to ship.
- unit tests on `WrappedView`: force-wrap line spanning N rows, tab rows, wide-char (CJK)
  rows, empty lines. assert arrow up/down from each row lands identically with the index
  on vs off.
- scroll reuse (phase 2): assert incremental view == fresh full rebuild for scroll-by-1
  up + down across a wrapped region.

## size estimate

phase 1 is small: ~one struct, one field, ~3 lines in `layout()`, ~15-line fast path in
`cursor_move_to_visual_internal`, plus the sanity check. phases 2-3 are independent
follow-ups.
