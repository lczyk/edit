//! Render-time quirks. Two independent toggles, both wired up by
//! `--quirks=...` cli flags:
//!
//! - [`set_ascii_only`] / [`ascii_only`] -- swap unicode glyphs for ASCII
//!   fall-backs across the UI.
//! - [`set_no_color`] / [`no_color`] -- suppress all SGR colour output
//!   (text attributes like bold/italic still emitted).
//! - [`set_no_animations`] / [`no_animations`] -- snap cursor/selection/
//!   scroll/menu motion to target instead of lerping. For users who find
//!   the animation distracting or run on a slow / high-latency terminal.

use crate::helpers::CoordType;
use std::sync::atomic::{AtomicBool, Ordering};

static ASCII_ONLY: AtomicBool = AtomicBool::new(false);
static NO_COLOR: AtomicBool = AtomicBool::new(false);
static NO_ANIMATIONS: AtomicBool = AtomicBool::new(false);

pub fn set_ascii_only(enabled: bool) {
    ASCII_ONLY.store(enabled, Ordering::Relaxed);
}

#[inline]
pub fn ascii_only() -> bool {
    ASCII_ONLY.load(Ordering::Relaxed)
}

pub fn set_no_color(enabled: bool) {
    NO_COLOR.store(enabled, Ordering::Relaxed);
}

#[inline]
pub fn no_color() -> bool {
    NO_COLOR.load(Ordering::Relaxed)
}

pub fn set_no_animations(enabled: bool) {
    NO_ANIMATIONS.store(enabled, Ordering::Relaxed);
}

#[inline]
pub fn no_animations() -> bool {
    NO_ANIMATIONS.load(Ordering::Relaxed)
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

// Minimap glyphs.
//
// Unicode mode: braille (U+2800..U+28FF) -- 8-bit dot mask packs 2 dot-columns
// x 4 dot-rows of inky sub-cells into one char. Bit ordering matches Unicode
// braille: dots 1-3 left col top-down, dots 4-6 right col top-down, dots 7/8
// the bottom row of each col.
//
// Ascii mode: a 5-step density ramp picks a block char from a fixed table.

pub fn minimap_braille(dots: u8) -> char {
    char::from_u32(0x2800u32 + dots as u32).unwrap()
}

pub fn minimap_ascii(level: u8) -> char {
    const RAMP: [char; 5] = [' ', '.', ':', '|', '#'];
    RAMP[(level as usize).min(RAMP.len() - 1)]
}

pub fn minimap_width() -> CoordType {
    if ascii_only() { 1 } else { 2 }
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

    #[test]
    fn no_color_default_off() {
        let _g = LOCK.lock().unwrap();
        set_no_color(false);
        assert!(!no_color());
    }

    #[test]
    fn no_color_toggle_round_trip() {
        let _g = LOCK.lock().unwrap();
        set_no_color(false);
        assert!(!no_color());
        set_no_color(true);
        assert!(no_color());
        set_no_color(false);
        assert!(!no_color());
    }

    #[test]
    fn minimap_braille_codepoints() {
        assert_eq!(minimap_braille(0), '\u{2800}');
        assert_eq!(minimap_braille(0xff), '\u{28ff}');
        assert_eq!(minimap_braille(0x01), '\u{2801}');
    }

    #[test]
    fn minimap_ascii_ramp() {
        assert_eq!(minimap_ascii(0), ' ');
        assert_eq!(minimap_ascii(1), '.');
        assert_eq!(minimap_ascii(2), ':');
        assert_eq!(minimap_ascii(3), '|');
        assert_eq!(minimap_ascii(4), '#');
        // Saturating clamp.
        assert_eq!(minimap_ascii(99), '#');
    }

    #[test]
    fn minimap_width_mode_swap() {
        let _g = LOCK.lock().unwrap();
        set_ascii_only(false);
        assert_eq!(minimap_width(), 2);
        set_ascii_only(true);
        assert_eq!(minimap_width(), 1);
        set_ascii_only(false);
    }

    #[test]
    fn no_color_independent_of_ascii_only() {
        let _g = LOCK.lock().unwrap();
        set_ascii_only(false);
        set_no_color(true);
        assert!(!ascii_only());
        assert!(no_color());
        set_no_color(false);
        set_ascii_only(true);
        assert!(ascii_only());
        assert!(!no_color());
        set_ascii_only(false);
    }
}
