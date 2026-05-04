---
status: open
date: 2026-05-05
description: polyflag flag aliases -- multiple input spellings resolve to one canonical token
---

# polyflag aliases

let `--quirks=nocolor` and `--quirks=no-color` (and any other configured spelling) refer to the same canonical entry in the set. removes the friction of guessing which spelling the author picked.

## current state

[crates/polyflag/src/lib.rs](../crates/polyflag/src/lib.rs):

```rust
pub fn apply(
    input: &str,
    known: &[&'static str],
    set: &mut HashSet<&'static str>,
) -> Result<(), UnknownToken> { ... }
```

`known` is a flat list of canonical token names. each token has exactly one accepted spelling. `nocolor` works, `no-color` errors with `unknown token "no-color"`. only one caller in this repo: `KNOWN_QUIRKS` in [crates/edit/src/bin/edit/main.rs](../crates/edit/src/bin/edit/main.rs).

## design

introduce a richer known-token shape. each token has exactly one preferred (canonical) name plus zero or more aliases. each alias has a status: `Alternative` (intentional, equally valid spelling) or `Deprecated` (accepted for backward-compat, callers may want to surface a warning). resolution maps any input spelling to the canonical entry; the set always stores canonical strings.

```rust
pub struct KnownToken {
    pub canonical: &'static str,
    pub aliases: &'static [Alias],
}

pub struct Alias {
    pub spelling: &'static str,
    pub status: AliasStatus,
}

pub enum AliasStatus {
    /// Intentional alternate spelling, equally valid as the canonical.
    Alternative,
    /// Still resolves but callers should warn or migrate.
    Deprecated,
    /// Resolves silently. Omitted from `--help` listings and from any
    /// public enumeration of accepted spellings. For undocumented
    /// compat with an old typo or removed convention.
    Hidden,
}
```

`apply` and `apply_env_for_flag` take `&[KnownToken]` instead of `&[&'static str]`. for each input token, walk the known list and match on `canonical` or any alias's spelling; insert `canonical` into the set on success. `-name` removal works the same way -- strip `-`, resolve alias, remove canonical.

helper ctors for the common cases:

```rust
impl KnownToken {
    pub const fn new(canonical: &'static str) -> Self {
        Self { canonical, aliases: &[] }
    }
}

impl Alias {
    pub const fn alt(spelling: &'static str) -> Self {
        Self { spelling, status: AliasStatus::Alternative }
    }
    pub const fn deprecated(spelling: &'static str) -> Self {
        Self { spelling, status: AliasStatus::Deprecated }
    }
}
```

### deprecation reporting

callers need to know when an input hit a deprecated alias so they can surface a warning. options:

- **callback param** -- `apply(input, known, set, on_deprecated: impl FnMut(&str, &'static str))`. caller passes `|_,_| {}` if they don't care. zero alloc, composes with any logging surface.
- **return type** -- `apply` returns `Result<DeprecationReport, UnknownToken>` where the report is a small `Vec<(String, &'static str)>` (input token, canonical). simpler signature; small alloc only when deprecations happen.
- **out-param** -- `&mut Vec<...>` last arg. ugly; rejected.

callback is cheaper and composes better with the existing `&mut HashSet` style. keep `apply` as the no-callback convenience that calls the callback variant with a no-op.

ship a `token!` macro alongside the struct so call sites read as a flat token table rather than a wall of struct literals:

```rust
const KNOWN_QUIRKS: &[KnownToken] = &[
    polyflag::token!("ascii"),
    polyflag::token!("nocolor"; "no-color"),
    polyflag::token!("noanimations"; "no-animations", deprecated "no_animations"),
    polyflag::token!("weird-filenames"),
    polyflag::token!("allow-create"; "allowcreate"),
];
```

each entry after `;` is either a bare string literal (= `Alternative`) or `deprecated <literal>` (= `Deprecated`). shape:

```rust
#[macro_export]
macro_rules! token {
    ($canon:expr) => {
        $crate::KnownToken { canonical: $canon, aliases: &[] }
    };
    ($canon:expr; $($t:tt)+) => {
        $crate::KnownToken { canonical: $canon, aliases: &$crate::__aliases!($($t)+) }
    };
}

#[macro_export]
macro_rules! __aliases {
    ($($spelling:literal),+ $(,)?) => { [ $($crate::Alias::alt($spelling)),+ ] };
    // mixed alt + deprecated handled via repeated push, see impl notes.
}
```

mixing `alt` and `deprecated` entries in one token call uses a recursive accumulator inside the macro. the call-site form is the monolithic one:

```rust
polyflag::token!("noanimations"; "no-animations", deprecated "no_animations")
```

bare literal -> `Alias::alt`; `deprecated <literal>` -> `Alias::deprecated`. terse and reads as a single conceptual entry.

## semantics

- input `name` -- exact match against canonical or any alias -> insert canonical.
- input `-name` -- same lookup; remove canonical.
- unknown -- error as today. the error text reports the input token verbatim (not canonical) so the user can find what they typed.
- aliases are input-only. iterating the set or asking `set.contains(alias)` returns false -- only canonicals live in the set.
- alias and canonical are interchangeable on both add and remove. e.g. `--quirks=nocolor` then `--quirks=-no-color` adds and then removes the same canonical entry; final state has `nocolor` absent.

## conflicts and validation

aliases that collide with another canonical or another token's alias produce silent ambiguity. options:

- **debug_assert in a `check_known(known)` helper** -- caller invokes once at startup; release builds skip. collision logs the conflict and aborts. typos slip through if the caller forgets to invoke.
- **first-match wins, no validation** -- simplest, but a typo in the alias list passes silently.
- **runtime-panic on first lookup that hits a collision** -- intrusive, but unmissable.

## backward compatibility

