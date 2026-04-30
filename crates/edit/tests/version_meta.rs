//! Meta-test: VERSION (source of truth) must match every other place the
//! version is recorded. Catches drift when bumping VERSION without syncing
//! Cargo.toml (or vice versa).

use std::path::PathBuf;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_version_file() -> String {
    let path = manifest_dir().join("../../VERSION");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    raw.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .unwrap_or_else(|| panic!("{} has no version line", path.display()))
        .to_string()
}

fn read_cargo_toml_version() -> String {
    let path = manifest_dir().join("Cargo.toml");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("version") {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                let rest = rest.trim_start();
                if let Some(after_open) = rest.strip_prefix('"') {
                    if let Some(end) = after_open.find('"') {
                        return after_open[..end].to_string();
                    }
                }
            }
        }
    }
    panic!("no `version = \"...\"` line in {}", path.display());
}

#[test]
fn version_file_matches_cargo_toml() {
    let file = read_version_file();
    let cargo = read_cargo_toml_version();
    assert_eq!(
        file, cargo,
        "VERSION ({file}) and crates/edit/Cargo.toml version ({cargo}) disagree. \
         VERSION is the source of truth -- update Cargo.toml to match."
    );
}

#[test]
fn version_file_matches_compile_time_env() {
    // build.rs overrides CARGO_PKG_VERSION from VERSION, so env!() at compile
    // time should already track VERSION. This guards against the build.rs
    // override silently breaking.
    let file = read_version_file();
    let compiled = env!("CARGO_PKG_VERSION");
    assert_eq!(
        file, compiled,
        "VERSION ({file}) and compile-time CARGO_PKG_VERSION ({compiled}) disagree. \
         build.rs override may be broken."
    );
}
