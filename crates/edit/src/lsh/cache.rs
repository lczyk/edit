use lsh::runtime::Highlight;
use stdext::arena::{Arena, scratch_arena};
use stdext::collections::BVec;

use crate::helpers::CoordType;
use crate::lsh::{HighlightKind, Highlighter, HighlighterState};

#[cfg(debug_assertions)]
const INTERVAL: CoordType = 16;
#[cfg(not(debug_assertions))]
const INTERVAL: CoordType = 1024;

#[derive(Default)]
pub struct HighlighterCache {
    checkpoints: Vec<HighlighterState>,
    /// Counts checkpoint-restoring seeks so the coherence check below can run
    /// on a fraction of them -- it re-parses the file from the top.
    #[cfg(feature = "sanity")]
    seeks: u32,
}

/// One in this many checkpoint-restoring seeks gets verified against a full
/// re-parse. Low enough to stay interactive on a large file.
#[cfg(feature = "sanity")]
const VERIFY_EVERY: u32 = 32;

impl HighlighterCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Drop any cached states starting at (including) the given logical line.
    pub fn invalidate_from(&mut self, line: CoordType) {
        self.checkpoints.truncate(Self::ceil_line_to_offset(line));
    }

    /// Parse the given logical line. Returns the highlight spans.
    pub fn parse_line<'a>(
        &mut self,
        arena: &'a Arena,
        highlighter: &mut Highlighter,
        line: CoordType,
    ) -> BVec<'a, Highlight<HighlightKind>> {
        let seeked = line != highlighter.logical_pos_y();

        // Do we need to random seek?
        if seeked {
            // If so, restore the nearest, preceding checkpoint...
            if !self.checkpoints.is_empty() {
                let n = Self::floor_line_to_offset(line);
                let n = n.min(self.checkpoints.len() - 1);
                highlighter.restore(&self.checkpoints[n]);
            } else {
                // The assumption is that you pass in a default constructed highlighter,
                // and this class handles random seeking for you. As such, there should
                // never be a case where we don't have a checkpoint for line 0,
                // but you have a highlighter for line >0.
                crate::sanity_assert!(
                    highlighter_at_line_zero,
                    highlighter.logical_pos_y() == 0,
                    "highlighter.logical_pos_y={} (expected 0 when no checkpoint)",
                    highlighter.logical_pos_y()
                );
            }

            // ...and then seek in front of the requested line.
            while highlighter.logical_pos_y() < line {
                // There's a bit of waste here, because we just throw away the results,
                // but that's better than duplicating the logic. The arena is very fast.
                let scratch = scratch_arena(Some(arena));
                _ = self.parse_line_impl(&scratch, highlighter);
            }
        }

        let spans = self.parse_line_impl(arena, highlighter);

        // Spans are half-open `[start, next.start)`, so a line's spans have to
        // step strictly forward. A duplicated or out-of-order start means the
        // compiler pipeline emitted overlapping tokens, which shows up only as
        // odd-looking colour.
        #[cfg(feature = "sanity")]
        if let Some(bad) = spans.windows(2).position(|w| w[1].start <= w[0].start) {
            crate::sanity_check!(
                highlighter_spans_monotonic,
                false,
                "line {line}: span {} starts at {} after {}",
                bad + 1,
                spans[bad + 1].start,
                spans[bad].start
            );
        }

        #[cfg(feature = "sanity")]
        if seeked {
            self.verify_against_full_reparse(arena, highlighter, line, &spans);
        }

        spans
    }

    /// Compares a checkpoint-restored parse against parsing the file from the
    /// top. The cache exists to avoid exactly that walk, so a mismatch means a
    /// checkpoint outlived the edit that should have invalidated it -- the
    /// stale-highlight class. Costs a full re-parse, hence [`VERIFY_EVERY`].
    #[cfg(feature = "sanity")]
    fn verify_against_full_reparse(
        &mut self,
        arena: &Arena,
        highlighter: &Highlighter,
        line: CoordType,
        spans: &[Highlight<HighlightKind>],
    ) {
        self.seeks = self.seeks.wrapping_add(1);
        if !self.seeks.is_multiple_of(VERIFY_EVERY) {
            return;
        }

        // Without a line-0 checkpoint there is nothing to re-parse from: the
        // `highlighter_at_line_zero` assert above already covers that case.
        let Some(first) = self.checkpoints.first() else {
            return;
        };

        let scratch = scratch_arena(Some(arena));
        let mut fresh = highlighter.clone();
        fresh.restore(first);
        while fresh.logical_pos_y() < line {
            let inner = scratch_arena(Some(&scratch));
            _ = fresh.parse_next_line(&inner);
        }
        let expected = fresh.parse_next_line(&scratch);

        crate::sanity_check!(
            highlighter_cache_coherent,
            &expected[..] == spans,
            "line {line}: cached {:?} != re-parsed {:?}",
            spans,
            &expected[..]
        );
    }

    fn parse_line_impl<'a>(
        &mut self,
        arena: &'a Arena,
        highlighter: &mut Highlighter,
    ) -> BVec<'a, Highlight<HighlightKind>> {
        // If we need to store a checkpoint for the start of the next line, do so now.
        if Self::floor_line_to_offset(highlighter.logical_pos_y()) == self.checkpoints.len() {
            self.checkpoints.push(highlighter.snapshot());
        }

        highlighter.parse_next_line(arena)
    }

    /// Since this line cache is super simplistic (no insertions, only append),
    /// we can directly map from line numbers to offsets in the cache.
    fn floor_line_to_offset(line: CoordType) -> usize {
        (line / INTERVAL).try_into().unwrap_or(0)
    }

    fn ceil_line_to_offset(line: CoordType) -> usize {
        ((line + INTERVAL - 1) / INTERVAL).try_into().unwrap_or(0)
    }
}
