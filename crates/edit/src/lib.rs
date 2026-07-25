#![cfg_attr(
    target_arch = "loongarch64",
    feature(stdarch_loongarch, stdarch_loongarch_feature_detection, loongarch_target_feature),
    allow(clippy::incompatible_msrv)
)]
#![allow(clippy::missing_transmute_annotations, clippy::new_without_default, stable_features)]

pub mod base64;
pub mod buffer;
pub mod cell;
pub mod clipboard;
pub mod document;
pub mod eat;
pub mod framebuffer;
pub mod glyphs;
pub mod hash;
pub mod helpers;
pub mod icu;
pub mod input;
pub mod langlist;
pub mod lsh;
pub mod mount;
pub mod oklab;
pub mod paint;
pub mod path;
pub mod sys;
pub mod term;
pub mod tui;
pub mod unicode;
pub mod vt;
pub mod watch;

// Both live in `stdext` so the other crates can use them too. Re-exported at
// this crate's root because the macros expand to `$crate::sanity::record`, and
// because it keeps every call site in this crate saying `crate::sanity_check!`.
pub use stdext::{notify, sanity, sanity_assert, sanity_check};
