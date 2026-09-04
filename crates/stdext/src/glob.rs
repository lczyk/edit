//! Simple glob matching.
//!
//! Supported patterns:
//! - `*` matches any characters except for path separators, including an empty string.
//! - `**` matches any characters, including an empty string.
//!   For convenience, `/**/` also matches `/`.
//! - `[abc]` matches one character from the set, `[a-z]` one from the range,
//!   and a leading `!` or `^` negates. A `]` in first position is a literal,
//!   as is a `-` in first or last position. A class never matches a path
//!   separator; an unterminated `[` is a literal.
//!
//! Matching is ASCII-case-insensitive throughout.

use std::path::is_separator;

#[inline]
pub fn glob_match<P: AsRef<[u8]>, N: AsRef<[u8]>>(pattern: P, name: N) -> bool {
    glob(pattern.as_ref(), name.as_ref())
}

fn glob(pattern: &[u8], name: &[u8]) -> bool {
    fast_path(pattern, name).unwrap_or_else(|| slow_path(pattern, name))
}

// Fast-pass for the most common patterns:
// * Matching files by extension (e.g., **/*.rs)
// * Matching files by name (e.g., **/Cargo.toml)
fn fast_path(pattern: &[u8], name: &[u8]) -> Option<bool> {
    // In either case, the glob must start with "**/".
    let mut suffix = pattern.strip_prefix(b"**/")?;
    if suffix.is_empty() {
        return None;
    }

    // Determine whether it's "**/" or "**/*".
    let mut needs_dir_anchor = true;
    if let Some(s) = suffix.strip_prefix(b"*") {
        suffix = s;
        needs_dir_anchor = false;
    }

    // Restrict down to anything we can handle with a suffix check.
    if suffix.is_empty() || contains_magic(suffix) {
        return None;
    }

    Some(
        match_path_suffix(name, suffix)
            && (
                // In case of "**/*extension" a simple suffix match is sufficient.
                !needs_dir_anchor
                // But for "**/filename" we need to ensure that path is either "filename"...
                || name.len() == suffix.len()
                // ...or that it is ".../filename".
                || is_separator(name[name.len() - suffix.len() - 1] as char)
            ),
    )
}

fn contains_magic(pattern: &[u8]) -> bool {
    pattern.iter().any(|&b| b == b'*' || b == b'[')
}

fn match_path_suffix(path: &[u8], suffix: &[u8]) -> bool {
    if path.len() < suffix.len() {
        return false;
    }

    let path = &path[path.len() - suffix.len()..];

    path.eq_ignore_ascii_case(suffix)
}

/// Match `c` against the bracket class starting at `pat[0] == b'['`, returning
/// the verdict and the class's length in pattern bytes. `None` means the class
/// has no closing `]`, which callers treat as a literal `[`.
fn class_match(pat: &[u8], c: Option<u8>) -> Option<(bool, usize)> {
    let mut i = 1;
    let negated = matches!(pat.get(i), Some(b'!' | b'^'));
    if negated {
        i += 1;
    }

    let mut hit = false;
    let opening = i;
    loop {
        match pat.get(i) {
            None => return None,
            // A ']' in first position is a literal, not the terminator.
            Some(b']') if i > opening => break,
            Some(_) => {}
        }

        let lo = pat[i];
        i += 1;
        // A '-' before the terminator opens a range; elsewhere it's a literal.
        if pat.get(i) == Some(&b'-') && pat.get(i + 1).is_some_and(|&b| b != b']') {
            let hi = pat[i + 1];
            i += 2;
            hit |= c.is_some_and(|c| in_range(lo, hi, c));
        } else {
            hit |= c.is_some_and(|c| lo.eq_ignore_ascii_case(&c));
        }
    }

    let matched = hit != negated && c.is_some_and(|c| !is_separator(c as char));
    Some((matched, i + 1))
}

fn in_range(lo: u8, hi: u8, c: u8) -> bool {
    let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
    (lo..=hi).contains(&c)
        || (lo..=hi).contains(&c.to_ascii_lowercase())
        || (lo..=hi).contains(&c.to_ascii_uppercase())
}

// Backtracking matcher. It's not particularly fast, but it doesn't need to be.
// It doesn't run often, and only for patterns the fast path can't reduce to a
// suffix compare.
#[cold]
fn slow_path(pattern: &[u8], name: &[u8]) -> bool {
    match_from(pattern, 0, name, 0)
}

