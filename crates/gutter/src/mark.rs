/// Per-line marker shown in the margin to indicate how a line differs from
/// its baseline (typically `HEAD:<path>`). The diff marks are computed
/// externally via [`crate::gutter_diff::compute_marks`]; `Conflict` comes
/// from the syntax highlighter and wins over them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GutterMark {
    None,
    Added,
    Modified,
    /// Lines were deleted immediately above this one.
    DeletedAbove,
    /// Lines were deleted immediately below this one (used at EOF).
    DeletedBelow,
    /// Inside an unresolved merge conflict: a marker line or either side.
    Conflict,
}
