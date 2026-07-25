//! Subprocess wrapper around `git` for the gutter-diff feature. No
//! libgit2; missing binary degrades to a graceful noop.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::{io, str};

#[derive(Debug, Clone)]
pub struct GitInfo {
    pub repo_root: PathBuf,
    pub rel_path: String,
}

/// Locate the git repository containing `path` and return its toplevel +
/// the path of `path` relative to that toplevel.
pub fn locate(path: &Path) -> io::Result<GitInfo> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let out = Command::new("git")
        .arg("-C")
        .arg(parent)
        .args(["rev-parse", "--show-toplevel"])
        .stderr(Stdio::null())
        .output()?;
    if !out.status.success() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "not a git repo"));
    }
    let root_str = str::from_utf8(&out.stdout)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "non-utf8 toplevel"))?
        .trim()
        .to_string();
    let repo_root = canonicalise(Path::new(&root_str))?;
    let abs = canonicalise(path)?;
    let rel = abs
        .strip_prefix(&repo_root)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "path outside repo"))?;
    let rel_path = rel
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "non-utf8 rel path"))?
        .to_string();
    Ok(GitInfo { repo_root, rel_path })
}

/// Read the baseline blob for `info`. Tries `HEAD:<rel>` first, then
/// `:<rel>` (the staged blob, useful for new-files-staged).
pub fn read_baseline(info: &GitInfo) -> io::Result<Vec<u8>> {
    if let Ok(bytes) = show_blob(&info.repo_root, &format!("HEAD:{}", info.rel_path)) {
        return Ok(bytes);
    }
    show_blob(&info.repo_root, &format!(":{}", info.rel_path))
}

fn show_blob(repo_root: &Path, spec: &str) -> io::Result<Vec<u8>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["show", spec])
        .stderr(Stdio::null())
        .output()?;
    if !out.status.success() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("{spec} missing")));
    }
    Ok(out.stdout)
}

fn canonicalise(p: &Path) -> io::Result<PathBuf> {
    // canonicalize resolves /var -> /private/var on macos so the strip_prefix
    // in locate() lines up.
    std::fs::canonicalize(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A path that cannot resolve. `locate` canonicalises both the file and
    /// the toplevel, so this fails whether or not `git` is on $PATH and
    /// whether or not the temp dir happens to sit inside a repository.
    fn missing_path() -> PathBuf {
        std::env::temp_dir().join(format!("gutter-no-such-file-{}", std::process::id()))
    }

    #[test]
    fn locating_a_missing_file_is_an_error_not_a_panic() {
        // The whole feature is best-effort: no repo, no git binary, or a path
        // outside the worktree all have to come back as a plain Err so the
        // caller can fall back to no marks.
        assert!(locate(&missing_path()).is_err());
    }

    #[test]
    fn a_baseline_for_a_missing_file_is_an_error() {
        let info = GitInfo {
            repo_root: std::env::temp_dir(),
            rel_path: format!("gutter-no-such-file-{}", std::process::id()),
        };
        assert!(read_baseline(&info).is_err());
    }

    #[test]
    fn loading_a_baseline_for_a_missing_file_yields_no_marks() {
        // The state constructor swallows every failure mode above; a `None`
        // here is what suppresses the margin rather than crashing the editor.
        let state = crate::gutter_diff::BaselineState::load(&missing_path());
        assert!(state.bytes.is_none());
    }

    #[test]
    fn this_repo_locates_and_has_a_baseline() {
        // Positive direction, skipped rather than failed when the environment
        // cannot provide it -- a source tarball with no .git, or no git binary.
        let here = Path::new(file!());
        if !here.exists() {
            return;
        }
        let Ok(info) = locate(here) else {
            return;
        };
        assert!(info.repo_root.is_absolute());
        assert!(info.rel_path.ends_with("git.rs"), "rel_path={}", info.rel_path);
        assert!(!info.rel_path.starts_with('/'));
        // This file is committed, so HEAD:<rel> resolves.
        let bytes = read_baseline(&info).expect("committed file has a baseline");
        assert!(bytes.starts_with(b"//!"), "baseline did not look like this file");
    }
}
