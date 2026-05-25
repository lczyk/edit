---
status: landed
date: 2026-05-06
description: busybox-style multicall -- when invoked as `eat`, the edit binary acts as a bat-like syntax-highlighting cat
---

# eat

## update 2026-05-25 -- superseded by [unification-plan.md](unification-plan.md)

eat shipped, then got absorbed back into the edit crate during the
unification work. the body of this proposal still describes the
original "new crate `crates/eat/`" + standalone `bin/eat` design;
that is historical. current architecture:

- code lives at `crates/edit/src/eat/` as the `edit::eat` module
  (was `crates/eat/src/`). intra-crate module, not a separate crate.
- no standalone `eat` binary in the source tree; the only way to get
  `eat` is via the `make install` `eat -> edit` symlink + the argv0
  short-circuit in `bin/edit/main.rs`.
- snapshot tui now drives through `edit::mount::mount`, not the
  bespoke alt-screen driver this proposal designed. follow tui still
  uses its own driver (phase C migration pending).
- crate-vs-module testability tradeoff (the bin/eat justification
  here) was resolved by absorption: integration tests live at
  `crates/edit/tests/` and exercise `edit::eat::*` directly.

read this doc for the why (multicall rationale, naming, the
single-binary install story). read [unification-plan.md](unification-plan.md)
for current state and phases B/C/D.

a busybox-style multicall persona for `edit`. when the binary is invoked under the name `eat` (via symlink), it acts like `bat`: read files, syntax-highlight via lsh, write to stdout, optionally page. the editor's tui never spins up. a separate standalone `eat` binary also exists in the source tree for testability and for environments where symlinks aren't available, but `make install` ships only the symlink.

name pun: _edit_ + _cat_, one syllable, verb-shaped. matches bat/cat register. three letters, no clash with coreutils on linux/macos.

## current state

[crates/lsh-bin/src/main.rs](../crates/lsh-bin/src/main.rs) has a `render` subcommand that does the highlighted-print job today, but it's wrapped in lsh-bin's debug-tool cli (`lsh-bin render --input foo.go defs/`) and bakes a fixed ansi colormap into the bin. it's not user-facing.

[crates/edit/src/bin/edit/main.rs](../crates/edit/src/bin/edit/main.rs) is the editor entry point -- panic hook, 512 MiB scratch arena, `sys::init`, settings/keybindings/colormap loads, raw mode, tui setup. zero rendering-to-stdout code path.

[crates/lsh/src/...](../crates/lsh/src) is the highlighter library: text -> tokens. no color, no theme, no rendering. matches the gitgum-style separation we want -- lsh produces tokens, doesn't care what consumers do with them.

## design

### shape

three artefacts:

1. **new crate `crates/eat/`** -- the cli + render glue. depends on `lsh` (for tokens), `stdext` (arena, glob), `argh` (flag parsing), no `which` (manual `$PATH` walk for `less`). contains the cli surface, the default theme, the printer, the pager, the file/stdin reading.
2. **`crates/edit/src/bin/edit/main.rs`** gains an argv0 short-circuit at the very top of `main()`, before any editor init. if `file_stem(argv[0]) == "eat"` -> `return eat::main()`.
3. **`crates/edit/src/bin/eat/main.rs`** -- a thin standalone bin that is literally `fn main() -> ExitCode { eat::main() }`. exists so `cargo install --bin eat` works and so the api-surface tests have a binary to drive without relying on a symlink at build time. not installed by `make install`.

both entry paths land on `eat::main()` in the eat crate's lib. the api the entry paths expose is the same; we test the api, not parity between the entry paths.

### lsh stays pure

reaffirmed boundary: `crates/lsh/` is text -> tokens. no color, no theme, no rendering. eat is the consumer that maps tokens -> ansi via its theme. lsh-bin's `render` subcommand stays as-is (debug tool, leave its cli surface alone) -- the "no rendering in lsh" rule applies to the _library_, not to the dev/debug bin.

### multicall dispatch

at the very first line of `edit::main()`:

```rust
fn main() -> process::ExitCode {
    let argv0 = env::args_os().next();
    let name = argv0.as_deref()
        .and_then(|p| Path::new(p).file_stem())
        .and_then(|s| s.to_str())
        .unwrap_or("edit");

    if name == "eat" {
        return eat::main();
    }

    // ... existing edit init: panic hook, arena, sys::init, etc.
}
```

