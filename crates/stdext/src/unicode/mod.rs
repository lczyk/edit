//! UTF-8 decoding. Grapheme segmentation and display-width measurement
//! live in `edit::unicode` instead, since they need the generated tables.

mod utf8;

pub use utf8::*;
