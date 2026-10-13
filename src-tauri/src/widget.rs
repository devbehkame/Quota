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
    /// Where the taskbar's own buttons are, refreshed by the keeper.
    #[cfg_attr(not(windows), allow(dead_code))]
    layout: Mutex<Option<TaskbarLayout>>,
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
                crate::hovercard::destroy(&app);
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
    make_tool_window(&w);
    if let Err(e) = crate::hovercard::create(app) {
        eprintln!("could not create the widget's hover card: {e}");
    }
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

/// No Alt+Tab entry, and clicking it never takes focus from what you're doing.
pub fn make_tool_window(_w: &WebviewWindow) {
    #[cfg(windows)]
    win::make_tool_window(_w);
}

/// Called by the widget page once it knows how big its content is.
#[tauri::command]
pub fn widget_layout(app: AppHandle, window: WebviewWindow, width: f64, height: f64) {
    if window.label() != LABEL {
        return;
    }
    let size = LogicalSize::new(width.clamp(24.0, 480.0).ceil(), height.clamp(20.0, 72.0).ceil());
    let physical: PhysicalSize<u32> = size.to_physical(window.scale_factor().unwrap_or(1.0));
    // Resize and move in one step: separate calls show a frame at the old
    // position with the new size, which looks like a flash on the taskbar.
    let pos = target_position(&app, physical);
    set_bounds(&window, pos, physical);
    let hidden_for_fullscreen = app.state::<WidgetState>().hidden_for_fullscreen.load(Ordering::Relaxed);
    if !hidden_for_fullscreen && !window.is_visible().unwrap_or(false) {
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

/// What the widget should show: exactly the slots chosen in Settings.
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
        chosen
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
    let pos = target_position(app, size);
    if window.outer_position().is_ok_and(|p| (p.x, p.y) == pos) {
        return;
    }
    let _ = window.set_position(PhysicalPosition::new(pos.0, pos.1));
}

/// Where a widget of `size` belongs; remembered so our own moves aren't taken for drags.
fn target_position(app: &AppHandle, size: PhysicalSize<u32>) -> (i32, i32) {
    let saved = app.state::<AppState>().config.lock().unwrap().widget_position;
    let pos = saved
        .filter(|&(x, y)| on_screen(app, x, y))
        .or_else(|| docked_position(app, size))
        .unwrap_or_else(|| fallback_position(app, size));
    *app.state::<WidgetState>().last_set.lock().unwrap() = Some(pos);
    pos
}

#[cfg(windows)]
fn set_bounds(window: &WebviewWindow, pos: (i32, i32), size: PhysicalSize<u32>) {
    match win::raw(window) {
        Some(h) => win::set_bounds(h, pos, (size.width as i32, size.height as i32)),
        None => {
            let _ = window.set_size(size);
            let _ = window.set_position(PhysicalPosition::new(pos.0, pos.1));
        }
    }
}

#[cfg(not(windows))]
fn set_bounds(window: &WebviewWindow, pos: (i32, i32), size: PhysicalSize<u32>) {
    let _ = window.set_size(size);
    let _ = window.set_position(PhysicalPosition::new(pos.0, pos.1));
}

fn on_screen(app: &AppHandle, x: i32, y: i32) -> bool {
    app.available_monitors().unwrap_or_default().iter().any(|m| {
        let (p, s) = (m.position(), m.size());
        x >= p.x && y >= p.y && x < p.x + s.width as i32 - 8 && y < p.y + s.height as i32 - 8
    })
}

#[cfg(windows)]
fn docked_position(app: &AppHandle, size: PhysicalSize<u32>) -> Option<(i32, i32)> {
    let layout = app.state::<WidgetState>().layout.lock().unwrap().clone().or_else(win::basic_layout)?;
    Some(dock_layout(&layout, (size.width as i32, size.height as i32)))
}

#[cfg(not(windows))]
fn docked_position(_app: &AppHandle, _size: PhysicalSize<u32>) -> Option<(i32, i32)> {
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

/// The taskbar, its notification area, and the spans its own buttons cover
/// (Start, search, pinned and running apps) along its length.
#[derive(Clone, Debug, PartialEq)]
pub struct TaskbarLayout {
    pub bar: Rect,
    pub tray: Option<Rect>,
    pub occupied: Vec<(i32, i32)>,
    pub at_top: bool,
}

/// Where the widget goes: in the free taskbar space closest to the clock, so it
/// never covers app buttons. If the taskbar is full, it floats just above (or
/// below, for a top taskbar) the clock instead.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn dock_layout(l: &TaskbarLayout, (w, h): (i32, i32)) -> (i32, i32) {
    let (bl, bt, br, bb) = l.bar;
    let horizontal = (br - bl) >= (bb - bt);
    if !horizontal {
        let anchor = l.tray.map(|tr| tr.1).filter(|y| *y > bt && *y <= bb).unwrap_or(bb - 260);
        return (bl + ((br - bl) - w) / 2, anchor - h - GAP);
    }
    let end = l.tray.map(|tr| tr.0).filter(|x| *x > bl && *x <= br).unwrap_or(br - 260);
    let y = bt + ((bb - bt) - h) / 2;

    // Free intervals between bar start and the tray, after removing button spans.
    let mut spans: Vec<(i32, i32)> =
        l.occupied.iter().map(|&(a, b)| (a.max(bl), b.min(end))).filter(|(a, b)| b > a).collect();
    spans.sort();
    let mut free = Vec::new();
    let mut cursor = bl;
    for (a, b) in spans {
        if a > cursor {
            free.push((cursor, a));
        }
        cursor = cursor.max(b);
    }
    if end > cursor {
        free.push((cursor, end));
    }
    // Prefer the gap nearest the clock.
    if let Some(&(_, right)) = free.iter().rev().find(|(a, b)| b - a >= w + 2 * GAP) {
        return (right - GAP - w, y);
    }
    let y_float = if l.at_top { bb + GAP } else { bt - h - GAP };
    (end - w - GAP, y_float)
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
            #[cfg(windows)]
            if tick.is_multiple_of(2) {
                rescan_layout(&app);
            }
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
            crate::hovercard::hide_now(app);
        }
        return;
    }
    if ws.hidden_for_fullscreen.swap(false, Ordering::Relaxed) {
        let _ = w.show();
    }
    // Re-dock when the taskbar moves, resizes, the tray grows, or apps open and
    // close (the button layout is read off the main thread, every few seconds).
    let bar = win::taskbar().map(|t| t.tray.unwrap_or(t.bar));
    if bar != *last_bar {
        *last_bar = bar;
        rescan_layout(app);
    }
    win::raise(hwnd);
}