fn match_from(pat: &[u8], mut px: usize, name: &[u8], mut nx: usize) -> bool {
    while px < pat.len() {
        match pat[px] {
            b'*' => {
                if pat.get(px + 1) == Some(&b'*') {
                    let rest = px + 2;
                    // For convenience, "/**/" also matches "/", and so does a
                    // leading "**/".
                    if pat.get(rest) == Some(&b'/')
                        && (px == 0 || pat[px - 1] == b'/')
                        && match_from(pat, rest + 1, name, nx)
                    {
                        return true;
                    }
                    return (nx..=name.len()).any(|i| match_from(pat, rest, name, i));
                }
                // A single star stops at the first separator.
                for i in nx..=name.len() {
                    if match_from(pat, px + 1, name, i) {
                        return true;
                    }
                    if name.get(i).is_some_and(|&b| is_separator(b as char)) {
                        break;
                    }
                }
                return false;
            }
            b'[' => match class_match(&pat[px..], name.get(nx).copied()) {
                Some((true, len)) => {
                    px += len;
                    nx += 1;
                }
                Some((false, _)) => return false,
                None if name.get(nx) == Some(&b'[') => {
                    px += 1;
                    nx += 1;
                }
                None => return false,
            },
            c => {
                if !name.get(nx).is_some_and(|b| b.eq_ignore_ascii_case(&c)) {
                    return false;
                }
                px += 1;
                nx += 1;
            }
        }
    }

    nx == name.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glob_match() {
        let tests = [
            // Test cases from https://research.swtch.com/glob.go
            ("", "", true),
            ("x", "", false),
            ("", "x", false),
            ("abc", "abc", true),
            ("*", "abc", true),
            ("*c", "abc", true),
            ("*b", "abc", false),
            ("a*", "abc", true),
            ("b*", "abc", false),
            ("a*", "a", true),
            ("*a", "a", true),
            ("a*b*c*d*e*", "axbxcxdxe", true),
            ("a*b*c*d*e*", "axbxcxdxexxx", true),
            ("*x", "xxx", true),
            // Test cases from https://github.com/golang/go/blob/master/src/path/filepath/match_test.go
            ("a*", "ab/c", false),
            ("a*b", "a/b", false),
            ("a*/b", "abc/b", true),
            ("a*/b", "a/c/b", false),
            ("a*b*c*d*e*/f", "axbxcxdxe/f", true),
            ("a*b*c*d*e*/f", "axbxcxdxexxx/f", true),
            ("a*b*c*d*e*/f", "axbxcxdxe/xxx/f", false),
            ("a*b*c*d*e*/f", "axbxcxdxexxx/fff", false),
            // Single star (*)
            // - Empty string
            ("*", "", true),
            // - Anything else is covered above
            // Double star (**)
            // - Empty string
            ("**", "", true),
            ("a**", "a", true),
            ("**a", "a", true),
            // - Prefix
            ("**", "abc", true),
            ("**", "foo/baz/bar", true),
            ("**c", "abc", true),
            ("**b", "abc", false),
            // - Infix
            ("a**c", "ac", true),
            ("a**c", "abc", true),
            ("a**c", "abd", false),
            ("a**d", "abc", false),
            ("a**c", "a/bc", true),
            ("a**c", "ab/c", true),
            ("a**c", "a/b/c", true),
            // -- Infix with left separator
            ("a/**c", "ac", false),
            ("a/**c", "a/c", true),
            ("a/**c", "b/c", false),
            ("a/**c", "a/d", false),
            ("a/**c", "a/b/c", true),
            ("a/**c", "a/b/d", false),
            ("a/**c", "d/b/c", false),
            // -- Infix with right separator
            ("a**/c", "ac", false),
            ("a**/c", "a/c", true),
            ("a**/c", "b/c", false),
            ("a**/c", "a/d", false),
            ("a**/c", "a/b/c", true),
            ("a**/c", "a/b/d", false),
            ("a**/c", "d/b/c", false),
            // - Infix with two separators
            ("a/**/c", "ac", false),
            ("a/**/c", "a/c", true),
            ("a/**/c", "b/c", false),
            ("a/**/c", "a/d", false),
            ("a/**/c", "a/b/c", true),
            ("a/**/c", "a/b/d", false),
            ("a/**/c", "d/b/c", false),
            // - * + * is covered above
            // - * + **
            ("a*b**c", "abc", true),
            ("a*b**c", "aXbYc", true),
            ("a*b**c", "aXb/Yc", true),
            ("a*b**c", "aXbY/Yc", true),
            ("a*b**c", "aXb/Y/c", true),
            ("a*b**c", "a/XbYc", false),
            ("a*b**c", "aX/XbYc", false),
            ("a*b**c", "a/X/bYc", false),
            // - ** + *
            ("a**b*c", "abc", true),
            ("a**b*c", "aXbYc", true),
            ("a**b*c", "aXb/Yc", false),
            ("a**b*c", "aXbY/Yc", false),
            ("a**b*c", "aXb/Y/c", false),
            ("a**b*c", "a/XbYc", true),
            ("a**b*c", "aX/XbYc", true),
            ("a**b*c", "a/X/bYc", true),
            // - ** + **
            ("a**b**c", "abc", true),
            ("a**b**c", "aXbYc", true),
            ("a**b**c", "aXb/Yc", true),
            ("a**b**c", "aXbY/Yc", true),
            ("a**b**c", "aXb/Y/c", true),
            ("a**b**c", "aXbYc", true),
            ("a**b**c", "a/XbYc", true),
            ("a**b**c", "aX/XbYc", true),
            ("a**b**c", "a/X/bYc", true),
            // Case insensitivity
            ("*.txt", "file.TXT", true),
            ("**/*.rs", "dir/file.RS", true),
            // Optimized patterns: **/*.ext and **/name
            ("**/*.rs", "foo.rs", true),
            ("**/*.rs", "dir/foo.rs", true),
            ("**/*.rs", "dir/sub/foo.rs", true),
            ("**/*.rs", "foo.txt", false),
            ("**/*.rs", "dir/foo.txt", false),
            ("**/Cargo.toml", "Cargo.toml", true),
            ("**/Cargo.toml", "dir/Cargo.toml", true),
            ("**/Cargo.toml", "dir/sub/Cargo.toml", true),
            ("**/Cargo.toml", "Cargo.lock", false),
            ("**/Cargo.toml", "dir/Cargo.lock", false),
            // Character classes
            ("[abc]", "a", true),
            ("[abc]", "c", true),
            ("[abc]", "d", false),
            ("[abc]", "", false),
            ("[abc]", "ab", false),
            ("a[bc]d", "abd", true),
            ("a[bc]d", "acd", true),
            ("a[bc]d", "add", false),
            // - Ranges
            ("[a-c]", "b", true),
            ("[a-c]", "d", false),
            ("[0-9]", "5", true),
            ("[0-9]", "x", false),
            ("[a-cx-z]", "y", true),
            ("[a-cx-z]", "m", false),
            // - Negation
            ("[!abc]", "d", true),
            ("[!abc]", "a", false),
            ("[^a-c]", "d", true),
            ("[^a-c]", "b", false),
            // - Case insensitivity, matching the rest of the engine
            ("[a-z]", "Q", true),
            ("[A-Z]", "q", true),
            ("[abc]", "B", true),
            // - Literals in the corner positions
            ("[]a]", "]", true),
            ("[]a]", "a", true),
            ("[!]a]", "b", true),
            ("[!]a]", "]", false),
            ("[-a]", "-", true),
            ("[a-]", "-", true),
            ("[a-]", "a", true),
            // - Never crosses a path separator
            ("[!a]", "/", false),
            ("a[!x]c", "a/c", false),
            // - Unterminated class is a literal '['
            ("[abc", "[abc", true),
            ("[abc", "a", false),
            ("a[", "a[", true),
            // - Combined with stars, and the man-page glob that motivated this
            ("**/*.[1-9]", "foo.1", true),
            ("**/*.[1-9]", "dir/foo.8", true),
            ("**/*.[1-9]", "foo.0", false),
            ("**/*.[1-9]", "foo.10", false),
            ("**/*.[1-9]", "foo.rs", false),
            ("*[0-9]*", "a5b", true),
            ("*[0-9]*", "abc", false),
        ];

        for (pattern, name, expected) in tests {
            let result = glob_match(pattern, name);
            assert_eq!(
                result, expected,
                "test case ({:?}, {:?}, {}) failed, got {}",
                pattern, name, expected, result
            );
        }
    }
}