`Path::file_stem` strips `.exe` on windows for free. when exec'd via a symlink `eat -> edit`, `argv[0]` carries the symlink name (not the resolved target) on linux and macos -- which is exactly what we need.

eat owns its own panic hook, arena (128 MiB, mirroring lsh-bin), and io setup. nothing of edit's heavy init runs in the eat path.

### v1 cli surface

```
eat [OPTIONS] [FILES...]

  -l, --language <NAME>    override syntax detection (required for stdin if no shebang)
  -p, --plain              disable highlighting, decorations, paging -- act like cat
  -n, --number             show line numbers
  --line-range <RANGE>     N | N: | :M | N:M
  --color <WHEN>           auto (default), always, never
  --paging <WHEN>          auto (default), always, never
  --version                print shared edit/eat version and exit
  -h, --help               help
```

deferred to v2: themes, `--theme`, multi-range `--line-range`, git gutter, `--diff`, `--show-all`, headers/grid `--style=`, file-globbing recursion. no `--list-languages` (not needed -- lsh defs are the source of truth; users can read `crates/lsh/definitions/`).

### language detection

bat-parity from day 1, in this priority:

1. explicit `-l <name>` -- wins. if name doesn't match a known entrypoint, hard error with `eat: unknown language 'foo'`, exit 2.
2. path glob -- existing `#[path = "**/*.go"]` matching against the file path. (n/a for stdin.)
3. shebang sniff -- read first line if it starts with `#!`, extract the interpreter token (basename of first word, or basename of second word if first is `env`), prefix-match against each entrypoint's `#[shebang = "..."]` tokens. first match wins.
4. fallback -- plain (no colors, no warning).

### shebang attribute on lsh defs

new repeatable attribute, mirrors `#[path]`:

```
#[shebang = "bash"]
#[shebang = "sh"]
#[shebang = "zsh"]
pub fn shellscript() { ... }
```

storage path:

- parser ([crates/lsh/src/compiler/frontend.rs](../crates/lsh/src/compiler/frontend.rs)): one new arm, `"shebang" => attributes.shebangs.push(value)`, mirroring `"path"`.
- compiler struct ([crates/lsh/src/compiler/mod.rs](../crates/lsh/src/compiler/mod.rs)): `Entrypoint.shebangs: Vec<String>`, mirroring `paths`.
- generator: emits the field through to the runtime.
- runtime / assembly: exposes `shebangs: &[String]` per entrypoint.

lsh stores the tokens; lsh _does not_ do the matching. eat reads the file's first line (capped at 256 bytes for sanity), extracts the interpreter token, and walks `Assembly.entrypoints` looking for a prefix match. semantics:

- `#[shebang = "python"]` matches `#!/usr/bin/python`, `#!/usr/bin/python3`, `#!/usr/bin/python3.11`, `#!/usr/bin/env python3.11`.
- prefix-match means: `interpreter_token.starts_with(shebang_value)`.
- env-handling: if the first whitespace-separated word's basename is `env`, look at the second word.
- no vim modelines, no emacs `-*- mode: -*-`, no other first-line dialects. shebang only.

shebangs land on at minimum: shellscript (`sh`, `bash`, `zsh`), python (`python`), ruby (`ruby`), powershell (`pwsh`, `powershell`). same pr -- the feature is dead on arrival without them.

### stdin handling (bat-style)

| stdin   | args        | behaviour                                                  |
|---------|-------------|------------------------------------------------------------|
| tty     | none        | print short help, exit 0                                   |
| pipe    | none        | read stdin, language from `-l` or shebang sniff, plain otherwise |
| any     | files only  | read files, ignore stdin even if piped                     |
| any     | files + `-` | concat, with `-` representing stdin at its position        |

the tty-no-args -> help rule (vs hang-on-stdin) is the load-bearing bit -- bat-the-tool learned this; we should too.

### multi-file output

| files | stdout  | header per file?       |
|-------|---------|------------------------|
| 1     | any     | no                     |
| 2+    | tty     | yes -- `--- <path> ---` in `\x1b[90m` (dim) when color is on |
| 2+    | pipe    | no -- act like `cat` so `eat *.go > combined.go` doesn't corrupt |

