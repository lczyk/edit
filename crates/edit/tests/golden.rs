//! Golden snapshot tests for eat -- ANSI-highlighted output.
//!
//! Reuses lsh test fixtures by path. Each fixture is syntax-highlighted
//! through eat's rendering path; the result is compared against a sibling
//! `<fixture>.snap.ansi`. Snapshots are plain ASCII: the escape byte and
//! anything outside printable ASCII is written as `\xNN`, so they diff and
//! commit like text. `UPDATE_GOLDEN=1` refreshes snapshots.

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use edit::eat::theme;
use lsh::runtime::Runtime;
use lsh_defs::{ASSEMBLY, CHARSETS, FILE_ASSOCIATIONS, PLAIN, STRINGS};
use stdext::arena::scratch_arena;
use stdext::glob::glob_match;

const SNAP_SUFFIX: &str = ".snap.ansi";

fn is_snap(p: &Path) -> bool {
    p.to_str().is_some_and(|s| s.ends_with(SNAP_SUFFIX))
}

fn snap_path(fixture: &Path) -> PathBuf {
    let mut name = fixture.file_name().unwrap().to_os_string();
    name.push(SNAP_SUFFIX);
    fixture.with_file_name(name)
}

/// Escape bytes and anything outside printable ASCII become `\xNN`, and a
/// literal backslash doubles, so a snapshot is plain text (readable in a
/// diff, acceptable to an ASCII-only commit guard) and no two byte strings
/// armour alike.
fn ascii_armour(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() + raw.len() / 8);
    for &b in raw {
        match b {
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\n' | b'\t' | 0x20..=0x7e => out.push(b),
            _ => out.extend_from_slice(format!("\\x{b:02x}").as_bytes()),
        }
    }
    out
}

fn lsh_fixtures_root() -> PathBuf {
    let eat_manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    eat_manifest.join("../lsh/tests/fixtures")
}

/// lsh's own golden test drops a `<fixture>.jsonl` next to every fixture.
/// A `.jsonl` file is one of those only if the fixture it was derived from
/// exists; otherwise it is a fixture in its own right (e.g. `sample.jsonl`).
fn is_lsh_snap(p: &Path) -> bool {
    let Some(s) = p.to_str() else { return false };
    let Some(stem) = s.strip_suffix(".jsonl") else { return false };
    Path::new(stem).exists()
}

fn discover_fixtures(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            discover_fixtures(&p, out);
        } else if !is_snap(&p) && !is_lsh_snap(&p) {
            out.push(p);
        }
    }
}

#[test]
fn golden() {
    let _ = stdext::arena::init(128 * 1024 * 1024);

    let color_map = theme::color_map();
    let max_kind = color_map.len() as u32;

    let fixtures_root = lsh_fixtures_root();
    let mut fixtures = Vec::new();
    discover_fixtures(&fixtures_root, &mut fixtures);
    fixtures.sort();
    assert!(!fixtures.is_empty(), "no fixtures found under {}", fixtures_root.display());

    let update = env::var_os("UPDATE_GOLDEN").is_some();
    let mut failures: Vec<String> = Vec::new();

    for fixture in &fixtures {
        let path_bytes = fixture.as_os_str().as_encoded_bytes();
        let by_glob = FILE_ASSOCIATIONS
            .iter()
            .find(|(pat, _)| glob_match(pat.as_bytes(), path_bytes))
            .map(|(_, lang)| *lang);
        // Plain claims no glob: only its own fixture dir falls back to it.
        let in_plain_dir =
            fixture.parent().and_then(|p| p.file_name()) == Some(OsStr::new("plain"));
        let Some(lang) = by_glob.or(in_plain_dir.then_some(PLAIN)) else {
            failures.push(format!("no entrypoint for {}", fixture.display()));
            continue;
        };

        let mut runtime = Runtime::new(&ASSEMBLY, &STRINGS, &CHARSETS, lang.entrypoint);

        let src = fs::read(fixture).unwrap();
        let mut snap = Vec::new();

        for (lineno, line) in src.split(|&b| b == b'\n').enumerate() {
            let line = match line.last() {
                Some(b'\r') => &line[..line.len() - 1],
                _ => line,
            };
            let scratch = scratch_arena(None);
            runtime.set_line_number(lineno as u32 + 1);
            let highlights = runtime.parse_next_line::<u32>(&scratch, line).spans;

            for w in highlights.windows(2) {
                let curr = &w[0];
                let next = &w[1];
                let start = curr.start;
                let end = next.start.min(line.len());
                let kind = curr.kind;
                let text = &line[start..end];

                if text.is_empty() {
                    continue;
                }

                if kind < max_kind && !color_map[kind as usize].is_empty() {
                    snap.extend_from_slice(color_map[kind as usize].as_bytes());
                    snap.extend_from_slice(text);
                    snap.extend_from_slice(b"\x1b[m");
                } else {
                    snap.extend_from_slice(text);
                }
            }
            snap.push(b'\n');
        }
        let snap = ascii_armour(&snap);

        let snap_file = snap_path(fixture);
        if update {
            fs::write(&snap_file, &snap).unwrap();
            continue;
        }
        // A missing snapshot is a failure, not something to fill in
        // silently. Writing one here would mean a new fixture blesses
        // whatever the highlighter happens to do, pass on the very same
        // run, and never be looked at by anyone.
        if !snap_file.exists() {
            failures.push(format!(
                "no snapshot for {} -- rerun with UPDATE_GOLDEN=1 and review the result",
                fixture.display()
            ));
            continue;
        }
        let existing = fs::read(&snap_file).unwrap_or_default();
        if existing != snap {
            let mut actual = snap_file.clone();
            let mut name = actual.file_name().unwrap().to_os_string();
            name.push(".actual");
            actual.set_file_name(name);
            fs::write(&actual, &snap).unwrap();
            failures.push(format!(
                "snapshot mismatch: {} (wrote {})",
                snap_file.display(),
                actual.display()
            ));
        }
    }

    if !failures.is_empty() {
        panic!(
            "{} golden failure(s); rerun with UPDATE_GOLDEN=1 to refresh:\n  {}",
            failures.len(),
            failures.join("\n  ")
        );
    }
}
