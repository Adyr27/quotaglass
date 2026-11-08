//! The one window: pill and panel geometry, docking, glass material, theme, always-on-top, and saving the pill's place.

use crate::commands::{emit_backdrop, AppState};
use crate::scheduler::lock;
use crate::settings::{Material, PanelSize, Theme, WindowRect, PANEL_MIN, PILL_WIDTH};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewWindow, WindowEvent,
};

const MARGIN: i64 = 24;
const SAVE_DELAY: Duration = Duration::from_millis(500);

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
    match monitors.iter().find(|m| contains(m, centre_x, centre_y)) {
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

/// Pill and panel sizes in logical px; multiply by the monitor's scale factor.
pub const PILL: (u32, u32) = (440, 44);
/// The panel's first size; the person can resize it, down to `PANEL_MIN`.
pub const PANEL: (u32, u32) = (520, 400);

/// Where the pill goes on first run: bottom-right of the work area, `MARGIN` (scaled) from both edges.
pub fn default_pill_rect(work: MonitorRect, scale: f64) -> WindowRect {
    let px = |v: i64| (v as f64 * scale).round() as i64;
    let (w, h, m) = (px(PILL.0.into()), px(PILL.1.into()), px(MARGIN));
    WindowRect {
        x: (i64::from(work.x) + i64::from(work.width) - w - m) as i32,
        y: (i64::from(work.y) + i64::from(work.height) - h - m) as i32,
        width: w as u32,
        height: h as u32,
    }
}

/// The panel opens from the pill: same left and bottom edges (grows up) when it fits above,
/// else same top edge (grows down); then it is moved to fit inside `work`.
pub fn panel_rect(pill: WindowRect, panel_w: u32, panel_h: u32, work: MonitorRect) -> WindowRect {
    // A saved size from a larger monitor is shrunk to fit this one.
    let (panel_w, panel_h) = (panel_w.min(work.width), panel_h.min(work.height));
    let (top, left) = (i64::from(work.y), i64::from(work.x));
    let bottom = top + i64::from(work.height);
    let right = left + i64::from(work.width);
    let grown_up = i64::from(pill.y) + i64::from(pill.height) - i64::from(panel_h);
    let y = if grown_up >= top {
        grown_up
    } else {
        i64::from(pill.y)
    };
    // `max` last: a panel taller than the work area keeps its top (and title) visible.
    let y = y.min(bottom - i64::from(panel_h)).max(top);
    let x = i64::from(pill.x).min(right - i64::from(panel_w)).max(left);
    WindowRect {
        x: x as i32,
        y: y as i32,
        width: panel_w,
        height: panel_h,
    }
}

/// Where the pill goes when the panel closes: at its bottom edge when the panel grew up and its
/// top edge when it grew down, and at the same horizontal offset (`dx`) it had when the panel
/// opened. A panel dragged or resized while open keeps the pill on that edge.
pub fn pill_after_close(
    pill_w: u32,
    pill_h: u32,
    dx: i32,
    panel_now: WindowRect,
    up: bool,
) -> WindowRect {
    let y = if up {
        i64::from(panel_now.y) + i64::from(panel_now.height) - i64::from(pill_h)
    } else {
        i64::from(panel_now.y)
    };
    // Same horizontal offset as at open, kept inside the panel if it was resized narrower.
    let right = i64::from(panel_now.x) + i64::from(panel_now.width) - i64::from(pill_w);
    let x = (i64::from(panel_now.x) + i64::from(dx))
        .min(right)
        .max(i64::from(panel_now.x));
    WindowRect {
        x: x as i32,
        y: y as i32,
        width: pill_w,
        height: pill_h,
    }
}

fn contains(m: &MonitorRect, x: i64, y: i64) -> bool {
    x >= i64::from(m.x)
        && x < i64::from(m.x) + i64::from(m.width)
        && y >= i64::from(m.y)
        && y < i64::from(m.y) + i64::from(m.height)
}

/// Panel state. `open` holds the pill and panel rects from the moment the panel opened; only
/// `set_panel_open` locks it. Main-thread code reads `panel_open` instead, so it never waits on a
/// lock held by a command that is itself waiting on the main thread.
#[derive(Default)]
pub struct WindowState {
    open: Mutex<Option<(WindowRect, WindowRect)>>,
    panel_open: AtomicBool,
    dark: AtomicBool,
    /// The UI's zoom (device px per CSS px) as f64 bits; 0 until the UI reports it.
    zoom: AtomicU64,
}

/// The UI's zoom, or the monitor scale until the UI has reported it. Windows text size
/// (Accessibility) zooms the web view on top of the monitor scale, so only the UI knows it.
fn zoom(app: &AppHandle, fallback: f64) -> f64 {
    match app.state::<WindowState>().zoom.load(Ordering::Relaxed) {
        0 => fallback,
        bits => f64::from_bits(bits),
    }
}

fn pill_size(css_width: Option<u32>, z: f64) -> (u32, u32) {
    (scaled(css_width.unwrap_or(PILL.0), z), scaled(PILL.1, z))
}

/// The pill stretches sideways only: its height is pinned by equal min and max heights.
fn pill_limits(window: &WebviewWindow, z: f64) -> tauri::Result<()> {
    let h = scaled(PILL.1, z);
    window.set_min_size(Some(PhysicalSize::new(scaled(PILL_WIDTH.0, z), h)))?;
    window.set_max_size(Some(PhysicalSize::new(scaled(PILL_WIDTH.1, z), h)))
}

struct Screen {
    full: MonitorRect,
    work: MonitorRect,
    scale: f64,
}

fn to_screen(m: &tauri::Monitor) -> Screen {
    let work = m.work_area();
    Screen {
        full: MonitorRect {
            x: m.position().x,
            y: m.position().y,
            width: m.size().width,
            height: m.size().height,
        },
        work: MonitorRect {
            x: work.position.x,
            y: work.position.y,
            width: work.size.width,
            height: work.size.height,
        },
        scale: m.scale_factor(),
    }
}

/// The monitor under the rect's centre, else the primary monitor.
fn screen_for(window: &WebviewWindow, rect: WindowRect) -> Option<Screen> {
    let x = i64::from(rect.x) + i64::from(rect.width) / 2;
    let y = i64::from(rect.y) + i64::from(rect.height) / 2;
    let monitors = window.available_monitors().unwrap_or_default();
    monitors
        .iter()
        .map(to_screen)
        .find(|s| contains(&s.full, x, y))
        .or_else(|| {
            window
                .primary_monitor()
                .ok()
                .flatten()
                .as_ref()
                .map(to_screen)
        })
}

fn monitor_rects(window: &WebviewWindow) -> Vec<MonitorRect> {
    let monitors = window.available_monitors().unwrap_or_default();
    monitors.iter().map(|m| to_screen(m).full).collect()
}

fn scaled(v: u32, scale: f64) -> u32 {
    (f64::from(v) * scale).round() as u32
}

fn current_rect(window: &WebviewWindow) -> tauri::Result<WindowRect> {
    let (pos, size) = (window.outer_position()?, window.outer_size()?);
    Ok(WindowRect {
        x: pos.x,
        y: pos.y,
        width: size.width,
        height: size.height,
    })
}

fn place(window: &WebviewWindow, r: WindowRect) -> tauri::Result<()> {
    window.set_size(PhysicalSize::new(r.width, r.height))?;
    window.set_position(PhysicalPosition::new(r.x, r.y))
}

pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    let window = app
        .get_webview_window("main")
        .ok_or(tauri::Error::WindowNotFound)?;
    app.manage(WindowState::default());
    let state = app.state::<AppState>();
    let settings = lock(&state.settings).clone();

    // The place is saved in device px, the width in CSS px; the UI's zoom corrects the size once it loads.
    let start = match settings.pill_position {
        Some(saved) => screen_for(&window, saved).map(|s| {
            let (width, height) = pill_size(settings.pill_width, s.scale);
            let sized = WindowRect {
                width,
                height,
                ..saved
            };
            (clamp_to_monitors(sized, &monitor_rects(&window)), s.scale)
        }),
        None => window
            .primary_monitor()?
            .map(|m| to_screen(&m))
            .map(|s| (default_pill_rect(s.work, s.scale), s.scale)),
    };
    if let Some((rect, scale)) = start {
        place(&window, rect)?;
        pill_limits(&window, scale)?;
    }

    apply_look(app, settings.theme, settings.material);
    window.set_always_on_top(settings.always_on_top)?;

    // Moved and Resized fire many times per drag: save once the window has been still for 500 ms.
    let generation = Arc::new(AtomicU64::new(0));
    let handle = app.clone();
    window.on_window_event(move |event| match event {
        WindowEvent::ThemeChanged(_) => {
            let (theme, material) = {
                let state = handle.state::<AppState>();
                let settings = lock(&state.settings);
                (settings.theme, settings.material)
            };
            if theme == Theme::System {
                apply_look(&handle, theme, material);
            }
        }
        WindowEvent::Moved(_) | WindowEvent::Resized(_) => {
            let mine = generation.fetch_add(1, Ordering::SeqCst) + 1;
            let generation = generation.clone();
            let handle = handle.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(SAVE_DELAY).await;
                if generation.load(Ordering::SeqCst) == mine {
                    save_current(&handle);
                }
            });
        }
        _ => {}
    });

    #[cfg(windows)]
    keep_glass::install(&window);
    window.show()
}