ascii separator `--- <path> ---` (no unicode box-drawing). a `--style={plain,header,full}` override lands in v2.

### theme (v1)

one hardcoded default ansi-16 colormap in `crates/eat/src/theme.rs`:

```rust
pub const DEFAULT: &[(&str, &str)] = &[
    ("comment",          "\x1b[32m"),  // green
    ("method",           "\x1b[93m"),  // bright yellow
    ("string",           "\x1b[91m"),  // bright red
    // ... mirrors lsh-bin's existing colormap
];
```

ansi-16 only -- respects user terminal palettes, no detection logic, works under light and dark themes. true-color and `--theme` deferred to v2.

### line numbers / gutter

mirror edit's existing gutter rendering -- separator char, color, padding, wrap-aware blanking on continuation rows. when implementing, look at edit's tui gutter code and copy. drift between editor and eat would just need a re-pass later.

target shape: right-aligned line number, padded to width-of-total-lines, separator, content. color on the gutter is dim/grey (likely `\x1b[90m`). multi-file numbering restarts at 1 per file.

padding width:

- file inputs: read fully into memory, count newlines, then iterate -- no extra disk pass.
- stdin (streaming): fixed width 4 (covers up to 9999 lines comfortably).

### paging

- resolution: `EAT_PAGER` -> `PAGER` -> `less` (manual `$PATH` walk -- no `which` dep).
- if resolved program is `less` and lacks `-R -F -X`, append them. other pagers used as-is.
- fallback: if no env var and no `less` on `$PATH`, silently skip paging. don't error.
- `--paging=auto` (default) triggers when stdout is a tty and a pager is available. `-F` makes the pager quit immediately for short output -- no need to count lines ourselves.
- when paging, force `--color=auto` to resolve to `always` -- our stdout-is-tty check returns false through the pipe to the pager, so without this we'd write uncolored output through `less`.
- broken pipe (user quits pager early): catch `EPIPE`, exit 0 silently.

### use as `MANPAGER`

eat can replace `less` as the man-page pager once a `man` lsh definition exists. groff's output carries ansi sequences and `_\b_`-style overstrike for bold/italic; strip both with sed before piping in:

```sh
export MANPAGER='sh -c "sed -u -e \"s/\\x1B\\[[0-9;]*m//g; s/.\\x08//g\" | eat -l man --color always"'
```

