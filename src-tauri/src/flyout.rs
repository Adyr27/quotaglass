//! The flyout window above the pill: a read-only hover overlay and a click popover.
//! Geometry and the hover protocol are pure and tested; the runtime applies their effects on the main thread.
//! Like the pill, the flyout is shown, hidden and focused only with raw calls (see `topmost::hide_window`).

use crate::scheduler::lock;
use crate::settings::WindowRect;
use crate::topmost;
use crate::window::{self, fit_in, MonitorRect};
use quotaglass_core::Service;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, WebviewWindow, WindowEvent};

pub const GAP_CSS: u32 = 6;
pub const WIDTH_CSS: u32 = 340;
pub const HEIGHT_MIN: f64 = 96.0;
pub const HEIGHT_MAX: f64 = 520.0;
pub const HEIGHT_START: f64 = 240.0;
pub const SHOW_DELAY_MS: u64 = 150;
pub const HIDE_DELAY_MS: u64 = 300;
pub const BLUR_GRACE_MS: u64 = 100;

/// A popover page other than the account list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum View {
    Settings,
    First,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Hidden,
    Hover(Service),
    Popover(Option<Service>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timer {
    Show,
    Hide,
}

/// What the runtime does after an input. No effect ever moves or resizes the pill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    None,
    Schedule {
        timer: Timer,
        after_ms: u64,
        generation: u64,
    },
    Hover(Service),
    Popover(Option<Service>, Option<View>),
    Hide,
}

/// The hover protocol. Every input bumps `generation` where it must cancel a pending timer;
/// a timer whose generation is old does nothing.
#[derive(Debug)]
pub struct Machine {
    pub mode: Mode,
    pill_inside: bool,
    flyout_inside: bool,
    hover_service: Option<Service>,
    generation: u64,
    popover_at_ms: u64,
}

impl Default for Machine {
    fn default() -> Self {
        Self {
            mode: Mode::Hidden,
            pill_inside: false,
            flyout_inside: false,
            hover_service: None,
            generation: 0,
            popover_at_ms: 0,
        }
    }
}

impl Machine {
    /// The pointer (or keyboard focus) entered or left a pill slot.
    pub fn hover(&mut self, service: Service, inside: bool) -> Effect {
        if !inside {
            // A leave can arrive after the next slot's enter; only the current slot's leave counts.
            if self.hover_service != Some(service) {
                return Effect::None;
            }
            self.pill_inside = false;
            return self.leave();
        }
        self.pill_inside = true;
        self.hover_service = Some(service);
        self.generation += 1;
        match self.mode {
            Mode::Hidden => self.schedule(Timer::Show, SHOW_DELAY_MS),
            Mode::Hover(open) if open != service => {
                self.mode = Mode::Hover(service);
                Effect::Hover(service)
            }
            _ => Effect::None,
        }
    }

    /// The pointer entered or left the flyout.
    pub fn pointer(&mut self, inside: bool) -> Effect {
        self.flyout_inside = inside;
        if inside {
            self.generation += 1;
            Effect::None
        } else {
            self.leave()
        }
    }

    /// Cancels pending timers; an open overlay hides after the delay unless the pointer comes back.
    fn leave(&mut self) -> Effect {
        self.generation += 1;
        match self.mode {
            Mode::Hover(_) => self.schedule(Timer::Hide, HIDE_DELAY_MS),
            _ => Effect::None,
        }
    }

    fn schedule(&self, timer: Timer, after_ms: u64) -> Effect {
        Effect::Schedule {
            timer,
            after_ms,
            generation: self.generation,
        }
    }

    pub fn timer(&mut self, timer: Timer, generation: u64) -> Effect {
        if generation != self.generation {
            return Effect::None;
        }
        match (timer, self.mode, self.hover_service) {
            (Timer::Show, Mode::Hidden, Some(service)) if self.pill_inside => {
                self.mode = Mode::Hover(service);
                Effect::Hover(service)
            }
            (Timer::Hide, Mode::Hover(_), _) if !self.pill_inside && !self.flyout_inside => {
                self.close()
            }
            _ => Effect::None,
        }
    }