/// Windows 11 draws its Mica and Acrylic backdrops only while the window is active. When a click lands outside the
/// pill, DWM paints a solid fallback instead. Every `WM_NCACTIVATE` is passed on as "active", so the backdrop stays
/// on. The window has no title bar, so the inactive look changes nothing else; focus and activation are untouched.
#[cfg(windows)]
mod keep_glass {
    use std::ffi::c_void;

    type Hwnd = *mut c_void;
    type SubclassProc = unsafe extern "system" fn(Hwnd, u32, usize, isize, usize, usize) -> isize;

    const WM_NCACTIVATE: u32 = 0x0086;
    const SUBCLASS_ID: usize = 0x5147;

    #[link(name = "comctl32")]
    extern "system" {
        fn SetWindowSubclass(hwnd: Hwnd, proc: SubclassProc, id: usize, data: usize) -> i32;
        fn DefSubclassProc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize;
    }

    unsafe extern "system" fn proc(
        hwnd: Hwnd,
        msg: u32,
        wparam: usize,
        lparam: isize,
        _id: usize,
        _data: usize,
    ) -> isize {
        let wparam = if msg == WM_NCACTIVATE { 1 } else { wparam };
        // SAFETY: forwards the message to the next handler in the chain, as the subclass contract requires.
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }

