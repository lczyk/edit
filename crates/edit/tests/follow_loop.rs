//! End-to-end loop tests for `eat -f`. These exercise the full pipeline --
//! `FollowSource` against a real tempfile, `tick`, `View` extension,
//! gutter recompute, `render_frame` -- against the kinds of file mutations
//! the user actually hits (append, in-place rewrite, atomic rename, touch).
//!
//! we drive the loop body manually here rather than spawning the binary
//! and emulating a tty: this gives deterministic timing, lets us assert on
//! the resulting frame string directly, and skips the script(1) noise.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

use edit::eat::follow::{FOLLOW_NUM_WIDTH, FileSource, FollowState, TickOutcome, tick};
use edit::eat::follow_tui::{LineBuf, View, render_frame};
use edit::eat::gutter_view::Gutter;
use edit::eat::theme;

// --- tempfile helpers -----------------------------------------------------

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_path(label: &str) -> PathBuf {
    let pid = process::id();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("eat-follow-test-{label}-{pid}-{n}"))
}

struct Tmp {
    pub path: PathBuf,
}

impl Tmp {
    fn new(label: &str, initial: &[u8]) -> Self {
        let path = unique_path(label);
        fs::write(&path, initial).unwrap();
        Self { path }
    }

    fn append(&self, bytes: &[u8]) {
        let mut f = fs::OpenOptions::new().append(true).open(&self.path).unwrap();
        f.write_all(bytes).unwrap();
    }

    fn rewrite(&self, bytes: &[u8]) {
        // truncate-and-write: same inode (matches `> file` from a shell).
        let mut f = fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .create(true)
            .open(&self.path)
            .unwrap();
        f.write_all(bytes).unwrap();
    }

    fn atomic_rename(&self, bytes: &[u8]) {
        // write to sibling tempfile, then rename over -- vim-style atomic save.
        let tmp = self.path.with_extension("incoming");
        fs::write(&tmp, bytes).unwrap();
        fs::rename(&tmp, &self.path).unwrap();
    }

