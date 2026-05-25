# crates

This directory contains the crates that make up Edit and its supporting tooling.

* `edit`: Main editor binary and library. Includes `edit::eat` (the
  busybox-style `bat`-flavoured cat persona) reachable via the
  `eat -> edit` symlink that `make install` creates, or `edit --eat`.
* `lsh`: Syntax-highlighting compiler and runtime.
* `lsh-bin`: A small CLI for experimenting with and debugging LSH output.
* `lsh-defs`: Bundled lsh language definitions (codegen) plus language
  detection helpers and the shared ansi-16 colourmap.
* `gutter`: Per-line gutter mark computation + render (git-diff overlays).
* `tty`: Platform glue -- stdin reader, write_stdout, raw-mode switch,
  window-size injection. Re-exported via `edit::sys`.
* `stdext`: Shared utility code used across the workspace (arena allocator,
  collections, SIMD helpers, glob, sys shims).
* `unicode-gen`: Code generation utilities for Unicode LUTs.
