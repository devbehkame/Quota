//! The taskbar widget: a tiny borderless, always-on-top strip that sits on the
//! Windows taskbar next to the clock and shows up to two accounts as
//! "icon + percentage". Windows 11 no longer lets apps embed into the taskbar, so
//! the strip floats over it, re-docks when the taskbar moves, steps aside for
//! full-screen apps, and can be dragged anywhere.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};
use usage_core::{ProviderKind, Snapshot};

use crate::commands::AppState;
use crate::config::MAX_WIDGET_SLOTS;
use crate::tray;

pub const LABEL: &str = "widget";
/// Gap between the widget and the clock / notification area, in physical pixels.
#[cfg_attr(not(windows), allow(dead_code))]
const GAP: i32 = 8;

#[derive(Default)]
pub struct WidgetState {
    /// The last position we set ourselves, to tell our moves apart from the user's drags.
    last_set: Mutex<Option<(i32, i32)>>,
    /// Bumped on every user drag; a delayed save only runs for the latest one.
    save_generation: AtomicU64,
    /// Hidden because a full-screen app is in front.
    hidden_for_fullscreen: AtomicBool,
    keeper_running: AtomicBool,
    /// Serialises create/destroy so rapid toggles can't create two windows.
    sync_lock: tokio::sync::Mutex<()>,
}

fn enabled(app: &AppHandle) -> bool {
    app.state::<AppState>().config.lock().unwrap().settings.widget.enabled
}

/// Creates or closes the widget to match Settings, and tells it to redraw.
///
/// Windows are created from a background task, never inline: building a webview
/// while the main thread is handling a command or menu click deadlocks on Windows
/// (the whole app freezes). The lock keeps quick on/off toggles in order.
pub fn sync(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let ws = app.state::<WidgetState>();
        let _serial = ws.sync_lock.lock().await;
        match (enabled(&app), app.get_webview_window(LABEL)) {
            (true, None) => {
                if let Err(e) = create(&app) {
                    eprintln!("could not create the taskbar widget: {e}");
                }
            }
            (false, Some(w)) => {
                let _ = w.destroy();
            }
            _ => notify(&app),
        }
    });
}

/// Asks the widget to reload what it shows (accounts or slots changed).
pub fn notify(app: &AppHandle) {
    let _ = app.emit("widget-update", ());
}

fn create(app: &AppHandle) -> tauri::Result<()> {
    let w = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("widget.html".into()))
        .title("UsageCheck widget")
        .inner_size(150.0, 32.0)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .focused(false)
        .visible(false)
        .build()?;
    #[cfg(windows)]
    win::make_tool_window(&w);
    let _ = w;
    spawn_keeper(app.clone());

    // The page normally shows the window once it has measured itself; if that
    // hasn't happened after a few seconds, show it at the default size anyway.
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(4)).await;
        if let Some(w) = app.get_webview_window(LABEL) {
            let hidden_for_fullscreen = app.state::<WidgetState>().hidden_for_fullscreen.load(Ordering::Relaxed);
            if !w.is_visible().unwrap_or(true) && !hidden_for_fullscreen {
                place(&app, &w);
                let _ = w.show();
            }
        }
    });
    Ok(())
}

