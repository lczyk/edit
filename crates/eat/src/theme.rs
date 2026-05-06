use crate::definitions::HighlightKind;

/// ANSI-16 colormap. maps highlight kinds to escape codes.
/// respects user terminal palettes, works under light and dark themes.
pub fn color_map() -> Vec<&'static str> {
    let mut map = Vec::new();
    let max_kind = max_highlight_kind();
    map.resize(max_kind as usize + 1, "");

    macro_rules! set {
        ($kind:ident, $color:expr) => {
            let idx = HighlightKind::$kind as usize;
            if idx >= map.len() {
                map.resize(idx + 1, "");
            }
            map[idx] = $color;
        };
    }

    set!(Other, "");
    set!(Comment, "\x1b[32m"); // green
    set!(Method, "\x1b[93m"); // bright yellow
    set!(String, "\x1b[91m"); // bright red
    set!(Variable, "\x1b[96m"); // bright cyan
    set!(ConstantLanguage, "\x1b[94m"); // bright blue
    set!(ConstantNumeric, "\x1b[92m"); // bright green
    set!(KeywordControl, "\x1b[95m"); // bright magenta
    set!(KeywordOther, "\x1b[94m"); // bright blue
    set!(StorageType, "\x1b[36m"); // cyan
    set!(SupportFunction, "\x1b[33m"); // yellow
    set!(MarkupBold, "\x1b[1m"); // bold
    set!(MarkupChanged, "\x1b[94m"); // bright blue
    set!(MarkupDeleted, "\x1b[91m"); // bright red
    set!(MarkupHeading, "\x1b[94m"); // bright blue
    set!(MarkupInserted, "\x1b[92m"); // bright green
    set!(MarkupItalic, "\x1b[3m"); // italic
    set!(MarkupLink, "\x1b[4m"); // underlined
    set!(MarkupList, "\x1b[94m"); // bright blue
    set!(MarkupStrikethrough, "\x1b[9m"); // strikethrough
    set!(MetaHeader, "\x1b[94m"); // bright blue

    map
}

fn max_highlight_kind() -> u32 {
    (HighlightKind::MarkupStrikethrough as u32)
        .max(HighlightKind::MetaHeader as u32)
        .max(HighlightKind::StorageType as u32)
        .max(HighlightKind::SupportFunction as u32)
}
