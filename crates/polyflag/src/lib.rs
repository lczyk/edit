// cspell:ignore polytest
//! Repeatable comma-separated set-style cli flags.
//!
//! Given a fixed list of known token names, parse one occurrence of a flag
//! whose value is a comma-separated list of those names, accumulating into
//! a [`HashSet`]. A `-` prefix on a token removes it from the set instead
//! of adding. Unknown tokens error.
//!
//! Designed for flags like `--quirks=foo,bar --quirks=-foo` where the final
//! state is `{bar}`. Order across flag occurrences matters; order within a
//! single flag occurrence also matters (left-to-right).
//!
//! # Example
//!
//! ```
//! use std::collections::HashSet;
//! use polyflag::apply;
//!
//! const KNOWN: &[&str] = &["foo", "bar", "baz"];
//! let mut set: HashSet<&'static str> = HashSet::new();
//!
//! apply("foo,bar", KNOWN, &mut set).unwrap();
//! apply("baz", KNOWN, &mut set).unwrap();
//! apply("bar,-foo", KNOWN, &mut set).unwrap();
//!
//! assert!(set.contains("bar") && set.contains("baz"));
//! assert!(!set.contains("foo"));
//! ```

use std::collections::HashSet;
use std::fmt;

/// Returned when an input token (with any leading `-` stripped) does not
/// appear in the caller's `known` list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownToken(pub String);

impl fmt::Display for UnknownToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown token {:?}", self.0)
    }
}

impl std::error::Error for UnknownToken {}

/// Apply one occurrence of a set flag's value to `set`.
///
/// `input` is the raw value (everything after `=` in `--flag=...`). It is
/// split on `,`; tokens are trimmed; empty tokens are skipped. A `-` prefix
/// on a token removes the named entry; otherwise the entry is inserted.
/// Inserted / removed values are the matching `&'static str` from `known`,
/// so the resulting set's lifetime is bound to the `known` slice.
///
/// Returns the first unknown token encountered (after stripping any `-`
/// prefix). On error, the set is left in its partially-mutated state --
/// callers that need atomic application should clone first.
pub fn apply(
    input: &str,
    known: &[&'static str],
    set: &mut HashSet<&'static str>,
) -> Result<(), UnknownToken> {
    for tok in input.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let (name, add) = match tok.strip_prefix('-') {
            Some(rest) => (rest, false),
            None => (tok, true),
        };
        match known.iter().find(|k| **k == name) {
            Some(&k) => {
                if add {
                    set.insert(k);
                } else {
                    set.remove(k);
                }
            }
            None => return Err(UnknownToken(name.to_owned())),
        }
    }
    Ok(())
}

/// Apply an env-var-sourced default for the named cli flag to `set`.
///
/// The env var name is derived from `prefix` and `flag` so the cli
/// surface (`--<flag>=...`) and the env surface stay in lock-step --
/// there's no second string to keep in sync. The mapping is:
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
/// Behaviour-wise, this is equivalent to a single occurrence of the
/// flag, applied with the env value, and applied **before** any cli
/// flag(s) the caller subsequently processes -- so a later
/// `--<flag>=-name` can negate an entry the env contributed. Unset,
/// empty, or non-UTF-8 values are no-ops, so the call is safe as an
/// unconditional default-providing step.
///
/// Token semantics (including `-name` removal) match [`apply`].
///
/// # Example
///
/// ```
/// use std::collections::HashSet;
/// # // SAFETY: this doc test is single-threaded.
/// unsafe { std::env::set_var("MY_FLAGS", "foo,bar"); }
///
/// const KNOWN: &[&str] = &["foo", "bar", "baz"];
/// let mut set: HashSet<&'static str> = HashSet::new();
/// // env var resolved as MY_FLAGS from prefix="my" + flag="flags".
/// polyflag::apply_env_for_flag("my", "flags", KNOWN, &mut set).unwrap();
/// assert!(set.contains("foo") && set.contains("bar"));
/// # unsafe { std::env::remove_var("MY_FLAGS"); }
/// ```
pub fn apply_env_for_flag(
    prefix: &str,
    flag: &str,
    known: &[&'static str],
    set: &mut HashSet<&'static str>,
) -> Result<(), UnknownToken> {
    let env_var = env_var_name(prefix, flag);
    let Ok(val) = std::env::var(&env_var) else { return Ok(()) };
    apply(&val, known, set)
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

#[cfg(test)]
mod tests {
    use super::*;

    const KNOWN: &[&str] = &["foo", "bar", "baz"];

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
    fn user_example_layering() {
        // From the original spec: foo,bar then baz then bar,-foo => {bar, baz}.
        let s = run(&["foo,bar", "baz", "bar,-foo"]).unwrap();
        assert_eq!(s, HashSet::from(["bar", "baz"]));
    }

    #[test]
    fn user_example_extra_add_at_end() {
        let s = run(&["foo,bar", "baz", "bar,-foo", "foo"]).unwrap();
        assert_eq!(s, HashSet::from(["foo", "bar", "baz"]));
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