`--color always` is required because eat sees a non-tty stdout (the pipe to `man`'s pager wrapper). paging stays on auto -- if `man` already wraps us in `less`, eat's auto detection sees a non-tty and skips paging; if invoked directly, eat pages itself.

### error handling

- file doesn't exist -- error to stderr, continue with remaining files (cat-style), exit 1 if any file failed.
- file is a directory -- error and skip, no recursion.
- `-l <unknown>` -- hard error before any output, exit 2.
- mid-stream io error -- print error to stderr, exit 1, no buffer-and-replay attempt.
- shebang miss -- silent plain output, no warning.

### `--plain`

`-p` / `--plain` disables _everything_ that distinguishes eat from cat: highlighting, decorations (line numbers, headers), paging. equivalent to `cat`. for the "highlighted but no decorations" middle ground, use the individual flags (`--color=never` etc).

### `--version`

shared with edit. one VERSION file at repo root (already wired into `crates/edit/Cargo.toml` via `make sync-version`). `eat --version` and `edit --version` print the same number; they ship together.

### make install

opinionated -- one target, no flags:

```make
.PHONY: install
install: sync-version
	cargo install --path crates/edit --force
	ln -sf edit "$${CARGO_INSTALL_ROOT:-$$HOME/.cargo}/bin/eat"
```

the standalone `bin/eat` exists in the source tree (so `cargo test` builds it for the integration tests) but is not installed.

### tests

three layers, all of them:

- **unit tests** in `crates/eat/src/` for cli parsing, theme map application, color/paging auto-detect logic, shebang token extraction (env-handling, basename), prefix-match.
- **golden snapshot tests** in `crates/eat/tests/golden.rs`, mirroring [crates/lsh/tests/golden.rs](../crates/lsh/tests/golden.rs). reuse lsh fixtures by symlink or path-back so new lsh fixtures auto-flow into eat coverage. snapshots are raw bytes incl. ansi escapes; `UPDATE_GOLDEN=1` to refresh.
- **pty/integration tests** in [tests/pty/](../tests/pty), two cases:
    - `test_eat_symlink.py` -- run `edit` invoked via a `eat` symlink (set up in tmpdir), assert highlighted output.
    - `test_eat_standalone.py` -- run the standalone `eat` bin, assert the same output.
    
    these prove the dispatch matrix -- both entry paths land on the same code -- not feature parity.

### docs

- `README.md` -- new section on the multicall persona.
- `AGENTS.md` -- bullet noting eat exists and is multicall-style.
- mdbook chapter -- defer until shipped.

## tradeoffs

- **multicall-in-edit vs multicall-in-lsh-bin** -- the rendering code lives closer to lsh-bin, but the user-facing story is "edit is a small tool suite that includes a cat-like" rather than "lsh-bin gained a face". keeping the multicall in edit makes the story coherent at the cost of pulling render glue into the edit-adjacent area. landed on edit because the user explicitly wants this.
- **single eat crate vs splitting render-as-lib into lsh** -- the gitgum-style separation says lsh shouldn't render, so render glue (theme, ansi, gutter, paging) lives in eat. cost: lsh-bin can't share the render code if it ever wanted to (it can't currently anyway, since it bakes its own colormap). benefit: clean separation of concerns.
- **ansi-16 vs true-color theme** -- ansi-16 is portable and respects user terminal palettes for free, true-color overrides them. v1 picks ansi-16 for portability. true-color theme variant is a v2 add.
- **paging dependency on `less`** -- almost universal but not literally guaranteed. fallback to no-paging keeps eat usable on stripped-down systems without erroring.
- **`-p` mimics cat vs bat-style "plain decorations only"** -- bat's `--plain` keeps highlighting; ours kills it. simpler semantics, one obvious meaning, costs the "decorations off but colors on" use case (which the individual flags cover anyway).
- **golden snapshots over ansi bytes** -- snapshots include escape codes, painful to read, easy to break under terminal driver changes. mitigated by `UPDATE_GOLDEN=1` and by the fact that lsh's own golden tests already do this -- pattern is established.

## open questions

- **edit's gutter rendering -- where exactly is the format defined?** mirror edit's existing rendering exactly. locate the gutter format at impl time (likely `crates/edit/src/tui/` or `crates/edit/src/bin/edit/draw_editor.rs`), copy the format verbatim. no drift.
- **edit -> argh as a driveby?** edit's `parse_args` is hand-rolled with polyflag for token-list flags. converting to argh+polyflag would homogenise the workspace (lsh-bin and eat would both use argh). flagged as a follow-up pr; bundling with eat balloons the diff and the test surface for a production binary.
- **paged-output color forcing -- what about `--color=never` + `--paging=always`?** user explicitly asked for both, no color, into pager. respect the explicit `never`; the auto -> always rule applies only when `--color=auto`. should fall out naturally but worth a test case.
- **shebang sniff for very short files (one-liner shell scripts)** -- if the file is literally `#!/bin/sh` and nothing else, eat reads one line, sniffs, then has nothing to highlight. not a bug, but worth a fixture.
- **multi-line shebang sniff cap** -- 256 bytes for the first line. pathological input with no `\n` for megabytes would hit the cap, fall through to plain. acceptable.
- **`EAT_PAGER` namespace** -- the var name commits to the tool name. if we ever rebrand, the env var follows. not a real concern but a thing to be aware of.
- **what does `eat -` mean with no other args?** `eat -` with tty stdin and no other args prints short help (same as `eat` with no args -- the tty-no-args -> help rule fires before arg parsing reaches `-`). `eat -` with piped stdin reads stdin. worth explicit tests for both.

## decisions

- **name = `eat`.** edit + cat. _confirmed in grilling._
- **multicall in edit + standalone bin in `crates/edit/src/bin/eat/main.rs`.** both call into the eat crate's `main()`. _confirmed._
- **new crate `crates/eat/`** rather than a module under edit. _confirmed; user asked not to be afraid of larger edits._
- **lsh stays pure -- no color, no rendering.** theme + ansi live in eat. lsh-bin's `render` subcommand is left as-is (debug tool, separate concern). _confirmed._
- **language detection: `-l` -> path glob -> shebang sniff -> plain.** full bat-parity from day 1. _confirmed._
- **shebang as `#[shebang = "<token>"]`** on lsh defs, repeatable, prefix-match-on-interpreter-token-after-env-handling, matching done in eat not lsh. _confirmed._
- **v1 surface = paging + line numbers + plain + color/paging mode flags + version + help.** _confirmed._
- **argv0 dispatch at the very first line of `edit::main()`** -- before panic hook, before arena init, before anything. _confirmed._
- **multi-file output: tty + 2+ files -> headers, otherwise concat.** ascii `--- <path> ---` separator. _confirmed._
- **`make install` is opinionated symlink-only.** standalone bin exists for tests but not installed. _confirmed._
- **stdin handling: bat-style, with tty-no-args -> help.** tty stdin + no args prints short help and exits 0 (avoids hanging on stdin). pipe stdin + no args reads stdin. _confirmed._
- **theme: one hardcoded ansi-16 default in v1.** _confirmed._
- **tests: unit + golden + two pty.** _confirmed._
- **paging: `EAT_PAGER` -> `PAGER` -> `less`, inject `-R -F -X` on `less`, manual `$PATH` walk, fallback skip-paging on missing.** force color when paging. catch EPIPE. _confirmed._
- **line numbers / gutter: mirror edit's rendering** -- separator, color, wrap-aware. impl-time investigation. _confirmed._
- **error handling: cat-style continue, hard-fail on `-l <unknown>`, silent plain on shebang miss.** _confirmed._
- **`-p` / `--plain` mimics cat** -- kills highlighting, decorations, paging. _confirmed._
- **shared VERSION** with edit. _confirmed._
- **arg parser: argh, plus polyflag if a token-list flag emerges.** edit -> argh conversion is a separate pr, not bundled. _confirmed._
- **`which` crate not added** -- manual `$PATH` walk. _confirmed._
- **docs: README + AGENTS.md, mdbook deferred.** _confirmed._
- **no windows support.** edit's install story is unix-shaped; no `ln -sf` on windows. standalone `bin/eat` covers cargo-install users on any platform but that's incidental, not a design target. _confirmed 2026-05-06._
- **no `--list-languages`.** lsh defs are the source of truth; users who need the list can read `crates/lsh/definitions/`. no reason to ship a runtime catalog. _confirmed 2026-05-06._
- **`eat` no-args: tty -> help, pipe -> read stdin.** matches bat's learned behaviour. `eat -` with tty stdin also prints help (tty check fires before arg parsing). _confirmed 2026-05-06._
- **gutter rendering: copy edit's format verbatim.** locate the gutter format at impl time, mirror exactly. no drift, no reinterpretation. _confirmed 2026-05-06._

## migration plan

1. **lsh: shebang attribute plumbing.** add `Entrypoint.shebangs: Vec<String>` through the parser, compiler struct, generator, runtime/assembly. zero-functionality change on its own (lsh exposes the field, nothing reads it yet).
2. **lsh defs: shebang values.** add `#[shebang = ...]` lines to shellscript, python, ruby, powershell.
3. **new crate `crates/eat/`.** lib only at first: cli types, theme, printer, pager, file/stdin reader, language-detection (path-glob + shebang-sniff via the new lsh field). depends on `lsh`, `stdext`, `argh`. no `eat::main()` body yet.
4. **eat lib: api surface.** `pub fn main() -> ExitCode`. argv parsing via argh, dispatch to `run_path` / `run_reader`.
5. **edit: argv0 dispatch.** add the short-circuit at the top of `edit::main()`. depend on `eat` from the edit crate's `Cargo.toml`.
6. **standalone bin: `crates/edit/src/bin/eat/main.rs`.** thin shim. `[[bin]] name = "eat"` in `Cargo.toml`.
7. **tests.** unit (eat lib), golden (with shared lsh fixtures), pty (`test_eat_symlink.py`, `test_eat_standalone.py`).
8. **make install.** add the `ln -sf` line.
9. **docs.** README section, AGENTS.md bullet.
10. **verify.** `make verify` (fmt-check + clippy + test). install locally, test the symlink dispatch end-to-end.

post-eat (separate prs):

- edit's `parse_args` -> argh.
- v2 features: `--theme`, true-color theme variant, multi-range `--line-range`, `--style=` decoration toggle, git gutter in eat, `--list-themes`.
