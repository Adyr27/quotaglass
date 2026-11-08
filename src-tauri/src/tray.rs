//! Tray icon: Show/Hide, Pin/Unpin, Settings, Quit. Left click toggles the window.

use crate::commands::{self, AppState};
use crate::scheduler::lock;
use crate::settings::SettingsPatch;
use crate::window;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let toggle = MenuItem::with_id(app, "toggle", "Show/Hide", true, None::<&str>)?;
    // Static label: the UI can also change `always_on_top`, so a Pin/Unpin label could go stale.
    let pin = MenuItem::with_id(app, "pin", "Pin or unpin", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&toggle, &pin, &settings, &quit])?;

    let mut tray = TrayIconBuilder::new()
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "toggle" => window::toggle(app),
            "pin" => toggle_pin(app),
            "settings" => {
                window::show(app);
                let _ = app.emit("open-settings", ());
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                window::toggle(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

fn toggle_pin(app: &AppHandle) {
    let on_top = lock(&app.state::<AppState>().settings).always_on_top;
    match commands::update(
        app,
        SettingsPatch {
            always_on_top: Some(!on_top),
            ..Default::default()
        },
    ) {
        Ok(settings) => {
            let _ = app.emit("settings", &settings);
        }
        Err(_) => tracing::warn!("could not change the pin setting"),
    }
}
