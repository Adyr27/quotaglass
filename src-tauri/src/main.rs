// Hides the console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod connect;
mod flyout;
mod notify;
mod reset_notify;
mod scheduler;
mod settings;
mod topmost;
mod tray;
mod window;

use commands::AppState;
use notify::{JsonStore, Warnings};
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
        .manage(window::WindowState::default())
        .manage(flyout::FlyoutState::default())
        .setup(|app| {
            let path = SettingsStore::default_path().ok_or("APPDATA is not set")?;
            let store = SettingsStore::new(path);
            let mut settings = store.load();
            // The OS entry is the truth, so the file and the OS cannot drift apart.
            if let Ok(enabled) = app.autolaunch().is_enabled() {
                settings.autostart = enabled;
            }

            let warn_store =
                JsonStore::new(JsonStore::in_appdata("warnings.json").ok_or("APPDATA is not set")?);
            app.manage(Warnings {
                state: Mutex::new(warn_store.load()),
                store: warn_store,
            });
            let reset_store =
                JsonStore::new(JsonStore::in_appdata("resets.json").ok_or("APPDATA is not set")?);
            app.manage(reset_notify::Resets::new(
                reset_store,
                settings.reset_toasts.clone(),
            ));

            let handle = app.handle().clone();
            let env = Env::from_system().map_err(|e| format!("Cannot start: {e:?}"))?;
            let scheduler = Scheduler::new(env, move |snapshot| {
                let _ = handle.emit("snapshot", snapshot);
                notify::on_snapshot(&handle, &handle.state::<Warnings>(), snapshot);
                reset_notify::on_snapshot(&handle, snapshot);
            });

            app.manage(AppState {
                settings: Mutex::new(settings.clone()),
                store,
                scheduler,
                acrylic: AtomicBool::new(true),
            });
            // The windows have `create: false`: built only now, so no page can call a command before its state is managed.
            // The release build loads the page from memory fast enough to win that race (`state() called before manage()`).
            for config in &app.config().app.windows {
                tauri::WebviewWindowBuilder::from_config(app.handle(), config)?.build()?;
            }
            window::setup(app.handle())?;
            flyout::setup(app.handle());
            tray::build(app.handle())?;
            reset_notify::start(app.handle());
            topmost::start(app.handle());
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
            commands::set_label,
            commands::connect_source,
            commands::open_url,
            commands::set_zoom,
            commands::set_pill_scale,
            commands::set_pill_slots,
            commands::set_pill_locked,
            commands::flyout_hover,
            commands::flyout_pointer,
            commands::flyout_open_popover,
            commands::flyout_close,
            commands::flyout_size,
        ])
        .run(tauri::generate_context!())
        .expect("error while running QuotaGlass");
}

#[cfg(test)]
mod tests {
    /// Tauri builds `create: true` windows before `setup`. Every window must be built in `setup`, after the last
    /// `manage`, or a command from its page can panic the release build.
    #[test]
    fn windows_are_built_after_state_is_managed() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let windows = conf["app"]["windows"].as_array().unwrap();
        assert_eq!(windows.len(), 2);
        for w in windows {
            assert_eq!(w["create"], false, "{}", w["label"]);
        }

        // Only the code above this test module counts; the strings below appear in it too.
        let code = &include_str!("main.rs")[..include_str!("main.rs").find("mod tests").unwrap()];
        let built = code.find("WebviewWindowBuilder::from_config").unwrap();
        let last_manage = code.rfind("app.manage(").unwrap();
        let window_setup = code.find("window::setup(app.handle())").unwrap();
        assert!(
            last_manage < built,
            "a window is built before the last manage"
        );
        assert!(built < window_setup, "window::setup needs the windows");
    }
}
