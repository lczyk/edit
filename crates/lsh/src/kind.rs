//! Highlight kinds the runtime emits itself, without a `yield` in any
//! definition. They are pinned to the first discriminants in [`BUILTIN`]
//! order: the compiler seeds them, the optimizer keeps them in front, and
//! the generated `HighlightKind` enum asserts the values line up.

/// Builtin kind identifiers, in discriminant order.
pub const BUILTIN: &[&str] = &["other", "markup.conflict.marker"];

/// No highlight. Also the zero value every line starts with.
pub const OTHER: u32 = 0;

/// A merge-conflict marker line (`<<<<<<<`, `|||||||`, `=======`, `>>>>>>>`).
pub const CONFLICT_MARKER: u32 = 1;

/// Whether `identifier` names a builtin kind, and its pinned value.
pub fn builtin_value(identifier: &str) -> Option<u32> {
    BUILTIN.iter().position(|b| *b == identifier).map(|i| i as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_constants_index_the_list() {
        assert_eq!(BUILTIN[OTHER as usize], "other");
        assert_eq!(BUILTIN[CONFLICT_MARKER as usize], "markup.conflict.marker");
        assert_eq!(builtin_value("markup.conflict.marker"), Some(CONFLICT_MARKER));
        assert_eq!(builtin_value("keyword"), None);
    }
}
