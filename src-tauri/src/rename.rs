//! The app used to be called "UsageCheck". Windows installs a renamed app next
//! to the old one instead of over it, so on its first start Quota quietly
//! removes the old copy: it carries over "start with Windows", then runs the old
//! uninstaller silently. Settings and saved keys are tied to the app identifier,
//! which didn't change, and a silent uninstall never deletes app data.

use tauri::AppHandle;

#[cfg(windows)]
pub fn spawn(app: AppHandle) {
    if cfg!(debug_assertions) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        // Let the installer that just started us finish first.
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        let _ = tauri::async_runtime::spawn_blocking(move || remove_old_install(&app)).await;
    });
}

#[cfg(not(windows))]
pub fn spawn(_app: AppHandle) {}

#[cfg(windows)]
const OLD_NAME: &str = "UsageCheck";

#[cfg(windows)]
fn remove_old_install(app: &AppHandle) {
    use std::os::windows::process::CommandExt;
    use std::path::PathBuf;
    use tauri_plugin_autostart::ManagerExt;

    use crate::registry as reg;
    use crate::startup::RUN_KEY;

    if reg::get_string(RUN_KEY, OLD_NAME).is_some() {
        let _ = app.autolaunch().enable();
        reg::delete_value(RUN_KEY, OLD_NAME);
    }

    let uninstall_key = format!("Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{OLD_NAME}");
    let Some(cmd) = reg::get_string(&uninstall_key, "UninstallString") else { return };
    let uninstaller = PathBuf::from(cmd.trim().trim_matches('"'));
    let ours = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf()));
    if !uninstaller.is_file() || uninstaller.parent().map(|d| d.to_path_buf()) == ours {
        return;
    }
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let _ = std::process::Command::new(&uninstaller).arg("/S").creation_flags(CREATE_NO_WINDOW).spawn();
}
