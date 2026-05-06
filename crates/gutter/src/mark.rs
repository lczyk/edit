/// Per-line marker shown in the margin to indicate how a line differs from
/// its baseline (typically `HEAD:<path>`). Computed externally via
/// [`crate::gutter_diff::compute_marks`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GutterMark {
    None,
    Added,
    Modified,
    /// Lines were deleted immediately above this one.
    DeletedAbove,
    /// Lines were deleted immediately below this one (used at EOF).
    DeletedBelow,
}
