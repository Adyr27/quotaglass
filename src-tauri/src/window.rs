//! The pill window: size from slots, scale and zoom; docking, lock, glass material, theme, always-on-top, and saving the pill's place.

use crate::commands::{emit_backdrop, AppState};
use crate::scheduler::lock;
use crate::settings::{clamp_scale, Material, Theme, WindowRect};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
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

/// Pill sizes in CSS px at scale 1.0: padding, one slot per service, gaps between slots.
pub const PILL_SLOT: u32 = 24;
pub const PILL_GAP: u32 = 8;
pub const PILL_PAD: u32 = 8;
pub const PILL_H: u32 = 32;

pub fn pill_css_size(slots: u32) -> (u32, u32) {
    let n = slots.clamp(1, 3);
    (PILL_PAD * 2 + n * PILL_SLOT + (n - 1) * PILL_GAP, PILL_H)
}

/// Device px: the CSS size times the (clamped) pill scale times the UI zoom.
pub fn pill_px_size(slots: u32, scale: f64, zoom: f64) -> (u32, u32) {
    let (w, h) = pill_css_size(slots);
    let f = clamp_scale(scale) * zoom;
    (scaled(w, f), scaled(h, f))
}

/// The new scale, or `None` while the pill is locked.
fn next_scale(locked: bool, requested: f64) -> Option<f64> {
    (!locked).then(|| clamp_scale(requested))
}

/// A locked or minimized pill never saves a new position.
fn should_save_position(locked: bool, minimized: bool) -> bool {
    !locked && !minimized
}

/// Where the pill goes on first run: bottom-right of the work area, `MARGIN` (scaled) from both edges.
pub fn default_pill_rect(work: MonitorRect, size: (u32, u32), scale: f64) -> WindowRect {
    let m = (MARGIN as f64 * scale).round() as i64;
    let (w, h) = (i64::from(size.0), i64::from(size.1));
    WindowRect {
        x: (i64::from(work.x) + i64::from(work.width) - w - m) as i32,
        y: (i64::from(work.y) + i64::from(work.height) - h - m) as i32,
        width: size.0,
        height: size.1,
    }
}

/// Moves a rect inside `work`; one larger than `work` keeps its top-left corner visible.
pub fn fit_in(r: WindowRect, work: MonitorRect) -> WindowRect {
    let right = i64::from(work.x) + i64::from(work.width) - i64::from(r.width);
    let bottom = i64::from(work.y) + i64::from(work.height) - i64::from(r.height);
    WindowRect {
        x: i64::from(r.x).min(right).max(i64::from(work.x)) as i32,
        y: i64::from(r.y).min(bottom).max(i64::from(work.y)) as i32,
        ..r
    }
}

/// A resized pill keeps its left edge and vertical centre, then stays inside `work`.
pub fn resized_rect(now: WindowRect, w: u32, h: u32, work: MonitorRect) -> WindowRect {
    let centre = i64::from(now.y) + i64::from(now.height) / 2;
    let y = (centre - i64::from(h) / 2) as i32;
    fit_in(
        WindowRect {
            x: now.x,
            y,
            width: w,
            height: h,
        },
        work,
    )
}

fn contains(m: &MonitorRect, x: i64, y: i64) -> bool {
    x >= i64::from(m.x)
        && x < i64::from(m.x) + i64::from(m.width)
        && y >= i64::from(m.y)
        && y < i64::from(m.y) + i64::from(m.height)
}

/// Pill window state. Main-thread code never takes `resize`, so a command holding it may wait on the main thread.
#[derive(Default)]
pub struct WindowState {
    /// One resize at a time (zoom, scale and slot changes).
    resize: Mutex<()>,
    /// Service slots the UI shows, 1 to 3; 0 until the UI reports it (treated as 3).
    slots: AtomicU32,
    dark: AtomicBool,
    /// The UI's zoom (device px per CSS px) as f64 bits; 0 until the UI reports it.
    zoom: AtomicU64,
}

fn slots(ws: &WindowState) -> u32 {
    match ws.slots.load(Ordering::Relaxed) {
        0 => 3,
        n => n,
    }
}