    pub fn open_popover(
        &mut self,
        service: Option<Service>,
        view: Option<View>,
        now_ms: u64,
    ) -> Effect {
        self.mode = Mode::Popover(service);
        self.generation += 1;
        self.popover_at_ms = now_ms;
        Effect::Popover(service, view)
    }

    /// Esc, the close button, or a fullscreen app.
    pub fn close(&mut self) -> Effect {
        if self.mode == Mode::Hidden {
            return Effect::None;
        }
        self.mode = Mode::Hidden;
        self.generation += 1;
        // A hidden window gets no leave event.
        self.flyout_inside = false;
        Effect::Hide
    }

    /// The flyout lost focus. Only the popover closes, and not in the first moments after it opened.
    pub fn blur(&mut self, now_ms: u64) -> Effect {
        match self.mode {
            Mode::Popover(_) if now_ms.saturating_sub(self.popover_at_ms) >= BLUR_GRACE_MS => {
                self.close()
            }
            _ => Effect::None,
        }
    }
}

/// Where the flyout goes, in device px: above the pill when it fits in the work area, else below; centred on the
/// slot (`anchor_x_px` from the pill's left edge); always inside the work area. True when it opens upward.
pub fn flyout_rect(
    pill: WindowRect,
    anchor_x_px: i32,
    w: u32,
    h: u32,
    work: MonitorRect,
    scale: f64,
) -> (WindowRect, bool) {
    let gap = (f64::from(GAP_CSS) * scale).round() as i64;
    let (w, h) = (w.min(work.width), h.min(work.height));
    let above = i64::from(pill.y) - gap - i64::from(h);
    let up = above >= i64::from(work.y);
    let y = if up {
        above
    } else {
        i64::from(pill.y) + i64::from(pill.height) + gap
    };
    let x = i64::from(pill.x) + i64::from(anchor_x_px) - i64::from(w) / 2;
    let r = WindowRect {
        x: x as i32,
        y: y as i32,
        width: w,
        height: h,
    };
    (fit_in(r, work), up)
}

// ---- Runtime: Tauri glue. Window calls run on the main thread. ----

struct Inner {
    machine: Machine,
    /// The hovered slot's centre, in pill CSS px from the pill's left edge.
    anchor_css: f64,
    /// The content height the flyout UI reported, in CSS px.
    height_css: f64,
}

pub struct FlyoutState {
    inner: Mutex<Inner>,
    started: Instant,
}

#[derive(Serialize, Clone)]
struct ModeEvent {
    mode: &'static str,
    service: Option<Service>,
    view: Option<View>,
}

impl Default for FlyoutState {
    fn default() -> Self {
        Self {
            inner: Mutex::new(Inner {
                machine: Machine::default(),
                anchor_css: 0.0,
                height_css: HEIGHT_START,
            }),
            started: Instant::now(),
        }
    }
}

/// `FlyoutState` is managed by the builder in `main`, before any web view can call a command.
pub fn setup(app: &AppHandle) {
    let Some(flyout) = app.get_webview_window("flyout") else {
        tracing::warn!("the flyout window is missing");
        return;
    };
    let handle = app.clone();
    flyout.on_window_event(move |event| {
        if let WindowEvent::Focused(false) = event {
            let now = now_ms(&handle);
            step(&handle, move |i| i.machine.blur(now));
        }
    });
}

fn now_ms(app: &AppHandle) -> u64 {
    app.state::<FlyoutState>().started.elapsed().as_millis() as u64
}

/// Runs one input on the main thread (inline when already there), then applies its effect with the lock released.
fn step(app: &AppHandle, input: impl FnOnce(&mut Inner) -> Effect + Send + 'static) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let effect = input(&mut lock(&handle.state::<FlyoutState>().inner));
        apply(&handle, effect);
    });
}

/// `anchor_x`: the slot centre in pill CSS px from the pill's left edge. Untrusted: a bad value becomes 0.
pub fn hover(app: &AppHandle, service: Service, inside: bool, anchor_x: f64) {
    let anchor = if anchor_x.is_finite() {
        anchor_x.clamp(0.0, 4096.0)
    } else {
        0.0
    };
    step(app, move |i| {
        if inside {
            i.anchor_css = anchor;
        }
        i.machine.hover(service, inside)
    });
}

