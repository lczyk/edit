use std::ops::Range;

use crate::document::ReadableDocument;

#[derive(Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Whitespace,
    Newline,
    Separator,
    Word,
}

const fn construct_classifier(separators: &[u8]) -> [CharClass; 256] {
    let mut classifier = [CharClass::Word; 256];

    classifier[b' ' as usize] = CharClass::Whitespace;
    classifier[b'\t' as usize] = CharClass::Whitespace;
    classifier[b'\n' as usize] = CharClass::Newline;
    classifier[b'\r' as usize] = CharClass::Newline;

    let mut i = 0;
    let len = separators.len();
    while i < len {
        let ch = separators[i];
        assert!(ch < 128, "Only ASCII separators are supported.");
        classifier[ch as usize] = CharClass::Separator;
        i += 1;
    }

    classifier
}

const WORD_CLASSIFIER: [CharClass; 256] =
    construct_classifier(br#"`~!@#$%^&*()-=+[{]}\|;:'",.<>/?"#);

/// Finds the next word boundary given a document cursor offset.
/// Returns the offset of the next word boundary.
pub fn word_forward(doc: &dyn ReadableDocument, offset: usize) -> usize {
    word_navigation(WordForward { doc, offset, chunk: &[], chunk_off: 0 })
}

/// The backward version of `word_forward`.
pub fn word_backward(doc: &dyn ReadableDocument, offset: usize) -> usize {
    word_navigation(WordBackward { doc, offset, chunk: &[], chunk_off: 0 })
}

/// Word navigation implementation. Matches the behaviour of VS Code.
fn word_navigation<T: WordNavigation>(mut nav: T) -> usize {
    // First, fill `self.chunk` with at least 1 grapheme.
    nav.read();

    // Skip one newline, if any.
    nav.skip_newline();

    // Skip any whitespace.
    nav.skip_class(CharClass::Whitespace);

    // Skip one word or separator and take note of the class.
    let class = nav.peek(CharClass::Whitespace);
    if matches!(class, CharClass::Separator | CharClass::Word) {
        nav.next();

        let off = nav.offset();

        // Continue skipping the same class.
        nav.skip_class(class);

        // If the class was a separator and we only moved one character,
        // continue skipping characters of the word class.
        if off == nav.offset() && class == CharClass::Separator {
            nav.skip_class(CharClass::Word);
        }
    }

    nav.offset()
}

trait WordNavigation {
    fn read(&mut self);
    fn skip_newline(&mut self);
    fn skip_class(&mut self, class: CharClass);
    fn peek(&self, default: CharClass) -> CharClass;
    fn next(&mut self);
    fn offset(&self) -> usize;
}

struct WordForward<'a> {
    doc: &'a dyn ReadableDocument,
    offset: usize,
    chunk: &'a [u8],
    chunk_off: usize,
}

impl WordNavigation for WordForward<'_> {
    fn read(&mut self) {
        self.chunk = self.doc.read_forward(self.offset);
        self.chunk_off = 0;
    }

    fn skip_newline(&mut self) {
        // We can rely on the fact that the document does not split graphemes across chunks.
        // = If there's a newline it's wholly contained in this chunk.
        // Unlike with `WordBackward`, we can't check for CR and LF separately as only a CR followed
        // by a LF is a newline. A lone CR in the document is just a regular control character.
        self.chunk_off += match self.chunk.get(self.chunk_off) {
            Some(&b'\n') => 1,
            Some(&b'\r') if self.chunk.get(self.chunk_off + 1) == Some(&b'\n') => 2,
            _ => 0,
        }
    }

    fn skip_class(&mut self, class: CharClass) {
        while !self.chunk.is_empty() {
            while self.chunk_off < self.chunk.len() {
                if WORD_CLASSIFIER[self.chunk[self.chunk_off] as usize] != class {
                    return;
                }
                self.chunk_off += 1;
            }

            self.offset += self.chunk.len();
            self.chunk = self.doc.read_forward(self.offset);
            self.chunk_off = 0;
        }
    }

    fn peek(&self, default: CharClass) -> CharClass {
        if self.chunk_off < self.chunk.len() {
            WORD_CLASSIFIER[self.chunk[self.chunk_off] as usize]
        } else {
            default
        }
    }

    fn next(&mut self) {
        self.chunk_off += 1;
    }

    fn offset(&self) -> usize {
        self.offset + self.chunk_off
    }
}

struct WordBackward<'a> {
    doc: &'a dyn ReadableDocument,
    offset: usize,
    chunk: &'a [u8],
    chunk_off: usize,
}

impl WordNavigation for WordBackward<'_> {
    fn read(&mut self) {
        self.chunk = self.doc.read_backward(self.offset);
        self.chunk_off = self.chunk.len();
    }

    fn skip_newline(&mut self) {
        // We can rely on the fact that the document does not split graphemes across chunks.
        // = If there's a newline it's wholly contained in this chunk.
        if self.chunk_off > 0 && self.chunk[self.chunk_off - 1] == b'\n' {
            self.chunk_off -= 1;
        }
        if self.chunk_off > 0 && self.chunk[self.chunk_off - 1] == b'\r' {
            self.chunk_off -= 1;
        }
    }

    fn skip_class(&mut self, class: CharClass) {
        while !self.chunk.is_empty() {
            while self.chunk_off > 0 {
                if WORD_CLASSIFIER[self.chunk[self.chunk_off - 1] as usize] != class {
                    return;
                }
                self.chunk_off -= 1;
            }

            self.offset -= self.chunk.len();
            self.chunk = self.doc.read_backward(self.offset);
            self.chunk_off = self.chunk.len();
        }
    }

    fn peek(&self, default: CharClass) -> CharClass {
        if self.chunk_off > 0 {
            WORD_CLASSIFIER[self.chunk[self.chunk_off - 1] as usize]
        } else {
            default
        }
    }

    fn next(&mut self) {
        self.chunk_off -= 1;
    }

    fn offset(&self) -> usize {
        self.offset - self.chunk.len() + self.chunk_off
    }
}

