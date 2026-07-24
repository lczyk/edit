//! Bundled lsh definitions, compiled into static tables at build time.
//!
//! The build script runs `lsh::compiler::Generator` over
//! `crates/lsh/definitions/` and writes the result into `OUT_DIR`. This
//! crate re-exports those tables so every consumer -- the editor, the `eat`
//! persona, `lsh-bin`, and anything future -- shares one codegen pass and
//! one source of truth.

include!(concat!(env!("OUT_DIR"), "/lsh_definitions.rs"));

pub mod detect;
pub mod document;
pub mod highlighter;
pub mod theme;

pub use document::ReadableDocument;
pub use highlighter::{Highlighter, HighlighterState};
