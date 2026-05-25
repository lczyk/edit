---
status: open
date: 2026-05-06
description: bat-parity less invocation -- version-detect `--no-init` and conditional `-K`
---

# eat: less version dance

eat currently invokes `less` with `-R -F`. that's the simple modern path: drop `-X` (which bat calls `--no-init`) so the alt-screen activates and xterm alternate-scroll forwards mouse-wheel events as arrow keys to less. works on ~all current installs.

bat does more. this proposal captures the gap.

## gap

eat's pager invocation lives at [crates/edit/src/eat/mod.rs](../crates/edit/src/eat/mod.rs) (in the pager-spawn block of `print_highlighted`, plus the duplicate plain-mode block):

```rust
if pager_name == "less" {
    args.extend_from_slice(&["-R", "-F"]);
}
```

bat's pager invocation ([~/ngit/bat/src/output.rs:131](~), reference) does three more things:

1. **`-K` / `--quit-on-intr`** -- on non-busybox less, makes ctrl-c quit cleanly instead of less swallowing it.
2. **`--no-init`** -- still passed for less <530 (or windows less <558) to dodge a long-fixed `--quit-if-one-screen` flash-clear bug. omitted on modern less (so mouse-wheel still works for everyone else).
3. **`-S`** -- on no-wrap mode, chops long lines instead of wrapping. n/a until eat grows a `--wrap` flag.

(2) is the load-bearing one for compatibility -- without it, busybox / pre-2017 less users get a screen flash on short content. (1) is a quality-of-life thing.

## options

### a. ship as-is, accept the gap

the simple `-R -F` invocation covers macOS (less 643), every current major distro (well past 530), and homebrew. busybox and ancient enterprise installs lose mouse-wheel parity with bat and may flash on short content. zero added code.

### b. full bat dance

port bat's [less.rs](../../ngit/bat/src/less.rs) (~30 loc + a few parse tests):

- enum `LessVersion { Less(usize), BusyBox }`
- `retrieve_less_version(path) -> Option<LessVersion>` that runs `less --version`, parses stdout for the `less <N>` prefix, falls back to stderr for busybox.
- gate `-K` on non-busybox, `--no-init` on `Less(<530)` / `Less(<558)` on windows / `None`.

cost beyond loc: one extra `less --version` subprocess per paged invocation. ~5-10ms cold. cache behind a `OnceLock<Option<LessVersion>>` to amortise across multi-file runs.

### c. half-measure: cache `LESS_IS_MORE`-style env var

let the user opt in via `EAT_LESS_OLD=1` or similar -- skip the version probe, force the conservative flag set. cheap to impl (5 loc), zero subprocess cost, but pushes the burden onto the user.

## tradeoffs

- **subprocess cost vs correctness**: option b is a one-time per-process probe; with `OnceLock` it's a single fork. not free but not visible. iff someone reports flash or stuck-pager, option b is the right hammer.
- **`-K` is independent of the version dance**: it only needs the busybox check, which is part of the same parse anyway. shipping just `-K` without `--no-init` gating still requires the probe.
- **non-less pagers**: all three options leave `EAT_PAGER`/`PAGER`-set non-less alone. user said `more`, user gets `more`-flavoured behaviour.

## open questions

- **is the busybox case real for eat's audience?** eat is a developer tool aimed at terminal-comfortable users on dev machines. busybox less is mostly alpine containers / embedded. probably rare.
- **windows**: edit/eat does build on windows. less 558 is the windows-specific cutoff in bat. if eat sees real windows usage via `less.exe` from chocolatey/scoop, option b matters more.
- **could the probe be lazy?** only run `less --version` the first time we actually spawn the pager, and only if we'd plausibly need a flag we'd otherwise skip. avoids the probe for `--paging never` and short-content auto-skip cases.

## not in scope

- replacing less with a builtin pager (bat has one behind a feature flag; complete different proposal).
- mouse support beyond what alt-screen forwarding gets us. real mouse-aware pager interactions belong in a tui-side feature, not a cli-side one.