    /// Runs on the main thread, which owns the window, as `SetWindowSubclass` requires.
    pub fn install(window: &tauri::WebviewWindow) {
        let Ok(hwnd) = window.hwnd() else {
            return;
        };
        // SAFETY: `hwnd` is this process's live main window, and `proc` is a static function that holds no state.
        if unsafe { SetWindowSubclass(hwnd.0, proc, SUBCLASS_ID, 0) } == 0 {
            tracing::warn!("could not keep the glass on while the window is inactive");
        }
    }
}

/// Clears every backdrop, then applies the chosen one. False when the OS refused it.
pub fn apply_material(window: &WebviewWindow, m: Material, dark: bool) -> bool {
    let _ = window_vibrancy::clear_acrylic(window);
    let _ = window_vibrancy::clear_mica(window);
    let _ = window_vibrancy::clear_blur(window);
    let tint = |on_dark, on_light| Some(if dark { on_dark } else { on_light });
    match m {
        Material::Acrylic => {
            window_vibrancy::apply_acrylic(window, tint((18, 18, 22, 140), (246, 246, 244, 150)))
                .is_ok()
        }
        Material::Clear => {
            window_vibrancy::apply_acrylic(window, tint((18, 18, 22, 60), (246, 246, 244, 70)))
                .is_ok()
        }
        Material::Mica => window_vibrancy::apply_mica(window, Some(dark)).is_ok(),
    }
}

/// Resolves the theme, tells the UI, and re-applies the material with the matching tint.
/// Takes values, not the settings lock: on the main thread `run_on_main_thread` runs inline.
pub fn apply_look(app: &AppHandle, theme: Theme, material: Material) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let Some(window) = handle.get_webview_window("main") else {
            return;
        };
        let dark = match theme {
            Theme::Dark => true,
            Theme::Light => false,
            Theme::System => window.theme().map_or(true, |t| t == tauri::Theme::Dark),
        };
        handle
            .state::<WindowState>()
            .dark
            .store(dark, Ordering::Relaxed);
        emit_theme(&handle, dark);
        let glass = apply_material(&window, material, dark);
        if !glass {
            tracing::warn!(
                "the glass backdrop is not available; the UI will use a solid background"
            );
        }
        handle
            .state::<AppState>()
            .acrylic
            .store(glass, Ordering::Relaxed);
        emit_backdrop(&handle, glass);
    });
}

fn emit_theme(app: &AppHandle, dark: bool) {
    let _ = app.emit("theme", if dark { "dark" } else { "light" });
}

/// Re-sends the theme for a UI that loaded after `setup` emitted it.
pub fn resend_theme(app: &AppHandle) {
    emit_theme(app, app.state::<WindowState>().dark.load(Ordering::Relaxed));
}

