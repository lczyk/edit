//! Byte-reader abstraction used by the highlighter to walk a buffer.
//!
//! Shared across consumers (edit's `TextBuffer`, eat's `Vec<u8>` heads, the
//! `Highlighter` tests' inline slices, anything else that owns bytes and
//! can hand them out in chunks).
//!
//! `WriteableDocument` -- the mutating sibling -- stays in `edit::document`;
//! lsh-defs only ever reads.

use std::path::PathBuf;

/// Read bytes from a text container at an arbitrary offset.
pub trait ReadableDocument {
    /// Read some bytes starting at (including) the given absolute offset.
    ///
    /// # Warning
    ///
    /// * Be lenient on inputs:
    ///   * The given offset may be out of bounds and you MUST clamp it.
    ///   * You should not assume that offsets are at grapheme cluster boundaries.
    /// * Be strict on outputs:
    ///   * You MUST NOT break grapheme clusters across chunks.
    ///   * You MUST NOT return an empty slice unless the offset is at or beyond the end.
    fn read_forward(&self, off: usize) -> &[u8];

    /// Read some bytes before (but not including) the given absolute offset.
    ///
    /// # Warning
    ///
    /// * Be lenient on inputs:
    ///   * The given offset may be out of bounds and you MUST clamp it.
    ///   * You should not assume that offsets are at grapheme cluster boundaries.
    /// * Be strict on outputs:
    ///   * You MUST NOT break grapheme clusters across chunks.
    ///   * You MUST NOT return an empty slice unless the offset is zero.
    fn read_backward(&self, off: usize) -> &[u8];
}

impl ReadableDocument for &[u8] {
    fn read_forward(&self, off: usize) -> &[u8] {
        let s = *self;
        &s[off.min(s.len())..]
    }

    fn read_backward(&self, off: usize) -> &[u8] {
        let s = *self;
        &s[..off.min(s.len())]
    }
}

impl ReadableDocument for String {
    fn read_forward(&self, off: usize) -> &[u8] {
        let s = self.as_bytes();
        &s[off.min(s.len())..]
    }

    fn read_backward(&self, off: usize) -> &[u8] {
        let s = self.as_bytes();
        &s[..off.min(s.len())]
    }
}

impl ReadableDocument for PathBuf {
    fn read_forward(&self, off: usize) -> &[u8] {
        let s = self.as_os_str().as_encoded_bytes();
        &s[off.min(s.len())..]
    }

    fn read_backward(&self, off: usize) -> &[u8] {
        let s = self.as_os_str().as_encoded_bytes();
        &s[..off.min(s.len())]
    }
}
