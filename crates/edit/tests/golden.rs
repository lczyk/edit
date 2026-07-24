//! Golden snapshot tests for eat -- ANSI-highlighted output.
//!
//! Reuses lsh test fixtures by path. Each fixture is syntax-highlighted
//! through eat's rendering path; the result (raw bytes including ANSI escapes)
//! is compared against a sibling `<fixture>.snap.ansi`. `UPDATE_GOLDEN=1`
//! refreshes snapshots.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use edit::eat::theme;
use lsh::runtime::Runtime;
use lsh_defs::{ASSEMBLY, CHARSETS, FILE_ASSOCIATIONS, STRINGS};
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

fn lsh_fixtures_root() -> PathBuf {
    let eat_manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    eat_manifest.join("../lsh/tests/fixtures")
}

fn discover_fixtures(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            discover_fixtures(&p, out);
        } else if !is_snap(&p) && p.extension().is_some_and(|e| e != "jsonl") {
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
        let Some(lang) = FILE_ASSOCIATIONS
            .iter()
            .find(|(pat, _)| glob_match(pat.as_bytes(), path_bytes))
            .map(|(_, lang)| *lang)
        else {
            failures.push(format!("no entrypoint for {}", fixture.display()));
            continue;
        };

        let mut runtime = Runtime::new(&ASSEMBLY, &STRINGS, &CHARSETS, lang.entrypoint);

        let src = fs::read(fixture).unwrap();
        let mut snap = Vec::new();

        for line in src.split(|&b| b == b'\n') {
            let line = match line.last() {
                Some(b'\r') => &line[..line.len() - 1],
                _ => line,
            };
            let scratch = scratch_arena(None);
            let highlights = runtime.parse_next_line::<u32>(&scratch, line);

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
