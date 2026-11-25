//! Win32 native windowing, 3 glass modes, DWM corners, `WS_EX_TOOLWINDOW`, `SetWindowSubclass`
//! (`WM_NCACTIVATE` + `WM_DISPLAYCHANGE` / `SPI_SETWORKAREA`), topmost/fullscreen checks,
//! monitor enumeration, and taskbar-relative anchor geometry.

use crate::settings::{clamp_scale, GlassMode, TaskbarAnchor, WindowRect};
use raw_window_handle::{HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle};
use std::num::NonZeroIsize;
use tokio::sync::mpsc;

const MARGIN: i64 = 24;
pub const GAP_CSS: u32 = 6;
pub const PILL_SLOT: u32 = 24;
pub const PILL_GAP: u32 = 8;
pub const PILL_PAD: u32 = 10;
pub const PILL_H: u32 = 32;

/// Non-zero Win32 `HWND` wrapper bridged from `iced::window::raw_id(id)` or `iced::window::Window`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HwndHandle(pub isize);

impl HwndHandle {
    /// Rejects `0` (`NULL` HWND).
    pub fn new(hwnd: isize) -> Option<Self> {
        NonZeroIsize::new(hwnd).map(|nz| Self(nz.get()))
    }

    /// Converts the `u64` returned by `iced::window::raw_id(id)`, rejecting `0`.
    pub fn try_from_raw_id(raw_id: u64) -> Option<Self> {
        if raw_id == 0 {
            return None;
        }
        Self::new(raw_id as isize)
    }

    /// Extracts an `HwndHandle` from any `HasWindowHandle` (such as `&dyn iced::window::Window`).
    pub fn from_window_handle(handle: &(impl HasWindowHandle + ?Sized)) -> Option<Self> {
        match handle.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(win32) => Some(Self(win32.hwnd.get())),
            _ => None,
        }
    }

    pub fn as_isize(self) -> isize {
        self.0
    }
}

impl HasWindowHandle for HwndHandle {
    fn window_handle(&self) -> Result<WindowHandle<'_>, raw_window_handle::HandleError> {
        let hwnd = NonZeroIsize::new(self.0).ok_or(raw_window_handle::HandleError::Unavailable)?;
        let raw = RawWindowHandle::Win32(Win32WindowHandle::new(hwnd));
        // SAFETY: `self.0` is non-zero and represents a live process HWND; the borrowed
        // `WindowHandle` lifetime is tied to `&self`.
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

/// Display topology or work-area change signaled by the Win32 window subclass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayEvent {
    DisplayChanged,
    WorkAreaChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubclassAction {
    KeepGlassActive,
    Signal(DisplayEvent),
    RemoveSubclass,
    Forward,
}

fn classify_window_message(msg: u32, wparam: usize) -> SubclassAction {
    const WM_SETTINGCHANGE: u32 = 0x001A;
    const WM_DISPLAYCHANGE: u32 = 0x007E;
    const WM_NCDESTROY: u32 = 0x0082;
    const WM_NCACTIVATE: u32 = 0x0086;
    const SPI_SETWORKAREA: usize = 0x002F;

    match msg {
        WM_NCACTIVATE => SubclassAction::KeepGlassActive,
        WM_DISPLAYCHANGE => SubclassAction::Signal(DisplayEvent::DisplayChanged),
        WM_SETTINGCHANGE if wparam == SPI_SETWORKAREA || wparam == 0 => {
            SubclassAction::Signal(DisplayEvent::WorkAreaChanged)
        }
        WM_NCDESTROY => SubclassAction::RemoveSubclass,
        _ => SubclassAction::Forward,
    }
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

fn busy_from(hresult: i32, state: i32) -> bool {
    hresult == 0 && pure_should_hide(state)
}

/// Desktop, taskbar, and Alt-Tab hosts cover the screen without being fullscreen apps.
pub fn is_shell_class(class: &str) -> bool {
    matches!(
        class,
        "Progman"
            | "WorkerW"
            | "Shell_TrayWnd"
            | "Shell_SecondaryTrayWnd"
            | "XamlExplorerHostIslandWindow"
    )
}

/// True when the foreground window exactly covers its monitor and is not a Windows shell host.
pub fn covers_screen(window: (i32, i32, i32, i32), monitor: (i32, i32, i32, i32), class: &str) -> bool {
    window == monitor && !is_shell_class(class)
}

/// Fullscreen auto-hide state machine: hides on busy fullscreen, restores only what it hid,
/// and never overrides `user_hidden`.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayMonitor {
    pub full: MonitorRect,
    pub work: MonitorRect,
    pub primary: bool,
    pub dpi: u32,
}

impl DisplayMonitor {
    pub fn dpi_percent(&self) -> u32 {
        let dpi = if self.dpi == 0 { 96 } else { self.dpi };
        ((f64::from(dpi) / 96.0) * 100.0).round() as u32
    }

