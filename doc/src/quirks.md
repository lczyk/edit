# Quirks

`--quirks=LIST` is `edit`'s toggle for behaviour switches. Each quirk has a positive canonical name and a default polarity. Two flavours:

- on-by-default guard rails / nice defaults (`safe-filenames`, `unicode`, `color`, `animations`). Disable with `--quirks=-NAME`.
- off-by-default opt-ins (`create`). Enable with `--quirks=NAME`.

## Syntax

`--quirks=` takes a comma-separated list of known names. A leading `-` on a name disables (or undoes a default). The flag can be passed at most once on the command line; multiple `--quirks=` occurrences error out (the env-var path below is for layering).

```sh
edit --quirks=-unicode,-color file.txt
edit --quirks=-color --quirks=color file.txt # error: --quirks may only be passed once
```

Unknown names error and abort startup. The error prints the full known list.

## `EDIT_QUIRKS`

`EDIT_QUIRKS` is the env-var equivalent of `--quirks=`. Same syntax (comma list, `-name` removal). It's applied *before* the cli flag, so `--quirks=NAME` on the command line can re-enable an entry the env disabled (and vice versa).

```sh
EDIT_QUIRKS=-unicode,-color edit file.txt           # disable both
EDIT_QUIRKS=-unicode edit --quirks=unicode file.txt # net: unicode back on (cli re-enables)
EDIT_QUIRKS=-unicode edit --quirks=-color file.txt  # net: -unicode + -color (additive)
```

Unset, empty, or non-utf-8 `EDIT_QUIRKS` is a no-op. Unknown tokens in the env var error and abort startup, same as the cli.

Use it for "always-on for this shell session" overrides without touching every invocation -- e.g. `export EDIT_QUIRKS=-color` for a screen-scraping pipeline.

## Known quirks

### `safe-filenames` (on by default)

By default `edit` refuses filenames that don't pass [`is_safe_filename`]. The rule:

- non-empty, not `.` or `..`.
- at most one leading dot (`.gitignore` ok, `..tilde` not).
- does not start with `-` (would collide with cli flags downstream).
- contains at least one ASCII letter (`123` is weird).
- ASCII only (no emoji / accented chars / CJK).
- stem is `[A-Za-z0-9_+-]`; extension parts are alphanumeric only. So `foo.tar.gz` is fine, `foo.b-c~d` is not.

Catches two cases: accidental creation of a surprising new name, and rare-but-real existing files left behind by buggy tools. Pass `--quirks=-safe-filenames` to skip the gate.

### `create` (off by default)

By default `edit` refuses to open a path that does not exist, and refuses to save into a missing file. Pass `--quirks=create` to opt into create-on-open / create-on-save. Even with the quirk on, `edit` never creates parent directories -- a missing parent always errors. Aliases: `allow-create`, `allowcreate`.

### `unicode` (on by default)

Unicode glyphs in the UI: box-drawing and other non-ASCII chars. Pass `--quirks=-unicode` for ASCII-only rendering -- for terminals with broken unicode width tables or fonts missing the relevant glyphs.

### `color` (on by default)

SGR colour escapes. Pass `--quirks=-color` to suppress all colour while keeping other text attributes (bold, underline, reverse). Focus is then conveyed via gutter glyphs and bracket-wrapping rather than colour. For monochrome terminals, screen scrapers, or recordings that need to survive a colour-stripping pipe. Alias: `colour`.

### `animations` (on by default)

Cursor / scroll / floater motion. Pass `--quirks=-animations` to disable visible interpolation; logic stays instant.

[`is_safe_filename`]: https://github.com/lczyk/edit/blob/main/crates/edit/src/bin/edit/main.rs
