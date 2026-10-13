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
const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";

#[cfg(windows)]
fn remove_old_install(app: &AppHandle) {
    use std::os::windows::process::CommandExt;
    use std::path::PathBuf;
    use tauri_plugin_autostart::ManagerExt;

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

#[cfg(windows)]
mod reg {
    use std::ffi::c_void;
    use std::ptr::null_mut;

    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{RegDeleteKeyValueW, RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    /// A REG_SZ value under HKEY_CURRENT_USER.
    pub fn get_string(key: &str, name: &str) -> Option<String> {
        let (key, name) = (wide(key), wide(name));
        let mut buf = vec![0u16; 1024];
        let mut size = (buf.len() * 2) as u32;
        // SAFETY: `buf` is writable for `size` bytes; key and name are NUL-terminated.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                null_mut(),
                buf.as_mut_ptr() as *mut c_void,
                &mut size,
            )
        };
        if status != ERROR_SUCCESS {
            return None;
        }
        let len = (size as usize / 2).min(buf.len());
        let s = String::from_utf16_lossy(&buf[..len]).trim_end_matches('\0').to_string();
        (!s.is_empty()).then_some(s)
    }

    pub fn delete_value(key: &str, name: &str) {
        let (key, name) = (wide(key), wide(name));
        // SAFETY: both strings are NUL-terminated; only this one value is removed.
        unsafe {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr());
        }
    }
}
