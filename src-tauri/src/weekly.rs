//! "Tokens this session / this week" on the Claude and ChatGPT plan cards: after
//! each refresh, decides where each limit window starts (see `usage_core::weekly`),
//! counts this PC's tokens since then from the local logs, and estimates this
//! PC's share of each limit. What it needs to remember is kept in `weekly.json`
//! next to the settings, so the estimates carry over app restarts.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::{Duration, Utc};
use tauri::{AppHandle, Manager};
use usage_core::providers::tokens;
use usage_core::weekly::{self, Track};
use usage_core::{Account, Meter, Period, ProviderKind, Usage};

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

/// One limit window to count: which gauge defines it and how long it lasts.
struct Window {
    period: Period,
    length: Duration,
    /// Key suffix in `weekly.json` (the week keeps the bare account id).
    key: &'static str,
    gauge: fn(&[Meter]) -> Option<&Meter>,
}

const WINDOWS: [Window; 2] = [
    Window { period: Period::Session, length: weekly::SESSION, key: ":session", gauge: tokens::session_meter },
    Window { period: Period::Week, length: weekly::WEEK, key: "", gauge: tokens::weekly_meter },
];

pub async fn apply(app: &AppHandle, account: &Account, usage: &mut Usage) {
    if !matches!(account.kind, ProviderKind::ClaudePlan | ProviderKind::CodexPlan) {
        return;
    }
    let now = Utc::now();
    let weeks = app.state::<Weeks>();

    // Decide where each window starts (this notices early resets), then count
    // this PC's tokens for all of them in one pass over the logs.
    let plans: Vec<(&Window, Option<(f64, Track)>)> = {
        let tracks = weeks.tracks.lock().unwrap();
        WINDOWS
            .iter()
            .map(|w| {
                let tracked = (w.gauge)(&usage.meters).map(|m| {
                    let pct = m.percent_used.unwrap_or(0.0);
                    let prev = tracks.get(&format!("{}{}", account.id, w.key));
                    (pct, weekly::begin(prev, pct, m.resets_at, w.length, now))
                });
                (w, tracked)
            })
            .collect()
    };
    let sinces: Vec<_> =
        plans.iter().map(|(w, t)| t.as_ref().map_or(now - w.length, |(_, t)| t.window_start)).collect();
    let acc = account.clone();
    let Some(counts) =
        tauri::async_runtime::spawn_blocking(move || tokens::for_account(&acc, &sinces)).await.ok().flatten()
    else {
        return; // no local logs: nothing to show
    };

    let mut tracks = weeks.tracks.lock().unwrap();
    let mut out = Vec::new();
    for ((w, tracked), mut count) in plans.into_iter().zip(counts) {
        count.period = w.period;
        if let Some((pct, mut t)) = tracked {
            t.observe(pct, weekly::units(&count), now);
            count.share = Some(t.share());
            tracks.insert(format!("{}{}", account.id, w.key), t);
        }
        let used = weekly::units(&count) > 0.0 || count.share.as_ref().is_some_and(|s| s.used > 0.0);
        if used {
            out.push(count);
        }
    }
    weeks.save(&tracks);
    // Show nothing when this PC hasn't used the tool all week.
    if out.iter().any(|c| c.period == Period::Week && weekly::units(c) > 0.0) {
        usage.tokens = out;
    }
}
