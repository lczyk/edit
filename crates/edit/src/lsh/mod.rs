//! Microsoft Edit's adapter to LSH.
//!
//! Static tables, detection helpers, and the stateful `Highlighter` wrapper
//! all live in `lsh-defs`, the workspace crate that owns the compile-time
//! `Generator` pass. The only edit-local piece left here is `cache`, the
//! line-keyed `HighlighterCache` used by edit's incremental render path.

pub mod cache;

pub use lsh::runtime::Language;
pub use lsh_defs::detect::{
    disambiguate_language, language_from_content, language_from_shebang,
    match_file_associations, process_file_associations,
};
pub use lsh_defs::{
    ASSEMBLY, CHARSETS, FILE_ASSOCIATIONS, HighlightKind, Highlighter, HighlighterState,
    LANGUAGES, STRINGS,
};