/// Opens the panel from the pill (one resize) or closes it back to the pill.
/// Returns true while the panel extends above the pill, so the UI keeps the pill row at its bottom.
pub fn set_panel_open(app: &AppHandle, open: bool) -> tauri::Result<bool> {
    let window = app
        .get_webview_window("main")
        .ok_or(tauri::Error::WindowNotFound)?;
    let ws = app.state::<WindowState>();
    let mut slot = lock(&ws.open);
    let now = current_rect(&window)?;
    let state = app.state::<AppState>();
    // Copied out: the settings lock is never held while a window call waits on the main thread.
    let (panel_size, pill_width) = {
        let s = lock(&state.settings);
        (s.panel_size, s.pill_width)
    };
    let Some(screen) = screen_for(&window, now) else {
        return Ok(false);
    };
    let z = zoom(app, screen.scale);
    if open {
        if let Some((pill, panel)) = *slot {
            return Ok(panel.y < pill.y);
        }
        let (w, h) = panel_size.map_or(PANEL, |p| (p.width, p.height));
        let panel = panel_rect(now, scaled(w, z), scaled(h, z), screen.work);
        // Set before resizing, so the resize is not saved as the pill's place.
        ws.panel_open.store(true, Ordering::SeqCst);
        window.set_max_size(None::<PhysicalSize<u32>>)?;
        place(&window, panel)?;
        window.set_min_size(Some(PhysicalSize::new(
            scaled(PANEL_MIN.0, z),
            scaled(PANEL_MIN.1, z),
        )))?;
        *slot = Some((now, panel));
        return Ok(panel.y < now.y);
    } else if let Some((pill, panel)) = slot.take() {
        // Keep the size the person left the panel at, in CSS px.
        {
            let mut s = lock(&state.settings);
            s.panel_size = Some(PanelSize {
                width: (f64::from(now.width) / z).round() as u32,
                height: (f64::from(now.height) / z).round() as u32,
            });
            s.clamp();
            if state.store.save(&s).is_err() {
                tracing::warn!("could not save the panel size");
            }
        }
        let (w, h) = pill_size(pill_width, z);
        let back = clamp_to_monitors(
            pill_after_close(w, h, pill.x - panel.x, now, panel.y < pill.y),
            &monitor_rects(&window),
        );
        window.set_min_size(None::<PhysicalSize<u32>>)?;
        place(&window, back)?;
        pill_limits(&window, z)?;
        ws.panel_open.store(false, Ordering::SeqCst);
    }
    Ok(false)
}

/// The UI reports its zoom; the pill is resized to match, keeping its bottom-left corner.
pub fn set_zoom(app: &AppHandle, z: f64) -> tauri::Result<()> {
    if !z.is_finite() {
        return Ok(());
    }
    let z = z.clamp(0.5, 5.0);
    let ws = app.state::<WindowState>();
    ws.zoom.store(z.to_bits(), Ordering::Relaxed);
    // Same lock as `set_panel_open`, so a resize never runs while the panel opens or closes.
    let slot = lock(&ws.open);
    if slot.is_some() {
        return Ok(()); // the next close sizes the pill
    }
    let window = app
        .get_webview_window("main")
        .ok_or(tauri::Error::WindowNotFound)?;
    let pill_width = lock(&app.state::<AppState>().settings).pill_width;
    let now = current_rect(&window)?;
    let (w, h) = pill_size(pill_width, z);
    let grown = WindowRect {
        x: now.x,
        y: now.y + now.height as i32 - h as i32,
        width: w,
        height: h,
    };
    // panel_rect with the pill's own size only moves it inside the work area.
    let fitted = match screen_for(&window, grown) {
        Some(s) => panel_rect(grown, w, h, s.work),
        None => grown,
    };
    window.set_min_size(None::<PhysicalSize<u32>>)?;
    window.set_max_size(None::<PhysicalSize<u32>>)?;
    place(&window, fitted)?;
    pill_limits(&window, z)
}

fn save_current(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    // While the panel is open the window is the panel, not the pill.
    if app.state::<WindowState>().panel_open.load(Ordering::SeqCst)
        || window.is_minimized().unwrap_or(false)
    {
        return;
    }
    let Ok(rect) = current_rect(&window) else {
        return;
    };

    let z = zoom(app, screen_for(&window, rect).map_or(1.0, |s| s.scale));
    let width = ((f64::from(rect.width) / z).round() as u32).clamp(PILL_WIDTH.0, PILL_WIDTH.1);

    let state = app.state::<AppState>();
    let mut settings = lock(&state.settings);
    if settings.pill_position == Some(rect) && settings.pill_width == Some(width) {
        return;
    }
    settings.pill_position = Some(rect);
    settings.pill_width = Some(width);
    if state.store.save(&settings).is_err() {
        tracing::warn!("could not save the window position");
    }
}

/// Tray left click.
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

