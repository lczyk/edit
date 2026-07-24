//! tty -- terminal i/o primitives.
//!
//! everything here was originally part of `edit::sys::unix` and lifted out
//! when `eat` was still a separate crate with its own tui. `eat` now lives
//! inside the `edit` crate, so `edit` is the only consumer left and this
//! crate boundary no longer buys sharing -- it only pins the surface to the
//! minimum: enter/leave raw mode, read stdin with a poll timeout, write
//! stdout, query / observe terminal size, and reopen stdin from /dev/tty
//! when the process is being piped to. folding it back into `edit::sys` is
//! a reasonable future cleanup.
//!
//! the implementation is deliberately unix-only and direct-libc; this matches
//! the project's existing approach (no third-party tui crates). windows
//! support has not historically been a goal and isn't a goal here.

#[cfg(unix)]
mod unix;

#[cfg(unix)]
pub use unix::*;
