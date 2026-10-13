use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};
use usage_core::Unit;

use crate::commands::AppState;

const TRAY_ID: &str = "main";

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Open UsageCheck", true, None::<&str>)?;
    let refresh = MenuItem::with_id(app, "refresh", "Refresh all", true, None::<&str>)?;
    let widget_on = app.state::<AppState>().config.lock().unwrap().settings.widget.enabled;
    let widget = CheckMenuItem::with_id(app, "widget", "Taskbar widget", true, widget_on, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&show, &refresh, &widget, &sep, &quit])?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("UsageCheck")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main(app),
            "refresh" => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move { crate::refresh::refresh_all(&app).await });
            }
            "widget" => toggle_widget(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                toggle_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

fn toggle_widget(app: &AppHandle) {
    let state = app.state::<AppState>();
    let settings = {
        let mut cfg = state.config.lock().unwrap();
        cfg.settings.widget.enabled = !cfg.settings.widget.enabled;
        let _ = state.persist(&cfg);
        cfg.settings.clone()
    };
    let _ = app.emit("settings-changed", &settings);
    crate::widget::sync(app);
}

pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn toggle_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        if w.is_visible().unwrap_or(false) && !w.is_minimized().unwrap_or(false) {
            let _ = w.hide();
        } else {
            show_main(app);
        }
    }
}

/// Shows the tightest limit of each account when hovering the tray icon.
pub fn update_tooltip(app: &AppHandle) {
    let state = app.state::<AppState>();
    let lines: Vec<String> = {
        let cfg = state.config.lock().unwrap();
        let snaps = state.snapshots.lock().unwrap();
        cfg.accounts
            .iter()
            .filter_map(|a| {
                let s = snaps.get(&a.id)?;
                let Some(usage) = &s.usage else { return Some(format!("{}: needs attention", a.label)) };
                let pct_left = usage
                    .meters
                    .iter()
                    .filter_map(|m| m.percent_used.map(|p| 100.0 - p))
                    .fold(None, |acc: Option<f64>, v| Some(acc.map_or(v, |a| a.min(v))));
                if let Some(p) = pct_left {
                    return Some(format!("{}: {:.0}% left", a.label, p));
                }
                let m = usage.meters.first()?;
                let v = m.remaining.or(m.used)?;
                let left = m.remaining.is_some();
                Some(match m.unit {
                    Unit::Count => format!("{}: {v:.0} {}", a.label, if left { "left" } else { "used" }),
                    unit => {
                        let sym = if unit == Unit::Cny { "¥" } else { "$" };
                        format!("{}: {sym}{v:.2} {}", a.label, if left { "left" } else { "spent" })
                    }
                })
            })
            .collect()
    };
    let mut text = if lines.is_empty() { "UsageCheck".to_string() } else { lines.join("\n") };
    // Windows truncates tray tooltips at 127 characters.
    if text.chars().count() > 120 {
        text = text.chars().take(117).collect::<String>() + "…";
    }
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_tooltip(Some(text));
    }
}
