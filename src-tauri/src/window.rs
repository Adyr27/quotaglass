//! The one window: restore its place, acrylic backdrop, always-on-top, and saving position and size.

use crate::commands::{emit_backdrop, AppState};
use crate::scheduler::lock;
use crate::settings::WindowRect;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, WindowEvent};

const MARGIN: i64 = 24;
const SAVE_DELAY: Duration = Duration::from_millis(500);
/// RGBA tint under the acrylic blur.
const ACRYLIC_TINT: (u8, u8, u8, u8) = (18, 18, 22, 140);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Keeps a saved window reachable after a monitor was unplugged or rearranged.
/// The size is clamped to the monitor. If the window's centre is on no monitor, it moves to the
/// top-right corner of the first monitor with a 24 px margin.
pub fn clamp_to_monitors(rect: WindowRect, monitors: &[MonitorRect]) -> WindowRect {
    let Some(first) = monitors.first() else {
        return rect;
    };
    let centre_x = i64::from(rect.x) + i64::from(rect.width) / 2;
    let centre_y = i64::from(rect.y) + i64::from(rect.height) / 2;
    let on = |m: &MonitorRect| {
        centre_x >= i64::from(m.x)
            && centre_x < i64::from(m.x) + i64::from(m.width)
            && centre_y >= i64::from(m.y)
            && centre_y < i64::from(m.y) + i64::from(m.height)
    };
    match monitors.iter().find(|m| on(m)) {
        Some(m) => WindowRect {
            width: rect.width.min(m.width),
            height: rect.height.min(m.height),
            ..rect
        },
        None => {
            // Fit inside the margins so a relocated window is fully visible.
            let inner = |side: u32| side.saturating_sub(2 * MARGIN as u32).max(1);
            let width = rect.width.min(inner(first.width));
            let height = rect.height.min(inner(first.height));
            WindowRect {
                x: (i64::from(first.x) + i64::from(first.width) - i64::from(width) - MARGIN) as i32,
                y: (i64::from(first.y) + MARGIN) as i32,
                width,
                height,
            }
        }
    }
}

pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    let window = app
        .get_webview_window("main")
        .ok_or(tauri::Error::WindowNotFound)?;
    let state = app.state::<AppState>();
    let settings = lock(&state.settings).clone();

    if let Some(saved) = settings.window {
        let monitors: Vec<MonitorRect> = window
            .available_monitors()?
            .iter()
            .map(|m| MonitorRect {
                x: m.position().x,
                y: m.position().y,
                width: m.size().width,
                height: m.size().height,
            })
            .collect();
        let rect = clamp_to_monitors(saved, &monitors);
        window.set_size(PhysicalSize::new(rect.width, rect.height))?;
        window.set_position(PhysicalPosition::new(rect.x, rect.y))?;
    }

    if window_vibrancy::apply_acrylic(&window, Some(ACRYLIC_TINT)).is_err() {
        tracing::warn!("acrylic backdrop is not available; the UI will use a solid background");
        state.acrylic.store(false, Ordering::Relaxed);
        emit_backdrop(app, false);
    }
    window.set_always_on_top(settings.always_on_top)?;

    // Moved and Resized fire many times per drag: save once the window has been still for 500 ms.
    let generation = Arc::new(AtomicU64::new(0));
    let handle = app.clone();
    window.on_window_event(move |event| {
        if !matches!(event, WindowEvent::Moved(_) | WindowEvent::Resized(_)) {
            return;
        }
        let mine = generation.fetch_add(1, Ordering::SeqCst) + 1;
        let generation = generation.clone();
        let handle = handle.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(SAVE_DELAY).await;
            if generation.load(Ordering::SeqCst) == mine {
                save_current(&handle);
            }
        });
    });

    window.show()
}

fn save_current(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if window.is_minimized().unwrap_or(false) {
        return;
    }
    let (Ok(pos), Ok(size)) = (window.outer_position(), window.outer_size()) else {
        return;
    };
    let rect = WindowRect {
        x: pos.x,
        y: pos.y,
        width: size.width,
        height: size.height,
    };

    let state = app.state::<AppState>();
    let mut settings = lock(&state.settings);
    if settings.window == Some(rect) {
        return;
    }
    settings.window = Some(rect);
    if state.store.save(&settings).is_err() {
        tracing::warn!("could not save the window position");
    }
}

/// Tray left click and the Show/Hide item.
pub fn toggle(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
    } else {
        show(app);
    }
}

pub fn show(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRIMARY: MonitorRect = MonitorRect {
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
    };
    const SECOND: MonitorRect = MonitorRect {
        x: 1920,
        y: 0,
        width: 1280,
        height: 1024,
    };

    #[test]
    fn offscreen_rect_moves_to_primary() {
        let gone = WindowRect {
            x: 5000,
            y: 4000,
            width: 360,
            height: 520,
        };
        let moved = clamp_to_monitors(gone, &[PRIMARY]);
        assert_eq!(
            moved,
            WindowRect {
                x: 1920 - 360 - 24,
                y: 24,
                width: 360,
                height: 520
            }
        );

        // Oversized window on no monitor: it shrinks to fit inside the margins.
        let huge = WindowRect {
            x: -9000,
            y: 0,
            width: 4000,
            height: 3000,
        };
        let moved = clamp_to_monitors(huge, &[PRIMARY]);
        assert_eq!(
            moved,
            WindowRect {
                x: 24,
                y: 24,
                width: 1920 - 48,
                height: 1080 - 48
            }
        );
    }

    #[test]
    fn visible_rect_is_kept() {
        let rect = WindowRect {
            x: 2000,
            y: 100,
            width: 360,
            height: 520,
        };
        assert_eq!(clamp_to_monitors(rect, &[PRIMARY, SECOND]), rect);
        assert_eq!(clamp_to_monitors(rect, &[]), rect);
    }
}
