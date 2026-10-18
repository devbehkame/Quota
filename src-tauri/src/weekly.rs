//! "Tokens this week" on the Claude and ChatGPT plan cards: after each refresh,
//! decides where the week starts (see `usage_core::weekly`), counts this PC's
//! tokens since then from the local logs, and estimates this PC's share of the
//! weekly limit. What it needs to remember is kept in `weekly.json` next to the
//! settings, so the estimate carries over app restarts.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::{Duration, Utc};
use tauri::{AppHandle, Manager};
use usage_core::weekly::{self, Track};
use usage_core::{Account, ProviderKind, Usage};

pub struct Weeks {
    path: PathBuf,
    tracks: Mutex<HashMap<String, Track>>,
}

impl Weeks {
    pub fn load(path: PathBuf) -> Self {
        let tracks = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        Weeks { path, tracks: Mutex::new(tracks) }
    }

    fn save(&self, tracks: &HashMap<String, Track>) {
        let Ok(json) = serde_json::to_vec(tracks) else { return };
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = self.path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &self.path);
        }
    }
}

pub async fn apply(app: &AppHandle, account: &Account, usage: &mut Usage) {
    if !matches!(account.kind, ProviderKind::ClaudePlan | ProviderKind::CodexPlan) {
        return;
    }
    let now = Utc::now();
    let weeks = app.state::<Weeks>();
    let week = usage_core::providers::tokens::weekly_meter(&usage.meters).map(|m| (m.percent_used.unwrap_or(0.0), m.resets_at));
    let prev = weeks.tracks.lock().unwrap().get(&account.id).cloned();
    let mut track = week.map(|(pct, resets_at)| weekly::begin(prev.as_ref(), pct, resets_at, now));
    let since = track.as_ref().map(|t| t.window_start).unwrap_or(now - Duration::days(7));

    let acc = account.clone();
    let tokens = tauri::async_runtime::spawn_blocking(move || usage_core::providers::tokens::for_account(&acc, since))
        .await
        .ok()
        .flatten();

    if let (Some(t), Some((pct, _))) = (track.as_mut(), week) {
        t.observe(pct, tokens.as_ref().map_or(0.0, weekly::units), now);
        let mut tracks = weeks.tracks.lock().unwrap();
        tracks.insert(account.id.clone(), t.clone());
        weeks.save(&tracks);
    }
    usage.tokens = tokens.map(|mut k| {
        k.share = track.as_ref().map(Track::share);
        k
    });
}