/// Returns the offset range of the "word" at the given offset.
/// Does not cross newlines. Works similar to VS Code.
pub fn word_select(doc: &dyn ReadableDocument, offset: usize) -> Range<usize> {
    let mut beg = offset;
    let mut end = offset;
    let mut class = CharClass::Newline;

    let mut chunk = doc.read_forward(end);
    if !chunk.is_empty() {
        // Not at the end of the document? Great!
        // We default to using the next char as the class, because in terminals
        // the cursor is usually always to the left of the cell you clicked on.
        class = WORD_CLASSIFIER[chunk[0] as usize];

        let mut chunk_off = 0;

        // Select the word, unless we hit a newline.
        if class != CharClass::Newline {
            loop {
                chunk_off += 1;
                end += 1;

                if chunk_off >= chunk.len() {
                    chunk = doc.read_forward(end);
                    chunk_off = 0;
                    if chunk.is_empty() {
                        break;
                    }
                }

                if WORD_CLASSIFIER[chunk[chunk_off] as usize] != class {
                    break;
                }
            }
        }
    }

    let mut chunk = doc.read_backward(beg);
    if !chunk.is_empty() {
        let mut chunk_off = chunk.len();

        // If we failed to determine the class, because we hit the end of the document
        // or a newline, we fall back to using the previous character, of course.
        if class == CharClass::Newline {
            class = WORD_CLASSIFIER[chunk[chunk_off - 1] as usize];
        }

        // Select the word, unless we hit a newline.
        if class != CharClass::Newline {
            loop {
                if WORD_CLASSIFIER[chunk[chunk_off - 1] as usize] != class {
                    break;
                }

                chunk_off -= 1;
                beg -= 1;

                if chunk_off == 0 {
                    chunk = doc.read_backward(beg);
                    chunk_off = chunk.len();
                    if chunk.is_empty() {
                        break;
                    }
                }
            }
        }
    }

    beg..end
}

// --- cursor movement compositions -------------------------------------
//
// Higher-level moves built from the primitives above and TextBuffer's own
// cursor API. They live here rather than in the editor binary because
// nothing about them is app-specific -- any embedder driving a textarea
// wants the same behaviour.

use crate::buffer::TextBuffer;
use crate::helpers::{CoordType, Point};

/// Move the cursor `delta` visual rows, keeping the preferred column and
/// scrolling by the same amount so the cursor holds its position on
/// screen rather than drifting toward an edge.
pub fn small_jump(tb: &mut TextBuffer, delta: CoordType) {
    let x = tb.preferred_column();
    let max_y = (tb.visual_line_count() - 1).max(0);
    let y = (tb.cursor_visual_pos().y + delta).clamp(0, max_y);
    tb.cursor_move_to_visual(Point { x, y });
    tb.request_scroll_delta_y(delta);
}

/// [`small_jump`], extending the selection instead of moving the cursor.
pub fn small_jump_select(tb: &mut TextBuffer, delta: CoordType) {
    let x = tb.preferred_column();
    let max_y = (tb.visual_line_count() - 1).max(0);
    let y = (tb.cursor_visual_pos().y + delta).clamp(0, max_y);
    tb.selection_update_visual(Point { x, y });
    tb.request_scroll_delta_y(delta);
}

/// Home, with the usual editor twist: go to the first non-whitespace
/// character, and only to column 0 if already there.
pub fn smart_line_start(tb: &mut TextBuffer, select: bool) {
    let cur = tb.cursor_logical_pos();
    let indent_end = tb.indent_end_logical_pos();
    let target = if cur.x > indent_end.x { indent_end } else { Point { x: 0, y: cur.y } };
    if select {
        tb.selection_update_logical(target);
    } else {
        tb.cursor_move_to_logical(target);
    }
    tb.set_preferred_column(tb.cursor_visual_pos().x);
    tb.make_cursor_visible();
}

/// End: move to the last column of the current logical line.
pub fn line_end(tb: &mut TextBuffer, select: bool) {
    let y = tb.cursor_logical_pos().y;
    let target = Point { x: CoordType::MAX, y };
    if select {
        tb.selection_update_logical(target);
    } else {
        tb.cursor_move_to_logical(target);
    }
    tb.set_preferred_column(tb.cursor_visual_pos().x);
    tb.make_cursor_visible();
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_word_navigation() {
        assert_eq!(word_forward(&"Hello World".as_bytes(), 0), 5);
        assert_eq!(word_forward(&"Hello,World".as_bytes(), 0), 5);
        assert_eq!(word_forward(&"   Hello".as_bytes(), 0), 8);
        assert_eq!(word_forward(&"\n\nHello".as_bytes(), 0), 1);

        assert_eq!(word_backward(&"Hello World".as_bytes(), 11), 6);
        assert_eq!(word_backward(&"Hello,World".as_bytes(), 10), 6);
        assert_eq!(word_backward(&"Hello   ".as_bytes(), 7), 0);
        assert_eq!(word_backward(&"Hello\n\n".as_bytes(), 7), 6);
    }
}