/// Called by the widget page once it knows how big its content is.
#[tauri::command]
pub fn widget_layout(app: AppHandle, window: WebviewWindow, width: f64, height: f64) {
    if window.label() != LABEL {
        return;
    }
    let size = LogicalSize::new(width.clamp(24.0, 480.0).ceil(), height.clamp(20.0, 72.0).ceil());
    let _ = window.set_size(size);
    place(&app, &window);
    if !app.state::<WidgetState>().hidden_for_fullscreen.load(Ordering::Relaxed) {
        let _ = window.show();
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WidgetItem {
    account_id: String,
    kind: ProviderKind,
    label: String,
    /// The gauge to show; `None` = the tightest one.
    meter: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WidgetView {
    items: Vec<WidgetItem>,
    snapshots: Vec<Snapshot>,
    /// Whether the Windows taskbar uses the light theme (for text colour).
    light_taskbar: bool,
}

/// What the widget should show. With no slots chosen, it shows the first two accounts.
#[tauri::command]
pub fn widget_state(app: AppHandle) -> WidgetView {
    let state = app.state::<AppState>();
    let items: Vec<WidgetItem> = {
        let cfg = state.config.lock().unwrap();
        let find = |id: &str| cfg.accounts.iter().find(|a| a.id == id);
        let chosen: Vec<WidgetItem> = cfg
            .settings
            .widget
            .slots
            .iter()
            .filter_map(|s| {
                let a = find(&s.account_id)?;
                Some(WidgetItem { account_id: a.id.clone(), kind: a.kind, label: a.label.clone(), meter: s.meter.clone() })
            })
            .collect();
        if chosen.is_empty() {
            cfg.accounts
                .iter()
                .take(MAX_WIDGET_SLOTS)
                .map(|a| WidgetItem { account_id: a.id.clone(), kind: a.kind, label: a.label.clone(), meter: None })
                .collect()
        } else {
            chosen
        }
    };
    let snapshots = {
        let snaps = state.snapshots.lock().unwrap();
        items.iter().filter_map(|i| snaps.get(&i.account_id).cloned()).collect()
    };
    WidgetView { items, snapshots, light_taskbar: light_taskbar() }
}

#[tauri::command]
pub fn widget_open_main(app: AppHandle) {
    tray::show_main(&app);
}

/// Forgets a dragged position and docks the widget next to the clock again.
#[tauri::command]
pub fn widget_reset_position(app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    {
        let mut cfg = state.config.lock().unwrap();
        cfg.widget_position = None;
        state.persist(&cfg)?;
    }
    if let Some(w) = app.get_webview_window(LABEL) {
        place(&app, &w);
    }
    Ok(())
}

/// Remembers where the user dragged the widget.
pub fn on_moved(app: &AppHandle, pos: PhysicalPosition<i32>) {
    let ws = app.state::<WidgetState>();
    let ours = *ws.last_set.lock().unwrap();
    if ours.is_some_and(|(x, y)| (x - pos.x).abs() <= 3 && (y - pos.y).abs() <= 3) {
        return;
    }
    let state = app.state::<AppState>();
    state.config.lock().unwrap().widget_position = Some((pos.x, pos.y));
    *ws.last_set.lock().unwrap() = Some((pos.x, pos.y));

    // Moves arrive many times per second while dragging; save once it settles.
    let generation = ws.save_generation.fetch_add(1, Ordering::Relaxed) + 1;
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(700)).await;
        if app.state::<WidgetState>().save_generation.load(Ordering::Relaxed) == generation {
            let state = app.state::<AppState>();
            let cfg = state.config.lock().unwrap();
            let _ = state.persist(&cfg);
        }
    });
}

/// Puts the widget where the user left it, or docks it beside the clock.
fn place(app: &AppHandle, window: &WebviewWindow) {
    let size = window.outer_size().unwrap_or(PhysicalSize::new(150, 32));
    let saved = app.state::<AppState>().config.lock().unwrap().widget_position;
    let pos = saved
        .filter(|&(x, y)| on_screen(app, x, y))
        .or_else(|| docked_position(size))
        .unwrap_or_else(|| fallback_position(app, size));
    *app.state::<WidgetState>().last_set.lock().unwrap() = Some(pos);
    let _ = window.set_position(PhysicalPosition::new(pos.0, pos.1));
}

fn on_screen(app: &AppHandle, x: i32, y: i32) -> bool {
    app.available_monitors().unwrap_or_default().iter().any(|m| {
        let (p, s) = (m.position(), m.size());
        x >= p.x && y >= p.y && x < p.x + s.width as i32 - 8 && y < p.y + s.height as i32 - 8
    })
}

#[cfg(windows)]
fn docked_position(size: PhysicalSize<u32>) -> Option<(i32, i32)> {
    let tb = win::taskbar()?;
    Some(dock_in(tb.bar, tb.tray, (size.width as i32, size.height as i32)))
}

