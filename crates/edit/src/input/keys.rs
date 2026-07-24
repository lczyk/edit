//! The input vocabulary: keys, modifiers, mouse events, and the [`Input`]
//! event type the tui consumes.
//!
//! Deliberately free of any wire format. The VT decoder in
//! [`super::vt_decode`] is one producer of these types; a UEFI or GUI
//! backend would be another, and would not need to pull the VT parser in
//! with it. That separation is the point of the split.

use std::fmt;

use crate::helpers::{Point, Size};

/// Represents a key/modifier combination.
///
/// TODO: Is this a good idea? I did it to allow typing `kbmod::CTRL | vk::A`.
/// The reason it's an awkward u32 and not a struct is to hopefully make ABIs easier later.
/// Of course you could just translate on the ABI boundary, but my hope is that this
/// design lets me realize some restrictions early on that I can't foresee yet.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct InputKey(u32);

impl InputKey {
    pub(crate) const fn new(v: u32) -> Self {
        Self(v)
    }

    pub(crate) const fn from_ascii(ch: char) -> Option<Self> {
        if ch == ' ' || (ch >= '0' && ch <= '9') {
            Some(Self(ch as u32))
        } else if ch >= 'a' && ch <= 'z' {
            Some(Self(ch as u32 & !0x20)) // Shift a-z to A-Z
        } else if ch >= 'A' && ch <= 'Z' {
            Some(Self(kbmod::SHIFT.0 | ch as u32))
        } else {
            None
        }
    }

    pub(crate) const fn value(&self) -> u32 {
        self.0
    }

    pub(crate) const fn key(&self) -> Self {
        Self(self.0 & 0x00FFFFFF)
    }

    pub(crate) const fn modifiers(&self) -> InputKeyMod {
        InputKeyMod(self.0 & 0xFF000000)
    }

    pub(crate) const fn modifiers_contains(&self, modifier: InputKeyMod) -> bool {
        (self.0 & modifier.0) != 0
    }

    pub(crate) const fn with_modifiers(&self, modifiers: InputKeyMod) -> Self {
        Self(self.0 | modifiers.0)
    }
}

/// A keyboard modifier. Ctrl/Alt/Shift/Cmd. Cmd (Super) is only reported by
/// terminals that implement the kitty keyboard protocol.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct InputKeyMod(u32);

impl InputKeyMod {
    const fn new(v: u32) -> Self {
        Self(v)
    }

    pub(crate) const fn contains(&self, modifier: Self) -> bool {
        (self.0 & modifier.0) != 0
    }
}

impl std::ops::BitOr<InputKeyMod> for InputKey {
    type Output = Self;

    fn bitor(self, rhs: InputKeyMod) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOr<InputKey> for InputKeyMod {
    type Output = InputKey;

    fn bitor(self, rhs: InputKey) -> InputKey {
        InputKey(self.0 | rhs.0)
    }
}

impl std::ops::BitOr for InputKeyMod {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for InputKeyMod {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl fmt::Display for InputKeyMod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut sep = "";
        if self.contains(kbmod::CTRL) {
            f.write_str(sep)?;
            f.write_str("Ctrl")?;
            sep = "+";
        }
        if self.contains(kbmod::ALT) {
            f.write_str(sep)?;
            f.write_str("Alt")?;
            sep = "+";
        }
        if self.contains(kbmod::SHIFT) {
            f.write_str(sep)?;
            f.write_str("Shift")?;
            sep = "+";
        }
        if self.contains(kbmod::CMD) {
            f.write_str(sep)?;
            f.write_str("Cmd")?;
            sep = "+";
        }
        let _ = sep;
        Ok(())
    }
}

impl fmt::Display for InputKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mods = self.modifiers();
        if mods.0 != 0 {
            write!(f, "{mods}+")?;
        }
        let k = self.key().0;
        match k {
            0x00 => f.write_str("Null"),
            0x08 => f.write_str("Back"),
            0x09 => f.write_str("Tab"),
            0x0D => f.write_str("Return"),
            0x1B => f.write_str("Escape"),
            0x20 => f.write_str("Space"),
            0x21 => f.write_str("PageUp"),
            0x22 => f.write_str("PageDown"),
            0x23 => f.write_str("End"),
            0x24 => f.write_str("Home"),
            0x25 => f.write_str("Left"),
            0x26 => f.write_str("Up"),
            0x27 => f.write_str("Right"),
            0x28 => f.write_str("Down"),
            0x2D => f.write_str("Insert"),
            0x2E => f.write_str("Delete"),
            0x30..=0x39 | 0x41..=0x5A => write!(f, "{}", char::from_u32(k).unwrap_or('?')),
            0x60..=0x69 => write!(f, "Numpad{}", k - 0x60),
            0x6A => f.write_str("Multiply"),
            0x6B => f.write_str("Add"),
            0x6C => f.write_str("Separator"),
            0x6D => f.write_str("Subtract"),
            0x6E => f.write_str("Decimal"),
            0x6F => f.write_str("Divide"),
            0x70..=0x87 => write!(f, "F{}", k - 0x6F),
            _ => write!(f, "0x{k:02X}"),
        }
    }
}