#[cfg(not(windows))]
fn keep(_app: &AppHandle, _w: &WebviewWindow, _last_bar: &mut Option<Rect>) {}

/// Re-reads where the taskbar buttons are (UI Automation, on a worker thread)
/// and re-docks the widget if that changed.
#[cfg(windows)]
fn rescan_layout(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let Some(layout) = win::full_layout() else { return };
        let ws = app.state::<WidgetState>();
        let changed = {
            let mut cur = ws.layout.lock().unwrap();
            let changed = cur.as_ref() != Some(&layout);
            *cur = Some(layout);
            changed
        };
        let manual = app.state::<AppState>().config.lock().unwrap().widget_position.is_some();
        if changed && !manual {
            if let Some(w) = app.get_webview_window(LABEL) {
                place(&app, &w);
            }
        }
    });
}

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
        SWP_NOSIZE, SWP_NOZORDER, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
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

    fn monitor_of(hwnd: HWND) -> Option<RECT> {
        // SAFETY: read-only monitor query with a correctly sized MONITORINFO.
        unsafe {
            let mut mi: MONITORINFO = zeroed();
            mi.cbSize = size_of::<MONITORINFO>() as u32;
            (GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut mi) != 0).then_some(mi.rcMonitor)
        }
    }

    fn bar_hwnd() -> Option<HWND> {
        // SAFETY: plain window lookup with a NUL-terminated class name.
        let bar = unsafe { FindWindowW(wide("Shell_TrayWnd").as_ptr(), null()) };
        (!bar.is_null()).then_some(bar)
    }

    /// Taskbar and tray only (cheap; used until the button scan has run).
    pub fn basic_layout() -> Option<super::TaskbarLayout> {
        let tb = taskbar()?;
        let at_top = bar_hwnd().and_then(monitor_of).is_some_and(|m| tb.bar.1 <= m.top + 2);
        Some(super::TaskbarLayout { bar: tb.bar, tray: tb.tray, occupied: Vec::new(), at_top })
    }

    /// Taskbar plus the spans covered by its buttons, read through UI Automation.
    pub fn full_layout() -> Option<super::TaskbarLayout> {
        let mut layout = basic_layout()?;
        let buttons = uia::buttons(bar_hwnd()?).unwrap_or_default();
        // Without a TrayNotifyWnd (Windows 11), the tray starts at its first button.
        if layout.tray.is_none() {
            layout.tray = buttons
                .iter()
                .filter(|(class, _)| class.starts_with("SystemTray."))
                .map(|(_, r)| *r)
                .min_by_key(|r| r.0);
        }
        let tray_left = layout.tray.map(|t| t.0).unwrap_or(i32::MAX);
        let (bl, bt, br, bb) = layout.bar;
        let horizontal = (br - bl) >= (bb - bt);
        layout.occupied = buttons
            .iter()
            .filter(|(class, r)| !class.starts_with("SystemTray.") && r.2 > r.0 && r.0 < tray_left)
            .map(|(_, r)| if horizontal { (r.0, r.2) } else { (r.1, r.3) })
            .collect();
        layout.occupied.sort();
        layout.occupied.dedup();
        Some(layout)
    }

    mod uia {
        use windows::core::Result;
        use windows::Win32::Foundation::HWND;
        use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
        use windows::Win32::System::Variant::{VARIANT, VT_I4};
        use windows::Win32::UI::Accessibility::{
            CUIAutomation, IUIAutomation, TreeScope_Descendants, UIA_ButtonControlTypeId, UIA_ControlTypePropertyId,
        };

        use super::super::Rect;

        /// (class name, rect) of every button on the taskbar.
        pub fn buttons(bar: windows_sys::Win32::Foundation::HWND) -> Result<Vec<(String, Rect)>> {
            // SAFETY: standard COM/UI Automation calls on a worker thread; every
            // interface is reference-counted and released when dropped.
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
                let root = uia.ElementFromHandle(HWND(bar))?;
                // A plain integer VARIANT (no resources to free) holding the Button control type.
                let mut button = VARIANT::default();
                (*button.Anonymous.Anonymous).vt = VT_I4;
                (*button.Anonymous.Anonymous).Anonymous.lVal = UIA_ButtonControlTypeId.0;
                let cond = uia.CreatePropertyCondition(UIA_ControlTypePropertyId, &button)?;
                let all = root.FindAll(TreeScope_Descendants, &cond)?;
                let mut out = Vec::new();
                for i in 0..all.Length()? {
                    let Ok(el) = all.GetElement(i) else { continue };
                    let (Ok(r), class) = (el.CurrentBoundingRectangle(), el.CurrentClassName().map(|b| b.to_string()))
                    else {
                        continue;
                    };
                    out.push((class.unwrap_or_default(), (r.left, r.top, r.right, r.bottom)));
                }
                Ok(out)
            }
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

    /// Moves and resizes in a single call, without changing focus or z-order.
    pub fn set_bounds(h: HWND, (x, y): (i32, i32), (w, ht): (i32, i32)) {
        // SAFETY: `h` is our own window; only its position and size change.
        unsafe {
            SetWindowPos(h, null_mut(), x, y, w, ht, SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER);
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
    use super::{dock_layout, TaskbarLayout};

    fn layout(occupied: Vec<(i32, i32)>) -> TaskbarLayout {
        TaskbarLayout { bar: (0, 1032, 1920, 1080), tray: Some((1700, 1032, 1920, 1080)), occupied, at_top: false }
    }

    #[test]
    fn docks_left_of_tray_when_free() {
        assert_eq!(dock_layout(&layout(vec![(800, 1100)]), (150, 32)), (1700 - 150 - 8, 1032 + 8));
    }

    #[test]
    fn moves_past_app_buttons() {
        // Apps run up to x=1650: no room next to the tray, so use the gap before the apps.
        let l = layout(vec![(0, 48), (700, 1650)]);
        assert_eq!(dock_layout(&l, (150, 32)), (700 - 8 - 150, 1040));
    }

    #[test]
    fn floats_above_when_taskbar_is_full() {
        let l = layout(vec![(0, 1690)]);
        assert_eq!(dock_layout(&l, (150, 32)), (1700 - 150 - 8, 1032 - 32 - 8));
        let mut top = l.clone();
        top.bar = (0, 0, 1920, 48);
        top.tray = Some((1700, 0, 1920, 48));
        top.at_top = true;
        assert_eq!(dock_layout(&top, (150, 32)), (1700 - 150 - 8, 48 + 8));
    }

    #[test]
    fn docks_above_tray_on_vertical_taskbar() {
        let l = TaskbarLayout { bar: (0, 0, 64, 1080), tray: Some((0, 900, 64, 1080)), occupied: vec![], at_top: false };
        assert_eq!(dock_layout(&l, (60, 30)), (2, 900 - 30 - 8));
    }
}
