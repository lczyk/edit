//! Shared low-level utilities: arena allocators, arena-backed collections,
//! SIMD scanners, UTF-8 decoding, glob matching, platform shims, the
//! geometry/units helpers everything else is built on, and the debug-only
//! sanity checks.
//!
//! [`sanity`] lives here rather than in `edit` so every crate in the
//! workspace can reach the macros -- `edit` re-exports them, so call sites
//! there keep saying `crate::sanity_check!`.

pub mod alloc;
pub mod arena;
pub mod collections;
pub mod glob;
mod helpers;
pub mod notify;
pub mod sanity;
pub mod simd;
pub mod sys;
pub mod unicode;

pub use helpers::*;
