//! `eat -f` -- follow a file as it grows and emit highlighted lines as they
//! arrive (mirrors `tail -F`: rotation / truncation resets and reprints).
//!
//! polling-based: every `poll_interval`, stat the path; on size growth read
//! the new bytes and feed completed lines through the syntax `Runtime`. on
//! size shrink or inode change, treat as rotation -- drop the runtime, build
//! a fresh one, and reprint from offset 0. tail -f inode-stickiness is
//! deliberately not offered; rotation-recovery is the useful default.
//!
//! the polling loop is split into a pure `tick` that performs one stat+read
//! pass, and a thin outer driver that owns the clock and the stdout lock.
//! tests instantiate `tick` directly against an in-memory `FollowSource`
//! and a `Vec<u8>` writer.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use lsh::runtime::{Language, Runtime};

use super::stream::write_highlighted_line;
use crate::watch::{self, FileDelta};
use lsh_defs::{ASSEMBLY, CHARSETS, STRINGS};

/// fixed line-number column width in follow mode. real width is unknowable
/// (file is unbounded); 6 fits up to 999_999 lines without wrapping, and the
/// column is mostly cosmetic anyway. used to seed `Gutter::compute(...,
/// FOLLOW_NUM_WIDTH)` so the column doesn't shift as the file grows.
pub const FOLLOW_NUM_WIDTH: usize = 6;

/// how many consecutive failed stats we tolerate before giving up. at the
/// default 250ms poll, 20 misses = 5s. covers brief logrotate windows.
pub(crate) const DEFAULT_MISS_BUDGET: u32 = 20;

/// what a single stat call sees. shared with the tui follow view via
/// [`crate::watch`] so both agree on what counts as a rotation.
pub use crate::watch::{FileStat as FollowStat, HEAD_FINGERPRINT_BYTES};

/// abstraction over the file being followed, so tests can drive ticks
/// against an in-memory buffer w/out touching the filesystem.
pub trait FollowSource {
    fn stat(&self) -> io::Result<FollowStat>;
    /// read the byte range `[offset, EOF)` and append into `buf`.
    fn read_from(&mut self, offset: u64, buf: &mut Vec<u8>) -> io::Result<()>;
    /// read up to `HEAD_FINGERPRINT_BYTES` bytes starting at offset 0.
    /// returning fewer bytes (eof) is fine; readers compare what's there.
    fn read_head(&mut self, buf: &mut Vec<u8>) -> io::Result<()> {
        // default impl in terms of read_from + truncation; fine for both
        // FileSource and the test MemSource.
        let mut tmp = Vec::with_capacity(HEAD_FINGERPRINT_BYTES);
        self.read_from(0, &mut tmp)?;
        tmp.truncate(HEAD_FINGERPRINT_BYTES);
        buf.extend_from_slice(&tmp);
        Ok(())
    }
}

/// abstraction over `thread::sleep` for tests. the production loop owns the
/// real clock; tests bypass the loop and call `tick` directly, so this trait
/// is currently used only by `RealClock`.
pub trait Clock {
    fn sleep(&self, d: Duration);
}

pub struct RealClock;
impl Clock for RealClock {
    fn sleep(&self, d: Duration) {
        thread::sleep(d);
    }
}

pub struct FileSource {
    path: PathBuf,
}

impl FileSource {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

#[cfg(unix)]
fn id_of(meta: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.ino()
}

#[cfg(not(unix))]
fn id_of(_meta: &std::fs::Metadata) -> u64 {
    0
}

impl FollowSource for FileSource {
    fn stat(&self) -> io::Result<FollowStat> {
        let m = std::fs::metadata(&self.path)?;
        let mtime_ns = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as i128)
            .unwrap_or(0);
        Ok(FollowStat { size: m.len(), id: id_of(&m), mtime_ns })
    }

    fn read_from(&mut self, offset: u64, buf: &mut Vec<u8>) -> io::Result<()> {
        let mut f = File::open(&self.path)?;
        if offset > 0 {
            f.seek(SeekFrom::Start(offset))?;
        }
        f.read_to_end(buf)?;
        Ok(())
    }

    fn read_head(&mut self, buf: &mut Vec<u8>) -> io::Result<()> {
        let f = File::open(&self.path)?;
        let mut take = f.take(HEAD_FINGERPRINT_BYTES as u64);
        take.read_to_end(buf)?;
        Ok(())
    }
}

