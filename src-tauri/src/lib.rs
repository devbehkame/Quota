mod commands;
mod config;
mod hovercard;
mod refresh;
#[cfg(windows)]
mod registry;
mod rename;
mod secrets;
mod startup;
mod tray;
mod updater;
mod widget;

use std::collections::HashMap;
use std::sync::Mutex;

use tauri::{Manager, WindowEvent};

use commands::AppState;

const MINIMIZED_ARG: &str = "--minimized";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Must be registered first: a second launch just focuses the running window.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| tray::show_main(app)))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![MINIMIZED_ARG]),
        ))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let config_path = app.path().app_config_dir()?.join("config.json");
            let cfg = config::load(&config_path);
            app.manage(AppState {
                client: usage_core::http_client(),
                config_path,
                config: Mutex::new(cfg),
                snapshots: Mutex::new(HashMap::new()),
                locks: Default::default(),
                pace: Default::default(),
            });
            app.manage(widget::WidgetState::default());
            app.manage(hovercard::CardState::default());
            app.manage(updater::UpdaterState::default());
            tray::create(app.handle())?;
            refresh::spawn_scheduler(app.handle().clone());
            widget::sync(app.handle());
            updater::spawn(app.handle().clone());
            rename::spawn(app.handle().clone());
            startup::sync(app.handle());

            // The window starts hidden to avoid a white flash; show it unless we were
            // launched at login with --minimized.
            if !std::env::args().any(|a| a == MINIMIZED_ARG) {
                tray::show_main(app.handle());
            }
            Ok(())
        })
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { api, .. } if window.label() == "main" => {
                let to_tray = window.state::<AppState>().config.lock().unwrap().settings.close_to_tray;
                if to_tray {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
            WindowEvent::Moved(pos) if window.label() == widget::LABEL => {
                widget::on_moved(window.app_handle(), *pos);
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::add_account,
            commands::update_account,
            commands::remove_account,
            commands::reorder_accounts,
            commands::refresh_account,
            commands::refresh_all,
            commands::test_account,
            commands::save_settings,
            commands::hide_window,
            commands::quit_app,
            commands::github_sign_in,
            updater::update_status,
            updater::check_for_updates,
            widget::widget_state,
            widget::widget_layout,
            widget::widget_open_main,
            widget::widget_reset_position,
            hovercard::widget_hover,
            hovercard::widget_unhover,
            hovercard::card_hover,
            hovercard::card_layout,
            hovercard::card_state,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Quota");
}