#[cfg(not(windows))]
fn docked_position(_size: PhysicalSize<u32>) -> Option<(i32, i32)> {
    None
}

/// Bottom-right corner of the main screen, above where a taskbar would be.
fn fallback_position(app: &AppHandle, size: PhysicalSize<u32>) -> (i32, i32) {
    match app.primary_monitor().ok().flatten() {
        Some(m) => {
            let (p, s) = (m.position(), m.size());
            (p.x + s.width as i32 - size.width as i32 - 24, p.y + s.height as i32 - size.height as i32 - 64)
        }
        None => (100, 100),
    }
}

/// Rect as (left, top, right, bottom).
pub type Rect = (i32, i32, i32, i32);

#[cfg_attr(not(windows), allow(dead_code))]
/// Where the widget goes on a taskbar: just before the notification area on a
/// horizontal taskbar, just above it on a vertical one.
pub fn dock_in(bar: Rect, tray: Option<Rect>, (w, h): (i32, i32)) -> (i32, i32) {
    let (l, t, r, b) = bar;
    let horizontal = (r - l) >= (b - t);
    if horizontal {
        let anchor = tray.map(|tr| tr.0).filter(|x| *x > l && *x <= r).unwrap_or(r - 260);
        (anchor - w - GAP, t + ((b - t) - h) / 2)
    } else {
        let anchor = tray.map(|tr| tr.1).filter(|y| *y > t && *y <= b).unwrap_or(b - 260);
        (l + ((r - l) - w) / 2, anchor - h - GAP)
    }
}

#[cfg(windows)]
fn light_taskbar() -> bool {
    win::light_taskbar()
}

#[cfg(not(windows))]
fn light_taskbar() -> bool {
    false
}

/// Keeps the widget above the taskbar, re-docks it when the taskbar changes, and
/// hides it while a full-screen app (game, video, presentation) is in front.
fn spawn_keeper(app: AppHandle) {
    let ws = app.state::<WidgetState>();
    if ws.keeper_running.swap(true, Ordering::SeqCst) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let mut last_bar = None;
        let mut last_light = light_taskbar();
        let mut tick: u32 = 0;
        loop {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            let Some(w) = app.get_webview_window(LABEL) else {
                app.state::<WidgetState>().keeper_running.store(false, Ordering::SeqCst);
                return;
            };
            tick = tick.wrapping_add(1);
            keep(&app, &w, &mut last_bar);
            if tick.is_multiple_of(10) {
                let light = light_taskbar();
                if light != last_light {
                    last_light = light;
                    notify(&app);
                }
            }
        }
    });
}

#[cfg(windows)]
fn keep(app: &AppHandle, w: &WebviewWindow, last_bar: &mut Option<Rect>) {
    let ws = app.state::<WidgetState>();
    let Some(hwnd) = win::raw(w) else { return };
    if win::fullscreen_app_active(hwnd) {
        if !ws.hidden_for_fullscreen.swap(true, Ordering::Relaxed) {
            let _ = w.hide();
        }
        return;
    }
    if ws.hidden_for_fullscreen.swap(false, Ordering::Relaxed) {
        let _ = w.show();
    }
    // Re-dock when the taskbar moves, resizes or the tray grows.
    let bar = win::taskbar().map(|t| t.tray.unwrap_or(t.bar));
    if bar != *last_bar {
        *last_bar = bar;
        if app.state::<AppState>().config.lock().unwrap().widget_position.is_none() {
            place(app, w);
        }
    }
    win::raise(hwnd);
}