    pub fn scale_factor(&self) -> f64 {
        let dpi = if self.dpi == 0 { 96 } else { self.dpi };
        f64::from(dpi) / 96.0
    }
}

/// Deterministic monitor topology key (`count:primary_w x primary_h @ dpi_pct`).
pub fn topology_signature(monitors: &[DisplayMonitor]) -> String {
    match monitors
        .iter()
        .find(|m| m.primary)
        .or_else(|| monitors.first())
    {
        Some(primary) => format!(
            "{}:{}x{}@{}",
            monitors.len(),
            primary.full.width,
            primary.full.height,
            primary.dpi_percent()
        ),
        None => "0:0x0@100".to_string(),
    }
}

fn contains_point(m: &MonitorRect, x: i64, y: i64) -> bool {
    x >= i64::from(m.x)
        && x < i64::from(m.x) + i64::from(m.width)
        && y >= i64::from(m.y)
        && y < i64::from(m.y) + i64::from(m.height)
}

fn clamp_i32(v: i64) -> i32 {
    v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// Captures a taskbar-relative anchor for the current monitor topology.
/// Detects whether the pill sits inside the primary monitor's bottom taskbar strip
/// (`work.y + work.height .. full.y + full.height`).
pub fn capture_taskbar_anchor(
    pill: WindowRect,
    monitors: &[DisplayMonitor],
) -> Option<(String, TaskbarAnchor)> {
    let primary = monitors
        .iter()
        .find(|m| m.primary)
        .or_else(|| monitors.first())?;
    let pill_cx = i64::from(pill.x) + i64::from(pill.width) / 2;
    let pill_cy = i64::from(pill.y) + i64::from(pill.height) / 2;

    let primary_left = i64::from(primary.full.x);
    let primary_right = primary_left + i64::from(primary.full.width);
    let work_bottom = i64::from(primary.work.y) + i64::from(primary.work.height);
    let full_bottom = i64::from(primary.full.y) + i64::from(primary.full.height);

    let on_primary_x = pill_cx >= primary_left && pill_cx < primary_right;
    let on_taskbar_strip =
        on_primary_x && full_bottom > work_bottom && pill_cy >= work_bottom && pill_cy < full_bottom;

    let anchor_mon = if on_taskbar_strip {
        primary
    } else {
        monitors
            .iter()
            .find(|m| contains_point(&m.full, pill_cx, pill_cy))?
    };

    let mon_right = i64::from(anchor_mon.full.x) + i64::from(anchor_mon.full.width);
    let taskbar_top = i64::from(anchor_mon.work.y) + i64::from(anchor_mon.work.height);
    let offset_from_right_px = clamp_i32(mon_right - (i64::from(pill.x) + i64::from(pill.width)));
    let offset_from_taskbar_top_px = clamp_i32(i64::from(pill.y) - taskbar_top);

    Some((
        topology_signature(monitors),
        TaskbarAnchor {
            offset_from_right_px,
            offset_from_taskbar_top_px,
            on_taskbar_strip,
            monitor_x: anchor_mon.full.x,
            monitor_y: anchor_mon.full.y,
        },
    ))
}

/// Resolves a saved [`TaskbarAnchor`] onto the active monitors.
/// When `on_taskbar_strip` is true, clamps vertically inside the primary bottom taskbar band
/// (or bottom edge of primary `full` if the band is narrower than the pill) so display switches
/// never push a taskbar-pinned pill above the taskbar.
pub fn resolve_taskbar_anchor(
    anchor: &TaskbarAnchor,
    pill_size: (u32, u32),
    monitors: &[DisplayMonitor],
) -> Option<WindowRect> {
    let primary = monitors
        .iter()
        .find(|m| m.primary)
        .or_else(|| monitors.first())?;
    let target = if anchor.on_taskbar_strip {
        primary
    } else {
        monitors
            .iter()
            .find(|m| m.full.x == anchor.monitor_x && m.full.y == anchor.monitor_y)
            .unwrap_or(primary)
    };

    let pw = pill_size.0.max(1).min(target.full.width.max(1));
    let ph = pill_size.1.max(1).min(target.full.height.max(1));

    let full_left = i64::from(target.full.x);
    let full_right = full_left + i64::from(target.full.width);
    let full_top = i64::from(target.full.y);
    let full_bottom = full_top + i64::from(target.full.height);
    let work_bottom = i64::from(target.work.y) + i64::from(target.work.height);

    let desired_x = full_right - i64::from(pw) - i64::from(anchor.offset_from_right_px);
    let x_max = (full_right - i64::from(pw)).max(full_left);
    let x = desired_x.clamp(full_left, x_max) as i32;

    let desired_y = work_bottom + i64::from(anchor.offset_from_taskbar_top_px);
    let y_full_max = (full_bottom - i64::from(ph)).max(full_top);
    let y = if anchor.on_taskbar_strip {
        if full_bottom > work_bottom && (full_bottom - work_bottom) >= i64::from(ph) {
            desired_y.clamp(work_bottom, y_full_max) as i32
        } else {
            // Pill is taller than the taskbar strip (or strip is auto-hidden): sit at bottom edge.
            y_full_max as i32
        }
    } else {
        desired_y.clamp(full_top, y_full_max) as i32
    };

    Some(WindowRect {
        x,
        y,
        width: pw,
        height: ph,
    })
}

/// Keeps a window rect reachable across monitor unplugs/rearrangements.
pub fn clamp_to_monitors(rect: WindowRect, monitors: &[MonitorRect]) -> WindowRect {
    let Some(first) = monitors.first() else {
        return rect;
    };
    let centre_x = i64::from(rect.x) + i64::from(rect.width) / 2;
    let centre_y = i64::from(rect.y) + i64::from(rect.height) / 2;
    match monitors.iter().find(|m| contains_point(m, centre_x, centre_y)) {
        Some(m) => {
            let width = rect.width.min(m.width);
            let height = rect.height.min(m.height);
            let sized = WindowRect {
                width,
                height,
                ..rect
            };
            if width < rect.width || height < rect.height {
                fit_in(sized, *m)
            } else {
                sized
            }
        }
        None => {
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

pub fn pill_css_size(slots: u32) -> (u32, u32) {
    let n = slots.clamp(1, 3);
    (PILL_PAD * 2 + n * PILL_SLOT + (n - 1) * PILL_GAP, PILL_H)
}

fn scaled(v: u32, scale: f64) -> u32 {
    (f64::from(v) * scale).round().max(1.0) as u32
}

pub fn pill_px_size(slots: u32, scale: f64, zoom: f64) -> (u32, u32) {
    let (w, h) = pill_css_size(slots);
    let z = if zoom.is_finite() && zoom > 0.0 {
        zoom
    } else {
        1.0
    };
    let f = clamp_scale(scale) * z;
    (scaled(w, f), scaled(h, f))
}

/// Default pill rect on first run: bottom-right of `work` with scaled margin.
pub fn default_pill_rect(work: MonitorRect, size: (u32, u32), scale: f64) -> WindowRect {
    let s = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let m = (MARGIN as f64 * s).round() as i64;
    let (w, h) = (i64::from(size.0), i64::from(size.1));
    let raw = WindowRect {
        x: clamp_i32(i64::from(work.x) + i64::from(work.width) - w - m),
        y: clamp_i32(i64::from(work.y) + i64::from(work.height) - h - m),
        width: size.0,
        height: size.1,
    };
    fit_in(raw, work)
}

/// Moves `r` inside `bounds`; an oversized rect keeps its top-left corner at `bounds` origin.
pub fn fit_in(r: WindowRect, bounds: MonitorRect) -> WindowRect {
    let w = r.width.min(bounds.width.max(1));
    let h = r.height.min(bounds.height.max(1));
    let right = i64::from(bounds.x) + i64::from(bounds.width) - i64::from(w);
    let bottom = i64::from(bounds.y) + i64::from(bounds.height) - i64::from(h);
    WindowRect {
        x: i64::from(r.x).min(right).max(i64::from(bounds.x)) as i32,
        y: i64::from(r.y).min(bottom).max(i64::from(bounds.y)) as i32,
        width: w,
        height: h,
    }
}

/// Computes a resized pill rect keeping its left edge and vertical center anchored, clamped to `bounds`.
pub fn resized_pill_rect(
    left_x: i32,
    center_y: i64,
    w: u32,
    h: u32,
    bounds: MonitorRect,
) -> WindowRect {
    let y = clamp_i32(center_y - i64::from(h) / 2);
    fit_in(
        WindowRect {
            x: left_x,
            y,
            width: w,
            height: h,
        },
        bounds,
    )
}

/// Places the popover cleanly above or below `pill` inside `work`, returning `(rect, opens_upward)`.
pub fn flyout_rect(
    pill: WindowRect,
    anchor_x_px: i32,
    w: u32,
    h: u32,
    work: MonitorRect,
    scale: f64,
) -> (WindowRect, bool) {
    let s = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let gap = (f64::from(GAP_CSS) * s).round() as i64;
    let (w, h) = (w.min(work.width.max(1)), h.min(work.height.max(1)));
    let above = i64::from(pill.y) - gap - i64::from(h);
    let up = above >= i64::from(work.y);
    let y = if up {
        above
    } else {
        i64::from(pill.y) + i64::from(pill.height) + gap
    };
    let x = i64::from(pill.x) + i64::from(anchor_x_px) - i64::from(w) / 2;
    let r = WindowRect {
        x: clamp_i32(x),
        y: clamp_i32(y),
        width: w,
        height: h,
    };
    (fit_in(r, work), up)
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::ffi::c_void;
    use std::sync::Mutex;

    type Hwnd = *mut c_void;
    type Hmonitor = *mut c_void;
    type Hkey = *mut c_void;
    type SubclassProc =
        unsafe extern "system" fn(Hwnd, u32, usize, isize, usize, usize) -> isize;
    type MonitorEnumProc =
        unsafe extern "system" fn(Hmonitor, *mut c_void, *mut Rect, isize) -> i32;

    const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWCP_ROUND: u32 = 2;

    const GWL_EXSTYLE: i32 = -20;
    const WS_EX_TOOLWINDOW: isize = 0x0000_0080;
    const WS_EX_APPWINDOW: isize = 0x0004_0000;

    const HWND_TOPMOST: isize = -1;
    const HWND_NOTOPMOST: isize = -2;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOMOVE: u32 = 0x0002;
    const SWP_NOZORDER: u32 = 0x0004;
    const SWP_NOACTIVATE: u32 = 0x0010;
    const SWP_FRAMECHANGED: u32 = 0x0020;
    const SWP_NOOWNERZORDER: u32 = 0x0200;

    const SW_HIDE: i32 = 0;
    const SW_SHOWNOACTIVATE: i32 = 4;

    const WM_NCACTIVATE: u32 = 0x0086;
    const SUBCLASS_ID: usize = 0x5147_0001;

    const MONITOR_DEFAULTTONEAREST: u32 = 2;
    const MONITORINFOF_PRIMARY: u32 = 1;
    const MDT_EFFECTIVE_DPI: i32 = 0;

    const SPI_GETCLIENTAREAANIMATION: u32 = 0x1042;

    // ((HKEY)(ULONG_PTR)((LONG)0x80000001))
    const HKEY_CURRENT_USER: Hkey = (-2147483647_i32 as isize) as Hkey;
    const KEY_QUERY_VALUE: u32 = 0x0001;
    const REG_DWORD: u32 = 4;

    static DISPLAY_TX: Mutex<Option<mpsc::UnboundedSender<DisplayEvent>>> = Mutex::new(None);

    #[repr(C)]
    #[derive(Default)]
    struct PointPx {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct MonitorInfo {
        size: u32,
        monitor: Rect,
        work: Rect,
        flags: u32,
    }

    #[link(name = "user32")]
    extern "system" {
        fn GetForegroundWindow() -> Hwnd;
        fn GetCursorPos(pt: *mut PointPx) -> i32;
        fn GetWindowRect(h: Hwnd, rect: *mut Rect) -> i32;
        fn GetClassNameW(h: Hwnd, name: *mut u16, max: i32) -> i32;
        fn MonitorFromWindow(h: Hwnd, flags: u32) -> Hmonitor;
        fn GetMonitorInfoW(monitor: Hmonitor, info: *mut MonitorInfo) -> i32;
        fn EnumDisplayMonitors(
            hdc: *mut c_void,
            clip: *const Rect,
            proc: MonitorEnumProc,
            data: isize,
        ) -> i32;
        fn SetWindowPos(
            h: Hwnd,
            after: Hwnd,
            x: i32,
            y: i32,
            cx: i32,
            cy: i32,
            flags: u32,
        ) -> i32;
        fn ShowWindow(h: Hwnd, cmd: i32) -> i32;
        fn SetForegroundWindow(h: Hwnd) -> i32;
        fn IsWindowVisible(h: Hwnd) -> i32;
        fn SendMessageW(h: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize;
        fn GetWindowLongPtrW(h: Hwnd, index: i32) -> isize;
        fn SetWindowLongPtrW(h: Hwnd, index: i32, new_long: isize) -> isize;
        fn SystemParametersInfoW(
            action: u32,
            param: u32,
            pv_param: *mut c_void,
            win_ini: u32,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetLastError() -> u32;
        fn SetLastError(err: u32);
    }

    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: Hwnd,
            attribute: u32,
            value: *const c_void,
            size: u32,
        ) -> i32;
    }

    #[link(name = "comctl32")]
    extern "system" {
        fn SetWindowSubclass(hwnd: Hwnd, proc: SubclassProc, id: usize, data: usize) -> i32;
        fn RemoveWindowSubclass(hwnd: Hwnd, proc: SubclassProc, id: usize) -> i32;
        fn DefSubclassProc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize;
    }

    #[link(name = "shell32")]
    extern "system" {
        fn SHQueryUserNotificationState(state: *mut i32) -> i32;
    }

    #[link(name = "shcore")]
    extern "system" {
        fn GetDpiForMonitor(
            monitor: Hmonitor,
            dpi_type: i32,
            dpi_x: *mut u32,
            dpi_y: *mut u32,
        ) -> i32;
    }

    #[link(name = "advapi32")]
    extern "system" {
        fn RegOpenKeyExW(
            hkey: Hkey,
            subkey: *const u16,
            options: u32,
            sam: u32,
            result: *mut Hkey,
        ) -> i32;
        fn RegQueryValueExW(
            hkey: Hkey,
            value_name: *const u16,
            reserved: *const u32,
            reg_type: *mut u32,
            data: *mut u8,
            cb_data: *mut u32,
        ) -> i32;
        fn RegCloseKey(hkey: Hkey) -> i32;
    }

    struct RegKey(Hkey);

    impl Drop for RegKey {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: `self.0` is an open registry key returned by `RegOpenKeyExW`.
                unsafe {
                    RegCloseKey(self.0);
                }
            }
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn read_personalize_dword(value_name: &str) -> Option<u32> {
        let subkey = wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
        let name = wide(value_name);
        let mut hkey: Hkey = std::ptr::null_mut();
        // SAFETY: `subkey` is null-terminated UTF-16 and `hkey` is a valid out-pointer.
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                KEY_QUERY_VALUE,
                &mut hkey,
            )
        };
        if status != 0 || hkey.is_null() {
            return None;
        }
        let _guard = RegKey(hkey);
        let mut reg_type: u32 = 0;
        let mut data: u32 = 0;
        let mut cb: u32 = std::mem::size_of::<u32>() as u32;
        // SAFETY: `hkey` is valid, `name` is null-terminated UTF-16, and `data` has room for 4 bytes.
        let q = unsafe {
            RegQueryValueExW(
                hkey,
                name.as_ptr(),
                std::ptr::null(),
                &mut reg_type,
                (&mut data as *mut u32).cast::<u8>(),
                &mut cb,
            )
        };
        (q == 0 && reg_type == REG_DWORD && cb == std::mem::size_of::<u32>() as u32)
            .then_some(data)
    }

    fn to_hwnd(hwnd: HwndHandle) -> Hwnd {
        hwnd.0 as Hwnd
    }

    fn set_immersive_dark_mode(hwnd: HwndHandle, dark: bool) {
        let val: u32 = u32::from(dark);
        // SAFETY: `hwnd` is a valid process HWND; `val` is a stack `u32` copied by DWM.
        unsafe {
            let _ = DwmSetWindowAttribute(
                to_hwnd(hwnd),
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                (&val as *const u32).cast(),
                std::mem::size_of::<u32>() as u32,
            );
        }
    }

    fn clear_backdrops(hwnd: HwndHandle) {
        let _ = window_vibrancy::clear_acrylic(hwnd);
        let _ = window_vibrancy::clear_mica(hwnd);
        let _ = window_vibrancy::clear_blur(hwnd);
    }

    pub fn apply_glass(hwnd: HwndHandle, mode: GlassMode, dark: bool) -> bool {
        clear_backdrops(hwnd);
        if !transparency_enabled() {
            return false;
        }
        set_immersive_dark_mode(hwnd, dark);
        // Never call `apply_blur` (`ACCENT_ENABLE_BLURBEHIND`): it blacks out Light mode on Win11
        // transparent wgpu windows. Calibrated `apply_acrylic` works reliably across all 3 modes.
        let tint = match (mode, dark) {
            (GlassMode::Clear, true) => (14, 17, 22, 42),
            (GlassMode::Clear, false) => (248, 250, 255, 52),
            (GlassMode::LiquidGlass, true) => (18, 22, 30, 140),
            (GlassMode::LiquidGlass, false) => (244, 248, 255, 150),
            (GlassMode::Frosted, true) => (20, 22, 28, 210),
            (GlassMode::Frosted, false) => (246, 246, 244, 215),
        };
        window_vibrancy::apply_acrylic(hwnd, Some(tint)).is_ok()
    }

    pub fn round_corners(hwnd: HwndHandle) {
        let preference = DWMWCP_ROUND;
        // SAFETY: `hwnd` is a valid process HWND and `preference` is a stack `u32` of size 4 bytes.
        let hr = unsafe {
            DwmSetWindowAttribute(
                to_hwnd(hwnd),
                DWMWA_WINDOW_CORNER_PREFERENCE,
                (&preference as *const u32).cast(),
                std::mem::size_of::<u32>() as u32,
            )
        };
        if hr != 0 {
            tracing::warn!("could not round window corners (HRESULT {hr:#x})");
        }
    }

    pub fn set_tool_window(hwnd: HwndHandle) {
        let raw = to_hwnd(hwnd);
        // SAFETY: `raw` is a valid process HWND. Clearing last error distinguishes a 0 ex-style from failure.
        unsafe {
            SetLastError(0);
            let ex = GetWindowLongPtrW(raw, GWL_EXSTYLE);
            if ex == 0 && GetLastError() != 0 {
                return;
            }
            let next = (ex | WS_EX_TOOLWINDOW) & !WS_EX_APPWINDOW;
            if next != ex {
                SetWindowLongPtrW(raw, GWL_EXSTYLE, next);
                SetWindowPos(
                    raw,
                    std::ptr::null_mut(),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                );
            }
        }
    }

    unsafe extern "system" fn subclass_proc(
        hwnd: Hwnd,
        msg: u32,
        wparam: usize,
        lparam: isize,
        id: usize,
        _data: usize,
    ) -> isize {
        let action = classify_window_message(msg, wparam);
        let forward_wparam = match action {
            SubclassAction::KeepGlassActive => 1,
            SubclassAction::Signal(ev) => {
                let tx = DISPLAY_TX
                    .lock()
                    .ok()
                    .and_then(|guard| guard.as_ref().cloned());
                if let Some(tx) = tx {
                    let _ = tx.send(ev);
                }
                wparam
            }
            SubclassAction::RemoveSubclass => {
                // SAFETY: `hwnd` is being destroyed (`WM_NCDESTROY`) and removing the installed subclass
                // with matching `(proc, id)` prevents callbacks after teardown.
                unsafe {
                    RemoveWindowSubclass(hwnd, subclass_proc, id);
                }
                wparam
            }
            SubclassAction::Forward => wparam,
        };
        // SAFETY: forwards message to the next handler in the subclass chain per `SetWindowSubclass` contract.
        unsafe { DefSubclassProc(hwnd, msg, forward_wparam, lparam) }
    }

    /// Installs the `WM_NCACTIVATE` + display/work-area change subclass on `hwnd`.
    /// MUST run on the window-owning thread (e.g., via `iced::window::run` or main thread).
    pub fn install_window_subclass(
        hwnd: HwndHandle,
        display_tx: mpsc::UnboundedSender<DisplayEvent>,
    ) {
        if let Ok(mut guard) = DISPLAY_TX.lock() {
            *guard = Some(display_tx);
        }
        // SAFETY: `hwnd` is owned by the calling thread, `subclass_proc` uses zero per-window heap
        // allocations (storing no pointer in `dwRefData`), and removes itself on `WM_NCDESTROY`.
        if unsafe { SetWindowSubclass(to_hwnd(hwnd), subclass_proc, SUBCLASS_ID, 0) } == 0 {
            tracing::warn!("could not install window subclass");
        }
    }

    pub fn mark_active(hwnd: HwndHandle) {
        // SAFETY: `hwnd` is a valid process HWND; `WM_NCACTIVATE` with `wparam = 1` paints active glass.
        unsafe {
            SendMessageW(to_hwnd(hwnd), WM_NCACTIVATE, 1, 0);
        }
    }

    pub fn assert_topmost(hwnd: HwndHandle) {
        let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER;
        // SAFETY: `hwnd` is a valid process HWND; `NOMOVE | NOSIZE` ignores zero position/size args.
        unsafe {
            SetWindowPos(to_hwnd(hwnd), HWND_TOPMOST as Hwnd, 0, 0, 0, 0, flags);
        }
    }

    pub fn set_topmost(hwnd: HwndHandle, topmost: bool) {
        let after = if topmost {
            HWND_TOPMOST
        } else {
            HWND_NOTOPMOST
        };
        let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER;
        // SAFETY: `hwnd` is a valid process HWND; only z-order is updated without activating or moving.
        unsafe {
            SetWindowPos(to_hwnd(hwnd), after as Hwnd, 0, 0, 0, 0, flags);
        }
    }

    pub fn show_no_activate(hwnd: HwndHandle) {
        // SAFETY: `hwnd` is a valid process HWND; `SW_SHOWNOACTIVATE` changes visibility only.
        unsafe {
            ShowWindow(to_hwnd(hwnd), SW_SHOWNOACTIVATE);
        }
    }

    pub fn hide_window(hwnd: HwndHandle) {
        // SAFETY: `hwnd` is a valid process HWND; `SW_HIDE` changes visibility only.
        unsafe {
            ShowWindow(to_hwnd(hwnd), SW_HIDE);
        }
    }

    pub fn focus_window(hwnd: HwndHandle) {
        // SAFETY: `hwnd` is a valid process HWND.
        unsafe {
            SetForegroundWindow(to_hwnd(hwnd));
        }
    }

    pub fn is_window_visible(hwnd: HwndHandle) -> bool {
        // SAFETY: `hwnd` is a valid process HWND.
        unsafe { IsWindowVisible(to_hwnd(hwnd)) != 0 }
    }

    pub fn move_and_resize_px(hwnd: HwndHandle, rect: WindowRect) -> bool {
        let cx = i32::try_from(rect.width).unwrap_or(i32::MAX);
        let cy = i32::try_from(rect.height).unwrap_or(i32::MAX);
        let flags = SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER;
        // SAFETY: `hwnd` is a valid process HWND; updates device-pixel bounds without z-order/focus change.
        unsafe { SetWindowPos(to_hwnd(hwnd), std::ptr::null_mut(), rect.x, rect.y, cx, cy, flags) != 0 }
    }

    pub fn cursor_pos_px() -> Option<(i32, i32)> {
        let mut pt = PointPx::default();
        // SAFETY: `pt` is a valid writable `POINT`.
        let ok = unsafe { GetCursorPos(&mut pt) };
        (ok != 0).then_some((pt.x, pt.y))
    }

    pub fn get_window_rect(hwnd: HwndHandle) -> Option<WindowRect> {
        let mut r = Rect::default();
        // SAFETY: `hwnd` is a valid process HWND and `r` is a writable `RECT`.
        let ok = unsafe { GetWindowRect(to_hwnd(hwnd), &mut r) };
        if ok == 0 {
            return None;
        }
        Some(WindowRect {
            x: r.left,
            y: r.top,
            width: r.right.saturating_sub(r.left).max(0) as u32,
            height: r.bottom.saturating_sub(r.top).max(0) as u32,
        })
    }

    pub fn fullscreen_busy() -> bool {
        let mut state = 0i32;
        // SAFETY: `state` is a valid writable `i32` for the call's duration.
        let hr = unsafe { SHQueryUserNotificationState(&mut state) };
        busy_from(hr, state) || foreground_covers_screen()
    }

    fn foreground_covers_screen() -> bool {
        let as_tuple = |r: &Rect| (r.left, r.top, r.right, r.bottom);
        // SAFETY: `GetForegroundWindow` takes no arguments and returns null or an OS window handle.
        let fg = unsafe { GetForegroundWindow() };
        if fg.is_null() {
            return false;
        }
        let mut win = Rect::default();
        // SAFETY: `fg` came from `GetForegroundWindow` and `win` is writable.
        if unsafe { GetWindowRect(fg, &mut win) } == 0 {
            return false;
        }
        // SAFETY: `fg` is an OS window handle; `MONITOR_DEFAULTTONEAREST` returns a monitor or null.
        let monitor = unsafe { MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST) };
        let mut info = MonitorInfo {
            size: std::mem::size_of::<MonitorInfo>() as u32,
            ..Default::default()
        };
        // SAFETY: `monitor` is non-null and `info.size` is initialized.
        if monitor.is_null() || unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
            return false;
        }
        let mut name = [0u16; 64];
        // SAFETY: `name` has length `name.len() as i32`.
        let len = unsafe { GetClassNameW(fg, name.as_mut_ptr(), name.len() as i32) };
        let class = String::from_utf16_lossy(&name[..len.max(0) as usize]);
        covers_screen(as_tuple(&win), as_tuple(&info.monitor), &class)
    }

    pub fn system_uses_dark_theme() -> bool {
        read_personalize_dword("AppsUseLightTheme").is_none_or(|v| v == 0)
    }

    pub fn transparency_enabled() -> bool {
        read_personalize_dword("EnableTransparency").is_none_or(|v| v != 0)
    }

    pub fn animations_enabled() -> bool {
        let mut enabled: i32 = 1;
        // SAFETY: `SPI_GETCLIENTAREAANIMATION` writes a `BOOL` (`i32`) into `enabled`.
        let ok = unsafe {
            SystemParametersInfoW(
                SPI_GETCLIENTAREAANIMATION,
                0,
                (&mut enabled as *mut i32).cast(),
                0,
            )
        };
        if ok == 0 {
            true
        } else {
            enabled != 0
        }
    }

    fn rect_to_monitor_rect(r: &Rect) -> MonitorRect {
        MonitorRect {
            x: r.left,
            y: r.top,
            width: r.right.saturating_sub(r.left).max(0) as u32,
            height: r.bottom.saturating_sub(r.top).max(0) as u32,
        }
    }

    unsafe extern "system" fn enum_monitor_proc(
        hmonitor: Hmonitor,
        _hdc: *mut c_void,
        _rect: *mut Rect,
        lparam: isize,
    ) -> i32 {
        let out = lparam as *mut Vec<DisplayMonitor>;
        if out.is_null() {
            return 0;
        }
        let mut info = MonitorInfo {
            size: std::mem::size_of::<MonitorInfo>() as u32,
            ..Default::default()
        };
        // SAFETY: `hmonitor` is provided by `EnumDisplayMonitors`; `info.size` is set.
        if unsafe { GetMonitorInfoW(hmonitor, &mut info) } == 0 {
            return 1;
        }
        let mut dpi_x: u32 = 96;
        let mut dpi_y: u32 = 96;
        // SAFETY: `hmonitor` is valid; `dpi_x` and `dpi_y` are writable `u32` out-pointers.
        let hr = unsafe { GetDpiForMonitor(hmonitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
        let dpi = if hr == 0 && dpi_x > 0 { dpi_x } else { 96 };
        let mon = DisplayMonitor {
            full: rect_to_monitor_rect(&info.monitor),
            work: rect_to_monitor_rect(&info.work),
            primary: (info.flags & MONITORINFOF_PRIMARY) != 0,
            dpi,
        };
        // SAFETY: `out` points to the stack `Vec<DisplayMonitor>` in `enumerate_monitors` for the
        // synchronous duration of `EnumDisplayMonitors`.
        unsafe {
            (*out).push(mon);
        }
        1
    }

    pub fn enumerate_monitors() -> Vec<DisplayMonitor> {
        let mut monitors: Vec<DisplayMonitor> = Vec::new();
        let ptr = &mut monitors as *mut Vec<DisplayMonitor> as isize;
        // SAFETY: `enum_monitor_proc` runs synchronously inside `EnumDisplayMonitors` while `monitors`
        // stays pinned on the stack and is never accessed concurrently.
        unsafe {
            let _ = EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), enum_monitor_proc, ptr);
        }
        monitors.sort_by_key(|m| !m.primary);
        monitors
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub fn apply_glass(hwnd: HwndHandle, mode: GlassMode, dark: bool) -> bool {
        let _ = (hwnd, mode, dark);
        false
    }
    pub fn round_corners(_hwnd: HwndHandle) {}
    pub fn set_tool_window(_hwnd: HwndHandle) {}
    pub fn install_window_subclass(
        _hwnd: HwndHandle,
        _display_tx: mpsc::UnboundedSender<DisplayEvent>,
    ) {
    }
    pub fn mark_active(_hwnd: HwndHandle) {}
    pub fn assert_topmost(_hwnd: HwndHandle) {}
    pub fn set_topmost(_hwnd: HwndHandle, _topmost: bool) {}
    pub fn show_no_activate(_hwnd: HwndHandle) {}
    pub fn hide_window(_hwnd: HwndHandle) {}
    pub fn focus_window(_hwnd: HwndHandle) {}
    pub fn is_window_visible(_hwnd: HwndHandle) -> bool {
        false
    }
    pub fn move_and_resize_px(_hwnd: HwndHandle, _rect: WindowRect) -> bool {
        false
    }
    pub fn cursor_pos_px() -> Option<(i32, i32)> {
        None
    }
    pub fn get_window_rect(_hwnd: HwndHandle) -> Option<WindowRect> {
        None
    }
    pub fn fullscreen_busy() -> bool {
        false
    }
    pub fn system_uses_dark_theme() -> bool {
        true
    }
    pub fn transparency_enabled() -> bool {
        false
    }
    pub fn animations_enabled() -> bool {
        true
    }
    pub fn enumerate_monitors() -> Vec<DisplayMonitor> {
        Vec::new()
    }
}

pub use imp::{
    animations_enabled, apply_glass, assert_topmost, cursor_pos_px, enumerate_monitors,
    focus_window, fullscreen_busy, get_window_rect, hide_window, install_window_subclass,
    is_window_visible, mark_active, move_and_resize_px, round_corners, set_tool_window,
    set_topmost, show_no_activate, system_uses_dark_theme, transparency_enabled,
};

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn mon(x: i32, y: i32, w: u32, h: u32, taskbar_h: u32, primary: bool, dpi: u32) -> DisplayMonitor {
        DisplayMonitor {
            full: MonitorRect {
                x,
                y,
                width: w,
                height: h,
            },
            work: MonitorRect {
                x,
                y,
                width: w,
                height: h.saturating_sub(taskbar_h),
            },
            primary,
            dpi,
        }
    }

    fn rect(x: i32, y: i32, width: u32, height: u32) -> WindowRect {
        WindowRect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn hwnd_handle_rejects_zero_and_wraps_nonzero() {
        assert_eq!(HwndHandle::new(0), None);
        assert_eq!(HwndHandle::try_from_raw_id(0), None);
        let h = HwndHandle::try_from_raw_id(0x1234).expect("nonzero");
        assert_eq!(h.as_isize(), 0x1234);
        let wh = h.window_handle().expect("valid handle");
        assert!(matches!(wh.as_raw(), RawWindowHandle::Win32(_)));
    }

    #[test]
    fn subclass_message_classification_matches_win32_rules() {
        assert_eq!(
            classify_window_message(0x0086, 0),
            SubclassAction::KeepGlassActive
        );
        assert_eq!(
            classify_window_message(0x007E, 0),
            SubclassAction::Signal(DisplayEvent::DisplayChanged)
        );
        assert_eq!(
            classify_window_message(0x001A, 0x002F),
            SubclassAction::Signal(DisplayEvent::WorkAreaChanged)
        );
        assert_eq!(
            classify_window_message(0x001A, 0),
            SubclassAction::Signal(DisplayEvent::WorkAreaChanged)
        );
        assert_eq!(classify_window_message(0x001A, 123), SubclassAction::Forward);
        assert_eq!(
            classify_window_message(0x0082, 0),
            SubclassAction::RemoveSubclass
        );
    }

    #[test]
    fn topology_signature_format() {
        let laptop = [mon(0, 0, 1920, 1080, 48, true, 120)];
        assert_eq!(topology_signature(&laptop), "1:1920x1080@125");

        let dual = [
            mon(-1920, 0, 1920, 1080, 40, false, 96),
            mon(0, 0, 2560, 1440, 56, true, 144),
        ];
        assert_eq!(topology_signature(&dual), "2:2560x1440@150");
        assert_eq!(topology_signature(&[]), "0:0x0@100");
    }

    #[test]
    fn taskbar_anchor_captures_and_restores_across_external_and_laptop() {
        let external = [mon(0, 0, 2560, 1440, 60, true, 144)];
        let laptop = [mon(0, 0, 1920, 1080, 48, true, 96)];

        // Pill pinned on external 2560x1440 taskbar (work_bottom=1380..1440), 180px from right.
        let pill_ext = rect(2560 - 156 - 180, 1380 + 8, 156, 48);
        let (sig, anchor) = capture_taskbar_anchor(pill_ext, &external).unwrap();
        assert_eq!(sig, "1:2560x1440@150");
        assert!(anchor.on_taskbar_strip);
        assert_eq!(anchor.offset_from_right_px, 180);
        assert_eq!(anchor.offset_from_taskbar_top_px, 8);

        // Same topology roundtrips cleanly.
        assert_eq!(
            resolve_taskbar_anchor(&anchor, (156, 48), &external),
            Some(pill_ext)
        );

        // Switching to 1920x1080 laptop (work_bottom=1032..1080) keeps pill inside the taskbar band!
        let restored_laptop = resolve_taskbar_anchor(&anchor, (104, 32), &laptop).unwrap();
        assert_eq!(restored_laptop.x, 1920 - 104 - 180);
        assert_eq!(restored_laptop.y, 1032 + 8);
        assert!(restored_laptop.y >= 1032);
        assert!(restored_laptop.y + 32 <= 1080);
    }

    #[test]
    fn fullscreen_and_visibility_rules_hold() {
        for s in [2, 3, 4] {
            assert!(pure_should_hide(s));
        }
        for s in [0, 1, 5, 6, 7] {
            assert!(!pure_should_hide(s));
        }
        assert!(!busy_from(0x8000_4005_u32 as i32, 3));
        assert!(busy_from(0, 3));

        let mon = (0, 0, 1920, 1080);
        assert!(covers_screen(mon, mon, "Chrome_WidgetWin_1"));
        assert!(!covers_screen(mon, mon, "Shell_TrayWnd"));
        assert!(!covers_screen((-8, -8, 1928, 1088), mon, "Notepad"));

        assert_eq!(next_visibility(true, false, true, true), Action::Keep);
        assert_eq!(next_visibility(true, false, false, true), Action::Hide);
        assert_eq!(next_visibility(false, true, false, false), Action::Show);
    }

    #[test]
    fn default_pill_and_flyout_anchor_cleanly() {
        let work = MonitorRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1040,
        };
        let pill = default_pill_rect(work, pill_px_size(3, 1.0, 1.0), 1.0);
        assert_eq!(pill, rect(1920 - 108 - 24, 1040 - 32 - 24, 108, 32));

        let (pop, up) = flyout_rect(pill, 54, 412, 240, work, 1.0);
        assert!(up);
        assert!(pop.y + pop.height as i32 <= pill.y);
        assert!(pop.x >= work.x && pop.x + pop.width as i32 <= work.x + work.width as i32);
    }

    #[test]
    fn resized_pill_rect_keeps_vertical_center_and_clamps_to_monitor() {
        let bounds = MonitorRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        // Center at y = 500 -> growing to h = 64 centers at 468..532.
        let r = resized_pill_rect(1200, 500, 216, 64, bounds);
        assert_eq!(r, rect(1200, 468, 216, 64));

        // Near bottom edge (center = 1070): clamps cleanly inside monitor bottom.
        let near_bottom = resized_pill_rect(1800, 1070, 216, 64, bounds);
        assert_eq!(near_bottom.x, 1920 - 216);
        assert_eq!(near_bottom.y, 1080 - 64);
    }

    #[test]
    fn system_queries_do_not_panic() {
        let _ = system_uses_dark_theme();
        let _ = transparency_enabled();
        let _ = animations_enabled();
        let _ = fullscreen_busy();
        let mons = enumerate_monitors();
        let sig = topology_signature(&mons);
        assert!(!sig.is_empty());
    }

    fn mon_strategy() -> impl Strategy<Value = DisplayMonitor> {
        (
            -3840i32..=3840,
            -2160i32..=2160,
            800u32..=3840,
            600u32..=2160,
            24u32..=96,
            prop_oneof![Just(96u32), Just(120u32), Just(144u32), Just(192u32)],
        )
            .prop_map(|(x, y, w, h, tb, dpi)| mon(x, y, w, h, tb, true, dpi))
    }

    proptest! {
        #[test]
        fn topology_signature_is_deterministic_and_encodes_primary(
            primary in mon_strategy(),
            extra in proptest::collection::vec(mon_strategy(), 0..3),
        ) {
            let mut monitors = vec![primary];
            for mut m in extra {
                m.primary = false;
                monitors.push(m);
            }
            let sig1 = topology_signature(&monitors);
            let sig2 = topology_signature(&monitors);
            prop_assert_eq!(&sig1, &sig2);
            let expected = format!(
                "{}:{}x{}@{}",
                monitors.len(),
                primary.full.width,
                primary.full.height,
                primary.dpi_percent()
            );
            prop_assert_eq!(sig1, expected);
        }

        #[test]
        fn taskbar_anchor_roundtrips_inside_strip(
            primary in mon_strategy(),
            pill_w in 40u32..=200,
            pill_h in 20u32..=40,
            x_frac in 0.0f64..=1.0,
            y_frac in 0.0f64..=1.0,
        ) {
            let strip_h = primary.full.height - primary.work.height;
            prop_assume!(strip_h >= pill_h);
            let work_bottom = primary.work.y + primary.work.height as i32;
            let max_x_off = (primary.full.width - pill_w) as f64;
            let max_y_off = (strip_h - pill_h) as f64;
            let pill = rect(
                primary.full.x + (max_x_off * x_frac).round() as i32,
                work_bottom + (max_y_off * y_frac).round() as i32,
                pill_w,
                pill_h,
            );
            let monitors = [primary];
            let (_, anchor) = capture_taskbar_anchor(pill, &monitors).expect("captured");
            prop_assert!(anchor.on_taskbar_strip);
            let restored = resolve_taskbar_anchor(&anchor, (pill_w, pill_h), &monitors).expect("resolved");
            prop_assert_eq!(restored, pill);
        }

        #[test]
        fn taskbar_pinned_pill_never_floats_above_taskbar_when_switching_monitors(
            src in mon_strategy(),
            dst in mon_strategy(),
            src_w in 60u32..=180,
            src_h in 24u32..=40,
            dst_w in 60u32..=180,
            dst_h in 24u32..=40,
            x_frac in 0.0f64..=1.0,
            y_frac in 0.0f64..=1.0,
        ) {
            let src_strip = src.full.height - src.work.height;
            let dst_strip = dst.full.height - dst.work.height;
            prop_assume!(src_strip >= src_h && dst_strip >= dst_h);

            let src_work_bottom = src.work.y + src.work.height as i32;
            let pill = rect(
                src.full.x + ((src.full.width - src_w) as f64 * x_frac).round() as i32,
                src_work_bottom + ((src_strip - src_h) as f64 * y_frac).round() as i32,
                src_w,
                src_h,
            );
            let (_, anchor) = capture_taskbar_anchor(pill, &[src]).unwrap();
            prop_assert!(anchor.on_taskbar_strip);

            let resolved = resolve_taskbar_anchor(&anchor, (dst_w, dst_h), &[dst]).unwrap();
            let dst_work_bottom = dst.work.y + dst.work.height as i32;
            let dst_full_bottom = dst.full.y + dst.full.height as i32;
            prop_assert!(resolved.x >= dst.full.x);
            prop_assert!(resolved.x + resolved.width as i32 <= dst.full.x + dst.full.width as i32);
            prop_assert!(resolved.y >= dst_work_bottom);
            prop_assert!(resolved.y + resolved.height as i32 <= dst_full_bottom);
        }

        #[test]
        fn clamp_to_monitors_keeps_rect_reachable_on_a_monitor(
            m1 in mon_strategy(),
            m2 in mon_strategy(),
            rx in -8000i32..=8000,
            ry in -8000i32..=8000,
            rw in 30u32..=4500,
            rh in 20u32..=2500,
        ) {
            let monitors = [m1.full, m2.full];
            let clamped = clamp_to_monitors(rect(rx, ry, rw, rh), &monitors);
            let cx = i64::from(clamped.x) + i64::from(clamped.width) / 2;
            let cy = i64::from(clamped.y) + i64::from(clamped.height) / 2;
            let fits_containing_monitor = monitors.iter().any(|m| {
                contains_point(m, cx, cy) && clamped.width <= m.width && clamped.height <= m.height
            });
            prop_assert!(fits_containing_monitor);
        }

        #[test]
        fn flyout_stays_in_work_area_and_disjoint_from_pill_when_room_exists(
            work_x in -1920i32..=1920,
            work_y in -1080i32..=1080,
            work_w in 600u32..=2560,
            work_h in 500u32..=1440,
            pill_dx in 0u32..=2400,
            pill_dy in 0u32..=1400,
            pill_w in 40u32..=180,
            pill_h in 24u32..=52,
            pop_w in 280u32..=380,
            pop_h in 120u32..=420,
            anchor_x in 0i32..=180,
        ) {
            let work = MonitorRect { x: work_x, y: work_y, width: work_w, height: work_h };
            let pill = rect(
                work_x + (pill_dx % work_w.saturating_sub(pill_w).max(1)) as i32,
                work_y + (pill_dy % (work_h + 60)) as i32,
                pill_w,
                pill_h,
            );
            let (pop, _) = flyout_rect(pill, anchor_x, pop_w, pop_h, work, 1.0);
            prop_assert!(pop.x >= work.x);
            prop_assert!(pop.y >= work.y);
            prop_assert!(pop.x + pop.width as i32 <= work.x + work.width as i32);
            prop_assert!(pop.y + pop.height as i32 <= work.y + work.height as i32);

            let gap = GAP_CSS as i32;
            let room_above = pill.y - work.y >= pop.height as i32 + gap;
            let room_below = (work.y + work.height as i32) - (pill.y + pill.height as i32) >= pop.height as i32 + gap;
            if room_above || room_below {
                let disjoint_y = pop.y + pop.height as i32 <= pill.y || pill.y + pill.height as i32 <= pop.y;
                prop_assert!(disjoint_y);
            }
        }
    }
}
