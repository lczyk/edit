use std::path::PathBuf;
use std::sync::LazyLock;

use edit::cell::{Ref, SemiRefCell};
use edit::lsh::{LANGUAGES, Language};
use stdext::arena::{read_to_string, scratch_arena};
use stdext::arena_format;

use crate::apperr;

pub struct Settings {
    pub path: PathBuf,
    pub file_associations: Vec<(String, &'static Language)>,
}

struct SettingsCell(SemiRefCell<Settings>);
unsafe impl Sync for SettingsCell {}
static SETTINGS: LazyLock<SettingsCell> =
    LazyLock::new(|| SettingsCell(SemiRefCell::new(Settings::new())));

impl Settings {
    fn new() -> Self {
        Settings { path: PathBuf::new(), file_associations: Vec::new() }
    }

    pub fn borrow() -> Ref<'static, Settings> {
        SETTINGS.0.borrow()
    }

    pub fn reload() -> apperr::Result<()> {
        let s = &mut *SETTINGS.0.borrow_mut();

        // Reset all members if we had been loaded previously.
        if !s.path.as_os_str().is_empty() {
            *s = Settings::new();
        }

        s.load()
    }

    fn load(&mut self) -> apperr::Result<()> {
        self.path = match associations_path() {
            Some(p) => p,
            None => return Ok(()),
        };

        let scratch = scratch_arena(None);
        let str = match read_to_string(&scratch, &self.path) {
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(err.into()),
            Ok(str) => str,
        };
        let Ok(doc) = toml_span::parse(&str) else {
            return Err(apperr::Error::SettingsInvalid("Invalid TOML"));
        };
        let Some(root) = doc.as_table() else {
            return Err(apperr::Error::SettingsInvalid("Non-table root"));
        };
        let Some((_, associations)) = root.iter().find(|(k, _)| k.name == "associations") else {
            return Ok(());
        };
        let Some(associations) = associations.as_table() else {
            return Err(apperr::Error::SettingsInvalid("[associations] not a table"));
        };

        for (k, v) in associations.iter() {
            let mut key: &str = &k.name;
            if !key.contains('/') {
                key = arena_format!(&*scratch, "**/{key}").leak();
            }
            let Some(id) = v.as_str() else {
                return Err(apperr::Error::SettingsInvalid("associations value"));
            };
            let Some(language) = LANGUAGES.iter().find(|lang| lang.id == id) else {
                return Err(apperr::Error::SettingsInvalid("language ID"));
            };
            self.file_associations.push((key.to_string(), language));
        }

        Ok(())
    }
}

fn associations_path() -> Option<PathBuf> {
    let mut config_dir = config_dir()?;
    config_dir.push("associations.toml");
    Some(config_dir)
}

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
