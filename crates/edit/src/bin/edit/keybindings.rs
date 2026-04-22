//! Keybindings config loader.
//!
//! TOML file at `<config_dir>/keybindings.toml`. Created on first run from
//! the embedded default. Missing actions after load fall back to the default.

use std::fs;
use std::path::PathBuf;
use std::sync::LazyLock;

use edit::cell::{Ref, SemiRefCell};
use edit::input::{InputKey, InputKeyMod, kbmod, vk};

use crate::apperr;
use crate::settings;

pub const DEFAULT_TOML: &str = include_str!("keybindings.default.toml");

/// Configurable user actions. Each has at most one chord in the config.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum Action {
    Save,
    SaveAs,
    Exit,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    Find,
    Replace,
    FocusStatusbar,
    GoToLine,
    ToggleWordWrap,
    OpenAbout,
    FocusMenubar,
}

const ACTION_COUNT: usize = 16;

const ACTION_KEYS: [(Action, &str); ACTION_COUNT] = [
    (Action::Save, "save"),
    (Action::SaveAs, "save_as"),
    (Action::Exit, "exit"),
    (Action::Undo, "undo"),
    (Action::Redo, "redo"),
    (Action::Cut, "cut"),
    (Action::Copy, "copy"),
    (Action::Paste, "paste"),
    (Action::SelectAll, "select_all"),
    (Action::Find, "find"),
    (Action::Replace, "replace"),
    (Action::FocusStatusbar, "focus_statusbar"),
    (Action::GoToLine, "go_to_line"),
    (Action::ToggleWordWrap, "toggle_word_wrap"),
    (Action::OpenAbout, "open_about"),
    (Action::FocusMenubar, "focus_menubar"),
];

pub struct Keybindings {
    chords: [InputKey; ACTION_COUNT],
}

impl Keybindings {
    fn from_defaults() -> Self {
        let mut chords = [vk::NULL; ACTION_COUNT];
        let parsed = parse_toml(DEFAULT_TOML).expect("default keybindings must parse");
        for &(action, name) in ACTION_KEYS.iter() {
            if let Some(&chord) = parsed.get(name) {
                chords[action as usize] = chord;
            }
        }
        Self { chords }
    }

    fn merge_file(&mut self, text: &str) -> apperr::Result<()> {
        let parsed =
            parse_toml(text).map_err(|_| apperr::Error::SettingsInvalid("keybindings.toml"))?;
        for &(action, name) in ACTION_KEYS.iter() {
            if let Some(&chord) = parsed.get(name) {
                self.chords[action as usize] = chord;
            }
        }
        Ok(())
    }

    pub fn chord(&self, action: Action) -> InputKey {
        self.chords[action as usize]
    }
}

struct KeybindingsCell(SemiRefCell<Keybindings>);
unsafe impl Sync for KeybindingsCell {}
static BINDINGS: LazyLock<KeybindingsCell> =
    LazyLock::new(|| KeybindingsCell(SemiRefCell::new(Keybindings::from_defaults())));

pub fn borrow() -> Ref<'static, Keybindings> {
    BINDINGS.0.borrow()
}

pub fn chord(action: Action) -> InputKey {
    borrow().chord(action)
}

/// Path of the keybindings file, or `None` when no config dir exists.
pub fn path() -> Option<PathBuf> {
    let mut p = settings::config_dir()?;
    p.push("keybindings.toml");
    Some(p)
}

/// Load the keybindings file, auto-creating it from [`DEFAULT_TOML`] if missing.
/// Called from `main` after args are parsed.
pub fn load_or_create() -> apperr::Result<()> {
    let Some(path) = path() else { return Ok(()) };

    let text = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            fs::write(&path, DEFAULT_TOML)?;
            DEFAULT_TOML.to_string()
        }
        Err(e) => return Err(e.into()),
    };

    BINDINGS.0.borrow_mut().merge_file(&text)
}

// ---- parsing ----

fn parse_toml(text: &str) -> Result<std::collections::HashMap<String, InputKey>, String> {
    use std::collections::HashMap;

    let root = toml_span::parse(text).map_err(|e| e.to_string())?;
    let table = root.as_table().ok_or_else(|| "non-table root".to_string())?;
    let kb = table
        .iter()
        .find(|(k, _)| k.name == "keybindings")
        .and_then(|(_, v)| v.as_table())
        .ok_or_else(|| "missing [keybindings] table".to_string())?;

    let mut out: HashMap<String, InputKey> = HashMap::new();
    for (k, v) in kb.iter() {
        let s = v.as_str().ok_or_else(|| format!("{}: not a string", k.name))?;
        let key = parse_chord(s).ok_or_else(|| format!("{}: unknown chord {:?}", k.name, s))?;
        out.insert(k.name.to_string(), key);
    }
    Ok(out)
}

/// Parse `"Ctrl+Shift+F"` / `"F10"` / `"A"` / `""` → [`InputKey`].
/// Empty string means "unbound" → returns [`vk::NULL`].
pub fn parse_chord(s: &str) -> Option<InputKey> {
    if s.is_empty() {
        return Some(vk::NULL);
    }
    let mut mods = kbmod::NONE;
    let mut key: Option<InputKey> = None;
    for token in s.split('+') {
        let token = token.trim();
        if let Some(m) = parse_modifier(token) {
            mods |= m;
        } else if key.is_some() {
            return None;
        } else {
            key = Some(parse_key(token)?);
        }
    }
    let key = key?;
    Some(key | mods)
}