pub fn pointer(app: &AppHandle, inside: bool) {
    step(app, move |i| i.machine.pointer(inside));
}

pub fn open_popover(app: &AppHandle, service: Option<Service>, view: Option<View>) {
    let now = now_ms(app);
    step(app, move |i| i.machine.open_popover(service, view, now));
}

pub fn close(app: &AppHandle) {
    step(app, |i| i.machine.close());
}

/// The UI's content height in CSS px, clamped to 96-520. An open flyout is placed again at once.
pub fn set_height(app: &AppHandle, height_css: f64) {
    if !height_css.is_finite() {
        return;
    }
    let height = height_css.clamp(HEIGHT_MIN, HEIGHT_MAX);
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let state = handle.state::<FlyoutState>();
        let open = {
            let mut i = lock(&state.inner);
            i.height_css = height;
            i.machine.mode != Mode::Hidden
        };
        if let Some(flyout) = open.then(|| handle.get_webview_window("flyout")).flatten() {
            place(&handle, &flyout);
        }
    });
}

fn px(css: f64, zoom: f64) -> u32 {
    (css * zoom).round().max(1.0) as u32
}

fn place(app: &AppHandle, flyout: &WebviewWindow) {
    let Some((pill, work, zoom)) = window::pill_frame(app) else {
        return;
    };
    let state = app.state::<FlyoutState>();
    let (anchor, height) = {
        let i = lock(&state.inner);
        (i.anchor_css, i.height_css)
    };
    let (rect, _) = flyout_rect(
        pill,
        px(anchor, zoom) as i32,
        px(f64::from(WIDTH_CSS), zoom),
        px(height, zoom),
        work,
        zoom,
    );
    let _ = window::place(flyout, rect);
}

fn emit_mode(app: &AppHandle, mode: &'static str, service: Option<Service>, view: Option<View>) {
    let event = ModeEvent {
        mode,
        service,
        view,
    };
    let _ = app.emit_to("flyout", "flyout-mode", event);
}

