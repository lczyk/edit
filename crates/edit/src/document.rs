//! Abstractions over reading/writing arbitrary text containers.
//!
//! [`ReadableDocument`] lives in `lsh-defs` (shared with the `Highlighter`);
//! re-exported here so edit's internal callers keep their existing import
//! paths. [`WriteableDocument`] is edit-only -- the buffer is the only thing
//! that mutates docs through this trait.

use std::ffi::OsString;
use std::mem;
use std::ops::Range;
use std::path::PathBuf;

use stdext::ReplaceRange as _;

pub use lsh_defs::ReadableDocument;

/// An abstraction over writing to text containers.
pub trait WriteableDocument: ReadableDocument {
    /// Replace the given range with the given bytes.
    ///
    /// # Warning
    ///
    /// * The given range may be out of bounds and you MUST clamp it.
    /// * The replacement may not be valid UTF8.
    fn replace(&mut self, range: Range<usize>, replacement: &[u8]);
}

impl WriteableDocument for String {
    fn replace(&mut self, range: Range<usize>, replacement: &[u8]) {
        // `replacement` is not guaranteed to be valid UTF-8, so we need to sanitize it.
        let utf8 = String::from_utf8_lossy(replacement);
        // SAFETY: `range` is guaranteed to be on codepoint boundaries.
        unsafe { self.as_mut_vec() }.replace_range(range, utf8.as_bytes());
    }
}

impl WriteableDocument for PathBuf {
    fn replace(&mut self, range: Range<usize>, replacement: &[u8]) {
        let mut vec = mem::take(self).into_os_string().into_encoded_bytes();
        vec.replace_range(range, replacement);
        *self = unsafe { Self::from(OsString::from_encoded_bytes_unchecked(vec)) };
    }
}
