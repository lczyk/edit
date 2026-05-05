---
status: landed
date: 2026-05-04
landed: 2026-05-04
landed-in: 98d39f6
description: vscode-style minimap in the right margin, braille-dot density render, ascii-block fallback
---

# minimap

vscode-parity "minimap" overview rail glued to the right edge of the editor pane. each visible cell summarises N source lines x M source columns of the document by lighting dots / shading blocks proportional to non-whitespace density. always-on once the file fits the heuristic; passive overlay; no buffer mutation; same lifecycle hooks as `gutter_diff.rs`.

## scope

- v1: density render + per-line dominant-colour tint sampled from `lsh`. no per-line diff tint, no current-selection highlight inside the minimap.
- v1: always-on iff document height > viewport height (otherwise the minimap is just a dim column -- skip it). no toggle yet; menubar checkbox + setting can follow once it stops surprising people.
- v1: viewport-window box (the bracket showing where the visible slice sits in the doc) drawn over the minimap as a recoloured background band.
- v1: click-to-jump on the minimap rail. left-click on minimap row R sets `scroll_y` such that R maps to viewport centre. click-drag scrolls continuously. middle/right click reserved for future. requires hit-test plumbing in `tui.rs`.
- v1: width is fixed at `2` cells in unicode mode and `1` cell in ascii mode (see [render strategies](#render-strategies)). knob lives in `Settings` once it stabilises.
- v1: read-only. mouse-click-to-jump and click-drag-to-scroll deferred. once the rail is on screen, the natural follow-up is "let me click it" -- but that needs hit-testing wired through `tui.rs`, so park it.

## placement

right-edge column order, left to right:

```
... source text ... | minimap (1-2 cells) | scrollbar (1 cell)
```

minimap sits *between* source and scrollbar, **not** replacing the scrollbar. the scrollbar stays authoritative for cursor-position-in-doc; the minimap is glanceable shape. in narrow terminals (`viewport.width < THRESHOLD`, propose 80) suppress the minimap entirely so source text isn't crowded -- scrollbar stays.

reserving the column happens in `tui.rs` near where the scrollbar already gets its `-1`. add a parallel `-2` (or `-1` ascii) when the minimap is active. plumbing-wise: extend `ScrollAreaCtx`-ish state with `minimap_width: CoordType` and account for it in the same spots that already do `width - 1` for the scrollbar.

## render strategies

terminal cells are app 2x4 sub-units once you commit to braille (U+2800..U+28FF), so the natural grid is:

- **strategy A -- braille (chosen for unicode mode).** each minimap cell covers `4 source rows x 2*W source columns`, where `W` is minimap width in cells. at `W=2` that's `4 rows x 4 source-cols-per-cell` summed. each of the 8 dots in a braille cell maps to one (row, col-bucket) sub-unit -- light the dot if any source char in that sub-unit is non-whitespace. yields 256 distinct glyphs; eyes parse it as a low-res photo of the file. zero colour requirement; reads fine on monochrome terminals as long as unicode is on.
- **strategy B -- ascii blocks (chosen for ascii / `--no-color` mode).** drop to `W=1`, one cell per `4 source rows x 8 source cols`. pick from `[' ', '.', ':', '|', '#']` by total inky-cell-count in the bucket (5 buckets, equal-spaced thresholds). same vertical compression, no horizontal sub-cell resolution -- ascii has nothing comparable to braille. uglier but legible without unicode.

mode switch hangs off the existing globals in [crates/edit/src/glyphs.rs](../crates/edit/src/glyphs.rs):

```rust
pub fn minimap_glyph(bucket: u8) -> char { ... }      // 0..=255 braille, or 0..=4 ascii
pub fn minimap_width() -> CoordType { if ascii_only() { 1 } else { 2 } }
```

`no_color()` does not force strategy B -- braille works fine without colour. only `ascii_only()` flips the strategy. the scrollbar already degrades cleanly under both modes; mirror that.

## viewport-window overlay

over the top of the rendered cells, paint a background-colour band covering the row range that corresponds to the currently visible slice of the document. computed from `scroll_y .. scroll_y + viewport_height` mapped to minimap rows:

```
minimap_row_top    = scroll_y * minimap_rows / content_rows
minimap_row_bottom = (scroll_y + viewport_height) * minimap_rows / content_rows
```

clamp to `[0, minimap_rows)`, paint inclusive. background tint only -- glyphs underneath stay visible. picks up the same dim-grey-on-default treatment the gutter `│` already uses.

## data flow

source-of-truth is the `TextBuffer`. minimap does not own a copy of anything; it samples on render.

per-frame:

1. `draw_editor.rs` calls a new `draw_minimap(rect, buffer, scroll_y)`.
2. that walks `buffer.lines()` (or moral equivalent -- whatever cheap line-iter the buffer exposes) over `[0, content_rows)` in chunks of 4. for each chunk:
   - per source line, count inky cells in each of the `2*W` column buckets of width `bucket_cols = (line_visual_width + buckets - 1) / buckets`.
   - "inky" = grapheme that is not `' '`, `'\t'`, or other whitespace. don't tokenise; this is a density map.
3. fold per-row dot bits into one braille code point (or one ascii bucket-count), emit at the minimap column.
4. paint viewport-window background last.

cost ceiling: the buffer's `memchr2` line scanner already does >100 GB/s; minimap walks the same bytes once per frame at worst. for files where that's still painful, gate behind:

- `MAX_MINIMAP_BYTES = 5 MiB` -- larger files render an empty rail with a `~` decoration top-and-bottom. matches how `gutter_diff` handles oversize.
- caching: store a `Vec<MinimapCell>` of length `minimap_rows`, dirty-bit per row chunk. invalidate on edit by chunk-index of `cursor_offset`. only the dirty chunks recompute next frame.

## interaction with existing rails

- **scrollbar stays.** minimap does not replace it. user has muscle memory for the scrollbar position.
- **gutter diff stays.** lives on the *left* margin in the line-numbers column; minimap is on the right. no conflict.
- **diff mode.** when diff mode is active (red/green stripes), the minimap renders the interleaved view as-is -- the buffer it samples already contains the stripes. no special-casing in v1; revisit if it reads poorly.

## settings

defer until v1 ships. anticipated knobs once it does:

- `minimap.enabled` -- bool, default `true`.
- `minimap.width` -- `1 | 2 | 3`, default `2` (unicode) / `1` (ascii). 3 means `4 rows x 6 cols per cell`, useful on very wide files.
- `minimap.min_terminal_width` -- threshold below which the minimap auto-suppresses, default `80`.

none of these block v1.

## decided

- **wide chars**: count visual width. a 2-col cjk grapheme contributes 2 inky bucket-units. minimap is visual-mass.
- **tabs**: expand to `tab_width` visual columns of whitespace, then normal whitespace-not-inky rule. consistent with visual-mass framing. consequence: leading-indent staircase will not show in the minimap (indent is whitespace -> non-inky). accepted tradeoff.
- **folded regions / soft-wrap**: n/a; edit has neither. revisit if folding lands.
- **click-to-jump**: in scope for v1 (see [click-to-jump](#click-to-jump)).
- **lsh colour sampling**: in scope for v1, per-line dominant colour (see [colour sampling](#colour-sampling)).

## click-to-jump

mouse handling pathway:

- `tui.rs` already routes mouse events through hit-tests on layout nodes. add a `MinimapNode` (or moral equivalent) that owns the rail's `Rect` and accepts `MouseDown` / `MouseDrag`.
- on `MouseDown` at minimap-row `R` (0-indexed within the rail):
  - `target_doc_row = R * content_rows / minimap_rows`
  - new `scroll_y = clamp(target_doc_row - viewport_height / 2, 0, content_rows - viewport_height)`
- on `MouseDrag`: same calc each event. no inertia, no animation.
- cursor itself does **not** move. minimap is navigation-only, parity with vscode.
- `MouseUp` outside the rail still completes drag normally.

open detail: the existing scrollbar drag uses a `thumb_grab_offset`-style trick so the thumb doesn't snap to cursor on grab. for the minimap, snap-to-centre is fine on initial click (it's a jump, not a grab), and drag tracks 1:1 thereafter. revisit if it feels jumpy.

## colour sampling

per-line dominant colour from `lsh`:

- when `lsh` highlights a line, it produces a stream of `(byte_range, style)` spans. sample the style covering the longest non-whitespace span on the line; that's the line's dominant style.
- minimap row covers 4 source rows -> pick the dominant style with the largest total non-whitespace byte coverage across those 4 lines. ties: first wins.
- foreground colour of that style becomes the minimap glyph's foreground for the whole minimap row. background stays default (or viewport-window tint where overlapping).
- ascii / `--no-color` mode: skip entirely. monochrome.
- cache: dominant colour stored in the per-chunk cache (`MinimapCell { glyphs: [char; W], fg: Color }`), invalidated by the same dirty-bit pathway as the density bits.
- cost: lsh already runs on the visible slice for syntax painting. for off-screen rows we either (a) run lsh lazily as the user scrolls, accepting one-frame lag in the minimap colour, or (b) extend lsh's cache to the whole document. lean (a) -- start without colour for unvisited rows, fill in as they're touched. simpler, no global lsh-cache pressure.

open detail: how to surface "no colour yet" in the minimap. options: render the glyph in default fg (looks like a hole), or render in dim grey (consistent but loses the "this row hasn't been highlighted" signal). lean default fg.

## file layout (anticipated)

- `crates/edit/src/bin/edit/minimap.rs` -- new module. `MinimapState`, `compute_chunk`, `render`.
- `crates/edit/src/bin/edit/draw_editor.rs` -- call site + width plumbing.
- `crates/edit/src/glyphs.rs` -- `minimap_glyph`, `minimap_width`, ascii-mode swap.
- `crates/edit/src/tui.rs` -- carve `minimap_width` cells off the right edge alongside the scrollbar carve-out.
- `crates/edit/src/bin/edit/state.rs` -- `Document::minimap: Option<MinimapState>` for the dirty-chunk cache.

no new crates. binary-size-neutral target: braille glyph table is computed (`0x2800 + bits`), no LUT.

## retrospective (landed `98d39f6`)

shipped end-to-end in one commit. notes on what changed vs. the plan above:

- **placement: minimap _replaces_ scrollbar.** proposal had them coexisting (`source | minimap | scrollbar`); landed mutually exclusive -- when the rail is visible it absorbs the navigation role. saved a column on narrow terminals; lost the "scrollbar stays authoritative for cursor-position" anchor the proposal banked on.
- **width adapts to terminal width.** proposal had fixed `2` unicode / `1` ascii with auto-suppress under 80 cols. landed: `0` below 30, `1` below 60, `2` above (ascii-quirk follows same thresholds). finer-grained than the proposal's binary on/off.
- **click-drag dropped.** proposal had click-drag-to-scroll 1:1 in v1 (despite a contradictory bullet deferring it). landed: snap-to-centre on fresh `MouseDown` only; drag continuations ignored b/c terminal mouse-drag is too choppy to track usefully.
- **debounce + eager open refresh added.** 300ms debounce on dirty-chunk recompute; eager full refresh on document-open so the first frame already has the rail. neither in the proposal -- both fell out of feeling the latency live.
- **--no-color path tweaked.** proposal had `no_color()` keep braille and skip only colour. landed: `--no-color` skips colour _and_ overwrites the viewport-window band with a heavier marker char (`@` ascii / `\u{2588}` unicode) so the band stays visible without sgr.
- **viewport-window band: constant height.** painted at constant height regardless of scroll position; proposal computed it as proportional fraction of doc. simpler, reads fine.
- **per-chunk dominant colour landed as planned.** 4-line chunks, lsh-derived, cached alongside density bits.
- **5 MiB cap landed as planned.**

still open / unaddressed:
- lazy lsh fill for off-screen rows (proposal option (a)) -- not explicitly verified; whatever lsh's existing slice behaviour is, that's what the rail gets.
- menubar checkbox + setting keys (`minimap.enabled`, `minimap.width`, `minimap.min_terminal_width`) -- explicitly deferred in the proposal, still deferred.
- diff-mode interaction never re-checked.
