fn main() {
    set_pkg_version_from_file();
    version::emit();
}

fn set_pkg_version_from_file() {
    let manifest_dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let version_path = manifest_dir.join("../../VERSION");
    let raw = std::fs::read_to_string(&version_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", version_path.display()));

    let ver = raw
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .unwrap_or_else(|| panic!("{} has no version line", version_path.display()));

    println!("cargo::rustc-env=CARGO_PKG_VERSION={ver}");
    println!("cargo::rerun-if-changed={}", version_path.display());
}