/// The UI's zoom, or the monitor scale until the UI has reported it. Windows text size
/// (Accessibility) zooms the web view on top of the monitor scale, so only the UI knows it.
fn zoom(app: &AppHandle, fallback: f64) -> f64 {
    match app.state::<WindowState>().zoom.load(Ordering::Relaxed) {
        0 => fallback,
        bits => f64::from_bits(bits),
    }
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

pub fn place(window: &WebviewWindow, r: WindowRect) -> tauri::Result<()> {
    window.set_size(PhysicalSize::new(r.width, r.height))?;
    window.set_position(PhysicalPosition::new(r.x, r.y))
}

/// What the flyout anchors to: the pill's rect, its monitor's work area, and the UI zoom.
pub fn pill_frame(app: &AppHandle) -> Option<(WindowRect, MonitorRect, f64)> {
    let window = app.get_webview_window("main")?;
    let rect = current_rect(&window).ok()?;
    let screen = screen_for(&window, rect)?;
    Some((rect, screen.work, zoom(app, screen.scale)))
}

pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    let window = app
        .get_webview_window("main")
        .ok_or(tauri::Error::WindowNotFound)?;
    // `WindowState` is managed by the builder in `main`: the config windows exist before `setup` runs,
    // so the UI can report its zoom and slots first. Those values size the pill here.
    let ws = app.state::<WindowState>();
    let state = app.state::<AppState>();
    let settings = lock(&state.settings).clone();

    // The place is saved in device px; the size comes from the scale (three slots until the UI reports its count).
    let size = |s: &Screen| pill_px_size(slots(&ws), settings.pill_scale, zoom(app, s.scale));
    let start = match settings.pill_position {
        Some(saved) => screen_for(&window, saved).map(|s| {
            let (width, height) = size(&s);
            let sized = WindowRect {
                width,
                height,
                ..saved
            };
            clamp_to_monitors(sized, &monitor_rects(&window))
        }),
        None => window
            .primary_monitor()?
            .map(|m| to_screen(&m))
            .map(|s| default_pill_rect(s.work, size(&s), s.scale)),
    };
    if let Some(rect) = start {
        place(&window, rect)?;
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
    {
        corners::round(&window);
        keep_glass::install(&window);
        if let Some(flyout) = app.get_webview_window("flyout") {
            corners::round(&flyout);
            keep_glass::install(&flyout);
        }
    }
    window.show()
}

/// The glass backdrop fills the whole window rectangle, so CSS rounding alone leaves square glass corners. Windows 11
/// rounds the window itself, and the backdrop follows. Older Windows ignores the request. The pill and the flyout are
/// both top-level windows, so the request applies to each.
#[cfg(windows)]
mod corners {
    use std::ffi::c_void;

    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWCP_ROUND: u32 = 2;

    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: *mut c_void,
            attribute: u32,
            value: *const c_void,
            size: u32,
        ) -> i32;
    }

    /// Runs on the main thread, which owns the window.
    pub fn round(window: &tauri::WebviewWindow) {
        let Ok(hwnd) = window.hwnd() else {
            return;
        };
        let preference = DWMWCP_ROUND;
        // SAFETY: `hwnd` is this process's live window, and `preference` is a `u32` that outlives the call; the size
        // passed is its size. DWM copies the value and keeps no pointer.
        let hr = unsafe {
            DwmSetWindowAttribute(
                hwnd.0,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                (&preference as *const u32).cast(),
                std::mem::size_of::<u32>() as u32,
            )
        };
        if hr != 0 {
            tracing::warn!("could not round the window corners (HRESULT {hr:#x})");
        }
    }
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

    #[link(name = "user32")]
    extern "system" {
        fn SendMessageW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize;
    }

    /// A window shown without activation never receives `WM_NCACTIVATE`, so the subclass has nothing to rewrite and
    /// DWM draws the backdrop as inactive (flat grey) while another app is in front. Send the message by hand.
    /// Runs on the main thread, which owns the window.
    pub fn mark_active(window: &tauri::WebviewWindow) {
        let Ok(hwnd) = window.hwnd() else {
            return;
        };
        // SAFETY: `hwnd` is this process's live window; the subclass passes the message on as "active", which only
        // changes how the frame and backdrop are drawn.
        unsafe { SendMessageW(hwnd.0, WM_NCACTIVATE, 1, 0) };
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

/// Keeps the glass backdrop drawn as active on a window that was shown without activation.
pub fn keep_glass_on(window: &WebviewWindow) {
    #[cfg(windows)]
    keep_glass::mark_active(window);
    #[cfg(not(windows))]
    let _ = window;
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
        if let Some(flyout) = handle.get_webview_window("flyout") {
            apply_material(&flyout, material, dark);
        }
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

/// Sizes the pill for the current slots, scale and zoom.
fn resize(app: &AppHandle) -> tauri::Result<()> {
    let window = app
        .get_webview_window("main")
        .ok_or(tauri::Error::WindowNotFound)?;
    let ws = app.state::<WindowState>();
    // The UI can report its zoom before `setup` made the app state; `setup` then sizes the pill itself.
    let Some(state) = app.try_state::<AppState>() else {
        return Ok(());
    };
    let _one = lock(&ws.resize);
    let scale = lock(&state.settings).pill_scale;
    let now = current_rect(&window)?;
    let Some(screen) = screen_for(&window, now) else {
        return Ok(());
    };
    let (w, h) = pill_px_size(slots(&ws), scale, zoom(app, screen.scale));
    let next = resized_rect(now, w, h, screen.work);
    if next != now {
        place(&window, next)?;
    }
    Ok(())
}

/// The pill UI reports its zoom; Windows text size changes it.
pub fn set_zoom(app: &AppHandle, z: f64) -> tauri::Result<()> {
    if !z.is_finite() {
        return Ok(());
    }
    let ws = app.state::<WindowState>();
    ws.zoom
        .store(z.clamp(0.5, 5.0).to_bits(), Ordering::Relaxed);
    resize(app)
}

/// The pill UI reports how many service slots it shows.
pub fn set_pill_slots(app: &AppHandle, count: u32) -> tauri::Result<()> {
    let ws = app.state::<WindowState>();
    ws.slots.store(count.clamp(1, 3), Ordering::Relaxed);
    resize(app)
}

/// The grip: a new scale, ignored while locked. The debounced move/resize save writes it to disk.
pub fn set_pill_scale(app: &AppHandle, scale: f64) -> tauri::Result<()> {
    {
        let state = app.state::<AppState>();
        let mut s = lock(&state.settings);
        match next_scale(s.pill_locked, scale) {
            Some(next) if next != s.pill_scale => s.pill_scale = next,
            _ => return Ok(()),
        }
    }
    resize(app)
}

/// Pin or unpin the pill. Locking saves the current rect as home. Tells every window and the tray.
pub fn set_pill_locked(app: &AppHandle, locked: bool) -> Result<(), String> {
    let rect = app
        .get_webview_window("main")
        .and_then(|w| current_rect(&w).ok());
    {
        let state = app.state::<AppState>();
        let mut s = lock(&state.settings);
        s.pill_locked = locked;
        if locked && rect.is_some() {
            s.pill_position = rect;
        }
        state
            .store
            .save(&s)
            .map_err(|_| "Could not save the pin.".to_string())?;
    }
    crate::tray::set_pin_text(app, locked);
    let _ = app.emit("pill-locked", locked);
    Ok(())
}

fn save_current(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let Ok(rect) = current_rect(&window) else {
        return;
    };
    let minimized = window.is_minimized().unwrap_or(false);

    let state = app.state::<AppState>();
    let mut settings = lock(&state.settings);
    if !should_save_position(settings.pill_locked, minimized)
        || settings.pill_position == Some(rect)
    {
        return;
    }
    settings.pill_position = Some(rect);
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
        crate::topmost::set_user_hidden(true);
        // Raw hide, never Tauri's: see `topmost::hide_window`.
        crate::topmost::hide_window(&window);
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
    crate::topmost::set_user_hidden(false);
    // Raw show, never Tauri's: see `topmost::hide_window`. Focus is then asked for on purpose.
    crate::topmost::show_no_activate(&window);
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
            default_pill_rect(WORK, pill_px_size(3, 1.0, 1.0), 1.0),
            rect(1920 - 104 - 24, 1040 - 32 - 24, 104, 32)
        );
        assert_eq!(
            default_pill_rect(WORK, pill_px_size(3, 1.0, 1.5), 1.5),
            rect(1920 - 156 - 36, 1040 - 48 - 36, 156, 48)
        );
    }

    #[test]
    fn pill_css_size_for_1_2_3_slots() {
        assert_eq!(pill_css_size(3), (104, 32));
        assert_eq!(pill_css_size(1), (40, 32));
        assert_eq!(pill_css_size(2), (72, 32));
        assert_eq!(pill_css_size(0), (40, 32), "at least one slot");
        assert_eq!(pill_css_size(9), (104, 32), "at most three slots");
    }

    #[test]
    fn pill_px_size_scales() {
        assert_eq!(pill_px_size(3, 0.65, 1.0), (68, 21));
        assert_eq!(pill_px_size(3, 1.6, 1.0), (166, 51));
        assert_eq!(pill_px_size(3, 1.0, 1.5), (156, 48));
        assert_eq!(pill_px_size(1, 1.0, 1.5), (60, 48));
    }

    #[test]
    fn scale_is_clamped() {
        assert_eq!(pill_px_size(3, 9.0, 1.0), pill_px_size(3, 1.6, 1.0));
        assert_eq!(pill_px_size(3, 0.1, 1.0), pill_px_size(3, 0.65, 1.0));
        assert_eq!(pill_px_size(3, f64::NAN, 1.0), pill_px_size(3, 1.0, 1.0));
        assert_eq!(next_scale(false, 9.0), Some(1.6));
    }

    #[test]
    fn locked_pill_ignores_scale_and_move_saves() {
        assert_eq!(next_scale(true, 1.2), None);
        assert_eq!(next_scale(false, 1.2), Some(1.2));
        assert!(!should_save_position(true, false));
        assert!(!should_save_position(false, true));
        assert!(should_save_position(false, false));
    }

    #[test]
    fn resize_keeps_left_edge_and_centre() {
        let now = rect(1000, 900, 104, 32);
        assert_eq!(
            resized_rect(now, 166, 51, WORK),
            rect(1000, 916 - 25, 166, 51)
        );
        // Grown past the right edge: moved back inside the work area.
        let edge = rect(1880, 900, 40, 32);
        assert_eq!(resized_rect(edge, 104, 32, WORK).x, 1920 - 104);
        // Off the bottom: moved up.
        let low = rect(0, 1030, 104, 32);
        assert_eq!(resized_rect(low, 104, 32, WORK).y, 1040 - 32);
    }
}
