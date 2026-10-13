//! Refreshing lives in the backend so the numbers stay current (tray, taskbar
//! widget) even while the main window is hidden.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use usage_core::{Snapshot, UsageError};

use crate::commands::AppState;
use crate::{secrets, tray, widget};

const STALE_PREFIX: &str = "Showing the last values seen.";

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RefreshState<'a> {
    account_id: &'a str,
    busy: bool,
}

/// Refreshes one account. Concurrent calls for the same account wait for the one
/// in flight and reuse its result instead of hitting the provider twice.
pub async fn refresh(app: &AppHandle, id: &str) -> Result<Snapshot, String> {
    let state = app.state::<AppState>();
    let lock = state.locks.lock().unwrap().entry(id.to_string()).or_default().clone();
    let _guard = lock.lock().await;

    if let Some(s) = state.snapshots.lock().unwrap().get(id) {
        if (chrono::Utc::now() - s.fetched_at).num_seconds() < 3 {
            return Ok(s.clone());
        }
    }

    let account = state.account(id)?;
    let secret = secrets::get(id)?;
    let _ = app.emit("refresh-state", RefreshState { account_id: id, busy: true });
    let result = usage_core::fetch(&state.client, &account, secret.as_deref().map(|s| s.as_str())).await;
    let _ = app.emit("refresh-state", RefreshState { account_id: id, busy: false });

    let snap = match result {
        // The local app (e.g. Antigravity) is closed: keep the last numbers we saw.
        Err(UsageError::Offline(msg)) => {
            let previous = state.snapshots.lock().unwrap().get(id).cloned();
            match previous {
                Some(mut prev) if prev.usage.is_some() => {
                    if let Some(u) = prev.usage.as_mut() {
                        u.notes.retain(|n| !n.starts_with(STALE_PREFIX));
                        u.notes.insert(0, format!("{STALE_PREFIX} {msg}"));
                    }
                    prev
                }
                _ => Snapshot::from_result(id, Err(UsageError::Offline(msg))),
            }
        }
        other => Snapshot::from_result(id, other),
    };

    // Ignore results for accounts deleted while the request was in flight.
    if state.account(id).is_ok() {
        state.snapshots.lock().unwrap().insert(id.to_string(), snap.clone());
        let _ = app.emit("snapshot", &snap);
        tray::update_tooltip(app);
        widget::notify(app);
    }
    Ok(snap)
}

pub async fn refresh_all(app: &AppHandle) {
    let ids: Vec<String> = app.state::<AppState>().config.lock().unwrap().accounts.iter().map(|a| a.id.clone()).collect();
    futures::future::join_all(ids.iter().map(|id| refresh(app, id))).await;
}

/// Refreshes everything at startup, then on the interval chosen in Settings.
pub fn spawn_scheduler(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        refresh_all(&app).await;
        let mut last = Instant::now();
        loop {
            tokio::time::sleep(Duration::from_secs(15)).await;
            let mins = app.state::<AppState>().config.lock().unwrap().settings.refresh_minutes;
            if mins > 0 && last.elapsed() >= Duration::from_secs(u64::from(mins) * 60) {
                last = Instant::now();
                refresh_all(&app).await;
            }
        }
    });
}

pub type AccountLocks = std::sync::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>;
