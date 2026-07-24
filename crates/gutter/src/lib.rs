//! gutter -- per-line "how does this differ from a baseline" marks, plus
//! the git-shell-out used to fetch the baseline. consumed by both of the
//! `edit` crate's personas -- the interactive editor and `eat` (one-shot /
//! live tail) -- which now share one binary; the crate boundary survives
//! to keep this api free of edit internals, not to span binaries.
//!
//! the diff is line-level Myers; baseline retrieval is a `git show
//! HEAD:<rel>` (with a fallback to `:<rel>` for staged-only files). no
//! libgit2 -- if `git` isn't on $PATH, callers get a graceful "no marks".

pub mod git;
pub mod gutter_diff;
pub mod linediff;
mod mark;

pub use mark::GutterMark;