/// per-loop state. keeping it separate from the runtime lets `tick` reset
/// the runtime in-place on rotation w/out re-allocating this struct.
pub struct FollowState {
    /// last seen stat. `None` before the first successful stat (initial bulk
    /// emission is modelled as "first tick from offset 0").
    pub last: Option<FollowStat>,
    /// the file's first `HEAD_FINGERPRINT_BYTES` bytes (or fewer if the file
    /// is smaller) at the time of the last successful read. used to detect
    /// in-place rewrites that don't shrink the file (and so wouldn't trip
    /// the size-shrink or id-change rotation triggers).
    ///
    /// stored as raw bytes rather than a hash so `tick` can compare over the
    /// `min(prev_len, curr_len)` window -- an append on a tiny file extends
    /// the head, but the leading bytes are unchanged, so we'd see a "no-
    /// change" on the prefix-length match.
    pub head_bytes: Vec<u8>,
    /// trailing bytes from the previous read that did not end in `\n`. we
    /// withhold these from the highlighter b/c emitting then re-emitting a
    /// line as it grows would corrupt output.
    pub partial: Vec<u8>,
    /// 1-based line counter for the line-number column.
    pub line_no: usize,
    /// consecutive failed stats since the last success.
    pub miss_count: u32,
    pub miss_budget: u32,
}