    fn touch(&self) {
        // bump mtime by reading + rewriting same bytes -- portable enough.
        let bytes = fs::read(&self.path).unwrap();
        // sleep a tick so the new mtime is observably different on fs that
        // only have second resolution.
        std::thread::sleep(std::time::Duration::from_millis(15));
        let mut f = fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .create(true)
            .open(&self.path)
            .unwrap();
        f.write_all(&bytes).unwrap();
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

// --- fixture: drive one tick + extend view + recompute gutter -------------

struct LoopFixture {
    src: FileSource,
    state: FollowState,
    view: View,
    sink: LineBuf,
    gutter: Option<Gutter>,
    color_map: Vec<&'static str>,
    path: PathBuf,
    show_numbers: bool,
}

impl LoopFixture {
    fn new(path: &PathBuf, show_numbers: bool) -> Self {
        let src = FileSource::new(path.clone());
        let view = View::new(80, 24);
        let gutter = if show_numbers {
            Some(Gutter::compute(path, &fs::read(path).unwrap_or_default(), FOLLOW_NUM_WIDTH))
        } else {
            None
        };
        Self {
            src,
            state: FollowState::new(20),
            view,
            sink: LineBuf::new(),
            gutter,
            color_map: theme::color_map(),
            path: path.clone(),
            show_numbers,
        }
    }

    fn step(&mut self) -> TickOutcome {
        let outcome = tick(
            &mut self.state,
            &mut self.src,
            None, // no syntax runtime -- we only care about content + gutter
            0,
            &self.color_map,
            None, // gutter composed at draw time
            false,
            &mut self.sink,
        )
        .unwrap();
        match outcome {
            TickOutcome::Reset(_) => {
                self.view.reset_lines();
                self.view.extend_lines(&self.sink.take_new());
                if self.show_numbers {
                    self.gutter = Some(Gutter::compute(
                        &self.path,
                        &fs::read(&self.path).unwrap_or_default(),
                        FOLLOW_NUM_WIDTH,
                    ));
                }
            }
            TickOutcome::Wrote(n) if n > 0 => {
                self.view.extend_lines(&self.sink.take_new());
                if self.show_numbers {
                    self.gutter = Some(Gutter::compute(
                        &self.path,
                        &fs::read(&self.path).unwrap_or_default(),
                        FOLLOW_NUM_WIDTH,
                    ));
                }
            }
            _ => {
                self.view.extend_lines(&self.sink.take_new());
            }
        }
        outcome
    }

    fn frame(&mut self) -> String {
        render_frame(&mut self.view, "test", "00:00:00", 100, self.gutter.as_ref(), false)
    }
}

// --- tests ----------------------------------------------------------------

#[test]
fn append_to_untracked_file_shows_new_lines() {
    let f = Tmp::new("append-untracked", b"alpha\n");
    let mut fx = LoopFixture::new(&f.path, false);

    let r = fx.step();
    assert!(matches!(r, TickOutcome::Wrote(1)));
    assert_eq!(fx.view.lines.len(), 1);
    assert_eq!(fx.view.lines[0], b"alpha");

    f.append(b"beta\n");
    let r = fx.step();
    assert!(matches!(r, TickOutcome::Wrote(1)));
    assert_eq!(fx.view.lines.len(), 2);
    assert_eq!(fx.view.lines[1], b"beta");

    f.append(b"gamma\n");
    let r = fx.step();
    assert!(matches!(r, TickOutcome::Wrote(1)));
    assert_eq!(fx.view.lines.len(), 3);
}

#[test]
fn idle_when_nothing_changes() {
    let f = Tmp::new("idle", b"alpha\n");
    let mut fx = LoopFixture::new(&f.path, false);

    fx.step(); // initial bulk
    let r = fx.step();
    assert!(matches!(r, TickOutcome::Idle));
    let r = fx.step();
    assert!(matches!(r, TickOutcome::Idle));
}

#[test]
fn in_place_rewrite_replaces_view_content() {
    // the user-reported bug: `printf 'X\n' > file` from another terminal
    // should make eat's view reflect the new content.
    let f = Tmp::new("in-place", b"old1\nold2\nold3\n");
    let mut fx = LoopFixture::new(&f.path, false);

    fx.step();
    assert_eq!(fx.view.lines.len(), 3);
    assert_eq!(fx.view.lines[0], b"old1");

    // need a fresh mtime; FAT and some macOS fs round to coarse resolution.
    std::thread::sleep(std::time::Duration::from_millis(15));
    f.rewrite(b"new1\nnew2\n");

    let r = fx.step();
    assert!(matches!(r, TickOutcome::Reset(2)), "expected Reset(2), got {r:?}");
    assert_eq!(fx.view.lines.len(), 2);
    assert_eq!(fx.view.lines[0], b"new1");
    assert_eq!(fx.view.lines[1], b"new2");
}

#[test]
fn in_place_rewrite_with_growth_still_resets() {
    // bytes grow, but the leading bytes change too -- head fingerprint
    // catches it. previously this would be misread as "append from prev.size".
    let f = Tmp::new("grow", b"old\n");
    let mut fx = LoopFixture::new(&f.path, false);

    fx.step();
    assert_eq!(fx.view.lines.len(), 1);

    std::thread::sleep(std::time::Duration::from_millis(15));
    f.rewrite(b"replaced line one\nreplaced line two\n");

    let r = fx.step();
    assert!(matches!(r, TickOutcome::Reset(2)));
    assert_eq!(fx.view.lines.len(), 2);
    assert!(fx.view.lines[0].starts_with(b"replaced"));
}

#[test]
fn atomic_rename_resets_via_inode_change() {
    // vim-style save: write tmp, rename over. inode changes -> Reset.
    let f = Tmp::new("rename", b"original\n");
    let mut fx = LoopFixture::new(&f.path, false);

    fx.step();
    assert_eq!(fx.view.lines.len(), 1);

    std::thread::sleep(std::time::Duration::from_millis(15));
    f.atomic_rename(b"replaced\n");

    let r = fx.step();
    assert!(matches!(r, TickOutcome::Reset(1)));
    assert_eq!(fx.view.lines[0], b"replaced");
}

#[test]
fn touch_does_not_emit() {
    // `touch file` (mtime bump, no content change) should be Idle.
    let f = Tmp::new("touch", b"unchanged\n");
    let mut fx = LoopFixture::new(&f.path, false);

    fx.step();
    let view_before = fx.view.lines.clone();

    f.touch();
    let r = fx.step();
    // some filesystems expose touch as a tiny mtime bump w/ identical bytes.
    // it should not produce Reset/Wrote -- either Idle or a defensive Reset(N)
    // if the fs reported the rewrite as a content change. either way the body
    // bytes shouldn't have shifted.
    if !matches!(r, TickOutcome::Idle) {
        assert!(matches!(r, TickOutcome::Reset(_)));
    }
    assert_eq!(fx.view.lines, view_before);
}

#[test]
fn heartbeat_only_does_not_change_view() {
    // simulate the "no change" case: tick repeatedly w/ no file mutation.
    // bodies should be identical across frames.
    let f = Tmp::new("heartbeat", b"line1\nline2\n");
    let mut fx = LoopFixture::new(&f.path, false);

    fx.step();
    let frame1 = fx.frame();
    let frame2 = fx.frame();
    let frame3 = fx.frame();
    // identical (no fake timestamp diff in the test fixture).
    assert_eq!(frame1, frame2);
    assert_eq!(frame2, frame3);
}

// --- gutter staleness regression ----------------------------------------

#[test]
fn gutter_marks_update_after_in_place_edit() {
    // open a file inside the local git repo so the gutter has a real
    // baseline to diff against. then mutate the file in place and assert
    // that subsequent frames show updated marks.
    //
    // requires `git` on PATH and the test repo to actually be a git
    // checkout. skip silently if neither holds.
    if !std::process::Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        eprintln!("skipping: git not on PATH");
        return;
    }

    // copy a known-tracked file into a test path *inside the repo* so
    // git::locate succeeds. we work in a non-tracked filename so the diff
    // result is stable (the file is "untracked" -> baseline is empty ->
    // every line is Added).
    let repo_root =
        match std::process::Command::new("git").args(["rev-parse", "--show-toplevel"]).output() {
            Ok(o) if o.status.success() => {
                PathBuf::from(String::from_utf8_lossy(&o.stdout).trim().to_string())
            }
            _ => {
                eprintln!("skipping: not in a git repo");
                return;
            }
        };
    let pid = process::id();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = repo_root.join(format!(".eat-test-{pid}-{n}.txt"));
    fs::write(&path, b"a\nb\nc\n").unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    let _g = Cleanup(path.clone());

    let mut fx = LoopFixture::new(&path, /* show_numbers */ true);
    fx.step();
    // untracked file in a tracked repo: BaselineState::load returns None ->
    // marks are all None.
    let g = fx.gutter.as_ref().expect("gutter present with show_numbers");
    assert!(g.marks.iter().all(|m| matches!(m, gutter::GutterMark::None)));
    let frame_before = fx.frame();

    // mutate the file (still untracked, marks stay None). the bug we're
    // guarding against is: even if marks didn't change here, the FRAME
    // composition uses the new gutter immediately. assert frame text
    // reflects the new content.
    std::thread::sleep(std::time::Duration::from_millis(15));
    f_path_rewrite(&path, b"X\nY\nZ\n");
    let r = fx.step();
    assert!(matches!(r, TickOutcome::Reset(3)), "expected Reset(3), got {r:?}");
    let frame_after = fx.frame();
    assert!(frame_after.contains('X'));
    assert!(frame_after.contains('Y'));
    assert!(frame_after.contains('Z'));
    assert_ne!(frame_before, frame_after);
}

fn f_path_rewrite(path: &PathBuf, bytes: &[u8]) {
    let mut f = fs::OpenOptions::new().write(true).truncate(true).create(true).open(path).unwrap();
    f.write_all(bytes).unwrap();
}

// --- tab artefact regression ------------------------------------------------

#[test]
fn tab_indented_lines_clear_eol_before_content() {
    // regression: `\t` in a body line advances the cursor w/out painting cells.
    // `clear_eol` must be emitted BEFORE the line bytes (cursor at col 1,
    // erasing the whole row), not after them. a post-content clear would leave
    // the cells skipped by the tab holding stale paint from the previous frame,
    // visible as background bleed-through on transparent terminals after scroll.
    //
    // invariant: for every body row the frame string must have the pattern
    //   \x1b[{row};1H  \x1b[K  <content>
    // not  \x1b[{row};1H  <content>  \x1b[K
    let f = Tmp::new("tab-erase", b"\tfirst\n\tsecond\n");
    let mut fx = LoopFixture::new(&f.path, false);
    fx.step();
    let frame = fx.frame();

    // check both body rows (header = row 1, body starts at row 2)
    for row in [2u16, 3u16] {
        let cur = format!("\x1b[{row};1H");
        let pos = frame.find(&cur).unwrap_or_else(|| panic!("cursor escape for row {row} missing"));
        let after = &frame[pos + cur.len()..];
        assert!(
            after.starts_with("\x1b[K"),
            "row {row}: clear_eol must precede tab content; got: {:?}",
            &after[..after.len().min(40)]
        );
    }
}

#[test]
fn tab_stale_cells_absent_after_scroll() {
    // build two frames: first renders lines without leading tabs; second renders
    // tab-indented lines at the same rows via scroll. assert that col-1 cells
    // in the second frame carry no text from the first frame.
    //
    // we do this by verifying `clear_eol` precedes the tab on every body row --
    // if it does, the terminal is guaranteed to see an erase-before-paint,
    // regardless of what the previous frame wrote to those cells.
    //
    // the file has 4 lines: first 2 plain (no tab), last 2 tab-indented.
    // after rendering with scroll_offset=0, we shift to scroll_offset=2 so
    // the tab-indented lines land on the same terminal rows as the plain ones.
    let f = Tmp::new("tab-scroll", b"plain_a\nplain_b\n\ttab_c\n\ttab_d\n");
    let mut fx = LoopFixture::new(&f.path, false);
    fx.step();

    // first frame: plain lines at rows 2-3
    let _frame1 = fx.frame();

    // scroll down 2 rows so tab-indented lines land at rows 2-3
    fx.view.scroll_offset = 2;
    fx.view.scroll_offset_visual = 2.0;
    let frame2 = fx.frame();

    // clear_eol must precede tab on rows 2 and 3
    for row in [2u16, 3u16] {
        let cur = format!("\x1b[{row};1H");
        let pos = frame2.find(&cur).unwrap_or_else(|| panic!("cursor escape for row {row} missing"));
        let after = &frame2[pos + cur.len()..];
        assert!(
            after.starts_with("\x1b[K"),
            "row {row} after scroll: clear_eol must precede tab; got: {:?}",
            &after[..after.len().min(40)]
        );
    }
}
