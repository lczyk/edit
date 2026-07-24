//! Find and replace, backed by ICU regex.
//!
//! The buffer is exposed to ICU through a `UText` adapter that reads
//! directly out of the gap buffer rather than copying the document into
//! a UTF-16 scratch, so a search over a large file stays cheap. The
//! adapter caches converted chunks keyed on the buffer generation; any
//! edit bumps the generation and invalidates them.

use super::*;

impl TextBuffer {
    /// Find the next occurrence of the given `pattern` and select it.
    pub fn find_and_select(&mut self, pattern: &str, options: SearchOptions) -> icu::Result<()> {
        if let Some(search) = &mut self.search {
            let search = search.get_mut();
            // When the search input changes we must reset the search.
            if search.pattern != pattern || search.options != options {
                self.search = None;
            }

            // When transitioning from some search to no search, we must clear the selection.
            if pattern.is_empty()
                && let Some(TextBufferSelection { beg, .. }) = self.selection
            {
                self.cursor_move_to_logical(beg);
            }
        }

        if pattern.is_empty() {
            return Ok(());
        }

        let search = match &self.search {
            Some(search) => unsafe { &mut *search.get() },
            None => {
                let search = self.find_construct_search(pattern, options)?;
                self.search = Some(UnsafeCell::new(search));
                unsafe { &mut *self.search.as_ref().unwrap().get() }
            }
        };

        // If we previously searched through the entire document and found 0 matches,
        // then we can avoid searching again.
        if search.no_matches {
            return Ok(());
        }

        // If the user moved the cursor since the last search, but the needle remained the same,
        // we still need to move the start of the search to the new cursor position.
        let next_search_offset = match self.selection {
            Some(TextBufferSelection { beg, end }) => {
                if self.selection_generation == search.selection_generation {
                    search.next_search_offset
                } else {
                    self.cursor_move_to_logical_internal(self.cursor, beg.min(end)).offset
                }
            }
            _ => self.cursor.offset,
        };

        self.find_select_next(search, next_search_offset, true);
        Ok(())
    }

    /// Find the next occurrence of the given `pattern` and replace it with `replacement`.
    pub fn find_and_replace(
        &mut self,
        pattern: &str,
        options: SearchOptions,
        replacement: &[u8],
    ) -> icu::Result<()> {
        // Editors traditionally replace the previous search hit, not the next possible one.
        if let (Some(search), Some(..)) = (&self.search, &self.selection) {
            let search = unsafe { &mut *search.get() };
            if search.selection_generation == self.selection_generation {
                let scratch = scratch_arena(None);
                let parsed_replacements =
                    Self::find_parse_replacement(&scratch, &mut *search, replacement);
                let replacement =
                    self.find_fill_replacement(&mut *search, replacement, &parsed_replacements);
                self.write(&replacement, self.cursor, true);
            }
        }

        self.find_and_select(pattern, options)
    }

    /// Find all occurrences of the given `pattern` and replace them with `replacement`.
    pub fn find_and_replace_all(
        &mut self,
        pattern: &str,
        options: SearchOptions,
        replacement: &[u8],
    ) -> icu::Result<()> {
        let scratch = scratch_arena(None);
        let mut search = self.find_construct_search(pattern, options)?;
        let mut offset = 0;
        let parsed_replacements = Self::find_parse_replacement(&scratch, &mut search, replacement);

        loop {
            self.find_select_next(&mut search, offset, false);
            if !self.has_selection() {
                break;
            }

            let replacement =
                self.find_fill_replacement(&mut search, replacement, &parsed_replacements);
            self.write(&replacement, self.cursor, true);
            offset = self.cursor.offset;
        }

        Ok(())
    }

    fn find_construct_search(
        &self,
        pattern: &str,
        options: SearchOptions,
    ) -> icu::Result<ActiveSearch> {
        if pattern.is_empty() {
            return Err(icu::ILLEGAL_ARGUMENT_ERROR);
        }

        let sanitized_pattern = if options.whole_word && options.use_regex {
            Cow::Owned(format!(r"\b(?:{pattern})\b"))
        } else if options.whole_word {
            let mut p = String::with_capacity(pattern.len() + 16);
            p.push_str(r"\b");

            // Escape regex special characters.
            let b = unsafe { p.as_mut_vec() };
            for &byte in pattern.as_bytes() {
                match byte {
                    b'*' | b'?' | b'+' | b'[' | b'(' | b')' | b'{' | b'}' | b'^' | b'$' | b'|'
                    | b'\\' | b'.' => {
                        b.push(b'\\');
                        b.push(byte);
                    }
                    _ => b.push(byte),
                }
            }

            p.push_str(r"\b");
            Cow::Owned(p)
        } else {
            Cow::Borrowed(pattern)
        };

        let mut flags = icu::Regex::MULTILINE;
        if !options.match_case {
            flags |= icu::Regex::CASE_INSENSITIVE;
        }
        if !options.use_regex && !options.whole_word {
            flags |= icu::Regex::LITERAL;
        }

        // Move the start of the search to the start of the selection,
        // or otherwise to the current cursor position.

        let text = unsafe { icu::Text::new(self)? };
        let regex = unsafe { icu::Regex::new(&sanitized_pattern, flags, &text)? };

        Ok(ActiveSearch {
            pattern: pattern.to_string(),
            options,
            text,
            regex,
            buffer_generation: self.buffer.generation(),
            selection_generation: 0,
            next_search_offset: 0,
            no_matches: false,
        })
    }