/// Keyboard keys.
///
/// The numbering follows the Win32 `VK_*` constants. Nothing here depends
/// on Windows -- it's just a pre-existing, densely packed keycode table
/// that the parser and keybinding config both index into.
pub mod vk {
    use super::InputKey;

    pub const NULL: InputKey = InputKey::new('\0' as u32);
    pub const BACK: InputKey = InputKey::new(0x08);
    pub const TAB: InputKey = InputKey::new('\t' as u32);
    pub const RETURN: InputKey = InputKey::new('\r' as u32);
    pub const ESCAPE: InputKey = InputKey::new(0x1B);
    pub const SPACE: InputKey = InputKey::new(' ' as u32);
    pub const PRIOR: InputKey = InputKey::new(0x21);
    pub const NEXT: InputKey = InputKey::new(0x22);

    pub const END: InputKey = InputKey::new(0x23);
    pub const HOME: InputKey = InputKey::new(0x24);

    pub const LEFT: InputKey = InputKey::new(0x25);
    pub const UP: InputKey = InputKey::new(0x26);
    pub const RIGHT: InputKey = InputKey::new(0x27);
    pub const DOWN: InputKey = InputKey::new(0x28);

    pub const INSERT: InputKey = InputKey::new(0x2D);
    pub const DELETE: InputKey = InputKey::new(0x2E);

    /// The ASCII `/` codepoint. Reaches the editor as a CSI-u keycode under
    /// the kitty disambiguate flag (e.g. `Ctrl+/` -> `CSI 47;5u`).
    pub const SLASH: InputKey = InputKey::new(b'/' as u32);

    pub const N0: InputKey = InputKey::new('0' as u32);
    pub const N1: InputKey = InputKey::new('1' as u32);
    pub const N2: InputKey = InputKey::new('2' as u32);
    pub const N3: InputKey = InputKey::new('3' as u32);
    pub const N4: InputKey = InputKey::new('4' as u32);
    pub const N5: InputKey = InputKey::new('5' as u32);
    pub const N6: InputKey = InputKey::new('6' as u32);
    pub const N7: InputKey = InputKey::new('7' as u32);
    pub const N8: InputKey = InputKey::new('8' as u32);
    pub const N9: InputKey = InputKey::new('9' as u32);

    pub const A: InputKey = InputKey::new('A' as u32);
    pub const B: InputKey = InputKey::new('B' as u32);
    pub const C: InputKey = InputKey::new('C' as u32);
    pub const D: InputKey = InputKey::new('D' as u32);
    pub const E: InputKey = InputKey::new('E' as u32);
    pub const F: InputKey = InputKey::new('F' as u32);
    pub const G: InputKey = InputKey::new('G' as u32);
    pub const H: InputKey = InputKey::new('H' as u32);
    pub const I: InputKey = InputKey::new('I' as u32);
    pub const J: InputKey = InputKey::new('J' as u32);
    pub const K: InputKey = InputKey::new('K' as u32);
    pub const L: InputKey = InputKey::new('L' as u32);
    pub const M: InputKey = InputKey::new('M' as u32);
    pub const N: InputKey = InputKey::new('N' as u32);
    pub const O: InputKey = InputKey::new('O' as u32);
    pub const P: InputKey = InputKey::new('P' as u32);
    pub const Q: InputKey = InputKey::new('Q' as u32);
    pub const R: InputKey = InputKey::new('R' as u32);
    pub const S: InputKey = InputKey::new('S' as u32);
    pub const T: InputKey = InputKey::new('T' as u32);
    pub const U: InputKey = InputKey::new('U' as u32);
    pub const V: InputKey = InputKey::new('V' as u32);
    pub const W: InputKey = InputKey::new('W' as u32);
    pub const X: InputKey = InputKey::new('X' as u32);
    pub const Y: InputKey = InputKey::new('Y' as u32);
    pub const Z: InputKey = InputKey::new('Z' as u32);

