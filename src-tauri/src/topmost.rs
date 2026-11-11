//! Keeps the pill above the taskbar and hides it while a fullscreen app runs.
//! Clicking the taskbar raises it above other top-most windows, so the pill re-asserts
//! `HWND_TOPMOST` every 2 s. No event hook, no activation, no move, no size change.

use crate::commands::AppState;
use crate::scheduler::lock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Manager, WebviewWindow};

const TICK: Duration = Duration::from_secs(2);

/// Set when the owner hides the pill (tray); the loop never shows it again.
static USER_HIDDEN: AtomicBool = AtomicBool::new(false);
/// Set when the loop hid the pill for a fullscreen app.
static AUTO_HIDDEN: AtomicBool = AtomicBool::new(false);

/// Called by the tray toggle and Show: the owner's choice wins over the loop.
pub fn set_user_hidden(hidden: bool) {
    USER_HIDDEN.store(hidden, Ordering::SeqCst);
    AUTO_HIDDEN.store(false, Ordering::SeqCst);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Keep,
    Hide,
    Show,
}

/// `QUERY_USER_NOTIFICATION_STATE`: 2 busy, 3 D3D fullscreen, 4 presentation mode.
pub fn pure_should_hide(state: i32) -> bool {
    matches!(state, 2..=4)
}

/// A failed query (HRESULT not `S_OK`) is never "busy": a broken query must not hide the pill.
fn busy_from(hresult: i32, state: i32) -> bool {
    hresult == 0 && pure_should_hide(state)
}

/// A window or monitor rect: (left, top, right, bottom) in device px.
type Rect = (i32, i32, i32, i32);

/// The desktop, taskbar and Alt-Tab view also fill the screen; they are never fullscreen apps.
fn is_shell_class(class: &str) -> bool {
    matches!(
        class,
        "Progman"
            | "WorkerW"
            | "Shell_TrayWnd"
            | "Shell_SecondaryTrayWnd"
            | "XamlExplorerHostIslandWindow"
    )
}

/// Windows often does not report browser fullscreen as busy, so the loop also checks the
/// foreground window. A fullscreen window is exactly its monitor; a maximized one is larger by its borders.
fn covers_screen(window: Rect, monitor: Rect, class: &str) -> bool {
    window == monitor && !is_shell_class(class)
}

/// The loop's rule: hide for fullscreen, show again only what it hid, never touch a user-hidden pill.
pub fn next_visibility(busy: bool, auto_hidden: bool, user_hidden: bool, visible: bool) -> Action {
    if user_hidden {
        Action::Keep
    } else if busy && visible {
        Action::Hide
    } else if !busy && auto_hidden && !visible {
        Action::Show
    } else {
        Action::Keep
    }
}

#[cfg(windows)]
mod ffi {
    use std::ffi::c_void;
    pub type Hwnd = *mut c_void;
    // Values checked against windows-sys 0.61.2 (generated from Microsoft's Win32 metadata).
    pub const HWND_TOPMOST: isize = -1;
    pub const SWP_NOSIZE: u32 = 0x0001;
    pub const SWP_NOMOVE: u32 = 0x0002;
    pub const SWP_NOACTIVATE: u32 = 0x0010;
    pub const SWP_NOOWNERZORDER: u32 = 0x0200;
    pub const SW_HIDE: i32 = 0;
    pub const SW_SHOWNOACTIVATE: i32 = 4;
    pub const MONITOR_DEFAULTTONEAREST: u32 = 2;
    #[repr(C)]
    #[derive(Default)]
    pub struct Rect {
        pub left: i32,
        pub top: i32,
        pub right: i32,
        pub bottom: i32,
    }
    #[repr(C)]
    #[derive(Default)]
    pub struct MonitorInfo {
        pub size: u32,
        pub monitor: Rect,
        pub work: Rect,
        pub flags: u32,
    }
    #[link(name = "user32")]
    extern "system" {
        pub fn GetForegroundWindow() -> Hwnd;
        pub fn GetWindowRect(h: Hwnd, rect: *mut Rect) -> i32;
        pub fn GetClassNameW(h: Hwnd, name: *mut u16, max: i32) -> i32;
        pub fn MonitorFromWindow(h: Hwnd, flags: u32) -> *mut c_void;
        pub fn GetMonitorInfoW(monitor: *mut c_void, info: *mut MonitorInfo) -> i32;
        pub fn SetWindowPos(
            h: Hwnd,
            after: Hwnd,
            x: i32,
            y: i32,
            cx: i32,
            cy: i32,
            flags: u32,
        ) -> i32;
        pub fn ShowWindow(h: Hwnd, cmd: i32) -> i32;
        pub fn SetForegroundWindow(h: Hwnd) -> i32;
    }
    #[link(name = "shell32")]
    extern "system" {
        pub fn SHQueryUserNotificationState(state: *mut i32) -> i32;
    }
}

