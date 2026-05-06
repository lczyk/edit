//! gutter -- per-line "how does this differ from a baseline" marks, plus
//! the git-shell-out used to fetch the baseline. shared between `edit`
//! (interactive) and `eat` (one-shot / live tail).
//!
//! the diff is line-level Myers; baseline retrieval is a `git show
//! HEAD:<rel>` (with a fallback to `:<rel>` for staged-only files). no
//! libgit2 -- if `git` isn't on $PATH, callers get a graceful "no marks".

pub mod git;
pub mod gutter_diff;
pub mod linediff;
mod mark;

pub use mark::GutterMark;
