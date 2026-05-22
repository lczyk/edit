---
status: in-progress
date: 2026-05-22
description: unify scattered ad-hoc anim machinery behind a physics/animator/draw split where physics = full frame IR, animator = pure perturbation, draw = pure consumer
---

# anim refactor: physics / animator / draw

## thesis

every animatable thing in the editor today is bolted on ad-hoc: cursor lerp on `TextareaContent`, scroll lerp on `TextareaContent`, dropdown timer in `Tui.slide_animations`, line-move sweep via a sidechannel on `TextBuffer`. tunings live in one place (`mod anim` in [tui.rs:169](../crates/edit/src/tui.rs)), but machinery is fragmented and anim state leaks into the model.

target architecture:

- **physics** -- post-layout, anim-free, _full_ frame IR. everything needed to draw a frame. if animator is nil (identity), `draw(physics)` produces a valid frame. no sidechannels.
- **animator** -- pure fn `(prev_anim_state, target_physics, t, dt) -> (next_anim_state, displayed_physics)`. only perturbs fields physics already carries. never invents geometry.
- **draw** -- pure fn `(physics, &mut framebuffer)`. no reads from `Tui` / `TextBuffer` / `TextareaContent` / time.

invariant: `frame = draw(animate(build_physics(world), t))`. nil animator == identity. `no_animations()` toggle collapses to skipping the animator stage, not sprinkled `if` branches across the codebase.

## current state inventory

scattered anim machinery:

- [tui.rs:169](../crates/edit/src/tui.rs) `mod anim` -- timing constants. one good thing already.
- [tui.rs:426](../crates/edit/src/tui.rs) `Tui.slide_animations: HashMap<u64, Instant>` -- dropdown/modal open timers.
- [tui.rs:985-1014](../crates/edit/src/tui.rs) `render_node` mutates `outer_clipped` / `inner_clipped` in-place for slide_down / scale_in.
- [tui.rs:4080](../crates/edit/src/tui.rs) `TextareaContent` -- holds `scroll_offset_visual`, `cursor_visual_anim`, `last_buffer_generation`, `line_move_anim`.
- [tui.rs:4469](../crates/edit/src/tui.rs) `advance_scroll_animation` / [tui.rs:4502](../crates/edit/src/tui.rs) `advance_cursor_animation` -- exp-lerp toward target.
- [tui.rs:4684](../crates/edit/src/tui.rs) `draw_line_move_sweep` -- half-block sweep overlay drawn _after_ `tb.render`.

sidechannels that violate "full IR" invariant:

- [buffer/mod.rs:377](../crates/edit/src/buffer/mod.rs) `TextBuffer.pending_line_move` -- buffer pokes renderer w/ ephemeral seed; drained by [buffer/mod.rs:674](../crates/edit/src/buffer/mod.rs) `take_pending_line_move`.
- [buffer/mod.rs:372](../crates/edit/src/buffer/mod.rs) `cursor_render_override` -- selection rendering at [buffer/mod.rs:2083](../crates/edit/src/buffer/mod.rs) reads the animated cursor to compute selection extent. logic-via-render-state.
- `frame_dt_secs` threaded into `tb.render` ([tui.rs:1186-1189](../crates/edit/src/tui.rs)) -- draw stage receives dt. wrong layer.

scattered killswitch sites: `no_animations()` checked at [tui.rs:986](../crates/edit/src/tui.rs), [tui.rs:1176](../crates/edit/src/tui.rs), [tui.rs:4466](../crates/edit/src/tui.rs), [tui.rs:4499](../crates/edit/src/tui.rs).

fused stages: `TextBuffer::render` (~[buffer/mod.rs:1800-2200](../crates/edit/src/buffer/mod.rs)) does layout (wrap, visual cursor pos, selection geometry) _and_ painting in one pass.

## target shape

