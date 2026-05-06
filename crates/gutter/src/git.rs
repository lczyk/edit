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
