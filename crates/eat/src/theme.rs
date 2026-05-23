//! ANSI-16 colour mapping used by eat's ansi-stream output path.
//!
//! Re-exported from `lsh-defs::theme` so any future consumer that wants
//! the same "respect the user's terminal palette" colourmap (e.g. an
//! `edit --print` mode) gets it from one place.

pub use lsh_defs::theme::ansi16_color_map as color_map;
