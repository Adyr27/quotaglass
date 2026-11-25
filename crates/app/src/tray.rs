//! System tray icon (`tray-icon` + `muda`) with pure-Rust synthesized ring icon
//! and Iced `Subscription` event bridge.

use iced::futures::Stream;
use iced::stream;
use muda::{CheckMenuItem, Menu, MenuEvent, MenuItem};
use std::sync::{Mutex, Once};
use tokio::sync::mpsc;
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

const TRAY_GUID: u128 = 0x7175_6f74_6167_6c61_7373_5f74_7261_7931;

static INIT_HANDLERS: Once = Once::new();
static TRAY_TX: Mutex<Option<mpsc::UnboundedSender<TrayEvent>>> = Mutex::new(None);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEvent {
    TogglePill,
    Show,
    Focus,
    TogglePin,
    RefreshNow,
    Settings,
    Quit,
}

pub fn pin_label(locked: bool) -> &'static str {
    if locked {
        "Unpin pill"
    } else {
        "Pin pill"
    }
}

pub fn map_menu_id(id: &str) -> Option<TrayEvent> {
    match id {
        "show" => Some(TrayEvent::Show),
        "focus" => Some(TrayEvent::Focus),
        "pin" => Some(TrayEvent::TogglePin),
        "refresh" => Some(TrayEvent::RefreshNow),
        "settings" => Some(TrayEvent::Settings),
        "quit" => Some(TrayEvent::Quit),
        _ => None,
    }
}

pub fn map_tray_event(event: &TrayIconEvent) -> Option<TrayEvent> {
    match event {
        TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } => Some(TrayEvent::TogglePill),
        _ => None,
    }
}

fn emit_tray_event(ev: TrayEvent) {
    let tx = TRAY_TX
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().cloned());
    if let Some(tx) = tx {
        let _ = tx.send(ev);
    }
}

fn ensure_global_handlers() {
    INIT_HANDLERS.call_once(|| {
        MenuEvent::set_event_handler(Some(|event: MenuEvent| {
            if let Some(ev) = map_menu_id(event.id().as_ref()) {
                emit_tray_event(ev);
            }
        }));
        TrayIconEvent::set_event_handler(Some(|event: TrayIconEvent| {
            if let Some(ev) = map_tray_event(&event) {
                emit_tray_event(ev);
            }
        }));
    });
}

/// Registers and yields tray/menu events on an unbounded channel.
pub fn event_channel() -> mpsc::UnboundedReceiver<TrayEvent> {
    ensure_global_handlers();
    let (tx, rx) = mpsc::unbounded_channel();
    if let Ok(mut guard) = TRAY_TX.lock() {
        *guard = Some(tx);
    }
    rx
}

/// Ready-to-wire `iced::Subscription<TrayEvent>` for `iced::daemon`.
pub fn subscription() -> iced::Subscription<TrayEvent> {
    iced::Subscription::run(tray_stream)
}

fn tray_stream() -> impl Stream<Item = TrayEvent> {
    stream::channel(64, async |mut output| {
        use iced::futures::SinkExt;

        let mut rx = event_channel();
        while let Some(ev) = rx.recv().await {
            if output.send(ev).await.is_err() {
                break;
            }
        }
    })
}

