use stdext::arena::scratch_arena;

fn main() {
    stdext::arena::init(128 * 1024 * 1024).unwrap();

    set_pkg_version_from_file();
    version::emit();

    let scratch = scratch_arena(None);
    let lsh_path = lsh::compiler::builtin_definitions_path();
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let out_path = format!("{out_dir}/lsh_definitions.rs");

    let mut generator = lsh::compiler::Generator::new(&scratch);
    match generator.read_directory(lsh_path).and_then(|_| generator.generate_rust()) {
        Ok(c) => std::fs::write(&out_path, c).unwrap(),
        Err(err) => panic!("failed to compile lsh definitions: {err}"),
    };

    println!("cargo::rerun-if-changed={}", lsh_path.display());
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