/// Re-asserts top-most without activating, moving or resizing the window.
pub fn assert_topmost(window: &WebviewWindow) {
    #[cfg(windows)]
    {
        use ffi::*;
        let Ok(hwnd) = window.hwnd() else {
            return;
        };
        let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER;
        // SAFETY: `hwnd` is this process's live window; with NOMOVE and NOSIZE the zeros are ignored.
        unsafe { SetWindowPos(hwnd.0, HWND_TOPMOST as Hwnd, 0, 0, 0, 0, flags) };
    }
    #[cfg(not(windows))]
    let _ = window;
}

/// True while a fullscreen app, game or presentation is in front.
pub fn fullscreen_busy() -> bool {
    #[cfg(windows)]
    {
        let mut state = 0i32;
        // SAFETY: `state` is a valid, writable i32 for the call's duration.
        let hr = unsafe { ffi::SHQueryUserNotificationState(&mut state) };
        busy_from(hr, state) || foreground_covers_screen()
    }
    #[cfg(not(windows))]
    false
}

/// True when the window in front is exactly the size of its monitor (and is not the desktop or taskbar).
#[cfg(windows)]
fn foreground_covers_screen() -> bool {
    use ffi::*;
    let as_tuple = |r: &ffi::Rect| (r.left, r.top, r.right, r.bottom);
    // SAFETY: no arguments; returns a window handle or null.
    let fg = unsafe { GetForegroundWindow() };
    if fg.is_null() {
        return false;
    }
    let mut win = ffi::Rect::default();
    // SAFETY: `fg` is a window handle from the OS; `win` is a valid, writable RECT.
    if unsafe { GetWindowRect(fg, &mut win) } == 0 {
        return false;
    }
    // SAFETY: `fg` is a window handle; NEAREST always returns a monitor.
    let monitor = unsafe { MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST) };
    let mut info = MonitorInfo {
        size: std::mem::size_of::<MonitorInfo>() as u32,
        ..Default::default()
    };
    // SAFETY: `monitor` came from MonitorFromWindow; `info` is writable and its size field is set.
    if monitor.is_null() || unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
        return false;
    }
    let mut name = [0u16; 64];
    // SAFETY: `name` is a writable buffer of exactly the length passed.
    let len = unsafe { GetClassNameW(fg, name.as_mut_ptr(), name.len() as i32) };
    let class = String::from_utf16_lossy(&name[..len.max(0) as usize]);
    covers_screen(as_tuple(&win), as_tuple(&info.monitor), &class)
}

/// Hides the pill with the same raw call that shows it.
/// Tauri keeps its own visible flag and skips `hide()` when that flag already says hidden. `show_no_activate`
/// does not update the flag, so after one hide/show cycle Tauri's `hide()` did nothing. After setup, the pill is
/// shown and hidden only through these two functions, so the stale flag is never consulted.
pub fn hide_window(window: &WebviewWindow) {
    #[cfg(windows)]
    {
        let Ok(hwnd) = window.hwnd() else {
            return;
        };
        // SAFETY: `hwnd` is this process's live window; SW_HIDE only changes visibility.
        unsafe { ffi::ShowWindow(hwnd.0, ffi::SW_HIDE) };
    }
    #[cfg(not(windows))]
    let _ = window.hide();
}

/// Shows the window without taking focus from the app the owner is using.
pub fn show_no_activate(window: &WebviewWindow) {
    #[cfg(windows)]
    {
        let Ok(hwnd) = window.hwnd() else {
            return;
        };
        // SAFETY: `hwnd` is this process's live window; SW_SHOWNOACTIVATE only changes visibility.
        unsafe { ffi::ShowWindow(hwnd.0, ffi::SW_SHOWNOACTIVATE) };
    }
    #[cfg(not(windows))]
    let _ = window.show();
}