fn apply(app: &AppHandle, effect: Effect) {
    let Some(flyout) = app.get_webview_window("flyout") else {
        return;
    };
    match effect {
        Effect::None => {}
        Effect::Schedule {
            timer,
            after_ms,
            generation,
        } => {
            let handle = app.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_millis(after_ms)).await;
                step(&handle, move |i| i.machine.timer(timer, generation));
            });
        }
        Effect::Hover(service) => {
            emit_mode(app, "hover", Some(service), None);
            place(app, &flyout);
            topmost::show_no_activate(&flyout);
            window::keep_glass_on(&flyout);
            topmost::assert_topmost(&flyout);
        }
        Effect::Popover(service, view) => {
            emit_mode(app, "popover", service, view);
            place(app, &flyout);
            topmost::show_no_activate(&flyout);
            window::keep_glass_on(&flyout);
            topmost::assert_topmost(&flyout);
            topmost::focus_window(&flyout);
        }
        Effect::Hide => {
            topmost::hide_window(&flyout);
            emit_mode(app, "hidden", None, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Service::{Claude, Codex};

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
    /// A docked pill near the bottom-right of the work area.
    const PILL: WindowRect = WindowRect {
        x: 1700,
        y: 984,
        width: 104,
        height: 32,
    };

    fn scheduled(e: Effect, timer: Timer) -> (u64, u64) {
        match e {
            Effect::Schedule {
                timer: t,
                after_ms,
                generation,
            } if t == timer => (after_ms, generation),
            other => panic!("expected {timer:?} timer, got {other:?}"),
        }
    }

    /// Hover shown for `service`; returns the machine.
    fn hovering(service: Service) -> Machine {
        let mut m = Machine::default();
        let (_, g) = scheduled(m.hover(service, true), Timer::Show);
        assert_eq!(m.timer(Timer::Show, g), Effect::Hover(service));
        m
    }

    #[test]
    fn flyout_opens_up_above_a_bottom_taskbar() {
        // Moved left so the clamp (its own test) does not touch the result.
        let pill = WindowRect { x: 1600, ..PILL };
        let (r, up) = flyout_rect(pill, 52, 340, 240, WORK, 1.0);
        assert!(up);
        assert_eq!(r, rect(1652 - 170, 984 - 6 - 240, 340, 240));
    }

    #[test]
    fn flyout_opens_down_when_no_room_above() {
        let top = rect(800, 10, 104, 32);
        let (r, up) = flyout_rect(top, 52, 340, 240, WORK, 1.0);
        assert!(!up);
        assert_eq!(r.y, 10 + 32 + 6);
        // The gap scales with the zoom.
        let (r, _) = flyout_rect(top, 52, 340, 240, WORK, 1.5);
        assert_eq!(r.y, 10 + 32 + 9);
    }

    #[test]
    fn flyout_is_centred_on_the_slot() {
        for anchor in [20, 52, 84] {
            let (r, _) = flyout_rect(rect(800, 984, 104, 32), anchor, 340, 240, WORK, 1.0);
            assert_eq!(r.x + 170, 800 + anchor);
        }
    }

    #[test]
    fn flyout_clamps_left_and_right() {
        let (r, _) = flyout_rect(rect(10, 984, 104, 32), 20, 340, 240, WORK, 1.0);
        assert_eq!(r.x, 0);
        let (r, _) = flyout_rect(rect(1816, 984, 104, 32), 84, 340, 240, WORK, 1.0);
        assert_eq!(r.x, 1920 - 340);
        // A work area that does not start at 0 (second monitor on the left).
        let left = MonitorRect {
            x: -1280,
            y: 0,
            width: 1280,
            height: 984,
        };
        let (r, _) = flyout_rect(rect(-1270, 900, 104, 32), 20, 340, 240, left, 1.0);
        assert_eq!(r.x, -1280);
    }

    #[test]
    fn flyout_taller_than_work_is_shrunk() {
        let small = MonitorRect {
            x: 0,
            y: 0,
            width: 300,
            height: 200,
        };
        let (r, _) = flyout_rect(rect(100, 160, 104, 32), 52, 340, 520, small, 1.0);
        assert_eq!((r.width, r.height), (300, 200));
        assert_eq!((r.x, r.y), (0, 0));
    }

    #[test]
    fn flyout_geometry_never_changes_the_pill_rect() {
        let pill = PILL;
        let (r, up) = flyout_rect(pill, 52, 340, 240, WORK, 1.0);
        assert_eq!(pill, PILL);
        // Opening up, the flyout ends a gap above the pill: they never overlap.
        assert!(up && r.y + r.height as i32 + 6 == pill.y);
    }

    #[test]
    fn state_hover_delay_150() {
        let mut m = Machine::default();
        let (after, g) = scheduled(m.hover(Claude, true), Timer::Show);
        assert_eq!(after, 150);
        assert_eq!(m.mode, Mode::Hidden, "nothing shows before the delay");
        assert_eq!(m.timer(Timer::Show, g), Effect::Hover(Claude));
        assert_eq!(m.mode, Mode::Hover(Claude));

        // Leaving before the delay cancels the show.
        let mut m = Machine::default();
        let (_, g) = scheduled(m.hover(Claude, true), Timer::Show);
        m.hover(Claude, false);
        assert_eq!(m.timer(Timer::Show, g), Effect::None);
        assert_eq!(m.mode, Mode::Hidden);
    }

    #[test]
    fn state_hide_delay_300() {
        let mut m = hovering(Claude);
        let (after, g) = scheduled(m.hover(Claude, false), Timer::Hide);
        assert_eq!(after, 300);
        assert_eq!(m.mode, Mode::Hover(Claude), "still open during the delay");
        assert_eq!(m.timer(Timer::Hide, g), Effect::Hide);
        assert_eq!(m.mode, Mode::Hidden);

        // Back on the slot before the delay: stays open.
        let mut m = hovering(Claude);
        let (_, g) = scheduled(m.hover(Claude, false), Timer::Hide);
        assert_eq!(m.hover(Claude, true), Effect::None);
        assert_eq!(m.timer(Timer::Hide, g), Effect::None);
        assert_eq!(m.mode, Mode::Hover(Claude));
    }

    #[test]
    fn moving_into_flyout_cancels_hide() {
        let mut m = hovering(Claude);
        let (_, g) = scheduled(m.hover(Claude, false), Timer::Hide);
        assert_eq!(m.pointer(true), Effect::None);
        assert_eq!(m.timer(Timer::Hide, g), Effect::None);
        assert_eq!(m.mode, Mode::Hover(Claude));

        // Leaving the flyout (not back to the pill) hides it after 300 ms.
        let (after, g) = scheduled(m.pointer(false), Timer::Hide);
        assert_eq!(after, 300);
        assert_eq!(m.timer(Timer::Hide, g), Effect::Hide);
    }

    #[test]
    fn switch_service_while_open() {
        let mut m = hovering(Claude);
        let (_, g) = scheduled(m.hover(Claude, false), Timer::Hide);
        assert_eq!(
            m.hover(Codex, true),
            Effect::Hover(Codex),
            "switches at once"
        );
        assert_eq!(m.mode, Mode::Hover(Codex));
        assert_eq!(m.timer(Timer::Hide, g), Effect::None);

        // A late leave from the old slot (arriving after the new enter) is ignored.
        let mut m = hovering(Claude);
        m.hover(Codex, true);
        assert_eq!(m.hover(Claude, false), Effect::None);
        assert_eq!(m.mode, Mode::Hover(Codex));
    }

    #[test]
    fn popover_ignores_pointer_leave() {
        let mut m = hovering(Claude);
        assert_eq!(
            m.open_popover(Some(Claude), None, 1_000),
            Effect::Popover(Some(Claude), None)
        );
        assert_eq!(m.hover(Claude, false), Effect::None);
        m.pointer(true);
        assert_eq!(m.pointer(false), Effect::None);
        assert_eq!(
            m.hover(Codex, true),
            Effect::None,
            "hover never replaces it"
        );
        assert_eq!(m.mode, Mode::Popover(Some(Claude)));
    }

    #[test]
    fn esc_hides() {
        let mut m = hovering(Claude);
        assert_eq!(m.close(), Effect::Hide);
        assert_eq!(m.mode, Mode::Hidden);
        assert_eq!(m.close(), Effect::None, "already hidden");

        let mut m = Machine::default();
        m.open_popover(None, Some(View::Settings), 0);
        assert_eq!(m.close(), Effect::Hide);
        assert_eq!(m.mode, Mode::Hidden);
    }

    #[test]
    fn blur_in_popover_hides_after_grace() {
        let mut m = Machine::default();
        m.open_popover(Some(Codex), None, 10_000);
        assert_eq!(
            m.blur(10_050),
            Effect::None,
            "opening must not close itself"
        );
        assert_eq!(m.mode, Mode::Popover(Some(Codex)));
        assert_eq!(m.blur(10_100), Effect::Hide);
        assert_eq!(m.mode, Mode::Hidden);
    }

    #[test]
    fn blur_in_hover_does_nothing() {
        let mut m = hovering(Claude);
        assert_eq!(m.blur(99_999), Effect::None);
        assert_eq!(m.mode, Mode::Hover(Claude));
        let mut m = Machine::default();
        assert_eq!(m.blur(99_999), Effect::None);
    }

    /// Acceptance check 1: open-close cycles never move a pinned pill. No `Effect` addresses the pill, and the
    /// geometry takes the pill rect by value; this walks ten hover and ten click cycles to prove both.
    #[test]
    fn home_survives_ten_flyout_cycles() {
        let home = PILL;
        let mut m = Machine::default();
        for i in 0..10u64 {
            let (_, g) = scheduled(m.hover(Claude, true), Timer::Show);
            assert_eq!(m.timer(Timer::Show, g), Effect::Hover(Claude));
            let (r, up) = flyout_rect(home, 52, 340, 240, WORK, 1.0);
            assert!(up && r.y + (r.height as i32) < home.y);
            let (_, g) = scheduled(m.hover(Claude, false), Timer::Hide);
            assert_eq!(m.timer(Timer::Hide, g), Effect::Hide);

            assert_eq!(
                m.open_popover(Some(Codex), None, i * 1_000),
                Effect::Popover(Some(Codex), None)
            );
            assert_eq!(m.close(), Effect::Hide);
        }
        assert_eq!(home, PILL);
        assert_eq!(m.mode, Mode::Hidden);
    }
}
