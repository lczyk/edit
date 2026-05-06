//! tty -- terminal i/o primitives shared by `edit` and `eat`.
//!
//! everything here was originally part of `edit::sys::unix` and lifted out
//! once `eat` grew its own tui (`eat -f`'s live pager). the module's surface
//! is the minimum needed by both consumers: enter/leave raw mode, read stdin
//! with a poll timeout, write stdout, query / observe terminal size, and
//! reopen stdin from /dev/tty when the process is being piped to.
//!
//! the implementation is deliberately unix-only and direct-libc; this matches
//! the project's existing approach (no third-party tui crates). windows
//! support has not historically been a goal and isn't a goal here.

#[cfg(unix)]
mod unix;

#[cfg(unix)]
pub use unix::*;