    pub const NUMPAD0: InputKey = InputKey::new(0x60);
    pub const NUMPAD1: InputKey = InputKey::new(0x61);
    pub const NUMPAD2: InputKey = InputKey::new(0x62);
    pub const NUMPAD3: InputKey = InputKey::new(0x63);
    pub const NUMPAD4: InputKey = InputKey::new(0x64);
    pub const NUMPAD5: InputKey = InputKey::new(0x65);
    pub const NUMPAD6: InputKey = InputKey::new(0x66);
    pub const NUMPAD7: InputKey = InputKey::new(0x67);
    pub const NUMPAD8: InputKey = InputKey::new(0x68);
    pub const NUMPAD9: InputKey = InputKey::new(0x69);
    pub const MULTIPLY: InputKey = InputKey::new(0x6A);
    pub const ADD: InputKey = InputKey::new(0x6B);
    pub const SEPARATOR: InputKey = InputKey::new(0x6C);
    pub const SUBTRACT: InputKey = InputKey::new(0x6D);
    pub const DECIMAL: InputKey = InputKey::new(0x6E);
    pub const DIVIDE: InputKey = InputKey::new(0x6F);

    pub const F1: InputKey = InputKey::new(0x70);
    pub const F2: InputKey = InputKey::new(0x71);
    pub const F3: InputKey = InputKey::new(0x72);
    pub const F4: InputKey = InputKey::new(0x73);
    pub const F5: InputKey = InputKey::new(0x74);
    pub const F6: InputKey = InputKey::new(0x75);
    pub const F7: InputKey = InputKey::new(0x76);
    pub const F8: InputKey = InputKey::new(0x77);
    pub const F9: InputKey = InputKey::new(0x78);
    pub const F10: InputKey = InputKey::new(0x79);
    pub const F11: InputKey = InputKey::new(0x7A);
    pub const F12: InputKey = InputKey::new(0x7B);
    pub const F13: InputKey = InputKey::new(0x7C);
    pub const F14: InputKey = InputKey::new(0x7D);
    pub const F15: InputKey = InputKey::new(0x7E);
    pub const F16: InputKey = InputKey::new(0x7F);
    pub const F17: InputKey = InputKey::new(0x80);
    pub const F18: InputKey = InputKey::new(0x81);
    pub const F19: InputKey = InputKey::new(0x82);
    pub const F20: InputKey = InputKey::new(0x83);
    pub const F21: InputKey = InputKey::new(0x84);
    pub const F22: InputKey = InputKey::new(0x85);
    pub const F23: InputKey = InputKey::new(0x86);
    pub const F24: InputKey = InputKey::new(0x87);
}

/// Keyboard modifiers.
pub mod kbmod {
    use super::InputKeyMod;

    pub const NONE: InputKeyMod = InputKeyMod::new(0x00000000);
    pub const CTRL: InputKeyMod = InputKeyMod::new(0x01000000);
    pub const ALT: InputKeyMod = InputKeyMod::new(0x02000000);
    pub const SHIFT: InputKeyMod = InputKeyMod::new(0x04000000);
    pub const CMD: InputKeyMod = InputKeyMod::new(0x08000000);

    pub const CTRL_ALT: InputKeyMod = InputKeyMod::new(0x03000000);
    pub const CTRL_SHIFT: InputKeyMod = InputKeyMod::new(0x05000000);
    pub const ALT_SHIFT: InputKeyMod = InputKeyMod::new(0x06000000);
    pub const CTRL_ALT_SHIFT: InputKeyMod = InputKeyMod::new(0x07000000);
}

/// Mouse input state. Up/Down, Left/Right, etc.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum InputMouseState {
    #[default]
    None,

    // These 3 carry their state between frames.
    Left,
    Middle,
    Right,

    // These 2 get reset to None on the next frame.
    Release,
    Scroll,
}

/// Mouse input.
#[derive(Clone, Copy)]
pub struct InputMouse {
    /// The state of the mouse.Up/Down, Left/Right, etc.
    pub state: InputMouseState,
    /// Any keyboard modifiers that are held down.
    pub modifiers: InputKeyMod,
    /// Position of the mouse in the viewport.
    pub position: Point,
    /// Scroll delta.
    pub scroll: Point,
}

/// Primary result type of the parser.
pub enum Input<'input> {
    /// Window resize event.
    Resize(Size),
    /// Text input.
    /// Note that [`Input::Keyboard`] events can also be text.
    Text(&'input str),
    /// A clipboard paste.
    Paste(Vec<u8>),
    /// Keyboard input.
    Keyboard(InputKey),
    /// Mouse input.
    Mouse(InputMouse),
}
