mod commands;
mod config;
mod secrets;
mod tray;

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
        .setup(|app| {
            let config_path = app.path().app_config_dir()?.join("config.json");
            let cfg = config::load(&config_path);
            app.manage(AppState {
                client: usage_core::http_client(),
                config_path,
                config: Mutex::new(cfg),
                snapshots: Mutex::new(HashMap::new()),
            });
            tray::create(app.handle())?;

            // The window starts hidden to avoid a white flash; show it unless we were
            // launched at login with --minimized.
            if !std::env::args().any(|a| a == MINIMIZED_ARG) {
                tray::show_main(app.handle());
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let to_tray = window.state::<AppState>().config.lock().unwrap().settings.close_to_tray;
                if to_tray {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::add_account,
            commands::update_account,
            commands::remove_account,
            commands::reorder_accounts,
            commands::refresh_account,
            commands::test_account,
            commands::save_settings,
            commands::hide_window,
            commands::quit_app,
        ])
        .run(tauri::generate_context!())
        .expect("error while running UsageCheck");
}
