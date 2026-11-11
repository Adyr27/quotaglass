//! Tray icon: Show, Focus QuotaGlass, Pin/Unpin pill, Settings, Quit. Left click shows or hides the pill.

use crate::commands::AppState;
use crate::scheduler::lock;
use crate::{flyout, window};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Wry};

/// The Pin item, kept so its text can follow the lock.
struct PinItem(MenuItem<Wry>);

fn pin_label(locked: bool) -> &'static str {
    if locked {
        "Unpin pill"
    } else {
        "Pin pill"
    }
}

/// Called after the lock changes. Never call it while holding the settings lock.
pub fn set_pin_text(app: &AppHandle, locked: bool) {
    if let Some(item) = app.try_state::<PinItem>() {
        let _ = item.0.set_text(pin_label(locked));
    }
}

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let locked = lock(&app.state::<AppState>().settings).pill_locked;
    let show = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
    let focus = MenuItem::with_id(app, "focus", "Focus QuotaGlass", true, None::<&str>)?;
    let pin = MenuItem::with_id(app, "pin", pin_label(locked), true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &focus, &pin, &settings, &quit])?;
    app.manage(PinItem(pin));

    let mut tray = TrayIconBuilder::new()
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "show" => window::show(app),
            "focus" => {
                window::show(app);
                let _ = app.emit_to("main", "focus-pill", ());
            }
            "pin" => {
                let locked = lock(&app.state::<AppState>().settings).pill_locked;
                let _ = window::set_pill_locked(app, !locked);
            }
            "settings" => {
                window::show(app);
                flyout::open_popover(app, None, Some(flyout::View::Settings));
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
