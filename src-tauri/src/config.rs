//! Non-secret settings, saved as JSON in the app's config folder.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use usage_core::Account;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Auto-refresh interval in minutes; 0 turns auto-refresh off.
    pub refresh_minutes: u32,
    pub close_to_tray: bool,
    /// "system", "light" or "dark".
    pub theme: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { refresh_minutes: 5, close_to_tray: true, theme: "system".into() }
    }
}

impl Settings {
    pub fn sanitized(mut self) -> Self {
        if self.refresh_minutes != 0 {
            self.refresh_minutes = self.refresh_minutes.clamp(1, 240);
        }
        if !["system", "light", "dark"].contains(&self.theme.as_str()) {
            self.theme = "system".into();
        }
        self
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub accounts: Vec<Account>,
    pub settings: Settings,
}

pub fn load(path: &Path) -> Config {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Config>(&s).ok())
        .map(|mut c| {
            c.settings = c.settings.sanitized();
            c
        })
        .unwrap_or_default()
}

/// Writes atomically (temp file + rename) so a crash never leaves a half-written config.
pub fn save(path: &Path, config: &Config) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("Could not create config folder: {e}"))?;
    }
    let json = serde_json::to_vec_pretty(config).map_err(|e| e.to_string())?;
    let tmp: PathBuf = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| format!("Could not save settings: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("Could not save settings: {e}"))
}
