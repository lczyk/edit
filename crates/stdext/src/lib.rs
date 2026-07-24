//! Shared low-level utilities: arena allocators, arena-backed collections,
//! SIMD scanners, UTF-8 decoding, glob matching, platform shims, and the
//! geometry/units helpers everything else is built on.

pub mod alloc;
pub mod arena;
pub mod collections;
pub mod glob;
mod helpers;
pub mod simd;
pub mod sys;
pub mod unicode;

pub use helpers::*;
