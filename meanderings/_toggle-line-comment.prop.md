---
status: implemented
date: 2026-04-30
description: vscode-parity Cmd+/ / Ctrl+/ to toggle line comments on selected lines
---

# toggle line comment -- plan

vscode-parity `editor.action.commentLine`. chord: `Cmd+/` (macos), `Ctrl+/` (linux). toggles line comments on the current line, or all lines touched by the selection.

block comment (`Shift+Alt+A`) is out of scope for now.

## semantics

mirrors vscode `LineCommentCommand`:

- pick the line-comment token from the active language. if `None`, noop. (TODO: once we have a user-facing warning/toast system, surface a hint instead of silent noop.)
- determine line range: selection's `[min(y), max(y)]`, or `[cursor.y, cursor.y]` if no selection.
- ignoring blank/whitespace-only lines, decide direction:
    - all non-blank lines start (after leading ws) with the token -> **uncomment**
    - else -> **comment**
- if every line in range is blank, noop.
- comment: insert `token + " "` at column = `min(indent_col)` over non-blank lines. blank lines untouched.
- uncomment: at each non-blank line, delete the token; if the next char is a single space, delete it too. blank lines untouched.
- preserve cursor and selection. shift `selection.beg.x` / `selection.end.x` by the per-line delta when those anchors sit on a modified line, same pattern as `TextBuffer::indent_change`.
- one undo group via `edit_begin_grouping` / `edit_end_grouping`.
- `read_only` buffers: noop (mirror `indent_change`'s guard).

trailing-space rule matches vscode: insert always emits `token + " "`; uncomment strips the token plus one optional following space.

## comment-token source

lsh definitions are the source of truth. small hardcoded fallback in the editor for extensions lsh doesn't cover. no language and no fallback match -> noop.

### lsh changes

extend `Language` with `pub line_comment: Option<&'static str>` and parse a `#[line_comment = "..."]` function attribute.

files:

- `crates/lsh/src/runtime.rs` -- add `line_comment` field to `Language`.
- `crates/lsh/src/compiler/frontend.rs` -- accept `"line_comment"` in `parse_attributes`.
- `crates/lsh/src/compiler/mod.rs` -- add `line_comment: Option<&'a str>` to `FunctionAttributes`, carry into the compiled struct.
- `crates/lsh/src/compiler/backend.rs` + `generator.rs` -- thread the field through codegen so emitted `Language { ... }` literals include it.

### `.lsh` definitions

add `#[line_comment = "..."]` to each definition that has a line-comment syntax:

- `//` -- `rust.lsh`, `go.lsh`, `javascript.lsh`, `lsh.lsh`
- `#` -- `python.lsh`, `shellscript.lsh`, `makefile.lsh`, `yaml.lsh`, `toml.lsh`, `properties.lsh`, `ignore.lsh`, `git_commit.lsh`, `git_rebase.lsh`, `powershell.lsh`

omit the attribute (no line-comment syntax) for `xml.lsh`, `markdown.lsh`, `diff.lsh`, `json.lsh`.

### editor fallback

`fallback_line_comment(path: &Path) -> Option<&'static str>`. small extension match for files lsh doesn't cover (e.g. `.c`, `.h`, `.cpp` -> `//`; `.sh`, `.bash` -> `#`). keep tight -- lsh stays canonical.

## editor wiring

### action + keybindings

- `crates/edit/src/bin/edit/keybindings.rs`
    - add `Action::ToggleLineComment` to the enum.
    - add `(Action::ToggleLineComment, "toggle_line_comment")` to `ACTION_KEYS`.
    - bump `ACTION_COUNT` from 24 to 25.
- `crates/edit/src/bin/edit/keybindings.macos.toml` -- add `toggle_line_comment = "Cmd+/"`.
- `crates/edit/src/bin/edit/keybindings.linux.toml` -- add `toggle_line_comment = "Ctrl+/"`.

### shortcut handler

`crates/edit/src/bin/edit/main.rs`, in `handle_global_shortcuts`:

```rust
} else if ctx.consume_shortcut(chord(Action::ToggleLineComment)) {
    let tok = state.document.language()
        .and_then(|l| l.line_comment)
        .or_else(|| fallback_line_comment(&state.document.path));
    if let Some(tok) = tok {
        state.document.buffer.borrow_mut().toggle_line_comment(tok);
    }
    // TODO: when we have a user-facing warning/toast system, surface a
    // "no comment syntax for this file" hint here instead of silent noop.
}
```

## buffer api

`crates/edit/src/buffer/mod.rs` -- new method:

```rust
pub fn toggle_line_comment(&mut self, token: &str)
```

structure mirrors `indent_change`:

1. early return if `self.read_only`.
2. snapshot `cursor`, `selection`. derive `[y0, y1]` from selection or cursor line.
3. first pass: scan lines in `[y0, y1]`. for each non-blank line record `(indent_col, indent_offset, starts_with_token: bool)`. compute `all_commented` over non-blank lines and `min_indent_col`. if zero non-blank lines, return.
4. `edit_begin_grouping()`.
5. second pass: for each non-blank line:
    - **uncomment**: move cursor to first non-ws position. delete `token.len()` graphemes. if the next char is a single `' '`, delete one more. `delta = -(token.len() as CoordType + space_was_there as CoordType)`.
    - **comment**: move cursor to `Point { x: min_indent_col, y }`. `write_canon((token.to_owned() + " ").as_bytes())`. `delta = token.len() as CoordType + 1`.
    - if `y == selection.beg.y` -> `selection.beg.x += delta` (clamped to >= indent col, like indent_change).
    - if `y == selection.end.y` -> `selection.end.x += delta`.
6. `edit_end_grouping()`.
7. restore selection (with shifted xs); restore cursor at the shifted equivalent of the original logical pos.

reuses existing primitives: `cursor_move_to_logical`, `delete(CursorMovement::Grapheme, n)`, `write_canon`, `selection_range_internal` patterns from `indent_change` / `move_selected_lines`.

## tests

buffer-level tests next to existing `indent_change` / `move_selected_lines` tests:

- single line, no selection: `Cmd+/` -> `// foo`. round-trip back to `foo`.
- multi-line selection, mixed indent: token aligns at `min(indent)`; deeper-indented lines keep their extra ws untouched.
- selection where every non-blank line already commented (with a space): uncomment removes `// ` cleanly.
- selection where lines are commented without a trailing space (`//foo`): uncomment removes only `//`.
- mixed commented/uncommented lines: comments all (because not _all_ lines are commented).
- blank/whitespace-only lines inside the selection: untouched in either direction.
- selection-only (no commentable line in range, e.g. one blank line): noop.
- read-only buffer: noop.
- selection anchors land on first/last lines: their `x`s shift by the per-line delta.

end-to-end: hit the chord on a `.rs` document via the existing input-driving harness (if there's one in `tests/`); confirm `// ` insertion and undo collapses to a single step.

## extension: block comments

`Language` also carries `block_comment: Option<(&'static str, &'static str)>` (lsh attributes `#[block_comment_open = "..."]` / `#[block_comment_close = "..."]`). two methods on `TextBuffer`:

- `toggle_per_line_block_comment(open, close)` -- per-line wrapping (`<!-- foo -->`). used as the `Cmd+/` fallback for languages with no `line_comment` (markdown, html/xml).
- `toggle_block_comment(open, close)` -- single-pair wrap of selection or current line. menu only (`Edit > Toggle Block Comment`), no keyboard shortcut. for languages without block syntax it falls back to `toggle_line_comment`, matching vscode `editor.action.blockComment`.

`.lsh` definitions: `/*` `*/` for rust/go/javascript/lsh. `<!--` `-->` for markdown/xml.

## open items

- the warning hookup mentioned in the TODO depends on a status/toast system that doesn't exist yet.

## post-impl: vscode-parity gap closure

after initial impl, audited against vscode `lineCommentCommand.ts`. closed gaps:

- **visible-column min indent (vscode `_normalizeInsertionPoint`)**: original impl tracked `min_indent_chars`. with mixed tab+space leading whitespace this misaligns the inserted token. switched to tracking `min_indent_cols` (visible col, tab-aware via `measure_indent_internal`'s second return). after pass 1, floor to the indent grid: `insert_cols = min_cols / tab_size * tab_size`. pass 2 computes per-line char offset for `insert_cols` via `measure_indent_internal(line_start, insert_cols)` -- which stops before any tab that would straddle the boundary, matching vscode's back-off branch. behaviour change vs prior impl: in pure-space files where min indent isn't on a tab boundary, insertion now floors leftward (e.g. `[4,2,6]` spaces with `tab=4` now inserts at col 0, not col 2).

remaining gaps tracked separately (block fallback shape, add/remove actions, block-remove in line fallback, token coverage, config knobs, multi-cursor).