#[cfg(not(windows))]
fn keep(_app: &AppHandle, _w: &WebviewWindow, _last_bar: &mut Option<Rect>) {}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::mem::{size_of, zeroed};
    use std::ptr::{null, null_mut};

    use tauri::WebviewWindow;
    use windows_sys::Win32::Foundation::{ERROR_SUCCESS, HWND, RECT};
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowExW, FindWindowW, GetClassNameW, GetForegroundWindow, GetWindowLongPtrW, GetWindowRect,
        SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER,
        SWP_NOSIZE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    };

    use super::Rect;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn rect_of(hwnd: HWND) -> Option<Rect> {
        // SAFETY: `hwnd` is a window handle; GetWindowRect only writes into `r`.
        unsafe {
            let mut r: RECT = zeroed();
            (GetWindowRect(hwnd, &mut r) != 0).then_some((r.left, r.top, r.right, r.bottom))
        }
    }

    pub struct Taskbar {
        pub bar: Rect,
        pub tray: Option<Rect>,
    }

    /// The primary taskbar and its notification area (clock, tray icons).
    pub fn taskbar() -> Option<Taskbar> {
        // SAFETY: plain window lookups with valid, NUL-terminated class names.
        unsafe {
            let bar = FindWindowW(wide("Shell_TrayWnd").as_ptr(), null());
            if bar.is_null() {
                return None;
            }
            let tray = FindWindowExW(bar, null_mut(), wide("TrayNotifyWnd").as_ptr(), null());
            Some(Taskbar { bar: rect_of(bar)?, tray: if tray.is_null() { None } else { rect_of(tray) } })
        }
    }

    pub fn raw(w: &WebviewWindow) -> Option<HWND> {
        w.hwnd().ok().map(|h| h.0 as HWND)
    }

    /// No Alt+Tab entry, and clicking it doesn't steal focus from what you're doing.
    pub fn make_tool_window(w: &WebviewWindow) {
        let Some(h) = raw(w) else { return };
        // SAFETY: `h` is our own top-level window.
        unsafe {
            let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
            SetWindowLongPtrW(h, GWL_EXSTYLE, ex | (WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE) as isize);
        }
    }

    /// Puts the widget back above the taskbar (which raises itself when clicked).
    pub fn raise(h: HWND) {
        // SAFETY: `h` is our own window; flags keep position, size and focus unchanged.
        unsafe {
            SetWindowPos(h, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER);
        }
    }

    /// True when the foreground window fills its whole monitor (games, videos, slides).
    pub fn fullscreen_app_active(own: HWND) -> bool {
        // SAFETY: read-only queries on the foreground window; buffers are sized correctly.
        unsafe {
            let fg = GetForegroundWindow();
            if fg.is_null() || fg == own {
                return false;
            }
            let mut buf = [0u16; 64];
            let n = GetClassNameW(fg, buf.as_mut_ptr(), buf.len() as i32);
            let class = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
            if matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") {
                return false;
            }
            let Some((l, t, r, b)) = rect_of(fg) else { return false };
            let mut mi: MONITORINFO = zeroed();
            mi.cbSize = size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST), &mut mi) == 0 {
                return false;
            }
            let m = mi.rcMonitor;
            l <= m.left && t <= m.top && r >= m.right && b >= m.bottom
        }
    }

    /// Windows' "system" (taskbar) colour mode, separate from the apps mode.
    pub fn light_taskbar() -> bool {
        let key = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
        let name = wide("SystemUsesLightTheme");
        let mut value: u32 = 0;
        let mut size = size_of::<u32>() as u32;
        // SAFETY: reads one DWORD into `value`; `size` describes its buffer.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_DWORD,
                null_mut(),
                &mut value as *mut u32 as *mut c_void,
                &mut size,
            )
        };
        status == ERROR_SUCCESS && value == 1
    }
}

#[cfg(test)]
mod tests {
    use super::dock_in;

    #[test]
    fn docks_left_of_tray_on_bottom_taskbar() {
        let bar = (0, 1032, 1920, 1080);
        let tray = Some((1700, 1032, 1920, 1080));
        assert_eq!(dock_in(bar, tray, (150, 32)), (1700 - 150 - 8, 1032 + 8));
    }

    #[test]
    fn docks_above_tray_on_vertical_taskbar() {
        let bar = (0, 0, 64, 1080);
        let tray = Some((0, 900, 64, 1080));
        assert_eq!(dock_in(bar, tray, (60, 30)), (2, 900 - 30 - 8));
    }

    #[test]
    fn falls_back_without_tray() {
        assert_eq!(dock_in((0, 1032, 1920, 1080), None, (100, 32)), (1920 - 260 - 100 - 8, 1040));
    }
}
