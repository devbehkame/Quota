//! "Start with Windows". The choice is kept in Settings as well as in the
//! registry's Run key, because the Run entry alone gets lost: running the setup
//! again over an installed copy uninstalls it first, which deletes the entry, and
//! the entry points at one exe path. So on every start, if the setting is on and
//! the entry is missing or points at another exe, Quota writes it again. An entry
//! that's there but switched off in Task Manager → Startup apps is left alone.

use tauri::AppHandle;

#[cfg(windows)]
pub const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";

#[cfg(windows)]
pub fn sync(app: &AppHandle) {
    use tauri::Manager;
    use tauri_plugin_autostart::ManagerExt;

    use crate::commands::AppState;

    // A dev build would register target\debug\Quota.exe.
    if cfg!(debug_assertions) {
        return;
    }
    if !app.state::<AppState>().config.lock().unwrap().settings.start_with_windows {
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    let exe = exe.display().to_string().to_lowercase();
    let entry = crate::registry::get_string(RUN_KEY, &app.package_info().name);
    if entry.is_some_and(|cmd| cmd.to_lowercase().contains(&exe)) {
        return;
    }
    let _ = app.autolaunch().enable();
}

#[cfg(not(windows))]
pub fn sync(_app: &AppHandle) {}
