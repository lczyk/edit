//! Colormap config loader, shared by `bin/edit` and `eat`'s mount-based
//! alt-screen modes.
//!
//! TOML file at `<config_dir>/colormap.toml`. Created on first run from
//! the embedded default. Missing keys fall back to the default palette.
//! `config_dir` follows `$XDG_CONFIG_HOME` then `$HOME/.config`, both
//! suffixed with `edit/`.

use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::LazyLock;

use crate::cell::{Ref, SemiRefCell};
use crate::framebuffer::{INDEXED_COLORS_COUNT, IndexedColor};
use crate::oklab::StraightRgba;

pub const DEFAULT_TOML: &str = include_str!("colormap.toml");

const COLOR_KEYS: [(IndexedColor, &str); INDEXED_COLORS_COUNT] = [
    (IndexedColor::Black, "black"),
    (IndexedColor::Red, "red"),
    (IndexedColor::Green, "green"),
    (IndexedColor::Yellow, "yellow"),
    (IndexedColor::Blue, "blue"),
    (IndexedColor::Magenta, "magenta"),
    (IndexedColor::Cyan, "cyan"),
    (IndexedColor::White, "white"),
    (IndexedColor::BrightBlack, "bright_black"),
    (IndexedColor::BrightRed, "bright_red"),
    (IndexedColor::BrightGreen, "bright_green"),
    (IndexedColor::BrightYellow, "bright_yellow"),
    (IndexedColor::BrightBlue, "bright_blue"),
    (IndexedColor::BrightMagenta, "bright_magenta"),
    (IndexedColor::BrightCyan, "bright_cyan"),
    (IndexedColor::BrightWhite, "bright_white"),
    (IndexedColor::Background, "background"),
    (IndexedColor::Foreground, "foreground"),
];

pub struct Colormap {
    pub palette: [StraightRgba; INDEXED_COLORS_COUNT],
    pub use_colormap: bool,
}

impl Colormap {
    fn from_defaults() -> Self {
        let (palette, use_colormap) =
            parse_toml(DEFAULT_TOML).expect("default colormap must parse");
        Self { palette, use_colormap }
    }

    fn merge_file(&mut self, text: &str) -> Result<(), String> {
        let (palette, use_colormap) = parse_toml(text)?;
        self.palette = palette;
        self.use_colormap = use_colormap;
        Ok(())
    }
}

struct ColormapCell(SemiRefCell<Colormap>);
// SAFETY: SemiRefCell single-threaded, but the process is single-threaded
// at the relevant load sites (lib + bin both serialise on a single ui
// thread); the static is gated by LazyLock so init runs once.
unsafe impl Sync for ColormapCell {}
static COLORMAP: LazyLock<ColormapCell> =
    LazyLock::new(|| ColormapCell(SemiRefCell::new(Colormap::from_defaults())));

pub fn borrow() -> Ref<'static, Colormap> {
    COLORMAP.0.borrow()
}

/// Resolves `<XDG_CONFIG_HOME or HOME/.config>/edit`. `None` if neither
/// env var is set.
pub fn config_dir() -> Option<PathBuf> {
    fn var_path(key: &str) -> Option<PathBuf> {
        std::env::var_os(key).map(PathBuf::from)
    }
    fn push(mut path: PathBuf, suffix: &str) -> PathBuf {
        path.push(suffix);
        path
    }
    var_path("XDG_CONFIG_HOME")
        .or_else(|| var_path("HOME").map(|p| push(p, ".config")))
        .map(|p| push(p, "edit"))
}

pub fn config_path() -> Option<PathBuf> {
    let mut p = config_dir()?;
    p.push("colormap.toml");
    Some(p)
}

/// Load the colormap file, auto-creating it from [`DEFAULT_TOML`] if
/// missing. Idempotent: safe to call from both the editor's main and
/// eat's mount entry. Invalid TOML returns InvalidData (caller decides
/// whether to log + fall back or hard-fail).
pub fn load_or_create() -> io::Result<()> {
    let Some(path) = config_path() else { return Ok(()) };

    let text = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            fs::write(&path, DEFAULT_TOML)?;
            DEFAULT_TOML.to_string()
        }
        Err(e) => return Err(e),
    };

    COLORMAP
        .0
        .borrow_mut()
        .merge_file(&text)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("colormap.toml: {e}")))
}

/// Wipe and rewrite `colormap.toml` from [`DEFAULT_TOML`]. Called by
/// the editor's `--force-reset-config` (debug only).
#[cfg(debug_assertions)]
pub fn force_reset() -> io::Result<()> {
    let Some(dir) = config_dir() else { return Ok(()) };
    let p = dir.join("colormap.toml");
    if let Err(e) = fs::remove_file(&p)
        && e.kind() != io::ErrorKind::NotFound
    {
        return Err(e);
    }
    fs::create_dir_all(&dir)?;
    fs::write(&p, DEFAULT_TOML)?;
    Ok(())
}

// ---- parsing ----

fn parse_toml(text: &str) -> Result<([StraightRgba; INDEXED_COLORS_COUNT], bool), String> {
    let root = toml_span::parse(text).map_err(|e| e.to_string())?;
    let table = root.as_table().ok_or_else(|| "non-table root".to_string())?;

    let mut use_colormap = true;
    if let Some((_, v)) = table.iter().find(|(k, _)| k.name == "use_colormap") {
        use_colormap = v.as_bool().ok_or_else(|| "use_colormap: not a bool".to_string())?;
    }

    let mut palette = crate::framebuffer::DEFAULT_THEME;

    if let Some((_, colors)) = table.iter().find(|(k, _)| k.name == "colors") {
        let colors = colors.as_table().ok_or_else(|| "[colors] not a table".to_string())?;
        for (k, v) in colors.iter() {
            let Some(&(idx, _)) = COLOR_KEYS.iter().find(|(_, n)| *n == k.name.as_ref()) else {
                return Err(format!("{}: unknown color key", k.name));
            };
            let s = v.as_str().ok_or_else(|| format!("{}: not a string", k.name))?;
            let rgba = parse_hex(s).ok_or_else(|| format!("{}: invalid hex {:?}", k.name, s))?;
            palette[idx as usize] = rgba;
        }
    }

    Ok((palette, use_colormap))
}

/// Parse `"#rrggbb"` / `"rrggbb"` / `"#rrggbbaa"` / `"rrggbbaa"` ->
/// [`StraightRgba`].
fn parse_hex(s: &str) -> Option<StraightRgba> {
    let s = s.strip_prefix('#').unwrap_or(s);
    let (rgb, alpha) = match s.len() {
        6 => (u32::from_str_radix(s, 16).ok()?, 0xffu32),
        8 => {
            let v = u32::from_str_radix(s, 16).ok()?;
            (v >> 8, v & 0xff)
        }
        _ => return None,
    };
    Some(StraightRgba::from_be((rgb << 8) | alpha))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_toml_parses() {
        let cm = Colormap::from_defaults();
        assert!(!cm.use_colormap);
        assert_eq!(cm.palette.len(), INDEXED_COLORS_COUNT);
    }

    #[test]
    fn parse_hex_forms() {
        assert_eq!(parse_hex("#000000"), Some(StraightRgba::from_be(0x000000ff)));
        assert_eq!(parse_hex("ffffff"), Some(StraightRgba::from_be(0xffffffff)));
        assert_eq!(parse_hex("#be2c2180"), Some(StraightRgba::from_be(0xbe2c2180)));
        assert_eq!(parse_hex("zzz"), None);
    }
}
