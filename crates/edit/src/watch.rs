//! Poll-based file-change detection.
//!
//! No inotify -- every consumer here already has a tick (the editor's 2s
//! disk poll, eat's follow interval), so a `stat` per tick is cheaper than
//! carrying a watcher.
//!
//! Two questions get asked of the same data, which is why one type answers
//! both:
//!
//! - *did this file change at all?* -- compare two [`FileStat`]s. Used by
//!   the editor's modified-on-disk flag and eat's snapshot view.
//! - *what kind of change?* -- [`classify`]. Appends can be drained
//!   incrementally; rotations force a reload. Used by both follow paths.
//!
//! Detecting rotation from size and inode alone is not enough: a
//! `> file` rewrite that lands on the same inode at the same length looks
//! identical to no change at all. [`classify`] therefore takes a
//! head-bytes verdict from the caller, who samples
//! [`HEAD_FINGERPRINT_BYTES`] at offset 0.

use std::fs::Metadata;
use std::io;
use std::path::Path;

/// Bytes sampled at offset 0 to detect in-place rewrites that leave the
/// file at (or near) its previous size. 256 fits in one block on every
/// filesystem we care about, so the read is essentially free.
pub const HEAD_FINGERPRINT_BYTES: usize = 256;

/// What a single `stat` saw. `id` is the inode on unix and 0 elsewhere,
/// where rotation is detected by size and content alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStat {
    pub size: u64,
    pub id: u64,
    /// Nanoseconds since the unix epoch. The cheap "did anything happen"
    /// gate before the head-fingerprint check.
    pub mtime_ns: i128,
}

#[cfg(unix)]
pub fn id_of(meta: &Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt as _;
    meta.ino()
}

#[cfg(not(unix))]
pub fn id_of(_meta: &Metadata) -> u64 {
    0
}

impl FileStat {
    pub fn from_metadata(meta: &Metadata) -> Self {
        let mtime_ns = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as i128)
            .unwrap_or(0);
        Self { size: meta.len(), id: id_of(meta), mtime_ns }
    }

    pub fn from_path(path: &Path) -> io::Result<Self> {
        Ok(Self::from_metadata(&std::fs::metadata(path)?))
    }

    /// Cheap pre-filter: is anything at all different? Callers use this to
    /// skip the head read on the overwhelmingly common idle tick.
    pub fn differs(&self, other: &Self) -> bool {
        self != other
    }
}

/// What changed between two [`FileStat`]s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileDelta {
    /// Nothing actionable. Includes a bare `touch`: mtime moved but size,
    /// identity and head bytes are all unchanged.
    Idle,
    /// Pure append. Everything before `from` is unchanged, so a consumer
    /// can read just `from..size`.
    Appended { from: u64 },
    /// Rotated, truncated, or rewritten in place. The previous contents
    /// are gone; reload from offset 0.
    Rotated,
}

/// Classify a stat transition.
///
/// `head_changed` is the caller's verdict on whether the first
/// [`HEAD_FINGERPRINT_BYTES`] differ, compared over the *common prefix*
/// of the old and new samples -- see [`head_changed`]. Callers may pass
/// `false` without sampling only when [`FileStat::differs`] was false,
/// in which case the result is [`FileDelta::Idle`] regardless.
///
/// The traps this defuses, in order:
///
/// - inode change or shrink -- classic rotation.
/// - nothing changed -- idle, the cheap fast path.
/// - mtime moved, size and identity static -- an in-place rewrite of the
///   same length, which only the head bytes can reveal.
/// - mtime moved and size grew -- usually a true append, but
///   "truncate, then write more than was there" also grows. The head
///   bytes disambiguate.
pub fn classify(prev: &FileStat, curr: &FileStat, head_changed: bool) -> FileDelta {
    if curr.id != prev.id || curr.size < prev.size || head_changed {
        return FileDelta::Rotated;
    }
    if curr.size > prev.size {
        return FileDelta::Appended { from: prev.size };
    }
    FileDelta::Idle
}

/// Compare two head samples over their common prefix.
///
/// Comparing the full window would false-positive on a small file that
/// grew: appending widens the sample, but the bytes inside the original
/// window are untouched. Only the overlap carries a verdict.
pub fn head_changed(prev: &[u8], curr: &[u8]) -> bool {
    let cmp_len = prev.len().min(curr.len());
    prev[..cmp_len] != curr[..cmp_len]
}

/// Read up to [`HEAD_FINGERPRINT_BYTES`] from offset 0. Best-effort:
/// a missing or unreadable file yields an empty sample, which compares
/// equal to anything and so reads as "no verdict".
pub fn read_head(path: &Path) -> Vec<u8> {
    use std::io::Read as _;
    let mut buf = Vec::with_capacity(HEAD_FINGERPRINT_BYTES);
    if let Ok(f) = std::fs::File::open(path) {
        let _ = f.take(HEAD_FINGERPRINT_BYTES as u64).read_to_end(&mut buf);
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stat(size: u64, id: u64, mtime_ns: i128) -> FileStat {
        FileStat { size, id, mtime_ns }
    }

    #[test]
    fn identical_stats_are_idle() {
        let s = stat(100, 7, 1);
        assert_eq!(classify(&s, &s, false), FileDelta::Idle);
        assert!(!s.differs(&s));
    }

    #[test]
    fn growth_is_an_append_from_the_old_size() {
        let a = stat(100, 7, 1);
        let b = stat(180, 7, 2);
        assert_eq!(classify(&a, &b, false), FileDelta::Appended { from: 100 });
    }

    #[test]
    fn shrink_is_rotation() {
        assert_eq!(classify(&stat(100, 7, 1), &stat(10, 7, 2), false), FileDelta::Rotated);
    }

    #[test]
    fn inode_change_is_rotation_even_at_the_same_size() {
        assert_eq!(classify(&stat(100, 7, 1), &stat(100, 8, 2), false), FileDelta::Rotated);
    }

    #[test]
    fn same_size_rewrite_is_rotation_only_via_the_head() {
        // The case size+inode alone cannot see: `> file` with the same
        // number of bytes. Without the head verdict it reads as idle.
        let a = stat(100, 7, 1);
        let b = stat(100, 7, 2);
        assert_eq!(classify(&a, &b, false), FileDelta::Idle);
        assert_eq!(classify(&a, &b, true), FileDelta::Rotated);
    }

    #[test]
    fn truncate_then_write_more_is_rotation_not_append() {
        // Grew, so it looks like an append -- but the head changed, so the
        // old contents are gone and draining from the old size would splice
        // new bytes onto a stale prefix.
        let a = stat(100, 7, 1);
        let b = stat(150, 7, 2);
        assert_eq!(classify(&a, &b, true), FileDelta::Rotated);
        assert_eq!(classify(&a, &b, false), FileDelta::Appended { from: 100 });
    }

    #[test]
    fn touch_alone_is_idle() {
        // mtime moved, nothing else did.
        let a = stat(100, 7, 1);
        let b = stat(100, 7, 999);
        assert!(a.differs(&b), "mtime alone still trips the cheap gate");
        assert_eq!(classify(&a, &b, false), FileDelta::Idle);
    }

    #[test]
    fn head_compare_uses_the_common_prefix() {
        // A small file that grew: the sample widens but the overlap is
        // unchanged, so this must not read as a rewrite.
        assert!(!head_changed(b"abc", b"abcdef"));
        assert!(head_changed(b"abc", b"xbcdef"));
        // An empty sample carries no verdict either way.
        assert!(!head_changed(b"", b"anything"));
        assert!(!head_changed(b"anything", b""));
    }
}
