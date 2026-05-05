// cspell:ignore polytest
//! Repeatable comma-separated set-style cli flags.
//!
//! Given a fixed list of known tokens, parse one occurrence of a flag whose
//! value is a comma-separated list of those tokens, accumulating into a
//! [`HashSet`]. A `-` prefix on a token removes it from the set instead of
//! adding. Unknown tokens error.
//!
//! Each [`KnownToken`] has a single canonical spelling plus zero or more
//! [`Alias`]es. Aliases let multiple input spellings (e.g. `nocolor` and
//! `no-color`) resolve to the same canonical entry, so callers test
//! `set.contains("nocolor")` regardless of which spelling the user typed.
//!
//! Designed for flags like `--quirks=foo,bar --quirks=-foo` where the
//! final state is `{bar}`. Order across flag occurrences matters; order
//! within a single occurrence also matters (left-to-right).
//!
//! Resolution complexity is `O(input_tokens x canonicals x aliases)`. Fine
//! for the small flag tables this crate targets; do not graft a runtime
//! plugin loader on top.
//!
//! # Example
//!
//! ```
//! use std::collections::HashSet;
//! use polyflag::{KnownToken, apply, token};
//!
//! const KNOWN: &[KnownToken] = &[
//!     token!("foo"),
//!     token!("bar"; "barre"),
//!     token!("baz"; "baz-alt", deprecated "old-baz"),
//! ];
//! let mut set: HashSet<&'static str> = HashSet::new();
//!
//! apply("foo,barre", KNOWN, &mut set).unwrap();
//! apply("baz",       KNOWN, &mut set).unwrap();
//! apply("bar,-foo",  KNOWN, &mut set).unwrap();
//!
//! assert!(set.contains("bar") && set.contains("baz"));
//! assert!(!set.contains("foo"));
//! // aliases never live in the set; only canonicals do.
//! assert!(!set.contains("barre"));
//! ```

use std::collections::HashSet;
use std::fmt;

/// Returned when an input token (with any leading `-` stripped) does not
/// appear as a canonical or alias in the caller's `known` list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownToken(pub String);

impl fmt::Display for UnknownToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown token {:?}", self.0)
    }
}

impl std::error::Error for UnknownToken {}

/// One entry in the caller's known-token table: a canonical spelling plus
/// zero or more aliases that resolve to it.
#[derive(Debug, Clone, Copy)]
pub struct KnownToken {
    pub canonical: &'static str,
    pub aliases: &'static [Alias],
}

impl KnownToken {
    /// Construct a token with no aliases. The [`token!`] macro is more
    /// ergonomic at call sites; this is the bare ctor.
    pub const fn new(canonical: &'static str) -> Self {
        Self { canonical, aliases: &[] }
    }
}

/// One alias spelling for a [`KnownToken`].
#[derive(Debug, Clone, Copy)]
pub struct Alias {
    pub spelling: &'static str,
    pub status: AliasStatus,
}

impl Alias {
    pub const fn alt(spelling: &'static str) -> Self {
        Self { spelling, status: AliasStatus::Alternative }
    }
    pub const fn deprecated(spelling: &'static str) -> Self {
        Self { spelling, status: AliasStatus::Deprecated }
    }
    pub const fn hidden(spelling: &'static str) -> Self {
        Self { spelling, status: AliasStatus::Hidden }
    }
}

/// How an alias is treated by callers that surface user-facing messaging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AliasStatus {
    /// Intentional alternate spelling, equally valid as the canonical.
    Alternative,
    /// Still resolves but callers should warn or migrate.
    Deprecated,
    /// Resolves silently. Omitted from `--help` listings and from any
    /// public enumeration of accepted spellings. For undocumented compat
    /// with an old typo or removed convention.
    Hidden,
}

/// Result of resolving one input token against a known-token list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolved {
    pub canonical: &'static str,
    pub kind: ResolvedKind,
}

/// Why an input matched -- mirrors [`AliasStatus`] plus a
/// [`ResolvedKind::Canonical`] variant for canonical-spelling input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedKind {
    Canonical,
    Alternative,
    Deprecated,
    Hidden,
}

