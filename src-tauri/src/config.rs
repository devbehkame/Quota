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
    /// Launch minimized to the tray at sign-in. On by default; see `startup`.
    pub start_with_windows: bool,
    /// "system", "light" or "dark".
    pub theme: String,
    pub widget: WidgetSettings,
    /// Kept for older config files; updates are always automatic now.
    pub auto_update: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            refresh_minutes: 5,
            close_to_tray: true,
            start_with_windows: true,
            theme: "system".into(),
            widget: WidgetSettings::default(),
            auto_update: true,
        }
    }
}

/// The small always-on-top strip that sits on the taskbar.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct WidgetSettings {
    pub enabled: bool,
    /// The accounts (and gauges) to show, in order. Any number up to MAX_WIDGET_SLOTS.
    pub slots: Vec<WidgetSlot>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WidgetSlot {
    pub account_id: String,
    /// Which gauge to show; `None` means the tightest one.
    #[serde(default)]
    pub meter: Option<String>,
}

/// Plenty for a taskbar; each slot is about 70 px wide.
pub const MAX_WIDGET_SLOTS: usize = 8;

impl Settings {
    pub fn sanitized(mut self) -> Self {
        if self.refresh_minutes != 0 {
            self.refresh_minutes = self.refresh_minutes.clamp(1, 240);
        }
        if !["system", "light", "dark"].contains(&self.theme.as_str()) {
            self.theme = "system".into();
        }
        self.widget.slots.retain(|s| !s.account_id.is_empty() && s.account_id.len() <= 64);
        self.widget.slots.truncate(MAX_WIDGET_SLOTS);
        for s in &mut self.widget.slots {
            s.meter = s.meter.take().filter(|m| !m.is_empty() && m.len() <= 120);
        }
        self
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub accounts: Vec<Account>,
    pub settings: Settings,
    /// Where the user dragged the widget (physical pixels). `None` docks it next to the clock.
    pub widget_position: Option<(i32, i32)>,
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
