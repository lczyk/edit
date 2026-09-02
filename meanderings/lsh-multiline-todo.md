---
status: open
date: 2026-09-01
description: audit of multi-line constructs across every lsh definition -- the `until /$/ { await input; }` idiom that can never suspend, plus the feature gaps the sweep turned up
---

# lsh multi-line audit

## the idiom

`until /pat/` tests its guard **before** each iteration of the body. so

```rs
until /$/ {
    yield string;
    if /CLOSE/ { yield string; break; }
    await input;
}
```

exits the moment the position reaches end of line, and the `await input`
inside it is unreachable in the only situation it exists for. every
construct written this way is silently truncated at the newline, and the
following lines get re-lexed from the top of the definition with the
wrong state.

the shape that works (`runtime.rs`'s
`a_loop_that_awaits_input_keeps_the_first_column_of_the_next_line` pins
it, and `xml.lsh` / `toml.lsh` / `markdown.lsh` already use it):

```rs
loop {
    if /[^`]+/ {}
    if /`/ { yield string; break; }
    yield string;
    await input;
}
```

36 unreachable awaits across 19 definitions. `await input` whose nearest
enclosing loop is an `until` with a guard that can match the empty
string is always one of these; an `until /"/` or an await placed outside
the `until` but inside an enclosing `loop` is fine.

## fixes

