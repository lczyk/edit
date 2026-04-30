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
}