/// Tray Show: bring the window back on screen if a monitor change left it off, then focus it.
pub fn show(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if let Ok(now) = current_rect(&window) {
        let back = clamp_to_monitors(now, &monitor_rects(&window));
        if back != now {
            let _ = place(&window, back);
        }
    }
    let _ = window.show();
    let _ = window.set_focus();
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

    const WORK: MonitorRect = MonitorRect {
        x: 0,
        y: 0,
        width: 1920,
        height: 1040,
    };
    fn rect(x: i32, y: i32, width: u32, height: u32) -> WindowRect {
        WindowRect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn default_pill_docks_bottom_right() {
        assert_eq!(
            default_pill_rect(WORK, 1.0),
            rect(1920 - 440 - 24, 1040 - 44 - 24, 440, 44)
        );
        assert_eq!(
            default_pill_rect(WORK, 1.5),
            rect(1920 - 660 - 36, 1040 - 66 - 36, 660, 66)
        );
    }

    #[test]
    fn panel_rect_fits_work_area() {
        // Pill at the bottom: the panel grows up, sharing the pill's bottom edge.
        let pill = rect(1000, 972, 440, 44);
        assert_eq!(
            panel_rect(pill, 440, 344, WORK),
            rect(1000, 972 + 44 - 344, 440, 344)
        );
        // Pill near the top: no room above, so it grows down from the pill's top edge.
        let pill = rect(1000, 10, 440, 44);
        assert_eq!(panel_rect(pill, 440, 344, WORK), rect(1000, 10, 440, 344));
        // Pill half off the right edge: shifted left to fit.
        let pill = rect(1700, 600, 440, 44);
        assert_eq!(
            panel_rect(pill, 440, 344, WORK),
            rect(1920 - 440, 600 + 44 - 344, 440, 344)
        );
        // Second monitor left of the primary (negative x), pill past its left edge.
        let left = MonitorRect {
            x: -1280,
            y: 0,
            width: 1280,
            height: 984,
        };
        let pill = rect(-1300, 900, 440, 44);
        assert_eq!(
            panel_rect(pill, 440, 344, left),
            rect(-1280, 900 + 44 - 344, 440, 344)
        );
        // A panel taller than the room in either direction is pinned to the work area top.
        let short = MonitorRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 400,
        };
        let pill = rect(0, 100, 440, 44);
        assert_eq!(
            panel_rect(pill, 440, 344, short),
            rect(0, 400 - 344, 440, 344)
        );
    }

    #[test]
    fn panel_rect_scale_150() {
        let pill = default_pill_rect(WORK, 1.5);
        let panel = panel_rect(pill, 660, 516, WORK);
        assert_eq!(panel, rect(pill.x, pill.y + 66 - 516, 660, 516));
    }

    #[test]
    fn close_restores_pill_rect() {
        let pill = rect(1000, 972, 440, 44);
        let panel = panel_rect(pill, 440, 344, WORK);
        assert_eq!(pill_after_close(440, 44, 0, panel, true), pill);
        // Dragged 50 px left and 100 px up while open: the pill follows.
        let moved = rect(panel.x - 50, panel.y - 100, 440, 344);
        assert_eq!(
            pill_after_close(440, 44, 0, moved, true),
            rect(950, 872, 440, 44)
        );
        // Resized from its top edge while grown up: the pill stays on the bottom edge.
        let taller = rect(panel.x, panel.y - 200, 600, 544);
        assert_eq!(pill_after_close(440, 44, 0, taller, true), pill);
        // Grown down: the pill sits on the panel's top edge, whatever its height.
        let down = rect(1000, 10, 600, 500);
        assert_eq!(
            pill_after_close(440, 44, 0, down, false),
            rect(1000, 10, 440, 44)
        );
    }

    #[test]
    fn close_returns_pill_to_its_spot_after_a_shifted_panel() {
        // Pill docked at the right edge: the wider panel opens shifted left to fit.
        let pill = rect(1456, 972, 440, 44);
        let panel = panel_rect(pill, 520, 400, WORK);
        assert_eq!(panel.x, 1920 - 520);
        // Open and close ten times: the pill comes back to the same place every time.
        let mut now = pill;
        for _ in 0..10 {
            let open = panel_rect(now, 520, 400, WORK);
            now = pill_after_close(440, 44, now.x - open.x, open, true);
        }
        assert_eq!(now, pill);
    }

    #[test]
    fn panel_larger_than_work_area_is_shrunk() {
        let small = MonitorRect {
            x: 0,
            y: 0,
            width: 800,
            height: 300,
        };
        let pill = rect(100, 250, 440, 44);
        assert_eq!(panel_rect(pill, 1200, 900, small), rect(0, 0, 800, 300));
    }
}