- [x] **shellscript.lsh** -- all four quote branches (`'` `$'` `"` `` ` ``).
      needs a new outer `\\.` branch too, otherwise `echo 'it'\''s'`
      opens an unterminated string that bleeds over the rest of the file.
      one line of `kitchen_sink.sh` changes, in the right direction.
- [x] **yaml.lsh** -- a blank line ends a block scalar. the body loop
      skips leading whitespace then breaks on `off <= indentation`; an
      empty line has no whitespace to skip, so `off` is 0. wants a
      `if /$/ { yield string; continue; }` arm ahead of the dedent test.
- [x] **slice_yaml.lsh** -- no `|` / `>` handling at all, so a chisel
      `mutate:` body is lexed line by line as yaml. porting yaml's loop
      is not enough on its own: it eats the leading whitespace of the
      line that closes the block, and re-entering
      `slice_yaml_in_slices_line()` mid-line does not re-dispatch from
      the top, so that line loses its slice-name colour. wrap the
      per-line body in a `loop` and `continue` after the block ends.
- [x] **utility.lsh** -- `single_quote_string()` / `double_quote_string()`
      use `until /$/` with no `await input` at all. line-bounded is
      correct here: every caller (`toml`, `man`, `lsh`, `properties`)
      forbids a raw newline in a quoted scalar. the actual defect is
      that an unterminated quote is left uncoloured entirely, because
      the only `yield string` sits on the closing-quote path. moving a
      `yield string` past the loop covers the remainder, and nothing
      else moves.
- [x] **languages with genuinely multi-line strings** -- rust, ruby,
      fish, gleam, justfile (all four of `"` `'` `` ` `` `{{ }}`), jq.
      `just` and `jq` were checked against their real CLIs: both fold a
      raw newline into the string rather than rejecting it.
- [x] **everywhere else** -- c, objc, glsl, javascript, python, go,
      dockerfile, makefile, json, awk, sed, hcl. a raw newline inside
      those strings is a syntax error in the language, so line-bounded
      is the right behaviour and only the dead `await input` goes.
      verified against `node` and the go compiler rather than assumed.
      backslash-newline continuation already renders acceptably; adding
      `if /\\$/ { await input; }` is a separate, optional improvement.
- [x] **lsh-bin render** picks the first glob match and never runs the
      content detector, so every `.yaml` renders as `slice_yaml` while
      `detect`, the editor and `golden.rs` all disambiguate properly.
      `.yml` is the only honest channel for testing plain yaml until
      this is fixed.
- [x] **compile-time guard** -- `nullable(&Regex)` over the AST that
      `compiler/regex.rs` already builds, a flag on `Context`, and an
      error in `parse_await` when the nearest enclosing loop is an
      `until` with a nullable guard. fires on exactly the 36 sites and
      nothing else, so it has to land last. the definitions README
      recommends `until /$/` without mentioning that an await inside it
      is dead.

## backlog

feature gaps the same sweep turned up. none are instances of the idiom;
all the idiom sites are fixed and the compiler now rejects new ones.
ranked by how often a real file hits it.

- [x] **toml multi-line arrays** -- `toml.lsh:18` has no bracket-depth
      tracking, so `key = [` followed by one element per line is scanned
      per line. every `Cargo.toml` in this workspace has one.
- [x] **shellscript heredoc delimiters** -- `shellscript.lsh:55` only
      recognised `EOF|END|HEREDOC`; `<<'SH'`, `<<-EOT`, `<<__END__` all
      missed. now `save $1` at the opener and `if $saved` at the close.
      here-strings (`<<<`) need their own branch ahead of it, or the
      loop steps one char and `<< "hello` opens a heredoc named hello.
- [x] **dockerfile `\` continuation** -- the body sits in a `loop`; a
      trailing backslash raises a flag, and once the line ends the loop
      awaits and goes round without re-running the instruction dispatch,
      so a leading word on a continuation line is never an instruction.
      `KEY=value` names moved into the value loop so every pair under
      `ENV` / `LABEL` / `ARG` is a variable, not just the first.
- [x] **backslash-newline continuation in c, objc, glsl, javascript,
      python** -- `if /\\$/ { await input; } else { break; }` in each
      string loop. a bare unterminated quote still stops at the line end.
      found on the way: `name = value;` used to rebind the name to a fresh
      register, so a flag set in a loop was invisible after it. fixed in
      the frontend; assignment writes the declared register now.
- [x] **ruby heredocs** -- `ruby.lsh` had none at all. `<<~`, `<<-`,
      plain, and quoted forms; the rest of the opener line stays code and
      a `heredoc` flag runs the body loop once the line ends. the
      delimiter must start with an uppercase letter or `_`, which keeps
      the shovel operator (`a <<b`) out.
- [ ] **markdown code spans** -- `markdown.lsh:277-282` are `until /$/`
      with no await, so a `` `code` `` span wrapped across a line break in
      prose loses its colour at the break.
- [x] **yaml quoted scalars** -- `'...'` / `"..."` get their own loops
      in both `yaml.lsh` and `slice_yaml_value()`: a `#` inside is text,
      `''` is an escaped quote, and the scalar may span lines.
- [x] **make `define` / `endef`** -- the body is verbatim text up to a
      line starting with `endef`; the name after `define` is a variable.
      `override define` and `export define` too.
- [x] **lsh block comments** -- `/* */` implemented, so the attributes
      finally tell the truth.
- [x] **sed `a\` `i\` `c\` text blocks** -- each text line but the last
      ends in a backslash; the GNU one-line form (`i text`) is covered
      too.

### shared root cause: no delimiter capture -- landed

the DSL could capture a group into a register but only compare registers
against `off` and literals, so nothing could remember `SQL` at `<<~SQL`
and test for it at the close. `save $N;` now copies the captured bytes
into a small buffer on the runtime (part of the snapshot the editor keeps
per line), and `if $saved` tests for them as a prefix at the current
position. two instructions, one IR node, one condition; the optimizer's
dead-store pass and the backend's liveness both had to learn that
`SaveSpan` reads its registers.

## checked, nothing wrong

`csv_quoted()` documents that multi-line fields are deliberately
unsupported -- rfc4180 allows them, so this is a limitation worth
revisiting, not a defect. `xml.lsh`, toml's block strings,
`powershell.lsh` and python's triple-quoted strings all use the loop
idiom correctly and were confirmed by render to span lines without
leaking state.
