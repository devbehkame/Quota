//! Refreshing lives in the backend so the numbers stay current (tray, taskbar
//! widget) even while the main window is hidden.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use usage_core::{Snapshot, UsageError};

use crate::commands::AppState;
use crate::{secrets, tray};

const STALE_PREFIX: &str = "Showing the last values seen.";
/// Automatic refreshes never hit a provider more often than this per account.
const MIN_AUTO_GAP: Duration = Duration::from_secs(60);
/// Clicking Refresh repeatedly re-uses a result this fresh instead of re-asking.
const MIN_MANUAL_GAP: Duration = Duration::from_secs(20);
/// After "too many requests", wait this long (doubling up to the max) before asking again.
const BACKOFF_START: Duration = Duration::from_secs(5 * 60);
const BACKOFF_MAX: Duration = Duration::from_secs(30 * 60);

/// Who asked for a refresh: the schedule, or a person clicking Refresh.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Auto,
    Manual,
}

#[derive(Default)]
pub struct Pace {
    last_fetch: HashMap<String, Instant>,
    /// account id -> (retry no earlier than, current backoff)
    cooldown: HashMap<String, (Instant, Duration)>,
}

pub type Pacing = Mutex<Pace>;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RefreshState<'a> {
    account_id: &'a str,
    busy: bool,
}

/// Refreshes one account. Concurrent calls for the same account wait for the one
/// in flight and reuse its result instead of hitting the provider twice, and
/// providers are never asked more often than the pacing rules above allow.
pub async fn refresh(app: &AppHandle, id: &str, trigger: Trigger) -> Result<Snapshot, String> {
    let state = app.state::<AppState>();
    let lock = state.locks.lock().unwrap().entry(id.to_string()).or_default().clone();
    let _guard = lock.lock().await;

    let previous = state.snapshots.lock().unwrap().get(id).cloned();
    let now = Instant::now();
    let (recent, cooling) = {
        let pace = state.pace.lock().unwrap();
        let gap = if trigger == Trigger::Manual { MIN_MANUAL_GAP } else { MIN_AUTO_GAP };
        let recent = pace.last_fetch.get(id).is_some_and(|t| now.duration_since(*t) < gap);
        let cooling = pace.cooldown.get(id).filter(|(until, _)| *until > now).map(|(until, _)| *until);
        (recent, cooling)
    };
    if let Some(prev) = previous.as_ref() {
        if recent || cooling.is_some() {
            return Ok(prev.clone());
        }
    }

    let account = state.account(id)?;
    let secret = secrets::get(id)?;
    state.pace.lock().unwrap().last_fetch.insert(id.to_string(), now);
    let _ = app.emit("refresh-state", RefreshState { account_id: id, busy: true });
    let result = match usage_core::fetch(&state.client, &account, secret.as_deref().map(|s| s.as_str())).await {
        Ok(mut usage) => {
            crate::weekly::apply(app, &account, &mut usage).await;
            Ok(usage)
        }
        err => err,
    };
    let _ = app.emit("refresh-state", RefreshState { account_id: id, busy: false });

    let keep_previous = |msg: String| -> Snapshot {
        match previous.clone() {
            Some(mut prev) if prev.usage.is_some() => {
                if let Some(u) = prev.usage.as_mut() {
                    u.notes.retain(|n| !n.starts_with(STALE_PREFIX));
                    u.notes.insert(0, format!("{STALE_PREFIX} {msg}"));
                }
                prev
            }
            _ => Snapshot::from_result(id, Err(UsageError::Offline(msg))),
        }
    };

    let snap = match result {
        // The local app (e.g. Antigravity) is closed: keep the last numbers we saw.
        Err(UsageError::Offline(msg)) => keep_previous(msg),
        // Too many requests: back off, and keep showing the last numbers meanwhile.
        Err(UsageError::RateLimited) => {
            let wait = {
                let mut pace = state.pace.lock().unwrap();
                let next = pace.cooldown.get(id).map(|(_, b)| (*b * 2).min(BACKOFF_MAX)).unwrap_or(BACKOFF_START);
                pace.cooldown.insert(id.to_string(), (Instant::now() + next, next));
                next
            };
            keep_previous(format!(
                "The provider asked Quota to slow down, so it will try again in {} minutes.",
                wait.as_secs() / 60
            ))
        }
        other => {
            if other.is_ok() {
                state.pace.lock().unwrap().cooldown.remove(id);
            }
            Snapshot::from_result(id, other)
        }
    };

    // Ignore results for accounts deleted while the request was in flight.
    if state.account(id).is_ok() {
        state.snapshots.lock().unwrap().insert(id.to_string(), snap.clone());
        let _ = app.emit("snapshot", &snap);
        tray::update_tooltip(app);
    }
    Ok(snap)
}

pub async fn refresh_all(app: &AppHandle, trigger: Trigger) {
    let ids: Vec<String> = app.state::<AppState>().config.lock().unwrap().accounts.iter().map(|a| a.id.clone()).collect();
    futures::future::join_all(ids.iter().map(|id| refresh(app, id, trigger))).await;
}

/// Refreshes everything at startup, then on the interval chosen in Settings.
pub fn spawn_scheduler(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        refresh_all(&app, Trigger::Auto).await;
        let mut last = Instant::now();
        loop {
            tokio::time::sleep(Duration::from_secs(15)).await;
            let mins = app.state::<AppState>().config.lock().unwrap().settings.refresh_minutes;
            if mins > 0 && last.elapsed() >= Duration::from_secs(u64::from(mins) * 60) {
                last = Instant::now();
                refresh_all(&app, Trigger::Auto).await;
            }
        }
    });
}

pub type AccountLocks = std::sync::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>;