/// Look up `input` (one bare token, no `-` prefix) against `known`.
/// Returns the canonical entry and how the input matched. `None` if the
/// input is not a canonical or alias for any known token.
pub fn canonicalize(input: &str, known: &[KnownToken]) -> Option<Resolved> {
    for kt in known {
        if kt.canonical == input {
            return Some(Resolved { canonical: kt.canonical, kind: ResolvedKind::Canonical });
        }
        for alias in kt.aliases {
            if alias.spelling == input {
                let kind = match alias.status {
                    AliasStatus::Alternative => ResolvedKind::Alternative,
                    AliasStatus::Deprecated => ResolvedKind::Deprecated,
                    AliasStatus::Hidden => ResolvedKind::Hidden,
                };
                return Some(Resolved { canonical: kt.canonical, kind });
            }
        }
    }
    None
}

/// Apply one occurrence of a set flag's value to `set`.
///
/// `input` is the raw value (everything after `=` in `--flag=...`). It is
/// split on `,`; tokens are trimmed; empty tokens are skipped. A `-` prefix
/// on a token removes the named entry; otherwise the entry is inserted.
/// Inserted / removed values are always the **canonical** `&'static str`
/// from `known`, regardless of which alias the input used.
///
/// On unknown token, returns the offending input verbatim (after stripping
/// any `-` prefix). The set is left in its partially-mutated state --
/// callers that need atomic application should clone first.
///
/// To learn when an input hit a [`AliasStatus::Deprecated`] alias, use
/// [`apply_with_callback`].
pub fn apply(
    input: &str,
    known: &[KnownToken],
    set: &mut HashSet<&'static str>,
) -> Result<(), UnknownToken> {
    apply_with_callback(input, known, set, |_, _| {})
}

/// Like [`apply`], but invokes `on_deprecated(input_spelling, canonical)`
/// each time an input token resolves through a [`AliasStatus::Deprecated`]
/// alias. `Hidden` aliases never call back; canonicals and `Alternative`
/// aliases never call back.
pub fn apply_with_callback(
    input: &str,
    known: &[KnownToken],
    set: &mut HashSet<&'static str>,
    mut on_deprecated: impl FnMut(&str, &'static str),
) -> Result<(), UnknownToken> {
    for tok in input.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let (name, add) = match tok.strip_prefix('-') {
            Some(rest) => (rest, false),
            None => (tok, true),
        };
        let Some(resolved) = canonicalize(name, known) else {
            return Err(UnknownToken(name.to_owned()));
        };
        if resolved.kind == ResolvedKind::Deprecated {
            on_deprecated(name, resolved.canonical);
        }
        if add {
            set.insert(resolved.canonical);
        } else {
            set.remove(resolved.canonical);
        }
    }
    Ok(())
}

/// Apply an env-var-sourced default for the named cli flag to `set`.
///
/// The env var name is derived from `prefix` and `flag` so the cli surface
/// (`--<flag>=...`) and the env surface stay in lock-step -- there's no
/// second string to keep in sync. The mapping is:
///
/// ```text
/// env_var = "{PREFIX}_{FLAG_AS_SCREAMING_SNAKE}"
/// ```
///
/// where `prefix` is uppercased verbatim and `flag` is uppercased with
/// `-` rewritten to `_`. Examples:
///
/// | `prefix` | `flag`         | env var               |
/// |----------|----------------|-----------------------|
/// | `"edit"` | `"quirks"`     | `EDIT_QUIRKS`         |
/// | `"app"`  | `"allow-create"`| `APP_ALLOW_CREATE`    |
///
/// Behaviour-wise, this is equivalent to a single occurrence of the flag,
/// applied with the env value, and applied **before** any cli flag(s) the
/// caller subsequently processes -- so a later `--<flag>=-name` can
/// negate an entry the env contributed. Unset, empty, or non-UTF-8 values
/// are no-ops, so the call is safe as an unconditional default-providing
/// step.
///
/// Token semantics (including `-name` removal and alias resolution) match
/// [`apply`].
pub fn apply_env_for_flag(
    prefix: &str,
    flag: &str,
    known: &[KnownToken],
    set: &mut HashSet<&'static str>,
) -> Result<(), UnknownToken> {
    apply_env_for_flag_with_callback(prefix, flag, known, set, |_, _| {})
}

