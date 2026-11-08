// Hides the console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod connect;
mod notify;
mod scheduler;
mod settings;
mod tray;
mod window;

use commands::AppState;
use notify::{WarnStore, Warnings};
use quotaglass_core::Env;
use scheduler::Scheduler;
use settings::SettingsStore;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;
use tauri::{Emitter, Manager};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(if cfg!(debug_assertions) {
            tracing::Level::DEBUG
        } else {
            tracing::Level::WARN
        })
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let path = SettingsStore::default_path().ok_or("APPDATA is not set")?;
            let store = SettingsStore::new(path);
            let mut settings = store.load();
            // The OS entry is the truth, so the file and the OS cannot drift apart.
            if let Ok(enabled) = app.autolaunch().is_enabled() {
                settings.autostart = enabled;
            }

            let warn_store = WarnStore::new(WarnStore::default_path().ok_or("APPDATA is not set")?);
            app.manage(Warnings {
                state: Mutex::new(warn_store.load()),
                store: warn_store,
            });

            let handle = app.handle().clone();
            let env = Env::from_system().map_err(|e| format!("Cannot start: {e:?}"))?;
            let scheduler = Scheduler::new(env, move |snapshot| {
                let _ = handle.emit("snapshot", snapshot);
                notify::on_snapshot(&handle, &handle.state::<Warnings>(), snapshot);
            });

            app.manage(AppState {
                settings: Mutex::new(settings.clone()),
                store,
                scheduler,
                acrylic: AtomicBool::new(true),
            });
            window::setup(app.handle())?;
            tray::build(app.handle())?;
            app.state::<AppState>().scheduler.apply(&settings);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_snapshot,
            commands::refresh_now,
            commands::detect,
            commands::get_settings,
            commands::update_settings,
            commands::connect,
            commands::add_account,
            commands::open_cli,
            commands::remove_account,
            commands::set_hidden,
            commands::set_pinned,
            commands::set_label,
            commands::connect_source,
            commands::open_url,
        ])
        .run(tauri::generate_context!())
        .expect("error while running QuotaGlass");
}
