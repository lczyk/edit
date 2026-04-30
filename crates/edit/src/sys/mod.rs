//! Platform abstractions.

mod unix;

pub use std::fs::canonicalize;

pub use unix::*;
