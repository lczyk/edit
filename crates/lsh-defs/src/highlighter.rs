//! Stateful line-by-line highlighter wrapper over [`lsh::runtime::Runtime`].
//!
//! Holds the current parse offset + the runtime's internal state machine;
//! `parse_next_line` advances one logical line and returns the spans for
//! that line. Snapshot/restore lets callers (e.g. edit's incremental
//! `HighlighterCache`) jump backwards across previously-seen lines without
//! re-parsing from the top.

use lsh::runtime::*;
use stdext::arena::{Arena, scratch_arena};
use stdext::collections::BVec;
use stdext::simd;
use stdext::{CoordType, KIBI};

use crate::ReadableDocument;
use crate::{ASSEMBLY, CHARSETS, HighlightKind, STRINGS};

const MAX_LINE_LEN: usize = 32 * KIBI;

#[derive(Clone)]
pub struct Highlighter<'a> {
    doc: &'a dyn ReadableDocument,
    offset: usize,
    logical_pos_y: CoordType,
    runtime: Runtime<'static, 'static, 'static>,
}

#[derive(Clone)]
pub struct HighlighterState {
    offset: usize,
    logical_pos_y: CoordType,
    state: RuntimeState,
}

impl<'doc> Highlighter<'doc> {
    pub fn new(doc: &'doc dyn ReadableDocument, language: &'static Language) -> Self {
        Self {
            doc,
            offset: 0,
            logical_pos_y: 0,
            runtime: Runtime::new(&ASSEMBLY, &STRINGS, &CHARSETS, language.entrypoint),
        }
    }

    pub fn logical_pos_y(&self) -> CoordType {
        self.logical_pos_y
    }

    /// Create a restorable snapshot of the current highlighter state
    /// so we can resume highlighting from this point later.
    pub fn snapshot(&self) -> HighlighterState {
        HighlighterState {
            offset: self.offset,
            logical_pos_y: self.logical_pos_y,
            state: self.runtime.snapshot(),
        }
    }

    /// Restore the highlighter state from a previously captured snapshot.
    pub fn restore(&mut self, snapshot: &HighlighterState) {
        self.offset = snapshot.offset;
        self.logical_pos_y = snapshot.logical_pos_y;
        self.runtime.restore(&snapshot.state);
    }

    pub fn parse_next_line<'a>(&mut self, arena: &'a Arena) -> BVec<'a, Highlight<HighlightKind>> {
        let scratch = scratch_arena(Some(arena));
        let (line_off, line) = self.read_next_line(&scratch);

        // Empty lines can be somewhat common.
        //
        // If the line is too long, we don't highlight it.
        // This is to prevent performance issues with very long lines.
        if line.is_empty() || line.len() >= MAX_LINE_LEN {
            return BVec::empty();
        }

        let line = strip_newline(line);
        let mut res = self.runtime.parse_next_line(arena, line);

        // Adjust the range to account for the line offset.
        for h in res.iter_mut() {
            h.start = line_off + h.start.min(line.len());
        }

        res
    }

    fn read_next_line<'a>(&mut self, arena: &'a Arena) -> (usize, &'a [u8])
    where
        'doc: 'a,
    {
        self.logical_pos_y += 1;

        let line_beg = self.offset;
        let mut chunk;
        let mut line_buf;

        // Try to read a chunk and see if it contains a newline.
        // In that case we can skip concatenating chunks.
        {
            chunk = self.doc.read_forward(self.offset);
            if chunk.is_empty() {
                return (line_beg, chunk);
            }

            let (off, line) = simd::lines_fwd(chunk, 0, 0, 1);
            self.offset += off;

            if line == 1 {
                return (line_beg, &chunk[..off]);
            }

            let next_chunk = self.doc.read_forward(self.offset);
            if next_chunk.is_empty() {
                return (line_beg, &chunk[..off]);
            }

            line_buf = BVec::empty();

            // Ensure we don't overflow the heap size with a 1GB long line.
            let end = off.min(MAX_LINE_LEN - line_buf.len());
            let end = end.min(chunk.len());
            line_buf.extend_from_slice(arena, &chunk[..end]);

            chunk = next_chunk;
        }

        // Concatenate chunks until we get a full line.
        while line_buf.len() < MAX_LINE_LEN {
            let (off, line) = simd::lines_fwd(chunk, 0, 0, 1);
            self.offset += off;

            // Ensure we don't overflow the heap size with a 1GB long line.
            let end = off.min(MAX_LINE_LEN - line_buf.len());
            let end = end.min(chunk.len());
            line_buf.extend_from_slice(arena, &chunk[..end]);

            // Start of the next line found.
            if line == 1 {
                break;
            }

            chunk = self.doc.read_forward(self.offset);
            if chunk.is_empty() {
                break;
            }
        }

        (line_beg, line_buf.leak())
    }
}

