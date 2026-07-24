//! Input events and the decoders that produce them.
//!
//! Two halves, kept apart on purpose (see each module's docs): [`keys`]
//! is the wire-format-free vocabulary, [`vt_decode`] is the VT-specific
//! decoder. Supporting a non-VT input scheme means adding a sibling to
//! the latter, not touching the former.

mod keys;
mod vt_decode;

pub use keys::*;
pub use vt_decode::*;