fn parse_modifier(token: &str) -> Option<InputKeyMod> {
    match token {
        s if s.eq_ignore_ascii_case("Ctrl") => Some(kbmod::CTRL),
        s if s.eq_ignore_ascii_case("Alt") => Some(kbmod::ALT),
        s if s.eq_ignore_ascii_case("Shift") => Some(kbmod::SHIFT),
        s if s.eq_ignore_ascii_case("Cmd") || s.eq_ignore_ascii_case("Super") => Some(kbmod::CMD),
        _ => None,
    }
}

fn parse_key(token: &str) -> Option<InputKey> {
    if token.len() == 1 {
        let c = token.as_bytes()[0];
        return match c {
            b'0'..=b'9' => Some(digit_vk(c - b'0')),
            b'a'..=b'z' => Some(letter_vk(c - b'a' + b'A')),
            b'A'..=b'Z' => Some(letter_vk(c)),
            _ => None,
        };
    }
    let upper = token.to_ascii_uppercase();
    Some(match upper.as_str() {
        "BACK" | "BACKSPACE" => vk::BACK,
        "TAB" => vk::TAB,
        "RETURN" | "ENTER" => vk::RETURN,
        "ESCAPE" | "ESC" => vk::ESCAPE,
        "SPACE" => vk::SPACE,
        "PAGEUP" | "PRIOR" => vk::PRIOR,
        "PAGEDOWN" | "NEXT" => vk::NEXT,
        "END" => vk::END,
        "HOME" => vk::HOME,
        "LEFT" => vk::LEFT,
        "UP" => vk::UP,
        "RIGHT" => vk::RIGHT,
        "DOWN" => vk::DOWN,
        "INSERT" => vk::INSERT,
        "DELETE" | "DEL" => vk::DELETE,
        s if s.starts_with('F') => {
            let n: u32 = s[1..].parse().ok()?;
            if (1..=24).contains(&n) {
                // vk::F1 starts at 0x70, F<n> = 0x6F + n
                match n {
                    1 => vk::F1,
                    2 => vk::F2,
                    3 => vk::F3,
                    4 => vk::F4,
                    5 => vk::F5,
                    6 => vk::F6,
                    7 => vk::F7,
                    8 => vk::F8,
                    9 => vk::F9,
                    10 => vk::F10,
                    11 => vk::F11,
                    12 => vk::F12,
                    13 => vk::F13,
                    14 => vk::F14,
                    15 => vk::F15,
                    16 => vk::F16,
                    17 => vk::F17,
                    18 => vk::F18,
                    19 => vk::F19,
                    20 => vk::F20,
                    21 => vk::F21,
                    22 => vk::F22,
                    23 => vk::F23,
                    24 => vk::F24,
                    _ => return None,
                }
            } else {
                return None;
            }
        }
        s if s.starts_with("NUMPAD") => {
            let n: u32 = s[6..].parse().ok()?;
            match n {
                0 => vk::NUMPAD0,
                1 => vk::NUMPAD1,
                2 => vk::NUMPAD2,
                3 => vk::NUMPAD3,
                4 => vk::NUMPAD4,
                5 => vk::NUMPAD5,
                6 => vk::NUMPAD6,
                7 => vk::NUMPAD7,
                8 => vk::NUMPAD8,
                9 => vk::NUMPAD9,
                _ => return None,
            }
        }
        _ => return None,
    })
}

fn letter_vk(ch: u8) -> InputKey {
    // vk::A = 'A' = 0x41 through vk::Z = 'Z' = 0x5A
    match ch {
        b'A' => vk::A,
        b'B' => vk::B,
        b'C' => vk::C,
        b'D' => vk::D,
        b'E' => vk::E,
        b'F' => vk::F,
        b'G' => vk::G,
        b'H' => vk::H,
        b'I' => vk::I,
        b'J' => vk::J,
        b'K' => vk::K,
        b'L' => vk::L,
        b'M' => vk::M,
        b'N' => vk::N,
        b'O' => vk::O,
        b'P' => vk::P,
        b'Q' => vk::Q,
        b'R' => vk::R,
        b'S' => vk::S,
        b'T' => vk::T,
        b'U' => vk::U,
        b'V' => vk::V,
        b'W' => vk::W,
        b'X' => vk::X,
        b'Y' => vk::Y,
        b'Z' => vk::Z,
        _ => vk::NULL,
    }
}

fn digit_vk(d: u8) -> InputKey {
    match d {
        0 => vk::N0,
        1 => vk::N1,
        2 => vk::N2,
        3 => vk::N3,
        4 => vk::N4,
        5 => vk::N5,
        6 => vk::N6,
        7 => vk::N7,
        8 => vk::N8,
        9 => vk::N9,
        _ => vk::NULL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_toml_parses() {
        let kb = Keybindings::from_defaults();
        assert_eq!(kb.chord(Action::Save), kbmod::CTRL | vk::S);
        assert_eq!(kb.chord(Action::FocusMenubar), vk::F10);
    }

    #[test]
    fn parse_chord_roundtrip() {
        assert_eq!(parse_chord("Ctrl+S"), Some(kbmod::CTRL | vk::S));
        assert_eq!(parse_chord("Cmd+Shift+P"), Some(kbmod::CMD | kbmod::SHIFT | vk::P));
        assert_eq!(parse_chord("F10"), Some(vk::F10));
        assert_eq!(parse_chord(""), Some(vk::NULL));
        assert_eq!(parse_chord("NotAKey"), None);
    }
}
