//! Grapheme segmentation and display-width measurement, driven by the
//! generated tables in [`tables`]. UTF-8 decoding lives in `stdext::unicode`.

mod measurement;
mod tables;

pub use measurement::*;
