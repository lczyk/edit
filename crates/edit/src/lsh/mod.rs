//! Microsoft Edit's adapter to LSH.
//!
//! Static tables and detection helpers come from `lsh-defs`, the workspace
//! crate that owns the compile-time `Generator` pass. Edit-local bits that
//! still live here:
//!
//! - `cache` -- line-keyed [`Highlighter`] cache used by the buffer.
//! - `highlighter` -- the stateful `Highlighter` wrapper.

pub mod cache;
mod highlighter;

pub use highlighter::*;
pub use lsh::runtime::Language;
pub use lsh_defs::detect::{
    disambiguate_language, language_from_content, language_from_shebang,
    match_file_associations, process_file_associations,
};
pub use lsh_defs::{
    ASSEMBLY, CHARSETS, FILE_ASSOCIATIONS, HighlightKind, LANGUAGES, STRINGS,
};