impl FollowState {
    pub fn new(miss_budget: u32) -> Self {
        Self {
            last: None,
            head_bytes: Vec::new(),
            partial: Vec::new(),
            line_no: 1,
            miss_count: 0,
            miss_budget,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum TickOutcome {
    /// no change since the previous tick.
    Idle,
    /// `n` complete lines were emitted as appends.
    Wrote(usize),
    /// rotation/truncation detected: runtime + state were reset and `n`
    /// lines were emitted from the new offset 0.
    Reset(usize),
    /// stat failed for more than `miss_budget` consecutive ticks.
    GoneTooLong,
}

/// drive one poll iteration: stat, branch on size/id, optionally read+emit.
/// caller owns sleeping between ticks. `runtime_entrypoint` rebuilds the
/// runtime from scratch when the file rotates. `gutter`, when `Some`,
/// prepends the line-number column + diff separator; `None` keeps the
/// historical bare-line output.
#[allow(clippy::too_many_arguments)]
pub fn tick<S: FollowSource>(
    state: &mut FollowState,
    src: &mut S,
    runtime: &mut Runtime<'static, 'static, 'static>,
    runtime_entrypoint: u32,
    color_map: &[&str],
    gutter: Option<&super::gutter_view::Gutter>,
    use_color: bool,
    writer: &mut dyn Write,
) -> io::Result<TickOutcome> {
    let stat = match src.stat() {
        Ok(s) => {
            state.miss_count = 0;
            s
        }
        Err(_) => {
            state.miss_count += 1;
            if state.miss_count > state.miss_budget {
                return Ok(TickOutcome::GoneTooLong);
            }
            return Ok(TickOutcome::Idle);
        }
    };

    let prev = state.last;
    state.last = Some(stat);

    // first tick: there's nothing to compare against. read everything from
    // offset 0 and report it as a Wrote (not Reset -- nothing to "reset").
    let Some(p) = prev else {
        state.head_bytes.clear();
        if stat.size > 0 {
            src.read_head(&mut state.head_bytes)?;
        }
        let (rotated, read_offset) = (false, 0u64);
        return finish_tick(
            state,
            src,
            runtime,
            runtime_entrypoint,
            color_map,
            gutter,
            use_color,
            writer,
            rotated,
            read_offset,
        );
    };

    // mtime is the cheap "did anything happen" gate. some filesystems update
    // mtime on touch w/out content changes; we still defer to the fingerprint
    // before reacting.
    if !stat.differs(&p) {
        return Ok(TickOutcome::Idle);
    }

    // sample the current head so `classify` can tell an append from an
    // in-place rewrite. see `watch::head_changed` for why the comparison is
    // over the common prefix rather than the full window.
    let mut curr_head = Vec::with_capacity(HEAD_FINGERPRINT_BYTES);
    if stat.size > 0 {
        src.read_head(&mut curr_head)?;
    }
    let head_changed = watch::head_changed(&state.head_bytes, &curr_head);
    state.head_bytes.clear();
    state.head_bytes.extend_from_slice(&curr_head);

    let (rotated, read_offset) = match watch::classify(&p, &stat, head_changed) {
        FileDelta::Rotated => (true, 0u64),
        FileDelta::Appended { from } => (false, from),
        FileDelta::Idle => return Ok(TickOutcome::Idle),
    };

    // Reading past EOF yields nothing, and a follow that reports Idle forever
    // looks like a quiet file rather than a bug. A shrink should have been
    // classified as a rotation and reset the offset to 0.
    crate::sanity_check!(
        follow_read_offset_within_file,
        read_offset <= stat.size,
        "read_offset={} size={} (prev size={}, rotated={})",
        read_offset,
        stat.size,
        p.size,
        rotated
    );

    finish_tick(
        state,
        src,
        runtime,
        runtime_entrypoint,
        color_map,
        gutter,
        use_color,
        writer,
        rotated,
        read_offset,
    )
}

/// the read-and-emit half of `tick`, factored out so the first-tick branch
/// can share it w/out duplicating the runtime-reset / partial-buffer dance.
#[allow(clippy::too_many_arguments)]
fn finish_tick<S: FollowSource>(
    state: &mut FollowState,
    src: &mut S,
    runtime: &mut Runtime<'static, 'static, 'static>,
    runtime_entrypoint: u32,
    color_map: &[&str],
    gutter: Option<&super::gutter_view::Gutter>,
    use_color: bool,
    writer: &mut dyn Write,
    rotated: bool,
    read_offset: u64,
) -> io::Result<TickOutcome> {
    // Line numbering restarts on rotation and only ever counts up otherwise, so
    // a non-rotated tick that rewinds it means a reset happened without the
    // rotation being detected -- the in-place-rewrite class.
    #[cfg(feature = "sanity")]
    let line_no_before = state.line_no;

    // on rotation, drop accumulated runtime state + partial line + numbering.
    if rotated {
        state.partial.clear();
        state.line_no = 1;
        *runtime = Runtime::new(&ASSEMBLY, &STRINGS, &CHARSETS, runtime_entrypoint);
    }

    let mut buf = Vec::new();
    src.read_from(read_offset, &mut buf)?;
    if buf.is_empty() {
        return Ok(if rotated { TickOutcome::Reset(0) } else { TickOutcome::Idle });
    }

    // splice partial-from-last-tick onto the front (only meaningful for
    // appends; for rotation `partial` was just cleared).
    let mut all = std::mem::take(&mut state.partial);
    all.extend_from_slice(&buf);

    let mut emitted = emit_lines(
        &all,
        runtime,
        color_map,
        gutter,
        use_color,
        writer,
        &mut state.line_no,
        &mut state.partial,
    )?;

    // reading from offset 0 means this is a fresh read of the file (first
    // tick or post-rotation). any trailing bytes without a newline are the
    // file's last line, not a mid-line append we should buffer. flush them.
    if read_offset == 0 && !state.partial.is_empty() {
        let cow = String::from_utf8_lossy(&state.partial);
        write_highlighted_line(writer, runtime, color_map, state.line_no, &cow, gutter, use_color)?;
        state.line_no += 1;
        emitted += 1;
        state.partial.clear();
    }

    #[cfg(feature = "sanity")]
    crate::sanity_check!(
        follow_line_numbering_monotonic,
        rotated || state.line_no >= line_no_before,
        "line_no went {line_no_before} -> {} without a rotation",
        state.line_no
    );

    writer.flush()?;
    Ok(if rotated { TickOutcome::Reset(emitted) } else { TickOutcome::Wrote(emitted) })
}

/// scan `all` for `\n`-terminated lines, write each through the highlighter,
/// and put any trailing partial (no-newline) bytes into `partial_out`.
/// returns the number of complete lines emitted.
#[allow(clippy::too_many_arguments)]
fn emit_lines(
    all: &[u8],
    runtime: &mut Runtime<'static, 'static, 'static>,
    color_map: &[&str],
    gutter: Option<&super::gutter_view::Gutter>,
    use_color: bool,
    writer: &mut dyn Write,
    line_no: &mut usize,
    partial_out: &mut Vec<u8>,
) -> io::Result<usize> {
    let mut emitted = 0usize;
    let mut start = 0usize;
    let mut i = 0usize;
    while i < all.len() {
        if all[i] == b'\n' {
            let mut end = i;
            // strip trailing CR for crlf inputs.
            if end > start && all[end - 1] == b'\r' {
                end -= 1;
            }
            let cow = String::from_utf8_lossy(&all[start..end]);
            write_highlighted_line(writer, runtime, color_map, *line_no, &cow, gutter, use_color)?;
            *line_no += 1;
            emitted += 1;
            start = i + 1;
        }
        i += 1;
    }
    partial_out.clear();
    partial_out.extend_from_slice(&all[start..]);
    Ok(emitted)
}

/// production driver: open the file, detect language (via callback so we
/// don't depend on lib.rs's private detection helpers), then loop forever
/// calling `tick`. exits on `GoneTooLong` or io error.
pub fn run(
    path: PathBuf,
    lang: &'static Language,
    show_numbers: bool,
    use_color: bool,
    poll_interval: Duration,
) -> io::Result<()> {
    let mut src = FileSource::new(path.clone());
    // eager stat: surface "no such file" / permission errors immediately
    // rather than burning the miss budget.
    src.stat()?;
    let color_map = super::theme::color_map();
    let mut runtime = Runtime::new(&ASSEMBLY, &STRINGS, &CHARSETS, lang.entrypoint);

    // build the gutter once at startup; appended lines past the initial
    // marks vec just get GutterMark::None in their gutter prefix. for the
    // streaming follow path we don't reissue past lines, so there's no point
    // recomputing on every tick.
    let gutter = if show_numbers {
        std::fs::read(&path)
            .ok()
            .map(|bytes| super::gutter_view::Gutter::compute(&path, &bytes, FOLLOW_NUM_WIDTH))
    } else {
        None
    };

    let stdout = io::stdout();
    let mut w = stdout.lock();

    let mut state = FollowState::new(DEFAULT_MISS_BUDGET);
    let clock = RealClock;

    loop {
        let outcome = tick(
            &mut state,
            &mut src,
            &mut runtime,
            lang.entrypoint,
            &color_map,
            gutter.as_ref(),
            use_color,
            &mut w,
        )?;
        if let TickOutcome::GoneTooLong = outcome {
            return Err(io::Error::new(io::ErrorKind::NotFound, "followed file disappeared"));
        }
        clock.sleep(poll_interval);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// in-memory `FollowSource`. exposes the moves a real filesystem can
    /// produce so we can drive `tick` against scripted scenarios:
    ///   - `append`: bytes added at the end (mtime advances).
    ///   - `rewrite_in_place`: content replaced w/out changing id (the bug
    ///     class the user hit -- `> file` from a shell).
    ///   - `replace`: content + id both replaced (atomic rename / logrotate).
    ///   - `truncate`: shrink to empty w/out changing id.
    ///   - `touch`: bump mtime, content unchanged (real `touch` from the cli).
    struct MemSource {
        content: Vec<u8>,
        id: u64,
        mtime_ns: i128,
        stat_fail: u32,
    }

    impl MemSource {
        fn new() -> Self {
            Self { content: Vec::new(), id: 1, mtime_ns: 1_000_000_000, stat_fail: 0 }
        }

        fn append(&mut self, bytes: &[u8]) {
            self.content.extend_from_slice(bytes);
            self.mtime_ns += 1_000_000;
        }

        fn rewrite_in_place(&mut self, bytes: &[u8]) {
            self.content.clear();
            self.content.extend_from_slice(bytes);
            self.mtime_ns += 1_000_000;
            // id stays the same -- truncate(2) + write(2) doesn't change inode.
        }

        fn replace(&mut self, bytes: &[u8]) {
            self.content.clear();
            self.content.extend_from_slice(bytes);
            self.id += 1;
            self.mtime_ns += 1_000_000;
        }

        fn truncate(&mut self) {
            self.content.clear();
            self.mtime_ns += 1_000_000;
        }

        fn touch(&mut self) {
            self.mtime_ns += 1_000_000;
        }
    }

    impl FollowSource for MemSource {
        fn stat(&self) -> io::Result<FollowStat> {
            if self.stat_fail > 0 {
                return Err(io::ErrorKind::NotFound.into());
            }
            Ok(FollowStat { size: self.content.len() as u64, id: self.id, mtime_ns: self.mtime_ns })
        }

        fn read_from(&mut self, offset: u64, buf: &mut Vec<u8>) -> io::Result<()> {
            let off = offset as usize;
            if off > self.content.len() {
                return Ok(());
            }
            buf.extend_from_slice(&self.content[off..]);
            Ok(())
        }
    }

    /// a plain-text runtime: raw line pass-through, so the loop logic can be
    /// asserted w/out caring about ansi escapes.
    fn plain() -> Runtime<'static, 'static, 'static> {
        Runtime::new(&ASSEMBLY, &STRINGS, &CHARSETS, lsh_defs::PLAIN.entrypoint)
    }

    fn step_plain(state: &mut FollowState, src: &mut MemSource, out: &mut Vec<u8>) -> TickOutcome {
        tick(state, src, &mut plain(), 0, &[], None, false, out).unwrap()
    }

    fn s(out: &[u8]) -> String {
        String::from_utf8(out.to_vec()).unwrap()
    }

    #[test]
    fn first_tick_emits_existing_complete_lines() {
        let mut src = MemSource::new();
        src.append(b"alpha\nbeta\n");
        let mut state = FollowState::new(20);
        let mut out = Vec::new();

        let r = step_plain(&mut state, &mut src, &mut out);
        assert_eq!(r, TickOutcome::Wrote(2));
        assert_eq!(s(&out), "alpha\nbeta\n");
        assert!(state.partial.is_empty());
        assert_eq!(state.line_no, 3);
    }

    #[test]
    fn first_tick_flushes_trailing_partial_as_last_line() {
        // file with no trailing newline: "hello\nhello"
        let mut src = MemSource::new();
        src.append(b"hello\nhello");
        let mut state = FollowState::new(20);
        let mut out = Vec::new();

        let r = step_plain(&mut state, &mut src, &mut out);
        assert_eq!(r, TickOutcome::Wrote(2));
        assert_eq!(s(&out), "hello\nhello\n");
        assert!(state.partial.is_empty());
        assert_eq!(state.line_no, 3);
    }

    #[test]
    fn partial_line_is_buffered_until_newline() {
        let mut src = MemSource::new();
        // seed with a complete line so the next tick reads from offset > 0
        // (mid-line append semantics, not initial read semantics).
        src.append(b"first\n");
        let mut state = FollowState::new(20);
        let mut out = Vec::new();

        step_plain(&mut state, &mut src, &mut out);
        out.clear();

        // append a partial line (no newline) -- should be buffered.
        src.append(b"hello, ");
        let r = step_plain(&mut state, &mut src, &mut out);
        assert_eq!(r, TickOutcome::Wrote(0));
        assert_eq!(out, b"");
        assert_eq!(state.partial, b"hello, ");

        // append the rest of the line -- partial + new bytes form one full line.
        src.append(b"world\n");
        let r = step_plain(&mut state, &mut src, &mut out);
        assert_eq!(r, TickOutcome::Wrote(1));
        assert_eq!(s(&out), "hello, world\n");
        assert!(state.partial.is_empty());
    }

    #[test]
    fn idle_when_nothing_changes() {
        let mut src = MemSource::new();
        src.append(b"line\n");
        let mut state = FollowState::new(20);
        let mut out = Vec::new();

        step_plain(&mut state, &mut src, &mut out);
        out.clear();

        // second tick with no changes
        let r = step_plain(&mut state, &mut src, &mut out);
        assert_eq!(r, TickOutcome::Idle);
        assert!(out.is_empty());
    }

    #[test]
    fn truncation_resets_and_re_emits() {
        let mut src = MemSource::new();
        src.append(b"old1\nold2\n");
        let mut state = FollowState::new(20);
        let mut out = Vec::new();

        step_plain(&mut state, &mut src, &mut out);
        assert_eq!(state.line_no, 3);
        out.clear();

        // truncate (size shrinks, id unchanged) -- treated as rotation.
        src.truncate();
        src.append(b"new\n");
        let r = step_plain(&mut state, &mut src, &mut out);
        assert_eq!(r, TickOutcome::Reset(1));
        assert_eq!(s(&out), "new\n");
        assert_eq!(state.line_no, 2); // reset to 1, then incremented
    }

    #[test]
    fn rotation_with_same_size_via_id_change_resets() {
        let mut src = MemSource::new();
        src.append(b"aaaa\n"); // 5 bytes
        let mut state = FollowState::new(20);
        let mut out = Vec::new();

        step_plain(&mut state, &mut src, &mut out);
        out.clear();

        // replace with same byte count but different id (logrotate move-and-recreate
        // where the new file happens to land at the same size by tick time).
        src.replace(b"bbbb\n"); // 5 bytes, id bumped
        let r = step_plain(&mut state, &mut src, &mut out);
        assert_eq!(r, TickOutcome::Reset(1));
        assert_eq!(s(&out), "bbbb\n");
    }

    // --- the bug: in-place rewrites that don't shrink + don't change id ---
    // these are the cases where a shell `> file` from another terminal lands
    // either at the same size or grows the file. classic id-based rotation
    // detection misses them entirely; we use the head-bytes fingerprint to
    // catch them.

    #[test]
    fn in_place_rewrite_same_size_resets() {
        // shell `printf 'AAAAA\n' > file` after the file was 'BBBBB\n'.
        // size identical (6 bytes), id identical, mtime advances. without
        // the head fingerprint this would be Idle -- the user's "no update"
        // bug.
        let mut src = MemSource::new();
        src.append(b"BBBBB\n");
        let mut state = FollowState::new(20);
        let mut out = Vec::new();

        step_plain(&mut state, &mut src, &mut out);
        out.clear();

        src.rewrite_in_place(b"AAAAA\n");
        let r = step_plain(&mut state, &mut src, &mut out);
        assert_eq!(r, TickOutcome::Reset(1));
        assert_eq!(s(&out), "AAAAA\n");
    }

    #[test]
    fn in_place_rewrite_growing_size_resets() {
        // grew from 6 -> 13 bytes. without the head fingerprint we'd see
        // grew=true and treat as append from offset 6, reading "world\n"
        // into the view -- the "AAAAA" prefix would still be on screen
        // pretending to be the file's first line. with the fingerprint we
        // see that bytes [0..6) changed and treat it as rotation.
        let mut src = MemSource::new();
        src.append(b"AAAAA\n");
        let mut state = FollowState::new(20);
        let mut out = Vec::new();

        step_plain(&mut state, &mut src, &mut out);
        out.clear();

        src.rewrite_in_place(b"hello world\n");
        let r = step_plain(&mut state, &mut src, &mut out);
        assert_eq!(r, TickOutcome::Reset(1));
        assert_eq!(s(&out), "hello world\n");
    }

    #[test]
    fn in_place_rewrite_keeping_same_prefix_is_append() {
        // tricky case: rewrite that happens to share the first N bytes with
        // the previous content. since we only sample `min(prev, curr)` bytes,
        // this LOOKS like an append. acceptable trade-off -- catching this
        // would require comparing the full prev content, which defeats the
        // point of a fixed-cost head check. document the behaviour so a
        // future reader doesn't try to "fix" it.
        let mut src = MemSource::new();
        src.append(b"hello, "); // no trailing newline
        let mut state = FollowState::new(20);
        let mut out = Vec::new();

        // first tick: read_offset=0, so trailing partial "hello, " is flushed
        // as a complete line (it's the file's last line, not a mid-line append).
        let r = step_plain(&mut state, &mut src, &mut out);
        assert_eq!(r, TickOutcome::Wrote(1));
        assert_eq!(s(&out), "hello, \n");
        assert!(state.partial.is_empty());
        out.clear();

        // rewrite shares prefix "hello, " -- head fingerprint match treats it
        // as append from offset 7, so we only get "world\n" (not the full
        // "hello, world\n"). the terminal sees two lines: "hello, " + "world\n".
        src.rewrite_in_place(b"hello, world\n");
        let r = step_plain(&mut state, &mut src, &mut out);
        assert_eq!(r, TickOutcome::Wrote(1));
        assert_eq!(s(&out), "world\n");
    }

    #[test]
    fn touch_is_idle() {
        // touch-style mtime bump on an unchanged file. mtime moves, size
        // and head are stable -> Idle.
        let mut src = MemSource::new();
        src.append(b"alpha\nbeta\n");
        let mut state = FollowState::new(20);
        let mut out = Vec::new();

        step_plain(&mut state, &mut src, &mut out);
        out.clear();

        src.touch();
        let r = step_plain(&mut state, &mut src, &mut out);
        assert_eq!(r, TickOutcome::Idle);
        assert!(out.is_empty());
    }

    #[test]
    fn rewrite_then_append_keeps_state_consistent() {
        // walk a multi-step scenario:
        //   t0: file = "a\nb\n"          -- bulk emit
        //   t1: rewrite to "X\nY\n"      -- Reset
        //   t2: append "Z\n"             -- Wrote
        //   t3: no change                -- Idle
        //   t4: rewrite to "P\n"         -- Reset
        let mut src = MemSource::new();
        src.append(b"a\nb\n");
        let mut state = FollowState::new(20);
        let mut out = Vec::new();

        let r = step_plain(&mut state, &mut src, &mut out);
        assert_eq!(r, TickOutcome::Wrote(2));

        out.clear();
        src.rewrite_in_place(b"X\nY\n");
        let r = step_plain(&mut state, &mut src, &mut out);
        assert_eq!(r, TickOutcome::Reset(2));
        assert_eq!(s(&out), "X\nY\n");
        assert_eq!(state.line_no, 3);

        out.clear();
        src.append(b"Z\n");
        let r = step_plain(&mut state, &mut src, &mut out);
        assert_eq!(r, TickOutcome::Wrote(1));
        assert_eq!(s(&out), "Z\n");
        assert_eq!(state.line_no, 4);

        out.clear();
        let r = step_plain(&mut state, &mut src, &mut out);
        assert_eq!(r, TickOutcome::Idle);

        out.clear();
        src.rewrite_in_place(b"P\n");
        let r = step_plain(&mut state, &mut src, &mut out);
        assert_eq!(r, TickOutcome::Reset(1));
        assert_eq!(s(&out), "P\n");
        assert_eq!(state.line_no, 2);
    }

    #[test]
    fn crlf_is_normalised() {
        let mut src = MemSource::new();
        src.append(b"a\r\nb\r\n");
        let mut state = FollowState::new(20);
        let mut out = Vec::new();

        let r = step_plain(&mut state, &mut src, &mut out);
        assert_eq!(r, TickOutcome::Wrote(2));
        assert_eq!(s(&out), "a\nb\n");
    }

    #[test]
    fn appending_in_pieces_matches_bulk_emission() {
        // invariant: the runtime instance is reused across ticks, so feeding
        // two lines via two ticks must produce byte-identical output to
        // feeding the same two lines via one tick. catches any regression
        // where rotation logic accidentally resets the runtime, where the
        // partial-line buffer corrupts boundaries, etc.
        use lsh_defs::LANGUAGES;
        let rust = LANGUAGES
            .iter()
            .find(|l| l.id.eq_ignore_ascii_case("rust"))
            .expect("rust language present");
        let color_map = crate::eat::theme::color_map();

        let lines = b"let x = 1;\nfn foo() {}\n";

        // path A: one tick.
        let mut rt_a = Runtime::new(&ASSEMBLY, &STRINGS, &CHARSETS, rust.entrypoint);
        let mut src_a = MemSource::new();
        src_a.append(lines);
        let mut state_a = FollowState::new(20);
        let mut out_a = Vec::new();
        tick(
            &mut state_a,
            &mut src_a,
            &mut rt_a,
            rust.entrypoint,
            &color_map,
            None,
            true,
            &mut out_a,
        )
        .unwrap();

        // path B: two ticks, line by line.
        let mut rt_b = Runtime::new(&ASSEMBLY, &STRINGS, &CHARSETS, rust.entrypoint);
        let mut src_b = MemSource::new();
        let mut state_b = FollowState::new(20);
        let mut out_b = Vec::new();
        src_b.append(b"let x = 1;\n");
        tick(
            &mut state_b,
            &mut src_b,
            &mut rt_b,
            rust.entrypoint,
            &color_map,
            None,
            true,
            &mut out_b,
        )
        .unwrap();
        src_b.append(b"fn foo() {}\n");
        tick(
            &mut state_b,
            &mut src_b,
            &mut rt_b,
            rust.entrypoint,
            &color_map,
            None,
            true,
            &mut out_b,
        )
        .unwrap();

        assert_eq!(s(&out_a), s(&out_b));
    }

    #[test]
    fn missing_file_within_budget_is_idle() {
        struct Flaky {
            fails_left: u32,
            content: Vec<u8>,
        }
        impl FollowSource for Flaky {
            fn stat(&self) -> io::Result<FollowStat> {
                if self.fails_left > 0 {
                    Err(io::ErrorKind::NotFound.into())
                } else {
                    Ok(FollowStat { size: self.content.len() as u64, id: 1, mtime_ns: 1 })
                }
            }
            fn read_from(&mut self, offset: u64, buf: &mut Vec<u8>) -> io::Result<()> {
                buf.extend_from_slice(&self.content[offset as usize..]);
                Ok(())
            }
        }

        let mut src = Flaky { fails_left: 3, content: b"x\n".to_vec() };
        let mut state = FollowState::new(5);
        let mut out = Vec::new();

        // 3 misses, each within budget -> Idle.
        for _ in 0..3 {
            let r =
                tick(&mut state, &mut src, &mut plain(), 0, &[], None, false, &mut out).unwrap();
            assert_eq!(r, TickOutcome::Idle);
            src.fails_left -= 1;
        }
        // recovery: stat now succeeds, content emits.
        let r = tick(&mut state, &mut src, &mut plain(), 0, &[], None, false, &mut out).unwrap();
        assert_eq!(r, TickOutcome::Wrote(1));
        assert_eq!(s(&out), "x\n");
    }

    #[test]
    fn missing_file_beyond_budget_yields_gone() {
        struct Dead;
        impl FollowSource for Dead {
            fn stat(&self) -> io::Result<FollowStat> {
                Err(io::ErrorKind::NotFound.into())
            }
            fn read_from(&mut self, _: u64, _: &mut Vec<u8>) -> io::Result<()> {
                Ok(())
            }
        }
        let mut src = Dead;
        let mut state = FollowState::new(2);
        let mut out = Vec::new();

        // budget=2 means: 1st miss Idle, 2nd Idle, 3rd > budget -> GoneTooLong.
        assert_eq!(
            tick(&mut state, &mut src, &mut plain(), 0, &[], None, false, &mut out).unwrap(),
            TickOutcome::Idle
        );
        assert_eq!(
            tick(&mut state, &mut src, &mut plain(), 0, &[], None, false, &mut out).unwrap(),
            TickOutcome::Idle
        );
        assert_eq!(
            tick(&mut state, &mut src, &mut plain(), 0, &[], None, false, &mut out).unwrap(),
            TickOutcome::GoneTooLong
        );
    }

    #[test]
    fn line_numbers_when_gutter_supplied() {
        use crate::eat::gutter_view::Gutter;
        use gutter::GutterMark;
        let g = Gutter { width: FOLLOW_NUM_WIDTH, marks: vec![GutterMark::None; 100] };
        let mut src = MemSource::new();
        src.append(b"a\nb\n");
        let mut state = FollowState::new(20);
        let mut out = Vec::new();
        tick(&mut state, &mut src, &mut plain(), 0, &[], Some(&g), false, &mut out).unwrap();
        let got = s(&out);
        assert!(got.contains("1"));
        assert!(got.contains("2"));
        assert!(got.contains("a\n"));
        assert!(got.contains("b\n"));
        // separator present
        assert!(got.contains("|") || got.contains("\u{2502}"));
    }

    /// The two follow checks fire on state that the classifier is supposed to
    /// rule out -- an offset past EOF, or numbering that rewinds without a
    /// rotation. Neither is reachable while `classify` is right, so what these
    /// pin is that the whole set of transitions it does allow keeps them quiet.
    #[cfg(feature = "sanity")]
    #[test]
    fn no_transition_trips_the_follow_checks() {
        use stdext::sanity::capture;

        let ((), msgs) = capture::trips(|| {
            let mut src = MemSource::new();
            let mut state = FollowState::new(20);
            let mut out = Vec::new();

            src.append(b"alpha\nbeta\n");
            step_plain(&mut state, &mut src, &mut out);

            // append, including a mid-line partial that a later tick completes
            src.append(b"gamma\n");
            step_plain(&mut state, &mut src, &mut out);
            src.append(b"partial");
            step_plain(&mut state, &mut src, &mut out);
            src.append(b" completed\n");
            step_plain(&mut state, &mut src, &mut out);

            // touch with no content change
            src.touch();
            step_plain(&mut state, &mut src, &mut out);

            // shrink without changing the inode
            src.truncate();
            step_plain(&mut state, &mut src, &mut out);
            src.append(b"after truncate\n");
            step_plain(&mut state, &mut src, &mut out);

            // same size, different content, same inode
            src.rewrite_in_place(b"AAAAAAAAAAAAAAA\n");
            step_plain(&mut state, &mut src, &mut out);

            // grew in place, same inode
            src.rewrite_in_place(b"BBBB\nCCCC\nDDDD\n");
            step_plain(&mut state, &mut src, &mut out);

            // atomic rename: new inode
            src.replace(b"rotated\n");
            step_plain(&mut state, &mut src, &mut out);

            // and a vanished file
            src.stat_fail = 1;
            step_plain(&mut state, &mut src, &mut out);
        });

        assert!(msgs.is_empty(), "{msgs:?}");
    }
}
