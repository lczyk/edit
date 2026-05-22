//! Compiles the bundled lsh definitions into a static rust table.
//! Both `edit` and `eat` (and any future consumer) pick up the result
//! through `lsh-defs`'s public surface so the codegen runs once per
//! workspace build instead of once per consuming crate.

use stdext::arena::scratch_arena;

fn main() {
    stdext::arena::init(128 * 1024 * 1024).unwrap();

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
