---
status: open
date: 2026-05-04
description: animated cursor / selection / scroll motion -- render-only interpolation, logic stays discrete
---

# smooth motion

cursor moves, selection extension, and viewport scroll currently snap one frame to the next. proposal: animate the *visible* representation while keeping all editing logic instantaneous. small tau (~25-40ms), still feels fast, just a touch of motion to track the eye.

## current state

- render is event-driven. main loop at [main.rs:128](../crates/edit/src/bin/edit/main.rs) blocks on `sys::read_stdin(read_timeout)` where `read_timeout = vt_parser.read_timeout().min(tui.read_timeout())`.
- `Tui::read_timeout` ([tui.rs:462](../crates/edit/src/tui.rs)) is one-shot deadline -- already used at `tui.rs:2347` to schedule a 25ms wakeup. animation infra effectively present.
- scroll lives in `TextareaContent.scroll_offset: Point` ([tui.rs:3755](../crates/edit/src/tui.rs)), updated synchronously by input.
- cursor visual pos in `TextBuffer.cursor.visual_pos`. selection drawn from logical positions per-frame in [buffer/mod.rs:1788-1944](../crates/edit/src/buffer/mod.rs).
- framebuffer is row-diff'd, emits VT deltas only -- intermediate frames cost ~only the delta.

## approach: render-only interpolation

logic continues snapping to target. animate only what hits the framebuffer. two layers.

### 1. animated scroll offset

- add `scroll_offset_visual: PointF` (or fixed-point) + treat existing `scroll_offset` as target on `TextareaContent`.
- logic continues writing target. renderer reads visual.
- per frame: lerp visual toward target with `1 - exp(-dt / tau)`, tau ~30-50ms. snap when within 0.5 cell.
- if not converged: `needs_rerender()` and `read_timeout = 16ms` (~60fps). loop already supports this.
- round visual to integer cells before draw -- terminal cannot sub-cell scroll. animation perceptible because cells flip in sequence, not jumping N rows at once.

### 2. animated cursor position

- add `cursor_visual_anim: PointF` mirrored from `cursor.visual_pos`.
- lerp same way, round to integer cell for `fb.set_cursor`.
- selection rect drawn from logical positions; phase 1 leaves selection snapping. phase 2 can lerp active end's visual coordinate.

### 3. tick driver

- helper on `Tui`: `request_animation_frame()` -> `read_timeout = min(read_timeout, 16ms)` + `needs_rerender()`.
- after lerp during render, if any animation in flight, call again.
- `dt` from `Instant::now()` delta stored on `Tui`.

## tunables

- `tau_scroll` ~25-40ms.
- `tau_cursor` ~15-25ms (cursor must stay responsive).
- threshold-snap to avoid lingering sub-cell drift.
- big jumps (PageDown across 40 lines): either hard-snap, or shorter tau so completion <80ms. slow lerp over big delta = motion sickness.
- skip animation when `|delta| <= 1` -- pointless cost.
- setting key in [settings.rs](../crates/edit/src/bin/edit/settings.rs): `smooth_motion = true|false`.

## tradeoffs / risks

- terminal cell granularity = no true sub-cell motion. reads as rapid stepping, not mac-style smooth scroll. good for 5-30 cell distances.
- extra frames = extra VT writes. row-based diff redraws whole textarea region each tick. on slow ssh: lag risk -- gate behind setting.
- mouse-drag selection: do **not** animate, would feel laggy. animate only programmatic cursor moves (arrows, PageUp/Down, goto), not mouse-driven.
- animation during fast key-repeat: each keystroke restarts lerp from current visual; need to verify it doesn't visibly lag behind held-arrow scroll. fallback: detect held-key cadence, snap.

## open questions

- per-source gating: which input sources skip animation? proposed skip-list: mouse drag, mouse wheel (already smooth from terminal), held-key repeat at high cadence, single-cell deltas.
- selection-end animation worth the complexity? phase 2 question.
- interaction with framebuffer diff: does animating scroll cause full-region rewrites every tick (~60 rows x cells x bytes)? measure before shipping.
- cursor-blink: existing blink interaction with animation interval -- ensure both share the same tick driver.

## phasing

1. tick infra + `dt` on `Tui`.
2. animated scroll offset only. ship, feel it.
3. animated cursor glyph.
4. setting toggle + per-source gating (skip mouse drag, skip 1-cell deltas).
