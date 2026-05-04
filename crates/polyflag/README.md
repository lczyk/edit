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

const KNOWN: &[&str] = &["foo", "bar", "baz"];
let mut set: HashSet<&'static str> = HashSet::new();

polyflag::apply("foo,bar", KNOWN, &mut set).unwrap();
polyflag::apply("baz",     KNOWN, &mut set).unwrap();
polyflag::apply("bar,-foo",KNOWN, &mut set).unwrap();

assert!(set.contains("bar") && set.contains("baz"));
assert!(!set.contains("foo"));
```

The caller owns the set, the loop over occurrences, and the error formatting.
`polyflag` only knows how to apply one occurrence's value.

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
- no value lifetime juggling. `&[&'static str]` known list returns
  `&'static str` keys, matching how `clap` / `argh` typically declare
  known tokens (`const KNOWN: &[&str]`).

### Known friction

- **`OsString` input.** Most parsers hand you `OsString`; polyflag wants
  `&str`. Caller does `.to_str().ok_or(...)?`. Fine for ascii-only tokens
  (the design assumption).
- **Static-only known list.** `&[&'static str]` excludes runtime-loaded
  token sets (e.g. plugin names from a config file). A `HashSet<String>`
  variant could be added if needed.
- **No clap adapter.** A separate `polyflag-clap` crate could wrap this
  as a clap `value_parser`. Out of scope for this crate.
