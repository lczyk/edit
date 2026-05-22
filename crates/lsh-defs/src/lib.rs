//! Bundled lsh definitions, compiled into static tables at build time.
//!
//! The build script runs `lsh::compiler::Generator` over
//! `crates/lsh/definitions/` and writes the result into `OUT_DIR`. This
//! crate re-exports those tables so every consumer (`edit`, `eat`, future)
//! shares one codegen pass and one source of truth.

include!(concat!(env!("OUT_DIR"), "/lsh_definitions.rs"));

pub mod detect;
