//! ANSI-16 colour mapping for the bundled `HighlightKind` table.
//!
//! Used by callers that emit raw ansi escape sequences (eat's
//! `write_highlighted_line`, future stdout-print modes). Respects the
//! user's terminal palette -- works under light and dark themes
//! without per-theme configuration.
//!
//! The canonical colour table lives on [`HighlightKind::default_color`];
//! this module just composes the `Ansi16 -> "\x1b[..m"` lookup with the
//! enum -> u32 walk so callers get a ready-to-index `&[&str]`.

use lsh::runtime::Ansi16;

use crate::HighlightKind;

/// Build the ansi-16 colourmap, indexed by `HighlightKind as usize`.
/// Pure-attribute kinds (bold / italic / link / strikethrough) carry no
/// foreground colour and get a style-only escape layered on at the end.
pub fn ansi16_color_map() -> Vec<&'static str> {
    let max_kind = (HighlightKind::MarkupStrikethrough as u32)
        .max(HighlightKind::MetaHeader as u32)
        .max(HighlightKind::StorageType as u32)
        .max(HighlightKind::SupportFunction as u32);
    let mut map = vec![""; max_kind as usize + 1];

    for i in 0..=max_kind {
        if let Ok(kind) = HighlightKind::try_from(i)
            && let Some(c) = kind.default_color()
        {
            map[i as usize] = ansi16_sgr(c);
        }
    }

    map[HighlightKind::MarkupBold as usize] = "\x1b[1m";
    map[HighlightKind::MarkupItalic as usize] = "\x1b[3m";
    map[HighlightKind::MarkupLink as usize] = "\x1b[4m";
    map[HighlightKind::MarkupStrikethrough as usize] = "\x1b[9m";

    map
}

/// Map an `Ansi16` enum value to its SGR escape sequence.
pub fn ansi16_sgr(c: Ansi16) -> &'static str {
    match c {
        Ansi16::Black => "\x1b[30m",
        Ansi16::Red => "\x1b[31m",
        Ansi16::Green => "\x1b[32m",
        Ansi16::Yellow => "\x1b[33m",
        Ansi16::Blue => "\x1b[34m",
        Ansi16::Magenta => "\x1b[35m",
        Ansi16::Cyan => "\x1b[36m",
        Ansi16::White => "\x1b[37m",
        Ansi16::BrightBlack => "\x1b[90m",
        Ansi16::BrightRed => "\x1b[91m",
        Ansi16::BrightGreen => "\x1b[92m",
        Ansi16::BrightYellow => "\x1b[93m",
        Ansi16::BrightBlue => "\x1b[94m",
        Ansi16::BrightMagenta => "\x1b[95m",
        Ansi16::BrightCyan => "\x1b[96m",
        Ansi16::BrightWhite => "\x1b[97m",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_non_empty_and_attribute_kinds_present() {
        let map = ansi16_color_map();
        assert!(!map.is_empty());
        assert_eq!(map[HighlightKind::Comment as usize], "\x1b[32m");
        assert_eq!(map[HighlightKind::String as usize], "\x1b[91m");
        assert_eq!(map[HighlightKind::Other as usize], "");
        assert_eq!(map[HighlightKind::MarkupBold as usize], "\x1b[1m");
        assert_eq!(map[HighlightKind::MarkupItalic as usize], "\x1b[3m");
    }
}