    fn find_select_next(&mut self, search: &mut ActiveSearch, offset: usize, wrap: bool) {
        if search.buffer_generation != self.buffer.generation() {
            unsafe { search.regex.set_text(&mut search.text, offset) };
            search.buffer_generation = self.buffer.generation();
            search.next_search_offset = offset;
        } else if search.next_search_offset != offset {
            search.next_search_offset = offset;
            search.regex.reset(offset);
        }

        let mut hit = search.regex.next();

        // If we hit the end of the buffer, and we know that there's something to find,
        // start the search again from the beginning (= wrap around).
        if wrap && hit.is_none() && search.next_search_offset != 0 {
            search.next_search_offset = 0;
            search.regex.reset(0);
            hit = search.regex.next();
        }

        search.selection_generation = if let Some(range) = hit {
            // Now the search offset is no more at the start of the buffer.
            search.next_search_offset = range.end;

            let beg = self.cursor_move_to_offset_internal(self.cursor, range.start);
            let end = self.cursor_move_to_offset_internal(beg, range.end);

            unsafe { self.set_cursor(end) };
            self.make_cursor_visible();

            self.set_selection(Some(TextBufferSelection {
                beg: beg.logical_pos,
                end: end.logical_pos,
            }))
        } else {
            // Avoid searching through the entire document again if we know there's nothing to find.
            search.no_matches = true;
            self.set_selection(None)
        };
    }

    fn find_parse_replacement<'a>(
        arena: &'a Arena,
        search: &mut ActiveSearch,
        replacement: &[u8],
    ) -> BVec<'a, RegexReplacement<'a>> {
        let mut res = BVec::empty();

        if !search.options.use_regex {
            return res;
        }

        let group_count = search.regex.group_count();
        let mut text = BVec::empty();
        let mut text_beg = 0;

        loop {
            let mut off = memchr2(b'$', b'\\', replacement, text_beg);

            // Push the raw, unescaped text, if any.
            if text_beg < off {
                text.extend_from_slice(arena, &replacement[text_beg..off]);
            }

            // Unescape any escaped characters.
            while off < replacement.len() && replacement[off] == b'\\' {
                off += 2;

                // If this backslash is the last character (e.g. because
                // `replacement` is just 1 byte long, holding just b"\\"),
                // we can't unescape it. In that case, we map it to `b'\\'` here.
                // This results in us appending a literal backslash to the text.
                let ch = replacement.get(off - 1).map_or(b'\\', |&c| c);

                // Unescape and append the character.
                text.push(
                    arena,
                    match ch {
                        b'n' => b'\n',
                        b'r' => b'\r',
                        b't' => b'\t',
                        ch => ch,
                    },
                );
            }

            // Parse out a group number, if any.
            let mut group = -1;
            if off < replacement.len() && replacement[off] == b'$' {
                let mut beg = off;
                let mut end = off + 1;
                let mut acc = 0i32;
                let mut acc_bad = true;

                if end < replacement.len() {
                    let ch = replacement[end];

                    if ch == b'$' {
                        // Translate "$$" to "$".
                        beg += 1;
                        end += 1;
                    } else if ch.is_ascii_digit() {
                        // Parse "$1234" into 1234i32.
                        // If the number is larger than the group count,
                        // we flag `acc_bad` which causes us to treat it as text.
                        acc_bad = false;
                        while {
                            acc =
                                acc.wrapping_mul(10).wrapping_add((replacement[end] - b'0') as i32);
                            acc_bad |= acc > group_count;
                            end += 1;
                            end < replacement.len() && replacement[end].is_ascii_digit()
                        } {}
                    }
                }

                if !acc_bad {
                    group = acc;
                } else {
                    text.extend_from_slice(arena, &replacement[beg..end]);
                }

                off = end;
            }

            if !text.is_empty() {
                res.push(arena, RegexReplacement::Text(text));
                text = BVec::empty();
            }
            if group >= 0 {
                res.push(arena, RegexReplacement::Group(group));
            }

            text_beg = off;
            if text_beg >= replacement.len() {
                break;
            }
        }

        res
    }

    fn find_fill_replacement<'a>(
        &self,
        search: &mut ActiveSearch,
        replacement: &'a [u8],
        parsed_replacements: &[RegexReplacement],
    ) -> Cow<'a, [u8]> {
        if !search.options.use_regex {
            Cow::Borrowed(replacement)
        } else {
            let mut res = Vec::new();

            for replacement in parsed_replacements {
                match replacement {
                    RegexReplacement::Text(text) => res.extend_from_slice(text),
                    RegexReplacement::Group(group) => {
                        if let Some(range) = search.regex.group(*group) {
                            self.buffer.extract_raw(range, &mut res, usize::MAX);
                        }
                    }
                }
            }

            Cow::Owned(res)
        }
    }
}