/// Synthesizes a 4x4 supersampled RGBA icon matching QuotaGlass's ring mark.
pub fn build_ring_icon_rgba(size: u32) -> Vec<u8> {
    let s = size.max(1) as usize;
    let mut rgba = vec![0u8; s * s * 4];
    let scale = s as f32;

    // Coordinates normalized to the 1024x1024 SVG viewBox.
    let half = 512.0_f32;
    let box_radius = 224.0_f32;
    let ring_r = 300.0_f32;
    let half_stroke = 44.0_f32; // stroke-width 88 / 2

    let blend = |dst: [f32; 4], src: [f32; 4]| -> [f32; 4] {
        let sa = src[3];
        let da = dst[3] * (1.0 - sa);
        let out_a = sa + da;
        if out_a <= 1e-5 {
            [0.0; 4]
        } else {
            [
                (src[0] * sa + dst[0] * da) / out_a,
                (src[1] * sa + dst[1] * da) / out_a,
                (src[2] * sa + dst[2] * da) / out_a,
                out_a,
            ]
        }
    };

    for y in 0..s {
        for x in 0..s {
            let mut acc = [0.0_f32; 4];
            for sy in 0..4 {
                for sx in 0..4 {
                    let px = ((x as f32) + (sx as f32 + 0.5) * 0.25) / scale * 1024.0;
                    let py = ((y as f32) + (sy as f32 + 0.5) * 0.25) / scale * 1024.0;

                    // Rounded square background (#0c0d0f).
                    let dx = (px - half).abs() - (half - box_radius);
                    let dy = (py - half).abs() - (half - box_radius);
                    let sdf_box = dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0) - box_radius;
                    let mut sample = [0.0_f32; 4];
                    if sdf_box <= 0.0 {
                        sample = [12.0 / 255.0, 13.0 / 255.0, 15.0 / 255.0, 1.0];
                    }

                    // Full ring track (#2a2b2e).
                    let vx = px - half;
                    let vy = py - half;
                    let dist = vx.hypot(vy);
                    if (dist - ring_r).abs() <= half_stroke {
                        sample = blend(sample, [42.0 / 255.0, 43.0 / 255.0, 46.0 / 255.0, 1.0]);
                    }

                    // 270-degree white arc: 12 o'clock clockwise to 9 o'clock (excluding top-left quadrant).
                    let in_top_left = vx < 0.0 && vy < 0.0;
                    let on_arc_band = !in_top_left && (dist - ring_r).abs() <= half_stroke;
                    let cap_top = vx.hypot(vy + ring_r) <= half_stroke;
                    let cap_left = (vx + ring_r).hypot(vy) <= half_stroke;
                    if on_arc_band || cap_top || cap_left {
                        sample = blend(sample, [1.0, 1.0, 1.0, 1.0]);
                    }

                    acc[0] += sample[0];
                    acc[1] += sample[1];
                    acc[2] += sample[2];
                    acc[3] += sample[3];
                }
            }

            let idx = (y * s + x) * 4;
            rgba[idx] = ((acc[0] / 16.0) * 255.0).round().clamp(0.0, 255.0) as u8;
            rgba[idx + 1] = ((acc[1] / 16.0) * 255.0).round().clamp(0.0, 255.0) as u8;
            rgba[idx + 2] = ((acc[2] / 16.0) * 255.0).round().clamp(0.0, 255.0) as u8;
            rgba[idx + 3] = ((acc[3] / 16.0) * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    rgba
}

/// Owns the system tray icon and context menu handle on the UI thread.
pub struct TrayController {
    _tray_icon: TrayIcon,
    pin_item: CheckMenuItem,
}

impl TrayController {
    pub fn new(locked: bool) -> Result<Self, String> {
        ensure_global_handlers();

        let show = MenuItem::with_id("show", "Show", true, None);
        let focus = MenuItem::with_id("focus", "Focus QuotaGlass", true, None);
        let pin = CheckMenuItem::with_id("pin", pin_label(locked), true, locked, None);
        let refresh = MenuItem::with_id("refresh", "Refresh Now", true, None);
        let settings = MenuItem::with_id("settings", "Settings", true, None);
        let quit = MenuItem::with_id("quit", "Quit", true, None);

        let menu = Menu::with_items(&[&show, &focus, &pin, &refresh, &settings, &quit])
            .map_err(|_| "Could not build tray menu.")?;
        let icon = Icon::from_rgba(build_ring_icon_rgba(32), 32, 32)
            .map_err(|_| "Could not build tray icon.")?;

        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_icon(icon)
            .with_tooltip("QuotaGlass")
            .with_menu_on_left_click(false)
            .with_guid(TRAY_GUID)
            .build()
            .map_err(|_| "Could not register tray icon.")?;

        Ok(Self {
            _tray_icon: tray_icon,
            pin_item: pin,
        })
    }

    pub fn set_pin_checked(&self, checked: bool) {
        self.pin_item.set_checked(checked);
    }

    pub fn set_pin_label(&self, locked: bool) {
        self.pin_item.set_text(pin_label(locked));
        self.pin_item.set_checked(locked);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tray_icon::dpi::PhysicalPosition;
    use tray_icon::{Rect, TrayIconId};

    #[test]
    fn ring_icon_32x32_has_valid_rgba_and_mark_geometry() {
        let rgba = build_ring_icon_rgba(32);
        assert_eq!(rgba.len(), 32 * 32 * 4);
        // Top-left corner outside rounded rect is transparent.
        assert_eq!(rgba[3], 0);
        // Center background is dark #0c0d0f opaque.
        let mid = (16 * 32 + 16) * 4;
        assert_eq!(&rgba[mid..mid + 4], &[12, 13, 15, 255]);
        // Right side of ring (x=25, y=16 -> 3 o'clock) hits white arc.
        let right_arc = (16 * 32 + 25) * 4;
        assert!(rgba[right_arc] > 200 && rgba[right_arc + 1] > 200 && rgba[right_arc + 2] > 200);
        // Top-left quadrant (x=9, y=9) sits on the muted track, not bright white.
        let top_left = (9 * 32 + 9) * 4;
        assert!(rgba[top_left] < 80);
    }

    #[test]
    fn menu_ids_and_left_click_map_to_tray_events() {
        assert_eq!(map_menu_id("show"), Some(TrayEvent::Show));
        assert_eq!(map_menu_id("focus"), Some(TrayEvent::Focus));
        assert_eq!(map_menu_id("pin"), Some(TrayEvent::TogglePin));
        assert_eq!(map_menu_id("refresh"), Some(TrayEvent::RefreshNow));
        assert_eq!(map_menu_id("settings"), Some(TrayEvent::Settings));
        assert_eq!(map_menu_id("quit"), Some(TrayEvent::Quit));
        assert_eq!(map_menu_id("unknown"), None);

        assert_eq!(pin_label(false), "Pin pill");
        assert_eq!(pin_label(true), "Unpin pill");

        let click_up = TrayIconEvent::Click {
            id: TrayIconId::new("t"),
            position: PhysicalPosition::default(),
            rect: Rect::default(),
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
        };
        assert_eq!(map_tray_event(&click_up), Some(TrayEvent::TogglePill));

        let click_down = TrayIconEvent::Click {
            id: TrayIconId::new("t"),
            position: PhysicalPosition::default(),
            rect: Rect::default(),
            button: MouseButton::Left,
            button_state: MouseButtonState::Down,
        };
        assert_eq!(map_tray_event(&click_down), None);
    }
}
