//! Self-updating: every 5 minutes, check the latest GitHub release and, when a
//! newer signed version exists, download and install it. Updates are verified
//! against the public key built into the app, so only builds signed with the
//! project's private key (kept in GitHub Actions secrets) can be installed.

use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;

use crate::commands::AppState;

const EVERY: Duration = Duration::from_secs(5 * 60);
const FIRST_CHECK_AFTER: Duration = Duration::from_secs(20);

#[derive(Serialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    /// "idle", "checking", "upToDate", "available", "downloading", "installing" or "error".
    state: &'static str,
    current_version: String,
    new_version: Option<String>,
    message: Option<String>,
    checked_at: Option<DateTime<Utc>>,
}

#[derive(Default)]
pub struct UpdaterState {
    status: Mutex<UpdateStatus>,
    busy: tokio::sync::Mutex<()>,
}

fn set(app: &AppHandle, f: impl FnOnce(&mut UpdateStatus)) {
    let status = {
        let st = app.state::<UpdaterState>();
        let mut s = st.status.lock().unwrap();
        f(&mut s);
        s.clone()
    };
    let _ = app.emit("update-status", &status);
}

fn auto_update_on(app: &AppHandle) -> bool {
    app.state::<AppState>().config.lock().unwrap().settings.auto_update
}

pub fn spawn(app: AppHandle) {
    let version = app.package_info().version.to_string();
    set(&app, |s| {
        s.state = "idle";
        s.current_version = version;
    });
    // Development builds aren't released, so there is nothing to update to.
    if cfg!(debug_assertions) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK_AFTER).await;
        loop {
            if auto_update_on(&app) {
                check(&app, true).await;
            }
            tokio::time::sleep(EVERY).await;
        }
    });
}

/// Checks for a newer release; installs it when `install` is true.
pub async fn check(app: &AppHandle, install: bool) -> UpdateStatus {
    let st = app.state::<UpdaterState>();
    let Ok(_guard) = st.busy.try_lock() else {
        return st.status.lock().unwrap().clone();
    };
    set(app, |s| {
        s.state = "checking";
        s.message = None;
    });

    let found = match app.updater() {
        Ok(u) => u.check().await,
        Err(e) => Err(e),
    };
    let update = match found {
        Ok(Some(u)) => u,
        Ok(None) => {
            set(app, |s| {
                s.state = "upToDate";
                s.new_version = None;
                s.checked_at = Some(Utc::now());
            });
            return current(app);
        }
        Err(e) => {
            let msg = friendly(&e.to_string());
            set(app, |s| {
                s.state = "error";
                s.message = Some(msg);
                s.checked_at = Some(Utc::now());
            });
            return current(app);
        }
    };

    let version = update.version.clone();
    if !install {
        set(app, |s| {
            s.state = "available";
            s.new_version = Some(version);
            s.checked_at = Some(Utc::now());
        });
        return current(app);
    }

    set(app, |s| {
        s.state = "downloading";
        s.new_version = Some(version.clone());
        s.checked_at = Some(Utc::now());
    });
    match update.download_and_install(|_, _| {}, || {}).await {
        Ok(()) => {
            // On Windows the installer has already taken over and closes the app;
            // elsewhere, restart into the new version.
            set(app, |s| s.state = "installing");
            app.restart();
        }
        Err(e) => {
            let msg = friendly(&e.to_string());
            set(app, |s| {
                s.state = "error";
                s.message = Some(msg);
            });
        }
    }
    current(app)
}

fn current(app: &AppHandle) -> UpdateStatus {
    app.state::<UpdaterState>().status.lock().unwrap().clone()
}

fn friendly(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    if lower.contains("404") || lower.contains("not found") {
        "No published release found yet (or the release page isn't public).".into()
    } else if lower.contains("signature") {
        "An update was found but its signature didn't match, so it was not installed.".into()
    } else if lower.contains("dns") || lower.contains("connect") || lower.contains("timed out") {
        "Couldn't reach GitHub to check for updates.".into()
    } else {
        raw.chars().take(160).collect()
    }
}

#[tauri::command]
pub fn update_status(app: AppHandle) -> UpdateStatus {
    current(&app)
}

/// "Check now" in Settings: installs straight away if a newer version exists.
#[tauri::command]
pub async fn check_for_updates(app: AppHandle) -> UpdateStatus {
    if cfg!(debug_assertions) {
        set(&app, |s| {
            s.state = "error";
            s.message = Some("Updates are only available in released builds.".into());
        });
        return current(&app);
    }
    check(&app, true).await
}
