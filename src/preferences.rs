use std::{
    fs,
    path::{Path, PathBuf},
};

use libadwaita::glib;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preferences {
    pub library: PathBuf,
    #[serde(default)]
    pub ask_editor_each_time: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        let documents = glib::user_special_dir(glib::UserDirectory::Documents)
            .unwrap_or_else(glib::user_data_dir);
        Self {
            library: documents.join("Flashcards"),
            ask_editor_each_time: false,
        }
    }
}

impl Preferences {
    pub fn load() -> Self {
        fs::read_to_string(Self::path())
            .ok()
            .and_then(|contents| serde_json::from_str(&contents).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        fs::create_dir_all(path.parent().unwrap_or_else(|| Path::new(".")))?;
        let contents = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        fs::write(path, format!("{contents}\n"))
    }

    fn path() -> PathBuf {
        glib::user_config_dir()
            .join("page.polli.Flashcards")
            .join("preferences.json")
    }
}
