//! Golden snapshot tests for the lsh highlighter.
//!
//! Layout: `tests/fixtures/<lang>/<case>.<ext>`. Each fixture has a sibling
//! `<case>.<ext>.snap` capturing the highlighter output. To accept new or
//! changed snapshots after a deliberate fix, rerun with
//! `UPDATE_GOLDEN=1 cargo test -p lsh --test golden`.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use lsh::compiler::{Generator, SerializedCharset};
use lsh::runtime::Runtime;
use stdext::arena::scratch_arena;
use stdext::glob::glob_match;

fn discover_fixtures(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            discover_fixtures(&p, out);
        } else if p.extension().and_then(|e| e.to_str()) != Some("snap") {
            out.push(p);
        }
    }
}

fn snap_path(fixture: &Path) -> PathBuf {
    let mut p = fixture.to_path_buf();
    let mut name = OsString::from(fixture.file_name().unwrap());
    name.push(".snap");
    p.set_file_name(name);
    p
}

fn escape(s: &[u8]) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            0x20..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("\\x{b:02x}")),
        }
    }
    out
}

#[test]
fn golden() {
    let _ = stdext::arena::init(128 * 1024 * 1024);

    let outer = scratch_arena(None);
    let mut generator = Generator::new(&outer);
    generator.read_directory(lsh::compiler::builtin_definitions_path()).unwrap();
    let assembly = generator.assemble().unwrap();

    let charsets: Vec<SerializedCharset> =
        assembly.charsets.iter().map(|cs| cs.serialize()).collect();

    let max_id = assembly.highlight_kinds.iter().map(|hk| hk.value).max().unwrap_or(0);
    let mut kind_names: Vec<&str> = vec![""; max_id as usize + 1];
    for hk in &assembly.highlight_kinds {
        kind_names[hk.value as usize] = hk.identifier;
    }

    let fixtures_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut fixtures = Vec::new();
    discover_fixtures(&fixtures_root, &mut fixtures);
    fixtures.sort();
    assert!(!fixtures.is_empty(), "no fixtures found under {}", fixtures_root.display());

    let update = env::var_os("UPDATE_GOLDEN").is_some();
    let mut failures: Vec<String> = Vec::new();

    for fixture in &fixtures {
        let path_bytes = fixture.as_os_str().as_encoded_bytes();
        let Some(entrypoint) = assembly
            .entrypoints
            .iter()
            .find(|ep| ep.paths.iter().any(|pat| glob_match(pat.as_bytes(), path_bytes)))
        else {
            failures.push(format!("no entrypoint for {}", fixture.display()));
            continue;
        };

        let mut runtime = Runtime::new(
            &assembly.instructions,
            &assembly.strings,
            &charsets,
            entrypoint.address as u32,
        );

        let src = fs::read(fixture).unwrap();
        let mut snap = String::new();
        // Split on '\n' so we preserve a trailing empty line if present.
        for (lineno, line) in src.split(|&b| b == b'\n').enumerate() {
            // Strip a trailing '\r' from CRLF.
            let line = match line.last() {
                Some(b'\r') => &line[..line.len() - 1],
                _ => line,
            };
            let scratch = scratch_arena(Some(&outer));
            let highlights = runtime.parse_next_line::<u32>(&scratch, line);
            for w in highlights.windows(2) {
                let curr = &w[0];
                let next = &w[1];
                let text = &line[curr.start..next.start];
                if text.is_empty() {
                    continue;
                }
                let kind = kind_names.get(curr.kind as usize).copied().unwrap_or("?");
                snap.push_str(&format!("{:>4} {:<22} {}\n", lineno + 1, kind, escape(text)));
            }
        }

        let snap_file = snap_path(fixture);
        if update || !snap_file.exists() {
            fs::write(&snap_file, &snap).unwrap();
            continue;
        }
        let existing = fs::read_to_string(&snap_file).unwrap_or_default();
        if existing != snap {
            let mut actual = snap_file.clone();
            let mut name = OsString::from(snap_file.file_name().unwrap());
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
