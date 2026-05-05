# polyflag

Repeatable comma-separated set-style cli flags with `-` prefix removal.

Given a fixed list of known token names, parse one occurrence of a flag whose
value is a comma-separated list of those names, accumulating into a
`HashSet<&'static str>`. A `-` prefix on a token removes it from the set
instead of adding. Unknown tokens error.

Designed for flags like:

```
--quirks=foo,bar  --quirks=-foo  --quirks=baz
```

where the final state is `{bar, baz}`.

## Usage

```rust
use std::collections::HashSet;
use polyflag::{KnownToken, apply, token};

const KNOWN: &[KnownToken] = &[
    token!("foo"),
    token!("bar"),
    token!("baz"),
];
let mut set: HashSet<&'static str> = HashSet::new();

apply("foo,bar", KNOWN, &mut set).unwrap();
apply("baz",     KNOWN, &mut set).unwrap();
apply("bar,-foo",KNOWN, &mut set).unwrap();

assert!(set.contains("bar") && set.contains("baz"));
assert!(!set.contains("foo"));
```

The caller owns the set, the loop over occurrences, and the error formatting.
`polyflag` only knows how to apply one occurrence's value.

### Aliases

Each `KnownToken` has one canonical spelling plus zero or more aliases.
Aliases let multiple input spellings resolve to the same canonical entry --
the canonical is always what lives in the set, regardless of which spelling
the user typed.

```rust
use polyflag::{KnownToken, apply, token};

const KNOWN: &[KnownToken] = &[
    token!("ascii"),
    token!("nocolor"; "no-color"),
    token!("noanimations";
        "no-animations",
        deprecated "no_animations",
        hidden "noanim",
    ),
];

let mut set = std::collections::HashSet::new();
apply("no-color,no_animations", KNOWN, &mut set).unwrap();
// canonicals only, regardless of spelling used:
assert!(set.contains("nocolor"));
assert!(set.contains("noanimations"));
```

Each alias has a status:

- **`Alternative`** -- intentional alternate spelling, equally valid as the
  canonical. (Bare literal in the `token!` macro.)
- **`Deprecated`** -- still resolves but the caller may want to warn or
  prompt migration. Use [`apply_with_callback`](#deprecation-warnings) to
  surface a warning. (`deprecated <literal>` in the macro.)
- **`Hidden`** -- resolves silently. Omitted from `--help` listings and from
  any public enumeration of accepted spellings; for undocumented compat with
  an old typo or removed convention. (`hidden <literal>` in the macro.)

Add and remove are interchangeable across canonical and alias: e.g.
`--quirks=nocolor` then `--quirks=-no-color` adds and then removes the
same canonical entry; final state has `nocolor` absent.

### Deprecation warnings

`apply_with_callback` invokes the callback once per input token that
resolves through a `Deprecated` alias:

```rust
polyflag::apply_with_callback(input, KNOWN, &mut set, |spelling, canonical| {
    eprintln!("warning: --quirks={spelling} is deprecated, use {canonical}");
})?;
```

`apply` is the no-callback convenience that calls the callback variant with
a no-op. The same split exists for env vars: `apply_env_for_flag` and
`apply_env_for_flag_with_callback`.

### Resolving without mutating a set

`canonicalize(input, known)` returns a `Resolved { canonical, kind }` for any
canonical or alias spelling, where `kind` is `Canonical | Alternative |
Deprecated | Hidden`. Useful when the caller wants to classify a match (e.g.
to decide between an inline warning and a structured deprecation report)
without re-walking the table.

### Validating the known-token table

`check_known(known)` panics on a duplicate or empty spelling anywhere in the
table. The body is `#[cfg(debug_assertions)]`-gated, so release builds
compile to a no-op -- callers may invoke it unconditionally at startup, and
typos in the static table get caught by `cargo test` / dev builds.

### Env-var defaults

`apply_env_for_flag(prefix, flag, known, set)` reads an env var derived from
the cli surface and applies it as if it were a leading occurrence of the flag:

| `prefix` | `flag`          | env var resolved   |
|----------|-----------------|--------------------|
| `"app"`  | `"quirks"`      | `APP_QUIRKS`       |
| `"app"`  | `"allow-create"`| `APP_ALLOW_CREATE` |

Mapping rule: `{PREFIX}_{FLAG}`, prefix uppercased, kebab-to-underscore on the
flag, uppercased. Env value semantics match `apply` (comma-list, `-name`
removal, unknown-token error). Unset / empty / non-utf-8 values are no-ops.

```rust
// Example: cli surface is `--quirks=...`, so the env surface is APP_QUIRKS.
polyflag::apply_env_for_flag("app", "quirks", KNOWN, &mut set)?;
// Then apply any cli occurrences -- they layer on top, so a cli `-name` can
// negate an entry the env contributed.
polyflag::apply(cli_value, KNOWN, &mut set)?;
```

The cli and env names stay in lock-step by construction -- no second string
to keep in sync. `env_var_name(prefix, flag)` is exposed if the caller wants
to surface the resolved name in error messages.

## Semantics

- input split on `,`; tokens trimmed; empty tokens skipped.
- token `name` -> `set.insert(name)`.
- token `-name` -> `set.remove(name)`.
- unknown name (after stripping any `-`) -> `Err(UnknownToken)`. The set is
  left in its partial state -- clone first if you need atomic application.
- order matters: across occurrences, and within an occurrence (left-to-right).

## Compatibility with cli parsers

polyflag operates on the _value_ of one flag occurrence (`"foo,bar,-baz"`),
not on the flag itself. It composes with any parser that hands you the raw
value(s) of a repeatable flag.

| Parser | Repeatable -> `Vec<String>`? | Value starting with `-` ok? | Integration |
|--------|------------------------------|------------------------------|-------------|
| `clap` (derive/builder) | yes via `ArgAction::Append` + `value_delimiter(',')` | only with `allow_hyphen_values(true)`, _or_ via `--flag=value` form | collect to `Vec<String>` per occurrence, loop + `polyflag::apply` |
| `argh` | yes (`#[argh(option)]` repeated) | via `--flag=value` form | same |
| `lexopt` | manual loop -- you get each value | yes (you control parsing) | call `polyflag::apply` inline |
| `pico-args` | similar to `lexopt` | yes | same |
| `gumdrop` | yes via `Vec<String>` | via `=` form | post-process |
| manual `env::args_os` | trivial | yes | direct |

The `-foo` tokens are the only friction. Most parsers accept them inside
`--flag=...` (the `=` form pins the value to the flag). The space-separated
form (`--flag -foo`) needs an opt-in on parsers that defend against stray
hyphen-tokens -- e.g. clap's `allow_hyphen_values(true)`.

### What polyflag deliberately doesn't do

- no flag-name recognition (`--quirks=`). that's the parser's job.
- no error formatting. caller emits the message in their app's voice.
- no value lifetime juggling. `&[KnownToken]` returns `&'static str` keys
  for the canonicals, since each `KnownToken.canonical` is itself
  `&'static str`.

### Known friction

- **`OsString` input.** Most parsers hand you `OsString`; polyflag wants
  `&str`. Caller does `.to_str().ok_or(...)?`. Fine for ascii-only tokens
  (the design assumption).
- **Static-only known list.** `&[KnownToken]` with `&'static str` canonicals
  / aliases excludes runtime-loaded token sets (e.g. plugin names from a
  config file). A `String`-keyed variant could be added if needed.
- **No clap adapter.** A separate `polyflag-clap` crate could wrap this
  as a clap `value_parser`. Out of scope for this crate.