/// Like [`apply_env_for_flag`], but threads a deprecation callback through
/// to [`apply_with_callback`].
pub fn apply_env_for_flag_with_callback(
    prefix: &str,
    flag: &str,
    known: &[KnownToken],
    set: &mut HashSet<&'static str>,
    on_deprecated: impl FnMut(&str, &'static str),
) -> Result<(), UnknownToken> {
    let env_var = env_var_name(prefix, flag);
    let Ok(val) = std::env::var(&env_var) else { return Ok(()) };
    apply_with_callback(&val, known, set, on_deprecated)
}

/// Compute the env var name corresponding to a flag, using the same
/// derivation as [`apply_env_for_flag`]. Exposed so callers can include
/// the resolved name in error messages without repeating the rule.
pub fn env_var_name(prefix: &str, flag: &str) -> String {
    let mut out = String::with_capacity(prefix.len() + 1 + flag.len());
    for c in prefix.chars() {
        out.push(c.to_ascii_uppercase());
    }
    out.push('_');
    for c in flag.chars() {
        out.push(if c == '-' { '_' } else { c.to_ascii_uppercase() });
    }
    out
}

/// Validate `known` for self-consistency. In debug builds, panics on:
///
/// - empty canonical or alias spelling.
/// - duplicate spelling (canonical or alias) anywhere in the table.
///
/// Release builds compile this to an empty body, so callers may invoke it
/// unconditionally at startup without paying for the walk in production.
/// The intent is to catch typos in the static token table at test time;
/// callers that ship `cargo test` before release will get the assertions
/// for free.
pub fn check_known(known: &[KnownToken]) {
    #[cfg(debug_assertions)]
    {
        let mut seen: Vec<&'static str> = Vec::new();
        for kt in known {
            assert!(!kt.canonical.is_empty(), "empty canonical spelling in known-token list");
            assert!(
                !seen.contains(&kt.canonical),
                "duplicate spelling {:?} in known-token list (canonical collides)",
                kt.canonical,
            );
            seen.push(kt.canonical);
            for alias in kt.aliases {
                assert!(
                    !alias.spelling.is_empty(),
                    "empty alias spelling for canonical {:?}",
                    kt.canonical,
                );
                assert!(
                    !seen.contains(&alias.spelling),
                    "duplicate spelling {:?} in known-token list (alias collides)",
                    alias.spelling,
                );
                seen.push(alias.spelling);
            }
        }
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = known;
    }
}

/// Construct a [`KnownToken`] tersely.
///
/// Forms:
///
/// ```text
/// token!("canonical")
/// token!("canonical"; "alt1", "alt2")
/// token!("canonical"; "alt", deprecated "old", hidden "internal")
/// ```
///
/// Bare string literals after `;` become [`AliasStatus::Alternative`].
/// `deprecated <literal>` and `hidden <literal>` produce the corresponding
/// [`AliasStatus`]. Mixing forms in one invocation is allowed.
#[macro_export]
macro_rules! token {
    ($canon:literal) => {
        $crate::KnownToken { canonical: $canon, aliases: &[] }
    };
    ($canon:literal; $($rest:tt)+) => {
        $crate::KnownToken {
            canonical: $canon,
            aliases: &$crate::__token_aliases!([] , $($rest)+),
        }
    };
}

/// Implementation detail of [`token!`]. Tt-munches the alias list,
/// accumulating [`Alias`] expressions into a fixed-size array literal.
#[doc(hidden)]
#[macro_export]
macro_rules! __token_aliases {
    ([$($acc:expr),* $(,)?] $(,)?) => {
        [$($acc),*]
    };
    ([$($acc:expr),* $(,)?] , deprecated $sp:literal $($rest:tt)*) => {
        $crate::__token_aliases!(
            [$($acc,)* $crate::Alias::deprecated($sp)] $($rest)*
        )
    };
    ([$($acc:expr),* $(,)?] , hidden $sp:literal $($rest:tt)*) => {
        $crate::__token_aliases!(
            [$($acc,)* $crate::Alias::hidden($sp)] $($rest)*
        )
    };
    ([$($acc:expr),* $(,)?] , $sp:literal $($rest:tt)*) => {
        $crate::__token_aliases!(
            [$($acc,)* $crate::Alias::alt($sp)] $($rest)*
        )
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    const KNOWN: &[KnownToken] = &[
        token!("foo"),
        token!("bar"; "barre"),
        token!("baz"; "baz-alt", deprecated "old-baz", hidden "b"),
    ];

    fn run(inputs: &[&str]) -> Result<HashSet<&'static str>, UnknownToken> {
        let mut set: HashSet<&'static str> = HashSet::new();
        for s in inputs {
            apply(s, KNOWN, &mut set)?;
        }
        Ok(set)
    }

    #[test]
    fn add_dedup() {
        let s = run(&["foo,bar", "foo"]).unwrap();
        assert_eq!(s, HashSet::from(["foo", "bar"]));
    }

    #[test]
    fn remove_after_add() {
        let s = run(&["foo,bar,baz", "-foo"]).unwrap();
        assert_eq!(s, HashSet::from(["bar", "baz"]));
    }

    #[test]
    fn add_after_remove() {
        let s = run(&["-foo", "foo"]).unwrap();
        assert_eq!(s, HashSet::from(["foo"]));
    }

    #[test]
    fn alias_resolves_to_canonical() {
        let s = run(&["barre"]).unwrap();
        assert_eq!(s, HashSet::from(["bar"]));
        // alias is input-only; the canonical is what lives in the set.
        assert!(!s.contains("barre"));
    }

    #[test]
    fn remove_via_alias_strips_canonical() {
        let s = run(&["bar,foo", "-barre"]).unwrap();
        assert_eq!(s, HashSet::from(["foo"]));
    }

    #[test]
    fn canonical_and_alias_are_interchangeable() {
        let a = run(&["bar"]).unwrap();
        let b = run(&["barre"]).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn deprecated_alias_fires_callback() {
        let mut set = HashSet::new();
        let mut warnings: Vec<(String, &'static str)> = Vec::new();
        apply_with_callback("old-baz", KNOWN, &mut set, |sp, canon| {
            warnings.push((sp.to_owned(), canon));
        })
        .unwrap();
        assert_eq!(set, HashSet::from(["baz"]));
        assert_eq!(warnings, vec![("old-baz".to_owned(), "baz")]);
    }

    #[test]
    fn alternative_alias_does_not_fire_callback() {
        let mut set = HashSet::new();
        let mut fired = false;
        apply_with_callback("barre", KNOWN, &mut set, |_, _| fired = true).unwrap();
        assert!(!fired);
    }

    #[test]
    fn hidden_alias_resolves_silently() {
        let mut set = HashSet::new();
        let mut fired = false;
        apply_with_callback("b", KNOWN, &mut set, |_, _| fired = true).unwrap();
        assert_eq!(set, HashSet::from(["baz"]));
        assert!(!fired);
    }

    #[test]
    fn canonicalize_classifies_match_kind() {
        assert_eq!(
            canonicalize("foo", KNOWN),
            Some(Resolved { canonical: "foo", kind: ResolvedKind::Canonical })
        );
        assert_eq!(
            canonicalize("barre", KNOWN),
            Some(Resolved { canonical: "bar", kind: ResolvedKind::Alternative })
        );
        assert_eq!(
            canonicalize("old-baz", KNOWN),
            Some(Resolved { canonical: "baz", kind: ResolvedKind::Deprecated })
        );
        assert_eq!(
            canonicalize("b", KNOWN),
            Some(Resolved { canonical: "baz", kind: ResolvedKind::Hidden })
        );
        assert_eq!(canonicalize("nope", KNOWN), None);
    }

    #[test]
    fn empty_tokens_skipped() {
        let s = run(&["", "foo,,bar,", " , foo "]).unwrap();
        assert_eq!(s, HashSet::from(["foo", "bar"]));
    }

    #[test]
    fn unknown_token_errors() {
        let mut set = HashSet::new();
        let err = apply("foo,nope", KNOWN, &mut set).unwrap_err();
        assert_eq!(err, UnknownToken("nope".into()));
        // partial application is observable: foo got added before the error.
        assert!(set.contains("foo"));
    }

    #[test]
    fn unknown_negative_token_errors() {
        let mut set = HashSet::new();
        let err = apply("-nope", KNOWN, &mut set).unwrap_err();
        assert_eq!(err, UnknownToken("nope".into()));
    }

    #[test]
    fn remove_absent_is_noop() {
        let s = run(&["-foo"]).unwrap();
        assert!(s.is_empty());
    }

    #[test]
    fn check_known_accepts_valid() {
        check_known(KNOWN);
    }

    #[test]
    #[should_panic(expected = "duplicate spelling")]
    fn check_known_rejects_canonical_collision() {
        const BAD: &[KnownToken] = &[token!("foo"), token!("foo")];
        check_known(BAD);
    }

    #[test]
    #[should_panic(expected = "duplicate spelling")]
    fn check_known_rejects_alias_canonical_collision() {
        const BAD: &[KnownToken] = &[token!("foo"; "bar"), token!("bar")];
        check_known(BAD);
    }

    #[test]
    #[should_panic(expected = "duplicate spelling")]
    fn check_known_rejects_alias_alias_collision() {
        const BAD: &[KnownToken] = &[token!("foo"; "x"), token!("bar"; "x")];
        check_known(BAD);
    }

    #[test]
    #[should_panic(expected = "empty")]
    fn check_known_rejects_empty_canonical() {
        const BAD: &[KnownToken] = &[token!("")];
        check_known(BAD);
    }

    #[test]
    #[should_panic(expected = "empty alias")]
    fn check_known_rejects_empty_alias() {
        const BAD: &[KnownToken] = &[token!("foo"; "")];
        check_known(BAD);
    }

    /// Set / unset the env var via the unsafe API. Tests in this module run
    /// sequentially per crate by default; we still avoid concurrent env
    /// access by using a unique var name per test.
    fn with_env<F: FnOnce()>(name: &str, value: Option<&str>, f: F) {
        unsafe {
            match value {
                Some(v) => std::env::set_var(name, v),
                None => std::env::remove_var(name),
            }
        }
        f();
        unsafe { std::env::remove_var(name) };
    }

    #[test]
    fn env_var_name_format() {
        assert_eq!(env_var_name("edit", "quirks"), "EDIT_QUIRKS");
        assert_eq!(env_var_name("app", "allow-create"), "APP_ALLOW_CREATE");
        assert_eq!(env_var_name("MIXED", "flag"), "MIXED_FLAG");
    }

    #[test]
    fn apply_env_unset_is_noop() {
        with_env("POLYTEST_UNSET", None, || {
            let mut set = HashSet::new();
            apply_env_for_flag("polytest", "unset", KNOWN, &mut set).unwrap();
            assert!(set.is_empty());
        });
    }

    #[test]
    fn apply_env_empty_is_noop() {
        with_env("POLYTEST_EMPTY", Some(""), || {
            let mut set = HashSet::new();
            apply_env_for_flag("polytest", "empty", KNOWN, &mut set).unwrap();
            assert!(set.is_empty());
        });
    }

    #[test]
    fn apply_env_resolves_aliases() {
        with_env("POLYTEST_ALIAS", Some("barre,old-baz"), || {
            let mut set: HashSet<&'static str> = HashSet::new();
            let mut deprecations: Vec<(String, &'static str)> = Vec::new();
            apply_env_for_flag_with_callback("polytest", "alias", KNOWN, &mut set, |sp, canon| {
                deprecations.push((sp.to_owned(), canon))
            })
            .unwrap();
            assert_eq!(set, HashSet::from(["bar", "baz"]));
            assert_eq!(deprecations, vec![("old-baz".to_owned(), "baz")]);
        });
    }

    #[test]
    fn apply_env_adds_then_cli_can_remove() {
        with_env("POLYTEST_LAYER", Some("foo,bar"), || {
            let mut set: HashSet<&'static str> = HashSet::new();
            apply_env_for_flag("polytest", "layer", KNOWN, &mut set).unwrap();
            apply("-foo", KNOWN, &mut set).unwrap();
            assert_eq!(set, HashSet::from(["bar"]));
        });
    }

    #[test]
    fn apply_env_unknown_token_errors() {
        with_env("POLYTEST_BAD", Some("foo,nope"), || {
            let mut set = HashSet::new();
            let err = apply_env_for_flag("polytest", "bad", KNOWN, &mut set).unwrap_err();
            assert_eq!(err, UnknownToken("nope".into()));
        });
    }

    #[test]
    fn apply_env_kebab_flag_resolves_to_screaming_snake() {
        with_env("POLYTEST_ALLOW_CREATE", Some("foo"), || {
            let mut set: HashSet<&'static str> = HashSet::new();
            apply_env_for_flag("polytest", "allow-create", KNOWN, &mut set).unwrap();
            assert_eq!(set, HashSet::from(["foo"]));
        });
    }
}
