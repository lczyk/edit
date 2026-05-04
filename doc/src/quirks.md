# Quirks

`--quirks=LIST` is `edit`'s opt-in toggle for non-default behaviour. Behaviour gated behind a quirk is one of:

- a guard rail that's right by default but wrong for some workflow (`weird-filenames`, `allow-create`).
- a rendering fallback for hostile environments (`ascii`, `nocolor`).

The default is "safer / prettier"; the quirk flag opts into "looser / plainer".

## Syntax

`--quirks=` takes a comma-separated list of known names. `-name` removes a name. The flag can be passed at most once on the command line; multiple `--quirks=` occurrences error out (the env-var path below is for layering).

```sh
edit --quirks=ascii,nocolor file.txt
edit --quirks=ascii,nocolor --quirks=-nocolor file.txt # error: --quirks may only be passed once
```

Unknown names error and abort startup. The error prints the full known list.

## `EDIT_QUIRKS`

`EDIT_QUIRKS` is the env-var equivalent of `--quirks=`. Same syntax (comma list, `-name` removal). It's applied *before* the cli flag, so `--quirks=-name` on the command line can negate an entry the env contributed.

```sh
EDIT_QUIRKS=ascii,nocolor edit file.txt              # both quirks on
EDIT_QUIRKS=ascii edit --quirks=-ascii file.txt      # net: no quirks (cli negates env)
EDIT_QUIRKS=ascii edit --quirks=nocolor file.txt     # net: ascii + nocolor (additive)
```

Unset, empty, or non-utf-8 `EDIT_QUIRKS` is a no-op. Unknown tokens in the env var error and abort startup, same as the cli.

Use it for "always-on for this shell session" defaults without touching every invocation -- e.g. `export EDIT_QUIRKS=nocolor` for a screen-scraping pipeline.

## Known quirks

### `weird-filenames`

By default `edit` refuses filenames that don't pass [`is_safe_filename`]. The rule:

- non-empty, not `.` or `..`.
- at most one leading dot (`.gitignore` ok, `..tilde` not).
- does not start with `-` (would collide with cli flags downstream).
- contains at least one ASCII letter (`123` is weird).
- ASCII only (no emoji / accented chars / CJK).
- stem is `[A-Za-z0-9_+-]`; extension parts are alphanumeric only. So `foo.tar.gz` is fine, `foo.b-c~d` is not.

Catches two cases: accidental creation of a surprising new name, and rare-but-real existing files left behind by buggy tools. Pass `--quirks=weird-filenames` to skip the gate.

### `allow-create`

By default `edit` refuses to open a path that does not exist, and refuses to save into a missing file. Pass `--quirks=allow-create` to opt into create-on-open / create-on-save. Even with the quirk on, `edit` never creates parent directories -- a missing parent always errors.

### `ascii`

Forces ASCII-only rendering: no box-drawing, no other unicode glyphs. For terminals with broken unicode width tables or fonts missing the relevant glyphs.

### `nocolor`

Suppresses all SGR colour escapes. Other text attributes (bold, underline, reverse) still emit. Focus is conveyed via gutter glyphs and bracket-wrapping rather than colour. For monochrome terminals, screen scrapers, or recordings that need to survive a colour-stripping pipe.

[`is_safe_filename`]: https://github.com/lczyk/edit/blob/main/crates/edit/src/bin/edit/main.rs