/// Strip a trailing `\n` or `\r\n` from a byte slice. Inlined here so
/// `lsh-defs` doesn't have to depend on `edit::unicode`.
fn strip_newline(mut text: &[u8]) -> &[u8] {
    if text.last() == Some(&b'\n') {
        text = &text[..text.len() - 1];
    }
    if text.last() == Some(&b'\r') {
        text = &text[..text.len() - 1];
    }
    text
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use stdext::arena::Arena;

    use super::*;
    use crate::{HighlightKind, LANGUAGES};

    fn lang(id: &str) -> &'static lsh::runtime::Language {
        LANGUAGES.iter().find(|l| l.id == id).unwrap()
    }

    /// Document that hands back at most `chunk` bytes per `read_forward`,
    /// to exercise the multi-chunk concat path in `read_next_line`.
    struct ChunkedDoc<'a> {
        data: &'a [u8],
        chunk: Cell<usize>,
    }

    impl<'a> ReadableDocument for ChunkedDoc<'a> {
        fn read_forward(&self, off: usize) -> &[u8] {
            let s = self.data;
            let off = off.min(s.len());
            let end = (off + self.chunk.get()).min(s.len());
            &s[off..end]
        }
        fn read_backward(&self, off: usize) -> &[u8] {
            let s = self.data;
            &s[..off.min(s.len())]
        }
    }

    fn collect(doc: &dyn ReadableDocument, lang_id: &str) -> Vec<(usize, &'static str)> {
        let arena = Arena::new(1 << 20).unwrap();
        let mut h = Highlighter::new(doc, lang(lang_id));
        let kind_name = |k: HighlightKind| -> &'static str {
            match k {
                HighlightKind::Other => "other",
                HighlightKind::Comment => "comment",
                HighlightKind::Keyword => "keyword",
                HighlightKind::Method => "method",
                HighlightKind::String => "string",
                HighlightKind::Variable => "variable",
                HighlightKind::ConstantCharacterEscape => "constant.character.escape",
                HighlightKind::ConstantLanguage => "constant.language",
                HighlightKind::ConstantNumeric => "constant.numeric",
                HighlightKind::KeywordControl => "keyword.control",
                HighlightKind::KeywordOther => "keyword.other",
                HighlightKind::MarkupBold => "markup.bold",
                HighlightKind::MarkupChanged => "markup.changed",
                HighlightKind::MarkupDeleted => "markup.deleted",
                HighlightKind::MarkupHeading => "markup.heading",
                HighlightKind::MarkupInserted => "markup.inserted",
                HighlightKind::MarkupItalic => "markup.italic",
                HighlightKind::MarkupLink => "markup.link",
                HighlightKind::MarkupList => "markup.list",
                HighlightKind::MarkupStrikethrough => "markup.strikethrough",
                HighlightKind::MetaHeader => "meta.header",
                HighlightKind::StorageType => "storage.type",
                HighlightKind::SupportFunction => "support.function",
            }
        };
        let mut out = Vec::new();
        loop {
            let spans = h.parse_next_line(&arena);
            if spans.is_empty() {
                break;
            }
            for s in spans.iter() {
                out.push((s.start, kind_name(s.kind)));
            }
        }
        out
    }

    #[test]
    fn offsets_are_absolute_across_lines() {
        // markdown: heading on line 1, list on line 3.
        let src = b"# Title\n\nlines\n- item\n";
        let spans = collect(&src.as_slice(), "markdown");
        // The heading span starts at offset 0 on the first line.
        assert!(spans.iter().any(|&(o, k)| o == 0 && k == "markup.heading"));
        // "- " on line 4 (after "# Title\n\nlines\n" = 15 bytes) -> start 15.
        assert!(
            spans.iter().any(|&(o, k)| o == 15 && k == "markup.list"),
            "expected markup.list at offset 15, got {spans:?}"
        );
    }

    #[test]
    fn chunking_preserves_tokens() {
        // Same source, but the doc returns one byte at a time. The line
        // concat path must reassemble lines and produce identical spans.
        let src: &[u8] = b"# Title\n\n- item\n";
        let baseline = collect(&src, "markdown");
        for chunk in [1usize, 2, 3, 4, 7, 16] {
            let doc = ChunkedDoc { data: src, chunk: Cell::new(chunk) };
            let got = collect(&doc, "markdown");
            assert_eq!(got, baseline, "chunked doc (chunk={chunk}) diverged from full read");
        }
    }

    #[test]
    fn very_long_line_yields_no_spans() {
        // Lines >= MAX_LINE_LEN should be skipped (returns empty spans).
        let mut data = vec![b'x'; MAX_LINE_LEN];
        data.push(b'\n');
        let h_data = data.as_slice();
        let arena = Arena::new(8 * 1024 * 1024).unwrap();
        let mut h = Highlighter::new(&h_data, lang("markdown"));
        let spans = h.parse_next_line(&arena);
        assert!(spans.is_empty(), "expected long line to be skipped");
    }
}