```rust
// crates/edit/src/anim/physics.rs
pub struct Physics<'a> {
    pub viewport: Size,
    pub frame: u64,                   // monotonic, for "first sighting" detection
    pub nodes: BVec<'a, NodeDraw<'a>>,
}

pub enum NodeDraw<'a> {
    Box { rect: Rect, bg: StraightRgba, fg: StraightRgba, bordered: bool, reverse: bool },
    Text { rect: Rect, runs: &'a [StyledRun], overflow: Overflow },
    Textarea(TextareaDraw<'a>),
    Scrollbar { track: Rect, offset_y: CoordType, content_rows: CoordType },
    Minimap { /* ... */ },
    LineMoveBand { rect: Rect, bands: &'a [BandRow], from_y: CoordType, to_y: CoordType, height: CoordType, born_frame: u64 },
    Floater { id: NodeId, bbox: Rect, kind: FloaterKind, born_frame: u64 },
}

pub struct TextareaDraw<'a> {
    pub id: NodeId,
    pub dest: Rect,
    pub scroll_offset: Point,         // target
    pub cursor_visual: Point,         // target
    pub selection: Option<SelectionGeom<'a>>,
    pub visual_lines: &'a [VisualLine<'a>],
    pub gutter: GutterDraw<'a>,
    pub focus: bool,
}
```

```rust
// crates/edit/src/anim/engine.rs
pub struct AnimState {
    scroll: HashMap<NodeId, ExpLerp2>,
    cursor: HashMap<NodeId, ExpLerp2>,
    floaters: HashMap<NodeId, FloaterAnim>,
    line_moves: HashMap<NodeId, LineMoveAnim>,
}

pub fn animate(prev: AnimState, target: Physics, t: Instant, dt: f32) -> (AnimState, Physics);
pub fn animate_nil(target: Physics) -> Physics { target }
```

```rust
// crates/edit/src/anim/draw.rs
pub fn draw(physics: &Physics, fb: &mut Framebuffer);
```

```rust
// crates/edit/src/tui.rs (after refactor)
pub fn render<'a>(&mut self, arena: &'a Arena) -> BString<'a> {
    let target = self.build_physics(arena);
    let (next_anim, displayed) = if crate::glyphs::no_animations() {
        (self.anim_state.take(), animate_nil(target))
    } else {
        animate(self.anim_state.take(), target, now, dt)
    };
    self.anim_state = next_anim;
    draw(&displayed, &mut self.framebuffer);
    self.framebuffer.render(arena)
}
```

## stages

### stage 0 -- prep (~1 day)

