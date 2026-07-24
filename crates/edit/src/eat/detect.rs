//! Language detection for the eat persona.
//!
//! Same precedence everywhere: an explicit `-l` wins, then the path
//! globs (with content-based dialect disambiguation when several
//! definitions share one glob), then a shebang sniff, then a content
//! sniff. [`resolve_language`] is the one place that ordering lives.

use std::fs::File;
use std::io::Read as _;
use std::path::Path;

use lsh::runtime::Language;
use lsh_defs::FILE_ASSOCIATIONS;
use lsh_defs::detect::{
    disambiguate_language, find_language, language_from_shebang, match_file_associations,
};

/// Join the first 64 lines of a `Vec<String>` (BufRead::lines() strips `\n`)
/// back into a contiguous byte buffer suitable for the shared `lsh_defs::detect`
/// fns, which take `head: &[u8]`. Allocates -- only called once per file on the
/// detection path.
pub(crate) fn head_bytes(lines: &[String]) -> Vec<u8> {
    let take = lines.iter().take(64);
    let mut out = Vec::with_capacity(take.clone().map(|l| l.len() + 1).sum());
    for l in take {
        out.extend_from_slice(l.as_bytes());
        out.push(b'\n');
    }
    out
}

/// Resolve a language from a path using the same content-aware dialect
/// disambiguation the editor uses (`documents.rs`): collect every glob
/// candidate, then probe each dialect's `detect_entrypoint` against `head`.
/// `disambiguate_language` only reads `head` when more than one definition
/// shares the glob -- e.g. yaml + slice_yaml both on `**/*.yaml`. Returns
/// `None` if no glob matched, so callers chain shebang/content fallbacks.
pub(crate) fn detect_by_path(path: &Path, head: &[u8]) -> Option<&'static Language> {
    let candidates = match_file_associations(FILE_ASSOCIATIONS, path);
    if candidates.is_empty() {
        return None;
    }
    disambiguate_language(&candidates, head)
}

/// Read up to 4 KiB from the start of `path` for content-based language
/// detection. Best-effort: returns empty on any error.
pub(crate) fn read_head(path: &Path) -> Vec<u8> {
    let mut buf = vec![0u8; 4096];
    match File::open(path).and_then(|mut f| f.read(&mut buf)) {
        Ok(n) => {
            buf.truncate(n);
            buf
        }
        Err(_) => Vec::new(),
    }
}

/// Resolve the language for a path: explicit override, then globs, then
/// a shebang sniff over the file's head.
///
/// Returns `Err` with the unknown name when `-l` names a language that
/// does not exist -- callers turn that into a usage error.
pub(crate) fn resolve_language(
    path: &Path,
    override_name: Option<&str>,
) -> Result<Option<&'static Language>, String> {
    match override_name {
        Some(name) => find_language(name).map(Some).ok_or_else(|| name.to_string()),
        None => {
            let head = read_head(path);
            Ok(detect_by_path(path, &head).or_else(|| language_from_shebang(&head)))
        }
    }
}
