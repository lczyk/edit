//! UI glyphs with an ASCII-only toggle. By default returns the unicode
//! variant; when [`set_ascii_only`] is set, the helpers fall back to ASCII
//! forms (driven by the `--quirks=ascii` cli flag).

use std::sync::atomic::{AtomicBool, Ordering};

static ASCII_ONLY: AtomicBool = AtomicBool::new(false);

pub fn set_ascii_only(enabled: bool) {
    ASCII_ONLY.store(enabled, Ordering::Relaxed);
}

#[inline]
pub fn ascii_only() -> bool {
    ASCII_ONLY.load(Ordering::Relaxed)
}

pub fn box_h() -> char {
    if ascii_only() { '-' } else { '─' }
}
pub fn box_v() -> char {
    if ascii_only() { '|' } else { '│' }
}
pub fn box_tl() -> char {
    if ascii_only() { '+' } else { '┌' }
}
pub fn box_tr() -> char {
    if ascii_only() { '+' } else { '┐' }
}
pub fn box_bl() -> char {
    if ascii_only() { '+' } else { '└' }
}
pub fn box_br() -> char {
    if ascii_only() { '+' } else { '┘' }
}
pub fn ellipsis() -> &'static str {
    if ascii_only() { "..." } else { "…" }
}
pub fn ellipsis_char() -> char {
    if ascii_only() { '~' } else { '…' }
}
pub fn modified_dot() -> &'static str {
    if ascii_only() { "* " } else { "● " }
}
pub fn scrollbar_thumb() -> &'static str {
    if ascii_only() { "#" } else { "█" }
}
pub fn wrap_dot() -> char {
    if ascii_only() { '.' } else { '∙' }
}
pub fn visual_space() -> &'static str {
    if ascii_only() { "_" } else { "･" }
}
pub fn visual_tab() -> &'static str {
    if ascii_only() { ">       " } else { "￫       " }
}
pub fn gutter_deleted_above() -> &'static str {
    if ascii_only() { "^" } else { "▴" }
}
pub fn gutter_deleted_below() -> &'static str {
    if ascii_only() { "v" } else { "▾" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialise the tests because the toggle is process-wide global state.
    static LOCK: Mutex<()> = Mutex::new(());

    fn assert_unicode_glyphs() {
        assert_eq!(box_h(), '─');
        assert_eq!(box_v(), '│');
        assert_eq!(box_tl(), '┌');
        assert_eq!(box_tr(), '┐');
        assert_eq!(box_bl(), '└');
        assert_eq!(box_br(), '┘');
        assert_eq!(ellipsis(), "…");
        assert_eq!(ellipsis_char(), '…');
        assert_eq!(modified_dot(), "● ");
        assert_eq!(scrollbar_thumb(), "█");
        assert_eq!(wrap_dot(), '∙');
        assert_eq!(visual_space(), "･");
        assert_eq!(visual_tab(), "￫       ");
        assert_eq!(gutter_deleted_above(), "▴");
        assert_eq!(gutter_deleted_below(), "▾");
    }

    fn assert_ascii_glyphs() {
        assert_eq!(box_h(), '-');
        assert_eq!(box_v(), '|');
        assert_eq!(box_tl(), '+');
        assert_eq!(box_tr(), '+');
        assert_eq!(box_bl(), '+');
        assert_eq!(box_br(), '+');
        assert_eq!(ellipsis(), "...");
        assert_eq!(ellipsis_char(), '~');
        assert_eq!(modified_dot(), "* ");
        assert_eq!(scrollbar_thumb(), "#");
        assert_eq!(wrap_dot(), '.');
        assert_eq!(visual_space(), "_");
        assert_eq!(visual_tab(), ">       ");
        assert_eq!(gutter_deleted_above(), "^");
        assert_eq!(gutter_deleted_below(), "v");
    }

    fn assert_pure_ascii(s: &str) {
        assert!(s.is_ascii(), "expected ASCII-only, got {s:?}");
    }

    #[test]
    fn default_is_unicode() {
        let _g = LOCK.lock().unwrap();
        set_ascii_only(false);
        assert!(!ascii_only());
        assert_unicode_glyphs();
    }

    #[test]
    fn ascii_mode_uses_only_ascii() {
        let _g = LOCK.lock().unwrap();
        set_ascii_only(true);
        assert!(ascii_only());
        assert_ascii_glyphs();
        // every fallback string must be pure ASCII (and chars must fit in 0x7f).
        assert!(box_h().is_ascii());
        assert!(box_v().is_ascii());
        assert!(box_tl().is_ascii());
        assert!(box_tr().is_ascii());
        assert!(box_bl().is_ascii());
        assert!(box_br().is_ascii());
        assert!(ellipsis_char().is_ascii());
        assert!(wrap_dot().is_ascii());
        assert_pure_ascii(ellipsis());
        assert_pure_ascii(modified_dot());
        assert_pure_ascii(scrollbar_thumb());
        assert_pure_ascii(visual_space());
        assert_pure_ascii(visual_tab());
        assert_pure_ascii(gutter_deleted_above());
        assert_pure_ascii(gutter_deleted_below());
        // visual_tab keeps the same cell width as the unicode form.
        assert_eq!(visual_tab().len(), 8);
        set_ascii_only(false);
    }

    #[test]
    fn toggle_round_trip() {
        let _g = LOCK.lock().unwrap();
        set_ascii_only(false);
        assert_unicode_glyphs();
        set_ascii_only(true);
        assert_ascii_glyphs();
        set_ascii_only(false);
        assert_unicode_glyphs();
    }
}