`polyflag` is internal to this repo with one caller. break the api -- migrate edit's `KNOWN_QUIRKS` in the same change. the migration is mechanical:

```rust
// before
const KNOWN_QUIRKS: &[&str] = &["weird-filenames", "ascii", "nocolor", ...];
// after
const KNOWN_QUIRKS: &[polyflag::KnownToken] = &[
    polyflag::KnownToken::new("weird-filenames"),
    polyflag::KnownToken::new("ascii"),
    polyflag::KnownToken { canonical: "nocolor", aliases: &["no-color"] },
    ...
];
```

a parallel `apply_with_aliases` would avoid breakage but doubles the api surface for one caller -- not worth it.

## migration plan

1. add `KnownToken`, `KnownToken::new`, optional `token!` macro, `check_known` debug-only validator.
2. update `apply` and `apply_env_for_flag` signatures + bodies.
3. rewrite tests to construct `KnownToken` literals; add new tests:
    - alias resolves to canonical.
    - `-alias` removes canonical.
    - alias collision detected by `check_known`.
    - canonical and alias are equivalent inputs (round-trip).
4. update `KNOWN_QUIRKS` in edit's main.rs and add the aliases that motivated the work (`no-color`, `no-animations`, etc).
5. README + crate docstring: add alias section, deprecation/hidden statuses, and a worked example along the lines of the [worked example](#worked-example) section.

## tradeoffs

- **api shape**: struct vs `(&str, &[&str])` tuple. struct named-fields read better at the call site (`canonical:` vs index). tuple is shorter to write.
- **alias normalization**: could auto-treat `-` and `_` as equivalent without explicit aliases, so `nocolor` / `no-color` / `no_color` all work for free. simpler for callers but couples the matching rule to the lib. explicit list is more intentional and lets callers ship `nocolor` w/o accepting `no_color`.
- **case sensitivity**: lowercase-only is the convention today; an alias mechanism doesn't change that. case-insensitive matching could be layered on top later but multiplies the surface.

## decisions

- **public `canonicalize(input, known) -> Option<Resolved>`** -- yes, with the rich return form so callers don't need to re-walk the table to figure out *why* the input matched:

  ```rust
  pub struct Resolved {
      pub canonical: &'static str,
      pub kind: ResolvedKind,
  }

  pub enum ResolvedKind {
      Canonical,             // input was the preferred spelling
      Alternative,           // input was an `Alternative` alias
      Deprecated,            // input was a `Deprecated` alias -- caller may warn
      Hidden,                // input was a `Hidden` alias -- caller stays silent
  }
  ```

  the deprecation-reporting callback inside `apply` becomes a thin wrapper: it fires only when `kind == Deprecated`.

- **per-alias status flag** -- yes. canonical is implicitly preferred; aliases carry one of `Alternative` / `Deprecated` / `Hidden` (see [design](#design)).
- **empty-string alias is an error** -- both at parse time and in `check_known`. an empty `spelling` in a known-token list is always a bug.

## open questions

- do we want the env-var path (`apply_env_for_flag`) to print the resolved env name alongside the unknown-token error so users can find their typo source? unrelated to aliases but adjacent ux.
- `check_known` being debug-only means typos slip through release builds. acceptable since author runs tests / dev builds before shipping?
- "did you mean?" suggestions on `UnknownToken` -- match against canonicals + aliases via levenshtein. small dep cost, real ux win.
- `#[non_exhaustive]` on `KnownToken` / `Alias` to leave room for future fields (`description: &str`, etc). blocks struct-literal construction outside the crate -- the `token!` macro side-steps that, so callers stay ergonomic. worth the extra friction or premature future-proofing?
- bump `polyflag` minor version (api break)? crate is internal-only -- no semver promise -- but flagging it in CHANGELOG would still be useful.
- audit other callers in the workspace before breaking the api -- only `KNOWN_QUIRKS` today, but worth a final grep before landing.
- `token!` macro reserves `deprecated` and `hidden` as marker keywords (see [worked example](#worked-example)). macro_rules matches by token so no rust-keyword collision; future statuses extend the same surface.
- complexity of resolution is `O(input_tokens x canonicals x aliases)`. fine for small flag tables; document the assumption in lib.rs so future-you doesn't ship a 10k-token pluggable variant on the same backend.

## worked example

end-to-end, including caller-side deprecation reporting:

```rust
use polyflag::{KnownToken, ResolvedKind, apply_with_callback, token};

const KNOWN_QUIRKS: &[KnownToken] = &[
    token!("ascii"),
    token!("nocolor"; "no-color", deprecated "no_color"),
    token!("noanimations";
        "no-animations",
        deprecated "no_animations",
        hidden "noanim",
    ),
    token!("weird-filenames"),
    token!("allow-create"; "allowcreate"),
];

fn parse_quirks(input: &str) -> Result<HashSet<&'static str>, String> {
    let mut set = HashSet::new();
    apply_with_callback(input, KNOWN_QUIRKS, &mut set, |spelling, canonical| {
        eprintln!("warning: --quirks={spelling} is deprecated, use {canonical}");
    })
    .map_err(|e| format!("{e}"))?;
    Ok(set)
}

// Resolution table for the entries above:
//   "ascii"          -> Resolved { canonical: "ascii",        kind: Canonical }
//   "nocolor"        -> Resolved { canonical: "nocolor",      kind: Canonical }
//   "no-color"       -> Resolved { canonical: "nocolor",      kind: Alternative }
//   "no_color"       -> Resolved { canonical: "nocolor",      kind: Deprecated } (warn)
//   "noanim"         -> Resolved { canonical: "noanimations", kind: Hidden }     (silent)
//   "-no-color"      -> remove "nocolor" from the set
//   "purple"         -> Err(UnknownToken("purple"))
```
