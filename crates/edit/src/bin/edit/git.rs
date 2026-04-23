// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Thin wrapper around `git` used by diff-mode editing.
//!
//! Shells out to the `git` binary. No libgit2 dependency (binary-size rule).

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Location of a file within a git worktree.
pub struct GitInfo {
    /// Absolute path to the worktree root (from `rev-parse --show-toplevel`).
    pub repo_root: PathBuf,
    /// Path relative to `repo_root`, using forward slashes (git's preferred form).
    pub rel_path: String,
}

/// Locate the worktree `path` lives in. Returns `NotFound` if not in a repo
/// or if `git` is not on `PATH`.
pub fn locate(path: &Path) -> io::Result<GitInfo> {
    let cwd = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));

    let output = Command::new("git")
        .current_dir(cwd)
        .args(["rev-parse", "--show-toplevel"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;

    if !output.status.success() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "not a git worktree"));
    }

    let mut root_bytes = output.stdout;
    while matches!(root_bytes.last(), Some(b'\n' | b'\r')) {
        root_bytes.pop();
    }
    if root_bytes.is_empty() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "empty git toplevel"));
    }

    let repo_root = PathBuf::from(
        String::from_utf8(root_bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "non-utf8 toplevel"))?,
    );

    // Resolve symlinks on both sides: on macOS `git` returns paths under
    // `/private/var/...` while `std::env::temp_dir()` hands out `/var/...`.
    let repo_root_norm =
        std::fs::canonicalize(&repo_root).unwrap_or_else(|_| normalize(&repo_root));
    let abs_path = std::fs::canonicalize(path).unwrap_or_else(|_| {
        if path.is_absolute() {
            normalize(path)
        } else {
            normalize(&std::env::current_dir().unwrap_or_default().join(path))
        }
    });

    let rel = abs_path
        .strip_prefix(&repo_root_norm)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path outside worktree"))?;

    let rel_path = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    if rel_path.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "path is worktree root"));
    }

    Ok(GitInfo { repo_root: repo_root_norm, rel_path })
}

/// Returns `true` if the file is tracked (committed or in the index).
pub fn is_tracked(info: &GitInfo) -> io::Result<bool> {
    let status = Command::new("git")
        .current_dir(&info.repo_root)
        .args(["ls-files", "--error-unmatch", "--", &info.rel_path])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    Ok(status.success())
}

/// Reads the file's baseline content from the repo. Tries `HEAD:<rel>` first
/// (last committed version); falls back to `:<rel>` (stage-0 in the index)
/// if the file has been added but never committed.
pub fn read_baseline(info: &GitInfo) -> io::Result<Vec<u8>> {
    match show(&info.repo_root, &format!("HEAD:{}", info.rel_path)) {
        Ok(bytes) => Ok(bytes),
        Err(_) => show(&info.repo_root, &format!(":{}", info.rel_path)),
    }
}

fn show(cwd: &Path, spec: &str) -> io::Result<Vec<u8>> {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(["show", spec])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    if !output.status.success() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("git show {spec} failed")));
    }
    Ok(output.stdout)
}

/// Resolve `..` / `.` without touching the filesystem. `std::fs::canonicalize`
/// would work but requires the path to exist and follows symlinks; we just
/// want a lexical form good enough for `strip_prefix`.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let path = std::env::temp_dir().join(format!("edit-git-test-{tag}-{nanos}-{n}"));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
    }

    fn run_git(cwd: &Path, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(cwd)
            .args(args)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    fn init_repo(tag: &str) -> TempDir {
        let td = TempDir::new(tag);
        run_git(td.path(), &["init", "-q", "-b", "main"]);
        td
    }

    #[test]
    fn locate_outside_repo_errors() {
        if !git_available() {
            return;
        }
        let td = TempDir::new("outside");
        let path = td.path().join("nope.txt");
        fs::write(&path, b"x").unwrap();
        assert!(locate(&path).is_err());
    }

    #[test]
    fn locate_finds_root_and_relpath() {
        if !git_available() {
            return;
        }
        let td = init_repo("locate");
        let sub = td.path().join("a/b");
        fs::create_dir_all(&sub).unwrap();
        let file = sub.join("c.txt");
        fs::write(&file, b"hello\n").unwrap();

        let info = locate(&file).unwrap();
        assert_eq!(info.repo_root, fs::canonicalize(td.path()).unwrap());
        assert_eq!(info.rel_path, "a/b/c.txt");
    }

    #[test]
    fn is_tracked_true_after_add() {
        if !git_available() {
            return;
        }
        let td = init_repo("tracked");
        let file = td.path().join("x.txt");
        fs::write(&file, b"hi\n").unwrap();
        let info = locate(&file).unwrap();
        assert!(!is_tracked(&info).unwrap());
        run_git(td.path(), &["add", "x.txt"]);
        assert!(is_tracked(&info).unwrap());
    }

    #[test]
    fn read_baseline_head() {
        if !git_available() {
            return;
        }
        let td = init_repo("head");
        let file = td.path().join("x.txt");
        fs::write(&file, b"one\ntwo\n").unwrap();
        run_git(td.path(), &["add", "x.txt"]);
        run_git(td.path(), &["commit", "-q", "-m", "init"]);

        // Modify after commit; baseline must still be the committed version.
        fs::write(&file, b"ONE\ntwo\nthree\n").unwrap();

        let info = locate(&file).unwrap();
        let bytes = read_baseline(&info).unwrap();
        assert_eq!(bytes, b"one\ntwo\n");
    }

    #[test]
    fn read_baseline_falls_back_to_index() {
        if !git_available() {
            return;
        }
        let td = init_repo("index");
        // One committed file so HEAD exists.
        let seed = td.path().join("seed.txt");
        fs::write(&seed, b"seed\n").unwrap();
        run_git(td.path(), &["add", "seed.txt"]);
        run_git(td.path(), &["commit", "-q", "-m", "seed"]);

        // New file: added to index but never committed. HEAD:x.txt will fail,
        // so read_baseline should fall back to :x.txt.
        let file = td.path().join("x.txt");
        fs::write(&file, b"staged\n").unwrap();
        run_git(td.path(), &["add", "x.txt"]);
        // Dirty the worktree to confirm baseline is not the worktree version.
        fs::write(&file, b"worktree\n").unwrap();

        let info = locate(&file).unwrap();
        let bytes = read_baseline(&info).unwrap();
        assert_eq!(bytes, b"staged\n");
    }

    #[test]
    fn read_baseline_untracked_errors() {
        if !git_available() {
            return;
        }
        let td = init_repo("untracked");
        // Empty repo, no HEAD yet. Untracked file should error on both paths.
        let file = td.path().join("x.txt");
        fs::write(&file, b"nope\n").unwrap();
        let info = locate(&file).unwrap();
        assert!(read_baseline(&info).is_err());
    }
}