- new module `crates/edit/src/anim/{mod,physics,engine,draw}.rs`. stubs.
- move `mod anim { ... }` ([tui.rs:169](../crates/edit/src/tui.rs)) into `anim::tuning`.
- write down sidechannels (this doc's "current state inventory") as todo list w/ planned replacement field.
- no behavioural change. one commit.

### stage 1 -- carve `Physics`, split layout from paint (~2-3 days)

biggest chunk. irreversible commit point.

substeps:

1. extract pure layout from `TextBuffer::render`:
   - add `TextBuffer::layout(viewport, dest, focus) -> TextareaLayout<'arena>`.
   - returns: `visual_lines`, `cursor_visual_pos`, `selection_geom`, `minimap_cells`, `line_move_bands`. no framebuffer writes, no `dt`.
   - keep `TextBuffer::paint(layout, fb)` for now -- old `render` becomes `let l = self.layout(...); self.paint(l, fb)`.
   - hardest sub-problem: selection geometry currently reads `cursor_render_override` ([buffer/mod.rs:2083](../crates/edit/src/buffer/mod.rs)). break this. `layout` computes selection at _target_ cursor. animator displaces the active edge later.

2. add `build_physics(tui, tree, arena) -> Physics`:
   - walks `prev_tree` like `render_node` does.
   - per-node, emits `NodeDraw` instead of painting.
   - calls `TextBuffer::layout` for textareas; embeds result in `TextareaDraw`.
   - no time reads. no dt.

3. wire flow: `Tui::render` calls `build_physics`, then walks `physics.nodes` and dispatches to old paint paths. anim state still on `TextareaContent` -- _not yet_ moved to `AnimState`. behaviour identical.

stop-criterion: `Tui::render` builds a `Physics`. existing tests pass. layout pulled out of paint.

### stage 2 -- pure draw (~1-2 days)

- implement `anim::draw::draw(physics, fb)`. moves: border drawing ([tui.rs:1021-1081](../crates/edit/src/tui.rs)), bg/fg blend, text run blit, scrollbar/minimap paint, sweep band paint, modal dim layer.
- delete `TextBuffer::render`. `TextBuffer::paint(layout, fb)` becomes private helper called from `draw`.
- `Tui::render` shrinks to `build_physics` + `draw`.

stop-criterion: framebuffer mutated only inside `anim::draw`. `Tui::render_node` deleted.

### stage 3 -- nil animator (~0.5 day)

- add `animate_nil`. wire as identity.
- pipeline becomes `build -> animate_nil -> draw`.
- delete every `if no_animations()` branch in `Tui` / `TextareaContent` ([tui.rs:986](../crates/edit/src/tui.rs), [tui.rs:1176](../crates/edit/src/tui.rs), [tui.rs:4466](../crates/edit/src/tui.rs), [tui.rs:4499](../crates/edit/src/tui.rs)).
- run editor in `--no-animations` mode. visual diff vs old build should be zero.

stop-criterion: editor works w/ identity animator. anim sidechannels still present in model but bypassed.

### stage 4 -- real animator, displace anim state (~2-3 days)

- introduce `AnimState` owned by `Tui`. keyed by `NodeId`.
- implement `animate`:
  - scroll: lerp `TextareaDraw.scroll_offset` toward target. exp-lerp w/ existing `SCROLL_TAU_SECS`.
  - cursor: lerp `TextareaDraw.cursor_visual` toward target. exp-lerp w/ existing `CURSOR_TAU_SECS`.
  - selection: post-process `SelectionGeom.active_edge` against animated cursor.
  - floaters: born-frame + duration -> clipped bbox. slide_down clips bottom, scale_in clips both edges from centre.
  - line-move band: visible iff `frame - born_frame` within duration. interpolates band position.
- delete dead state:
  - `TextBuffer.pending_line_move`, `take_pending_line_move` -- physics now reads "last move op happened at row R, frame F" directly from a stable buffer field.
  - `TextBuffer.cursor_render_override`, `set_cursor_render_override` -- gone.
  - `TextareaContent.scroll_offset_visual`, `cursor_visual_anim`, `line_move_anim`, `last_buffer_generation` -- moved to `AnimState`.
- `frame_dt_secs` no longer threaded into `tb.render` -- lives on `AnimState`.
- buffer edit detection (currently via `generation()` at [tui.rs:1162](../crates/edit/src/tui.rs)) becomes animator's job: if buffer generation changed, snap lerps to target.
- read_timeout management: animator returns "still animating?" bool, `Tui` clamps `read_timeout` from there.

stop-criterion: `TextareaContent` is just layout config (focus, single_line). all anim state in `AnimState`. all sidechannels deleted.

### stage 5 -- testability harvest (~1 day)

- snapshot tests for `build_physics` -- pin physics output across buffer fixtures + viewport sizes. catches layout regressions w/out a terminal.
- snapshot tests for `draw` -- pin framebuffer output given a hand-built `Physics`. catches paint regressions w/out a buffer.
- table tests for `animate` -- scripted `(prev_state, target, t_sequence) -> expected fields`. catches feel regressions w/out real time.
- existing tests stay; they exercise the composed pipeline.

## risks / footnotes

- **stage 1 is the cliff** -- splitting `TextBuffer::render` is the irreversible commit. everything after is mechanical. recc starting w/ a spike: try extracting just `layout()` on a branch and see how messy selection geometry gets. if selection extraction is impossible w/out major buffer surgery, scope grows.
- **layout cost** -- currently `TextBuffer::render` reuses internal cursor walk for layout+paint. splitting doubles allocation pressure unless layout output reuses arena. mitigate: `Physics` allocated in `arena_next` (already framewise).
- **gutter coupling** -- [gutter/src/lib.rs](../crates/gutter/src/lib.rs) has its own animated bits. fold into same model in stage 4 or punt to a follow-up.
- **frame counter** -- `Physics.frame` and "born_frame" tracking needs `Tui.frame_counter: u64`, incremented at top of `render`. small addition; mention only b/c animator depends on it for "first sighting" detection.
- **scrollbar drag** -- mouse drag on scrollbar reads `tc.scroll_offset_y_drag_start`. stays on `TextareaContent` (input state, not anim state). animator only consumes target scroll.
- **textbuffer cache** -- `RcTextBuffer` caches across frames ([tui.rs:217](../crates/edit/src/tui.rs)). still valid -- the buffer itself isn't anim state, only the anim fields _on_ `TextareaContent` are.

## sizing

total: ~7-10 focused days. stage 1 is half of it. stages 2-5 are mechanical once 1 lands.

milestones worth committing on:
- end of stage 0 -- new module skeleton.
- end of stage 1 -- `Physics` built, layout split. (one big commit or split per substep.)
- end of stage 2 -- draw pure.
- end of stage 3 -- nil animator live, all `if no_animations()` deleted.
- end of stage 4 -- sidechannels deleted.
- end of stage 5 -- tests in.

## status -- 2026-05-22

partial-landed on `lczyk-remix` (57 commits past `v0.10.0`). per-stage state:

- **stage 0** -- done. `crate::anim` module w/ `mod.rs`, `draw.rs`, `engine.rs`, `physics.rs`.
- **stage 1** -- done. `TextBuffer::layout(&self, ...) -> Option<TextareaLayout>` extracted; `render()` is `layout + paint pass + lsh + overlays`. `Physics` IR (`TextareaPhysics`, `VisualLine`, `TextareaLayout`, `SelectionGeom`) defined in `anim::physics`. `build_textarea_physics(&tb, ...)` calls `layout()` + populates the IR.
- **stage 2** -- done structurally. all per-row + post-paint blends live in `anim::draw` (11 paint helpers + `textarea_lines` consumer + `textarea_overlays` bundle). `TextBuffer::render` has zero direct fb mutation outside `replace_text` and the `anim::draw` calls.
- **stage 3** -- partial. `animate_nil` identity fn + `animate(state, target, ...)` real wrapper exist in `anim::engine`. **not yet wired**: `Tui::render_textarea_content` still calls `tb.render(...)` (which internally does the right thing) + invokes the per-feature lerps individually. wiring `build_textarea_physics + animate + draw` end-to-end at the Tui level requires private-getter exposure on `TextBuffer` (margin_width, word_wrap_column, ruler, line_highlight_enabled, overtype -- some are already pub) and careful borrow choreography around `render_apply_highlights`'s `&mut tb` need.
- **stage 4** -- done. `TextareaContent` holds zero anim state -- it all lives in `Tui::anim` (`TextareaAnimState` keyed by node id, `TuiAnimState` for the floater map + frame counter). both renderer sidechannels eliminated: `cursor_render_override` is now an explicit `render()` arg; `take_pending_line_move` is now non-draining `peek_pending_line_move` w/ a generation counter.
- **stage 5** -- substantial. 30+ unit tests on pure `anim::engine` + `anim::physics` fns + 9 `TextBuffer::render` smoke tests + 6 `TextBuffer::layout` purity tests + 3-test triad for the `animate` wrapper.

remaining work: wire stage 3 in `Tui::render_textarea_content` to use `build_textarea_physics + animate + draw_via_anim`. mostly mechanical once the borrow choreography is set up. blocking on a focused session that can hold the multi-commit move in a single context window.
