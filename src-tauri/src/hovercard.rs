//! The hover card: when the pointer rests on a taskbar-widget item, a popup with
//! that account's full details (every limit, reset times, notes) appears just
//! above it, like the card in the main window.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};
use usage_core::{Account, Snapshot};

use crate::commands::AppState;
use crate::widget;

pub const LABEL: &str = "widget-card";
const GAP: i32 = 8;
const HIDE_DELAY: Duration = Duration::from_millis(250);

#[derive(Default)]
pub struct CardState {
    /// The account the card is showing and the hovered item's left edge inside the widget (CSS px).
    target: Mutex<Option<(String, f64)>>,
    over_widget: AtomicBool,
    over_card: AtomicBool,
    hide_generation: AtomicU64,
}

/// Built hidden alongside the widget, from a background task (never inline in a command).
pub fn create(app: &AppHandle) -> tauri::Result<()> {
    if app.get_webview_window(LABEL).is_some() {
        return Ok(());
    }
    let w = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("card.html".into()))
        .title("UsageCheck details")
        .inner_size(320.0, 200.0)
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
    widget::make_tool_window(&w);
    Ok(())
}

pub fn destroy(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.destroy();
    }
}

pub fn hide_now(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.hide();
    }
}

/// The pointer entered a widget item.
#[tauri::command]
pub fn widget_hover(app: AppHandle, account_id: String, offset_x: f64) {
    let cs = app.state::<CardState>();
    cs.over_widget.store(true, Ordering::Relaxed);
    cs.hide_generation.fetch_add(1, Ordering::Relaxed);
    let changed = {
        let mut t = cs.target.lock().unwrap();
        let changed = t.as_ref().map(|(id, _)| id != &account_id).unwrap_or(true);
        *t = Some((account_id.clone(), offset_x));
        changed
    };
    if changed {
        let _ = app.emit("card-show", &account_id);
    } else if let Some(w) = app.get_webview_window(LABEL) {
        position(&app, &w);
        let _ = w.show();
    }
}

/// The pointer left the widget.
#[tauri::command]
pub fn widget_unhover(app: AppHandle) {
    app.state::<CardState>().over_widget.store(false, Ordering::Relaxed);
    schedule_hide(&app);
}

/// The pointer entered or left the card itself (so it can be read without vanishing).
#[tauri::command]
pub fn card_hover(app: AppHandle, inside: bool) {
    app.state::<CardState>().over_card.store(inside, Ordering::Relaxed);
    if !inside {
        schedule_hide(&app);
    }
}

/// The card page has rendered and measured itself.
#[tauri::command]
pub fn card_layout(app: AppHandle, window: WebviewWindow, width: f64, height: f64) {
    if window.label() != LABEL {
        return;
    }
    let _ = window.set_size(LogicalSize::new(width.clamp(200.0, 420.0).ceil(), height.clamp(60.0, 640.0).ceil()));
    let cs = app.state::<CardState>();
    let hovered = cs.over_widget.load(Ordering::Relaxed) || cs.over_card.load(Ordering::Relaxed);
    // A refresh re-renders the card too; it must only appear if the pointer is
    // really there (a missed "pointer left" event would otherwise pop it up on
    // every update).
    if hovered && pointer_over(&app) {
        position(&app, &window);
        let _ = window.show();
    } else if !hovered {
        let _ = window.hide();
    }
}

/// Whether the mouse pointer is over the widget or the card right now.
fn pointer_over(app: &AppHandle) -> bool {
    let Ok(p) = app.cursor_position() else { return true };
    [widget::LABEL, LABEL].iter().filter_map(|l| app.get_webview_window(l)).any(|w| {
        let visible = w.is_visible().unwrap_or(false);
        let (Ok(pos), Ok(size)) = (w.outer_position(), w.outer_size()) else { return false };
        visible
            && p.x >= pos.x as f64
            && p.y >= pos.y as f64
            && p.x < (pos.x + size.width as i32) as f64
            && p.y < (pos.y + size.height as i32) as f64
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CardView {
    account: Option<Account>,
    snapshot: Option<Snapshot>,
    theme: String,
}

#[tauri::command]
pub fn card_state(app: AppHandle, account_id: String) -> CardView {
    let state = app.state::<AppState>();
    let (account, theme) = {
        let cfg = state.config.lock().unwrap();
        (cfg.accounts.iter().find(|a| a.id == account_id).cloned(), cfg.settings.theme.clone())
    };
    let snapshot = state.snapshots.lock().unwrap().get(&account_id).cloned();
    CardView { account, snapshot, theme }
}

fn schedule_hide(app: &AppHandle) {
    let cs = app.state::<CardState>();
    let generation = cs.hide_generation.fetch_add(1, Ordering::Relaxed) + 1;
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(HIDE_DELAY).await;
        let cs = app.state::<CardState>();
        let still_away = !cs.over_widget.load(Ordering::Relaxed) && !cs.over_card.load(Ordering::Relaxed);
        if still_away && cs.hide_generation.load(Ordering::Relaxed) == generation {
            hide_now(&app);
        }
    });
}

/// Above the hovered item (or below it when the taskbar is at the top), kept on screen.
fn position(app: &AppHandle, card: &WebviewWindow) {
    let Some(w) = app.get_webview_window(widget::LABEL) else { return };
    let (Ok(wpos), Ok(wsize), Ok(csize)) = (w.outer_position(), w.outer_size(), card.outer_size()) else { return };
    let scale = w.scale_factor().unwrap_or(1.0);
    let offset = app.state::<CardState>().target.lock().unwrap().as_ref().map(|t| t.1).unwrap_or(0.0);
    let (cw, ch) = (csize.width as i32, csize.height as i32);

    let mut x = wpos.x + (offset * scale) as i32;
    let mut y = wpos.y - ch - GAP;
    if let Ok(Some(m)) = w.current_monitor() {
        let (mp, ms) = (m.position(), m.size());
        if y < mp.y {
            y = wpos.y + wsize.height as i32 + GAP;
        }
        x = x.min(mp.x + ms.width as i32 - cw - GAP).max(mp.x + GAP);
    }
    let _ = card.set_position(PhysicalPosition::new(x, y));
}