/// Gives the window keyboard focus. Raw call: Tauri's `set_focus` does nothing while its own visible flag says
/// hidden, and a window shown with `show_no_activate` never sets that flag.
pub fn focus_window(window: &WebviewWindow) {
    #[cfg(windows)]
    {
        let Ok(hwnd) = window.hwnd() else {
            return;
        };
        // SAFETY: `hwnd` is this process's live window; the call only asks to bring it to the foreground.
        unsafe { ffi::SetForegroundWindow(hwnd.0) };
    }
    #[cfg(not(windows))]
    let _ = window.set_focus();
}

fn tick(app: &AppHandle, on_top: bool) {
    let Some(main) = app.get_webview_window("main") else {
        return;
    };
    let flyout = app.get_webview_window("flyout");
    let visible = main.is_visible().unwrap_or(false);
    let action = next_visibility(
        fullscreen_busy(),
        AUTO_HIDDEN.load(Ordering::SeqCst),
        USER_HIDDEN.load(Ordering::SeqCst),
        visible,
    );
    match action {
        Action::Hide => {
            hide_window(&main);
            crate::flyout::close(app);
            AUTO_HIDDEN.store(true, Ordering::SeqCst);
        }
        Action::Show => {
            show_no_activate(&main);
            AUTO_HIDDEN.store(false, Ordering::SeqCst);
        }
        Action::Keep => {}
    }
    if on_top && action != Action::Hide && main.is_visible().unwrap_or(false) {
        assert_topmost(&main);
        if let Some(f) = flyout.filter(|f| f.is_visible().unwrap_or(false)) {
            assert_topmost(&f);
        }
    }
}

/// Starts the 2 s loop. Window calls run on the main thread, which owns the windows.
pub fn start(app: &AppHandle) {
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut timer = tokio::time::interval(TICK);
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            timer.tick().await;
            // Read before the hop: main-thread code must not take the settings lock.
            let on_top = lock(&handle.state::<AppState>().settings).always_on_top;
            let h = handle.clone();
            let _ = handle.run_on_main_thread(move || tick(&h, on_top));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fullscreen_states_map() {
        for s in [1, 5, 6, 7] {
            assert!(!pure_should_hide(s), "state {s}");
        }
        for s in [2, 3, 4] {
            assert!(pure_should_hide(s), "state {s}");
        }
        assert!(!pure_should_hide(0) && !pure_should_hide(8));
    }

    #[test]
    fn hresult_error_is_not_busy() {
        // E_FAIL with a busy-looking state left in the out value.
        assert!(!busy_from(0x8000_4005_u32 as i32, 3));
        assert!(busy_from(0, 3));
    }

    #[test]
    fn foreground_filling_its_monitor_is_fullscreen() {
        let mon = (0, 0, 1920, 1080);
        assert!(covers_screen(mon, mon, "Chrome_WidgetWin_1"));
        // Second monitor, left of the primary.
        let left = (-2560, 0, 0, 1440);
        assert!(covers_screen(left, left, "Chrome_WidgetWin_1"));
        // Maximized: bigger than the monitor by the 8 px borders.
        assert!(!covers_screen(
            (-8, -8, 1928, 1088),
            mon,
            "Chrome_WidgetWin_1"
        ));
        // Maximized with a visible taskbar, and a normal window.
        assert!(!covers_screen((-8, -8, 1928, 1040), mon, "Notepad"));
        assert!(!covers_screen((100, 100, 900, 700), mon, "Notepad"));
    }

    #[test]
    fn desktop_and_taskbar_are_never_fullscreen() {
        let mon = (0, 0, 1920, 1080);
        for class in [
            "Progman",
            "WorkerW",
            "Shell_TrayWnd",
            "Shell_SecondaryTrayWnd",
            "XamlExplorerHostIslandWindow",
        ] {
            assert!(is_shell_class(class), "{class}");
            assert!(!covers_screen(mon, mon, class), "{class}");
        }
        assert!(!is_shell_class("Chrome_WidgetWin_1"));
    }

    #[test]
    fn user_hidden_is_never_shown_by_the_loop() {
        for busy in [false, true] {
            for auto in [false, true] {
                for visible in [false, true] {
                    assert_eq!(next_visibility(busy, auto, true, visible), Action::Keep);
                }
            }
        }
        // Not user-hidden: hide on fullscreen, show again only what the loop hid.
        assert_eq!(next_visibility(true, false, false, true), Action::Hide);
        assert_eq!(next_visibility(false, true, false, false), Action::Show);
        assert_eq!(next_visibility(false, false, false, false), Action::Keep);
        assert_eq!(next_visibility(true, true, false, false), Action::Keep);
        assert_eq!(next_visibility(false, false, false, true), Action::Keep);
    }
}
