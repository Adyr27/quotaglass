// Hides the console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use iced::futures::Stream;
use iced::keyboard::{self, key::Named, Key};
use iced::mouse;
use iced::window::settings::platform::CornerPreference;
use iced::window::settings::PlatformSpecific;
use iced::{stream, theme, time, window, Color, Element, Point, Size, Subscription, Task, Theme};
use quotaglass_app::autostart;
use quotaglass_app::connect::{self, CliAction};
use quotaglass_app::notify::{self, Resets, Warnings};
use quotaglass_app::scheduler::{Detected, Scheduler, SchedulerEvent};
use quotaglass_app::settings::{
    clamp_scale, AddedAccount, GlassMode, Settings, SettingsStore, WindowRect,
};
use quotaglass_app::state::{now_utc_ms, JsonStore, SnapshotCache};
use quotaglass_app::tray::{self, TrayController, TrayEvent};
use quotaglass_app::ui::tokens::{resolve_theme_dark, DesignTokens, UI_FONT};
use quotaglass_app::ui::{
    format_first_run_flag, pill, popover, popover_logical_height, select_tab_and_persist,
    tab_for_service, Page, UiMessage, UiState,
};
use quotaglass_app::update;
use quotaglass_app::win32::{
    self, Action as FullscreenAction, DisplayEvent, DisplayMonitor, HwndHandle, MonitorRect,
};
use quotaglass_core::view::Snapshot;
use quotaglass_core::{Env, Service};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch, Notify};
use tokio::task::JoinHandle;

const DRAG_THRESHOLD_PX: i32 = 8;
const OPEN_BLUR_GRACE: Duration = Duration::from_millis(150);
const PILL_CLICK_SUPPRESS_AFTER_BLUR: Duration = Duration::from_millis(300);

struct ChannelSeed<T>(u64, Arc<Mutex<Option<T>>>);

impl<T> Clone for ChannelSeed<T> {
    fn clone(&self) -> Self {
        Self(self.0, self.1.clone())
    }
}

impl<T> Hash for ChannelSeed<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

#[derive(Debug, Clone)]
struct PillPress {
    origin_px: Option<(i32, i32)>,
    dragged: bool,
}

#[derive(Debug, Clone)]
struct GripDrag {
    start_x_px: i32,
    start_left_x: i32,
    start_center_y: i64,
    start_width_px: u32,
    start_scale: f64,
    zoom: f64,
    slots: u32,
    monitor_bounds: MonitorRect,
}

#[derive(Debug, Clone)]
enum Message {
    Ui(UiMessage),
    PillCreated(Option<HwndHandle>),
    PopoverCreated(Option<HwndHandle>),
    WindowEvent(window::Id, window::Event),
    MouseMoveOnWindow(window::Id, Point),
    MouseReleaseOnWindow(window::Id),
    EscapePressed(window::Id),
    SystemThemeChanged(theme::Mode),
    Scheduler(SchedulerEvent),
    Tray(TrayEvent),
    Display(DisplayEvent),
    UpdateAvailable(Option<String>),
    ToastActivated(Option<Service>),
    Frame(Instant),
    TopmostTick,
    ResetTick,
    AddAccountDone {
        result: Result<AddedAccount, String>,
    },
    RemoveAccountDone {
        account_id: String,
        forget_card: bool,
        profile_id: String,
        result: Result<(), String>,
    },
    SignInDone {
        result: Result<(), String>,
    },
    #[allow(dead_code)]
    Noop,
}

struct App {
    settings_store: Option<SettingsStore>,
    settings: Settings,
    snapshot: Snapshot,
    ui: UiState,
    detected: Option<Detected>,
    update_version: Option<String>,
    now_ms: i64,

    // OS / display environment
    system_dark: bool,
    os_glass: bool,
    animations_enabled: bool,
    monitors: Vec<DisplayMonitor>,

    // Windows & Win32 HWNDs
    pill_id: window::Id,
    popover_id: window::Id,
    pill_hwnd: Option<HwndHandle>,
    popover_hwnd: Option<HwndHandle>,
    pill_visible: bool,
    popover_visible: bool,
    user_hidden: bool,
    auto_hidden: bool,
    popover_opened_at: Option<Instant>,
    suppress_pill_toggle_until: Option<Instant>,
    last_slot_count: u32,
    anchor_service: Option<Service>,
    open_onboarding_when_ready: bool,

    // Pointer / drag state
    pill_press: Option<PillPress>,
    grip_drag: Option<GripDrag>,
    drag_moved_dirty: bool,
    last_frame_at: Option<Instant>,

    // Runtime services
    scheduler: Option<Arc<Scheduler>>,
    warnings: Option<Arc<Warnings>>,
    resets: Option<Arc<Resets>>,
    toast_cb: Option<notify::ToastActivatedCb>,
    tray: Option<TrayController>,
    check_updates_flag: Arc<AtomicBool>,
    update_wake: Option<Arc<Notify>>,
    _reset_task: Option<JoinHandle<()>>,
    _update_task: Option<JoinHandle<()>>,

    // Subscription channel seeds (taken once on subscription start)
    scheduler_rx_seed: ChannelSeed<mpsc::UnboundedReceiver<SchedulerEvent>>,
    display_rx_seed: ChannelSeed<mpsc::UnboundedReceiver<DisplayEvent>>,
    update_rx_seed: ChannelSeed<watch::Receiver<Option<String>>>,
    toast_rx_seed: ChannelSeed<mpsc::UnboundedReceiver<Option<Service>>>,
}

impl App {
    fn boot() -> (Self, Task<Message>) {
        let settings_store = SettingsStore::default_path().map(SettingsStore::new);
        let mut settings = settings_store
            .as_ref()
            .map_or_else(Settings::default, SettingsStore::load);
        settings.clamp();

        let system_dark = win32::system_uses_dark_theme();
        let os_glass = win32::transparency_enabled();
        let animations_enabled = win32::animations_enabled();
        let monitors = win32::enumerate_monitors();

        let (display_tx, display_rx) = mpsc::unbounded_channel();
        let (toast_tx, toast_rx) = mpsc::unbounded_channel();
        let toast_cb: notify::ToastActivatedCb = Arc::new(move |svc| {
            let _ = toast_tx.send(svc);
        });

        let readings_store = JsonStore::in_appdata("readings.json").map(JsonStore::new);
        let snapshot_cache = SnapshotCache::in_appdata();
        let warnings = JsonStore::in_appdata("warnings.json")
            .map(|p| Arc::new(Warnings::new(JsonStore::new(p))));
        let resets = JsonStore::in_appdata("resets.json")
            .map(|p| Arc::new(Resets::new(JsonStore::new(p), settings.reset_toasts.clone())));

        let mut ui = UiState::default();
        let mut snapshot = Snapshot {
            accounts: Vec::new(),
            services: Vec::new(),
            generated_at_ms: now_utc_ms(),
        };
        let mut detected = None;

        let (scheduler, sched_rx) = match Env::from_system() {
            Ok(env) => {
                let (sched, rx) = Scheduler::new(env);
                if let Some(store) = readings_store {
                    sched.keep_readings(store);
                }
                if let Some(cache) = snapshot_cache {
                    if let Some(cached) = sched.keep_snapshot_cache(cache) {
                        snapshot = cached;
                        ui.has_snapshot = true;
                    }
                }
                detected = Some(sched.detected());
                sched.apply(&settings);
                (Some(Arc::new(sched)), Some(rx))
            }
            Err(e) => {
                tracing::warn!(error = ?e, "could not initialize system environment");
                (None, None)
            }
        };

        let reset_task = resets
            .as_ref()
            .map(|r| notify::start_reset_timer(r.clone(), Some(toast_cb.clone())));

        let check_updates_flag = Arc::new(AtomicBool::new(settings.check_updates));
        let (update_rx, update_wake, update_task) = update::spawn_daily(check_updates_flag.clone());

        let tray = match TrayController::new(settings.pill_locked) {
            Ok(t) => Some(t),
            Err(err) => {
                tracing::warn!(error = %err, "tray icon unavailable");
                None
            }
        };

        let first_run = format_first_run_flag(&settings);
        if first_run {
            ui.page = Page::Onboarding;
        }
        ui.animations
            .sync_targets(&snapshot, settings.active_tab, None, false);

        let slots = (snapshot.services.len() as u32).clamp(1, 3);
        let initial_pill = resolve_pill_rect(&settings, slots, &monitors);
        let zoom = primary_zoom(&monitors);
        let pill_logical = Size::new(
            (initial_pill.width as f64 / zoom) as f32,
            (initial_pill.height as f64 / zoom) as f32,
        );

        let level = if settings.always_on_top {
            window::Level::AlwaysOnTop
        } else {
            window::Level::Normal
        };

        let (pill_id, open_pill) = window::open(window::Settings {
            size: pill_logical,
            position: window::Position::Specific(Point::new(
                (initial_pill.x as f64 / zoom) as f32,
                (initial_pill.y as f64 / zoom) as f32,
            )),
            visible: true,
            resizable: false,
            decorations: false,
            transparent: true,
            level,
            exit_on_close_request: false,
            platform_specific: win32_platform_specific(),
            ..Default::default()
        });

        let pop_w = quotaglass_app::ui::tokens::primitives::POPOVER_WIDTH;
        let initial_mon = monitor_for_rect(initial_pill, &monitors);
        let initial_work = initial_mon.map_or(
            MonitorRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1040,
            },
            |m| m.work,
        );
        let pop_h = clamp_popover_height_to_work(
            popover_logical_height(&ui, &settings, &snapshot, None),
            initial_work,
            zoom,
        );
        let (popover_id, open_popover) = window::open(window::Settings {
            size: Size::new(pop_w, pop_h),
            position: window::Position::Default,
            visible: false,
            resizable: false,
            decorations: false,
            transparent: true,
            level: window::Level::AlwaysOnTop,
            exit_on_close_request: false,
            platform_specific: win32_platform_specific(),
            ..Default::default()
        });

        let dark = resolve_theme_dark(settings.theme, system_dark);
        let glass_mode = settings.effective_glass_mode();
        let always_on_top = settings.always_on_top;

        let init_pill = init_window_chrome(
            open_pill,
            display_tx.clone(),
            glass_mode,
            dark,
            always_on_top,
            Some(initial_pill),
            Message::PillCreated,
        );
        let init_popover = init_window_chrome(
            open_popover,
            display_tx.clone(),
            glass_mode,
            dark,
            true,
            None,
            Message::PopoverCreated,
        );

        let app = Self {
            settings_store,
            settings,
            snapshot,
            ui,
            detected,
            update_version: None,
            now_ms: now_utc_ms(),
            system_dark,
            os_glass,
            animations_enabled,
            monitors,
            pill_id,
            popover_id,
            pill_hwnd: None,
            popover_hwnd: None,
            pill_visible: true,
            popover_visible: false,
            user_hidden: false,
            auto_hidden: false,
            popover_opened_at: None,
            suppress_pill_toggle_until: None,
            last_slot_count: slots,
            anchor_service: None,
            open_onboarding_when_ready: first_run,
            pill_press: None,
            grip_drag: None,
            drag_moved_dirty: false,
            last_frame_at: None,
            scheduler,
            warnings,
            resets,
            toast_cb: Some(toast_cb),
            tray,
            check_updates_flag,
            update_wake: Some(update_wake),
            _reset_task: reset_task,
            _update_task: Some(update_task),
            scheduler_rx_seed: ChannelSeed(1, Arc::new(Mutex::new(sched_rx))),
            display_rx_seed: ChannelSeed(2, Arc::new(Mutex::new(Some(display_rx)))),
            update_rx_seed: ChannelSeed(3, Arc::new(Mutex::new(Some(update_rx)))),
            toast_rx_seed: ChannelSeed(4, Arc::new(Mutex::new(Some(toast_rx)))),
        };

        (app, Task::batch([init_pill, init_popover]))
    }

    fn is_dark(&self) -> bool {
        resolve_theme_dark(self.settings.theme, self.system_dark)
    }

    fn tokens(&self) -> DesignTokens {
        DesignTokens::resolve(
            self.settings.theme,
            self.settings.effective_glass_mode(),
            self.system_dark,
            self.os_glass,
        )
    }

    fn title(&self, _window: window::Id) -> String {
        "QuotaGlass".to_string()
    }

    fn theme(&self, _window: window::Id) -> Theme {
        if self.is_dark() {
            Theme::Dark
        } else {
            Theme::Light
        }
    }

    fn style(&self, _theme: &Theme) -> theme::Style {
        let tokens = self.tokens();
        theme::Style {
            background_color: Color::TRANSPARENT,
            text_color: tokens.semantic.text_primary,
        }
    }

    fn save_settings(&self) {
        if let Some(store) = &self.settings_store {
            if let Err(e) = store.save(&self.settings) {
                tracing::warn!(error = %e, "could not save settings.json");
            }
        }
    }

    fn apply_glass_to_windows(&self) {
        let dark = self.is_dark();
        let mode = self.settings.effective_glass_mode();
        if let Some(h) = self.pill_hwnd {
            win32::apply_glass(h, mode, dark);
            win32::mark_active(h);
        }
        if let Some(h) = self.popover_hwnd {
            win32::apply_glass(h, mode, dark);
            if self.popover_visible {
                win32::mark_active(h);
            }
        }
    }

    fn capture_anchor_and_save(&mut self) {
        if let Some(h) = self.pill_hwnd {
            if let Some(rect) = win32::get_window_rect(h) {
                self.settings.pill_position = Some(rect);
                if let Some((sig, anchor)) = win32::capture_taskbar_anchor(rect, &self.monitors) {
                    self.settings.set_taskbar_anchor(sig, anchor);
                }
                self.save_settings();
            }
        }
    }

    fn current_pill_rect(&self) -> WindowRect {
        if let Some(h) = self.pill_hwnd {
            if let Some(rect) = win32::get_window_rect(h) {
                return rect;
            }
        }
        let slots = (self.snapshot.services.len() as u32).clamp(1, 3);
        resolve_pill_rect(&self.settings, slots, &self.monitors)
    }

    fn resize_pill_for_slots(&mut self) -> Task<Message> {
        let slots = (self.snapshot.services.len() as u32).clamp(1, 3);
        if slots == self.last_slot_count {
            return Task::none();
        }
        self.last_slot_count = slots;
        let current = self.current_pill_rect();
        let mon = monitor_for_rect(current, &self.monitors);
        let zoom = mon.map_or(1.0, DisplayMonitor::scale_factor);
        let (new_w, new_h) = win32::pill_px_size(slots, self.settings.pill_scale, zoom);
        // Preserve right edge when expanding/shrinking so taskbar-pinned pills stay anchored cleanly.
        let right = i64::from(current.x) + i64::from(current.width);
        let desired = WindowRect {
            x: (right - i64::from(new_w)) as i32,
            y: current.y,
            width: new_w,
            height: new_h,
        };
        let bounds = mon.map_or(
            MonitorRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            |m| m.full,
        );
        let fitted = win32::fit_in(desired, bounds);
        if let Some(h) = self.pill_hwnd {
            win32::move_and_resize_px(h, fitted);
        }
        self.settings.pill_position = Some(fitted);
        if let Some((sig, anchor)) = win32::capture_taskbar_anchor(fitted, &self.monitors) {
            self.settings.set_taskbar_anchor(sig, anchor);
        }
        self.save_settings();
        window::resize(
            self.pill_id,
            Size::new((fitted.width as f64 / zoom) as f32, (fitted.height as f64 / zoom) as f32),
        )
    }

    fn resnap_pill_for_display_change(&mut self) -> Task<Message> {
        self.monitors = win32::enumerate_monitors();
        self.os_glass = win32::transparency_enabled();
        self.animations_enabled = win32::animations_enabled();
        self.system_dark = win32::system_uses_dark_theme();
        self.apply_glass_to_windows();

        let slots = (self.snapshot.services.len() as u32).clamp(1, 3);
        let rect = resolve_pill_rect(&self.settings, slots, &self.monitors);
        let zoom = monitor_for_rect(rect, &self.monitors).map_or(1.0, DisplayMonitor::scale_factor);

        if let Some(h) = self.pill_hwnd {
            win32::move_and_resize_px(h, rect);
            win32::set_topmost(h, self.settings.always_on_top);
            win32::mark_active(h);
        }
        self.settings.pill_position = Some(rect);
        let resize_pill = window::resize(
            self.pill_id,
            Size::new((rect.width as f64 / zoom) as f32, (rect.height as f64 / zoom) as f32),
        );
        if self.popover_visible {
            Task::batch([resize_pill, self.sync_popover_bounds()])
        } else {
            resize_pill
        }
    }

    fn sync_popover_bounds(&self) -> Task<Message> {
        let pill_rect = self.current_pill_rect();
        let mon = monitor_for_rect(pill_rect, &self.monitors);
        let zoom = mon.map_or(1.0, DisplayMonitor::scale_factor);
        let work = mon.map_or(
            MonitorRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1040,
            },
            |m| m.work,
        );
        let pop_w_logical = quotaglass_app::ui::tokens::primitives::POPOVER_WIDTH;
        let pop_h_logical = clamp_popover_height_to_work(
            popover_logical_height(
                &self.ui,
                &self.settings,
                &self.snapshot,
                self.update_version.as_deref(),
            ),
            work,
            zoom,
        );
        let pop_w_px = (f64::from(pop_w_logical) * zoom).round() as u32;
        let pop_h_px = (f64::from(pop_h_logical) * zoom).round() as u32;
        let anchor_x = pill::slot_anchor_x_px(&self.snapshot.services, self.anchor_service, pill_rect.width);
        let (pop_rect, _) = win32::flyout_rect(pill_rect, anchor_x, pop_w_px, pop_h_px, work, zoom);
        if let Some(h) = self.popover_hwnd {
            win32::move_and_resize_px(h, pop_rect);
        }
        window::resize(self.popover_id, Size::new(pop_w_logical, pop_h_logical))
    }

    fn show_popover(&mut self, service: Option<Service>, page: Option<Page>) -> Task<Message> {
        self.now_ms = now_utc_ms();
        self.os_glass = win32::transparency_enabled();
        self.anchor_service = service;
        if let Some(p) = page {
            self.ui.page = p;
        } else if format_first_run_flag(&self.settings) {
            self.ui.page = Page::Onboarding;
        } else if self.ui.page == Page::Onboarding {
            self.ui.page = Page::List;
        }
        if let Some(svc) = service {
            let tab = tab_for_service(Some(svc));
            select_tab_and_persist(&mut self.settings, &mut self.ui, tab, &self.snapshot.accounts);
            self.save_settings();
        }
        if let Some(sched) = &self.scheduler {
            self.detected = Some(sched.detected());
            if sched.refresh_on_open() {
                self.ui.pending.refreshing = true;
            }
        }
        self.ui.animations.sync_targets(
            &self.snapshot,
            self.settings.active_tab,
            self.ui.expanded_id.as_deref(),
            self.animations_enabled,
        );

        let resize_task = self.sync_popover_bounds();
        if let Some(h) = self.popover_hwnd {
            win32::show_no_activate(h);
            win32::mark_active(h);
            win32::assert_topmost(h);
            win32::focus_window(h);
        }
        self.popover_visible = true;
        self.popover_opened_at = Some(Instant::now());
        resize_task
    }

    fn hide_popover(&mut self) {
        if let Some(h) = self.popover_hwnd {
            win32::hide_window(h);
        }
        self.popover_visible = false;
        self.popover_opened_at = None;
        self.ui.add_picker_open = false;
        self.ui.confirm_remove_id = None;
        self.ui.rename = None;
    }

    fn show_pill(&mut self) {
        self.user_hidden = false;
        self.auto_hidden = false;
        if let Some(h) = self.pill_hwnd {
            win32::show_no_activate(h);
            win32::mark_active(h);
            win32::set_topmost(h, self.settings.always_on_top);
        }
        self.pill_visible = true;
    }

    fn hide_pill(&mut self) {
        self.hide_popover();
        if let Some(h) = self.pill_hwnd {
            win32::hide_window(h);
        }
        self.pill_visible = false;
    }

    fn has_rate_limited_service(&self) -> bool {
        self.snapshot.services.iter().any(|s| s.rate_limited)
    }

    fn toggle_pin(&mut self) {
        let next = !self.settings.pill_locked;
        self.settings.pill_locked = next;
        if let Some(tray) = &self.tray {
            tray.set_pin_label(next);
        }
        self.capture_anchor_and_save();
    }

    fn apply_settings_to_runtime(&mut self) {
        self.save_settings();
        if let Some(sched) = &self.scheduler {
            sched.apply(&self.settings);
        }
        if let Some(resets) = &self.resets {
            resets.set_toasts(&self.settings.reset_toasts);
        }
        self.check_updates_flag
            .store(self.settings.check_updates, Ordering::Release);
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::PillCreated(hwnd) => {
                self.pill_hwnd = hwnd;
                Task::none()
            }
            Message::PopoverCreated(hwnd) => {
                self.popover_hwnd = hwnd;
                if self.open_onboarding_when_ready {
                    self.open_onboarding_when_ready = false;
                    return self.show_popover(None, Some(Page::Onboarding));
                }
                Task::none()
            }
            Message::WindowEvent(id, event) => match event {
                window::Event::Moved(_) if id == self.pill_id => {
                    if self.pill_press.as_ref().is_some_and(|p| p.dragged) {
                        self.drag_moved_dirty = true;
                    }
                    Task::none()
                }
                window::Event::Unfocused if id == self.popover_id => {
                    let past_grace = self
                        .popover_opened_at
                        .is_some_and(|t| t.elapsed() >= OPEN_BLUR_GRACE);
                    if self.popover_visible && past_grace {
                        if let Some((cx, cy)) = win32::cursor_pos_px() {
                            let pr = self.current_pill_rect();
                            if cx >= pr.x
                                && cx < pr.x + pr.width as i32
                                && cy >= pr.y
                                && cy < pr.y + pr.height as i32
                            {
                                self.suppress_pill_toggle_until =
                                    Some(Instant::now() + PILL_CLICK_SUPPRESS_AFTER_BLUR);
                            }
                        }
                        self.hide_popover();
                    }
                    Task::none()
                }
                window::Event::CloseRequested => Task::none(),
                _ => Task::none(),
            },
            Message::MouseMoveOnWindow(id, _pos) => {
                if id != self.pill_id {
                    return Task::none();
                }
                if let Some(drag) = self.grip_drag.clone() {
                    if let Some((cx, _)) = win32::cursor_pos_px() {
                        let next_scale = clamp_scale(
                            drag.start_scale
                                * (f64::from(drag.start_width_px) + f64::from(cx - drag.start_x_px))
                                / f64::from(drag.start_width_px.max(1)),
                        );
                        if (next_scale - self.settings.pill_scale).abs() > 0.005 {
                            self.settings.pill_scale = next_scale;
                            let (nw, nh) =
                                win32::pill_px_size(drag.slots, next_scale, drag.zoom);
                            let next_rect = win32::resized_pill_rect(
                                drag.start_left_x,
                                drag.start_center_y,
                                nw,
                                nh,
                                drag.monitor_bounds,
                            );
                            if let Some(h) = self.pill_hwnd {
                                win32::move_and_resize_px(h, next_rect);
                            }
                            return window::resize(
                                self.pill_id,
                                Size::new(
                                    (nw as f64 / drag.zoom) as f32,
                                    (nh as f64 / drag.zoom) as f32,
                                ),
                            );
                        }
                    }
                    return Task::none();
                }

                if self.drag_moved_dirty {
                    self.drag_moved_dirty = false;
                    self.pill_press = None;
                    self.suppress_pill_toggle_until =
                        Some(Instant::now() + PILL_CLICK_SUPPRESS_AFTER_BLUR);
                    self.capture_anchor_and_save();
                }

                if !self.settings.pill_locked {
                    if let Some(press) = &mut self.pill_press {
                        if !press.dragged {
                            if let (Some((ox, oy)), Some((cx, cy))) =
                                (press.origin_px, win32::cursor_pos_px())
                            {
                                if (cx - ox).abs() >= DRAG_THRESHOLD_PX
                                    || (cy - oy).abs() >= DRAG_THRESHOLD_PX
                                {
                                    press.dragged = true;
                                    self.hide_popover();
                                    return window::drag(self.pill_id);
                                }
                            }
                        }
                    }
                }
                Task::none()
            }
            Message::MouseReleaseOnWindow(id) => {
                if id != self.pill_id {
                    return Task::none();
                }
                if self.grip_drag.take().is_some() {
                    self.suppress_pill_toggle_until =
                        Some(Instant::now() + PILL_CLICK_SUPPRESS_AFTER_BLUR);
                    self.capture_anchor_and_save();
                    return Task::none();
                }
                let was_dragged = self.pill_press.take().is_some_and(|p| p.dragged);
                if self.drag_moved_dirty || was_dragged {
                    self.drag_moved_dirty = false;
                    self.suppress_pill_toggle_until =
                        Some(Instant::now() + PILL_CLICK_SUPPRESS_AFTER_BLUR);
                    self.capture_anchor_and_save();
                }
                Task::none()
            }
            Message::EscapePressed(id) => {
                if (id == self.pill_id || id == self.popover_id) && self.popover_visible {
                    if self.ui.step_back_on_escape() {
                        return self.sync_popover_bounds();
                    }
                    self.hide_popover();
                }
                Task::none()
            }
            Message::SystemThemeChanged(mode) => {
                self.system_dark = matches!(mode, theme::Mode::Dark);
                self.apply_glass_to_windows();
                Task::none()
            }
            Message::Scheduler(ev) => match ev {
                SchedulerEvent::Refreshing(busy) => {
                    self.ui.pending.refreshing = busy;
                    Task::none()
                }
                SchedulerEvent::Snapshot(snap) => {
                    self.now_ms = now_utc_ms();
                    self.snapshot = snap;
                    self.ui.has_snapshot = true;
                    self.ui.reconcile_accounts(&self.snapshot.accounts);
                    self.ui.animations.sync_targets(
                        &self.snapshot,
                        self.settings.active_tab,
                        self.ui.expanded_id.as_deref(),
                        self.animations_enabled,
                    );
                    if let Some(w) = &self.warnings {
                        w.on_snapshot(&self.snapshot, self.toast_cb.clone());
                    }
                    if let Some(r) = &self.resets {
                        r.on_snapshot(&self.snapshot, self.toast_cb.clone());
                    }
                    let resize_pill = self.resize_pill_for_slots();
                    if self.popover_visible {
                        Task::batch([resize_pill, self.sync_popover_bounds()])
                    } else {
                        resize_pill
                    }
                }
            },
            Message::Tray(ev) => match ev {
                TrayEvent::TogglePill => {
                    if self.pill_visible {
                        self.user_hidden = true;
                        self.hide_pill();
                    } else {
                        self.show_pill();
                    }
                    Task::none()
                }
                TrayEvent::Show => {
                    self.show_pill();
                    Task::none()
                }
                TrayEvent::Focus => {
                    self.show_pill();
                    self.show_popover(None, None)
                }
                TrayEvent::TogglePin => {
                    self.toggle_pin();
                    Task::none()
                }
                TrayEvent::RefreshNow => {
                    if let Some(sched) = &self.scheduler {
                        sched.refresh_now();
                    }
                    Task::none()
                }
                TrayEvent::Settings => {
                    self.show_pill();
                    self.show_popover(None, Some(Page::Settings))
                }
                TrayEvent::Quit => iced::exit(),
            },
            Message::Display(_ev) => self.resnap_pill_for_display_change(),
            Message::UpdateAvailable(ver) => {
                self.update_version = ver;
                if self.popover_visible {
                    self.sync_popover_bounds()
                } else {
                    Task::none()
                }
            }
            Message::ToastActivated(service) => {
                self.show_pill();
                self.show_popover(service, Some(Page::List))
            }
            Message::Frame(now) => {
                let dt = self
                    .last_frame_at
                    .map_or(1.0 / 60.0, |prev| now.duration_since(prev).as_secs_f32());
                self.last_frame_at = Some(now);
                let shimmer = self.popover_visible && self.ui.is_skeleton_loading(&self.snapshot);
                let pulse = self.pill_visible
                    && (self.ui.pending.refreshing || self.has_rate_limited_service());
                let keep_going =
                    self.ui
                        .animations
                        .advance(dt, self.animations_enabled, shimmer, pulse);
                if !keep_going {
                    self.last_frame_at = None;
                }
                Task::none()
            }
            Message::TopmostTick => {
                if self.drag_moved_dirty {
                    self.drag_moved_dirty = false;
                    self.pill_press = None;
                    self.suppress_pill_toggle_until =
                        Some(Instant::now() + PILL_CLICK_SUPPRESS_AFTER_BLUR);
                    self.capture_anchor_and_save();
                }
                let busy = win32::fullscreen_busy();
                match win32::next_visibility(busy, self.auto_hidden, self.user_hidden, self.pill_visible)
                {
                    FullscreenAction::Hide => {
                        self.auto_hidden = true;
                        self.hide_pill();
                    }
                    FullscreenAction::Show => {
                        self.auto_hidden = false;
                        self.show_pill();
                    }
                    FullscreenAction::Keep => {}
                }
                if self.pill_visible && self.settings.always_on_top {
                    if let Some(h) = self.pill_hwnd {
                        win32::assert_topmost(h);
                    }
                }
                if self.popover_visible {
                    if let Some(h) = self.popover_hwnd {
                        win32::assert_topmost(h);
                    }
                }
                Task::none()
            }
            Message::ResetTick => {
                self.now_ms = now_utc_ms();
                Task::none()
            }
            Message::AddAccountDone { result } => {
                self.ui.pending.adding_service = None;
                match result {
                    Ok(account) => {
                        if !self.settings.added_accounts.iter().any(|a| a.id == account.id) {
                            self.settings.added_accounts.push(account);
                        }
                        self.ui.add_picker_open = false;
                        self.apply_settings_to_runtime();
                        if let Some(sched) = &self.scheduler {
                            sched.refresh_now();
                        }
                    }
                    Err(msg) => {
                        self.ui.problem_banner = Some(msg);
                    }
                }
                if self.popover_visible {
                    self.sync_popover_bounds()
                } else {
                    Task::none()
                }
            }
            Message::RemoveAccountDone {
                account_id,
                forget_card,
                profile_id,
                result,
            } => {
                self.ui.pending.removing_account = None;
                self.ui.confirm_remove_id = None;
                match result {
                    Ok(()) => {
                        self.settings.forget_added(&profile_id, &account_id, forget_card);
                        if self.ui.expanded_id.as_deref() == Some(account_id.as_str()) {
                            self.ui.expanded_id = None;
                        }
                        self.apply_settings_to_runtime();
                    }
                    Err(msg) => {
                        self.ui.problem_banner = Some(msg);
                    }
                }
                if self.popover_visible {
                    self.sync_popover_bounds()
                } else {
                    Task::none()
                }
            }
            Message::SignInDone { result } => {
                self.ui.pending.signing_in_account = None;
                if let Err(msg) = result {
                    self.ui.problem_banner = Some(msg);
                    if self.popover_visible {
                        return self.sync_popover_bounds();
                    }
                }
                Task::none()
            }
            Message::Ui(msg) => self.handle_ui_message(msg),
            Message::Noop => Task::none(),
        }
    }

    fn handle_ui_message(&mut self, msg: UiMessage) -> Task<Message> {
        match msg {
            UiMessage::PillHovered(hovered) => {
                self.ui.pill_hovered = hovered;
                Task::none()
            }
            UiMessage::PillSlotHovered(svc) => {
                self.ui.hovered_slot = svc;
                Task::none()
            }
            UiMessage::PillGripHovered(hovered) => {
                self.ui.grip_hovered = hovered;
                Task::none()
            }
            UiMessage::PillSlotPressed(_svc) => {
                self.pill_press = Some(PillPress {
                    origin_px: win32::cursor_pos_px(),
                    dragged: false,
                });
                Task::none()
            }
            UiMessage::PillSlotReleased(svc) => {
                if self.grip_drag.take().is_some() {
                    self.capture_anchor_and_save();
                    return Task::none();
                }
                let was_dragged = self.pill_press.take().is_some_and(|p| p.dragged);
                if was_dragged {
                    self.capture_anchor_and_save();
                    return Task::none();
                }
                if self
                    .suppress_pill_toggle_until
                    .is_some_and(|until| Instant::now() <= until)
                {
                    self.suppress_pill_toggle_until = None;
                    return Task::none();
                }
                let target_tab = tab_for_service(svc);
                if self.popover_visible
                    && (svc.is_none() || self.settings.active_tab == target_tab)
                {
                    self.hide_popover();
                    return Task::none();
                }
                self.show_popover(svc, None)
            }
            UiMessage::PillGripPressed => {
                if self.settings.pill_locked {
                    return Task::none();
                }
                let rect = self.current_pill_rect();
                let mon = monitor_for_rect(rect, &self.monitors);
                let zoom = mon.map_or(1.0, DisplayMonitor::scale_factor);
                let monitor_bounds = mon.map_or(
                    MonitorRect {
                        x: 0,
                        y: 0,
                        width: 1920,
                        height: 1080,
                    },
                    |m| m.full,
                );
                let slots = (self.snapshot.services.len() as u32).clamp(1, 3);
                if let Some((cx, _)) = win32::cursor_pos_px() {
                    self.grip_drag = Some(GripDrag {
                        start_x_px: cx,
                        start_left_x: rect.x,
                        start_center_y: i64::from(rect.y) + i64::from(rect.height) / 2,
                        start_width_px: rect.width,
                        start_scale: self.settings.pill_scale,
                        zoom,
                        slots,
                        monitor_bounds,
                    });
                }
                Task::none()
            }
            UiMessage::SelectTab(tab) => {
                select_tab_and_persist(
                    &mut self.settings,
                    &mut self.ui,
                    tab,
                    &self.snapshot.accounts,
                );
                self.save_settings();
                self.ui.animations.sync_targets(
                    &self.snapshot,
                    self.settings.active_tab,
                    self.ui.expanded_id.as_deref(),
                    self.animations_enabled,
                );
                self.sync_popover_bounds()
            }
            UiMessage::RefreshNow => {
                if self.ui.pending.can_refresh() {
                    if let Some(sched) = &self.scheduler {
                        if sched.refresh_now() {
                            self.ui.pending.refreshing = true;
                        }
                    }
                }
                Task::none()
            }
            UiMessage::ToggleExpand(id) => {
                self.ui.toggle_expand(&id);
                self.ui.animations.sync_targets(
                    &self.snapshot,
                    self.settings.active_tab,
                    self.ui.expanded_id.as_deref(),
                    self.animations_enabled,
                );
                self.sync_popover_bounds()
            }
            UiMessage::StartRename(id) => {
                let existing = self.settings.labels.get(&id).map(String::as_str);
                self.ui.start_rename(&id, existing);
                self.sync_popover_bounds()
            }
            UiMessage::RenameDraftChanged(text) => {
                self.ui.update_rename_draft(text);
                Task::none()
            }
            UiMessage::SubmitRename => {
                match self.ui.commit_rename(&mut self.settings) {
                    Ok(true) => {
                        self.apply_settings_to_runtime();
                        self.sync_popover_bounds()
                    }
                    Ok(false) => Task::none(),
                    Err(_) => self.sync_popover_bounds(),
                }
            }
            UiMessage::CancelRename => {
                self.ui.cancel_rename();
                self.sync_popover_bounds()
            }
            UiMessage::HideAccount(id) => {
                self.settings.set_hidden(&id, true);
                if self.ui.expanded_id.as_deref() == Some(id.as_str()) {
                    self.ui.expanded_id = None;
                }
                self.apply_settings_to_runtime();
                self.sync_popover_bounds()
            }
            UiMessage::RequestRemove(id) => {
                self.ui.request_remove(&id);
                self.sync_popover_bounds()
            }
            UiMessage::CancelRemove => {
                self.ui.cancel_remove();
                self.sync_popover_bounds()
            }
            UiMessage::ConfirmRemove(id) => {
                if self.ui.pending.is_removing(&id) {
                    return Task::none();
                }
                let Some(root) = connect::profiles_root() else {
                    self.ui.problem_banner = Some("Could not locate the QuotaGlass profiles folder.".into());
                    return Task::none();
                };
                let removal = match connect::removal_target(
                    &id,
                    &self.snapshot.accounts,
                    &self.settings.added_accounts,
                    &root,
                ) {
                    Ok(r) => r,
                    Err(msg) => {
                        self.ui.problem_banner = Some(msg.to_string());
                        return Task::none();
                    }
                };
                let tool = self
                    .scheduler
                    .as_ref()
                    .and_then(|s| s.sign_in_tool(removal.account.service));
                let profile_id = removal.account.id.clone();
                let forget_card = removal.forget_card;
                let account_id = id.clone();
                self.ui.pending.removing_account = Some(id);

                Task::perform(
                    async move {
                        let res = tokio::task::spawn_blocking(move || -> Result<(), String> {
                            if let Some(t) = tool {
                                let profile = connect::plain(&removal.account.profile_dir);
                                let _ = connect::logout(removal.account.service, Some(&t), &profile);
                            }
                            connect::delete_profile(&root, &removal.account.profile_dir)
                                .map_err(str::to_string)
                        })
                        .await
                        .map_err(|_| "Could not finish removing the profile.".to_string())?;
                        res
                    },
                    move |result| Message::RemoveAccountDone {
                        account_id,
                        forget_card,
                        profile_id,
                        result,
                    },
                )
            }
            UiMessage::SignInAccount(id) => {
                if self.ui.pending.is_signing_in(&id) {
                    return Task::none();
                }
                let task = match connect::resolve_account(
                    &id,
                    &self.snapshot.accounts,
                    &self.settings.added_accounts,
                ) {
                    Ok(t) => t,
                    Err(msg) => {
                        self.ui.problem_banner = Some(msg.to_string());
                        return Task::none();
                    }
                };
                let (service, action, profile_dir) = match task {
                    connect::AccountTask::CurrentLogin(svc) => (svc, CliAction::Login, None),
                    connect::AccountTask::Profile(acct) => {
                        (acct.service, CliAction::Open, Some(acct.profile_dir))
                    }
                };
                let tool = self
                    .scheduler
                    .as_ref()
                    .and_then(|s| s.sign_in_tool(service));
                self.ui.pending.signing_in_account = Some(id);
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            let plain_dir = profile_dir.as_deref().map(connect::plain);
                            connect::open_console(
                                service,
                                tool.as_deref(),
                                action,
                                plain_dir.as_deref(),
                            )
                            .map_err(str::to_string)
                        })
                        .await
                        .map_err(|_| "Could not open the sign-in window.".to_string())?
                    },
                    |result| Message::SignInDone { result },
                )
            }
            UiMessage::ToggleAddPicker => {
                if self.ui.pending.can_add() {
                    self.ui.add_picker_open = !self.ui.add_picker_open;
                }
                self.sync_popover_bounds()
            }
            UiMessage::AddAccount(service) => {
                if !self.ui.pending.can_add() {
                    return Task::none();
                }
                let Some(root) = connect::profiles_root() else {
                    self.ui.problem_banner = Some("Could not locate the QuotaGlass profiles folder.".into());
                    return Task::none();
                };
                let tool = self
                    .scheduler
                    .as_ref()
                    .and_then(|s| s.sign_in_tool(service));
                let taken: Vec<String> = self
                    .settings
                    .added_accounts
                    .iter()
                    .map(|a| a.id.clone())
                    .collect();
                self.ui.pending.adding_service = Some(service);
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || -> Result<AddedAccount, String> {
                            let (id, dir) = connect::create_profile_dir(&root, &taken)
                                .map_err(|_| "Could not create profile folder.".to_string())?;
                            let plain_dir = connect::plain(&dir);
                            if let Err(err) = connect::open_console(
                                service,
                                tool.as_deref(),
                                CliAction::Login,
                                Some(&plain_dir),
                            ) {
                                let _ = connect::delete_profile(&root, &dir);
                                return Err(err.to_string());
                            }
                            Ok(AddedAccount {
                                id,
                                service,
                                profile_dir: dir,
                            })
                        })
                        .await
                        .map_err(|_| "Could not add account.".to_string())?
                    },
                    move |result| Message::AddAccountDone { result },
                )
            }
            UiMessage::TogglePin => {
                self.toggle_pin();
                Task::none()
            }
            UiMessage::OpenPage(page) => {
                self.ui.page = page;
                self.ui.add_picker_open = false;
                self.sync_popover_bounds()
            }
            UiMessage::BackToList => {
                self.ui.page = Page::List;
                self.sync_popover_bounds()
            }
            UiMessage::OpenUrl(target) => {
                if let Err(msg) = connect::open_url(target) {
                    self.ui.problem_banner = Some(msg.to_string());
                    if self.popover_visible {
                        return self.sync_popover_bounds();
                    }
                }
                Task::none()
            }
            UiMessage::DismissProblem => {
                self.ui.problem_banner = None;
                self.sync_popover_bounds()
            }
            UiMessage::SetTheme(theme) => {
                self.os_glass = win32::transparency_enabled();
                self.settings.theme = theme;
                self.save_settings();
                self.apply_glass_to_windows();
                Task::none()
            }
            UiMessage::SetGlassMode(mode) => {
                self.os_glass = win32::transparency_enabled();
                self.settings.set_glass_mode(mode);
                self.save_settings();
                self.apply_glass_to_windows();
                Task::none()
            }
            UiMessage::SetRefreshMinutes(mins) => {
                self.settings.refresh_minutes = mins.clamp(1, 5);
                self.apply_settings_to_runtime();
                Task::none()
            }
            UiMessage::SetAlwaysOnTop(on) => {
                self.settings.always_on_top = on;
                self.save_settings();
                if let Some(h) = self.pill_hwnd {
                    win32::set_topmost(h, on);
                }
                let level = if on {
                    window::Level::AlwaysOnTop
                } else {
                    window::Level::Normal
                };
                window::set_level(self.pill_id, level)
            }
            UiMessage::SetAutostart(on) => {
                match autostart::set_enabled(on) {
                    Ok(()) => {
                        self.settings.autostart = on;
                        self.save_settings();
                    }
                    Err(msg) => {
                        self.ui.problem_banner = Some(msg.to_string());
                        if self.popover_visible {
                            return self.sync_popover_bounds();
                        }
                    }
                }
                Task::none()
            }
            UiMessage::SetCheckUpdates(on) => {
                self.settings.check_updates = on;
                self.apply_settings_to_runtime();
                if on {
                    if let Some(wake) = &self.update_wake {
                        wake.notify_one();
                    }
                } else {
                    self.update_version = None;
                }
                if self.popover_visible {
                    self.sync_popover_bounds()
                } else {
                    Task::none()
                }
            }
            UiMessage::SetResetToastsEnabled(on) => {
                self.settings.reset_toasts.enabled = on;
                self.apply_settings_to_runtime();
                Task::none()
            }
            UiMessage::SetResetToastService(service, on) => {
                match service {
                    Service::Claude => self.settings.reset_toasts.claude = on,
                    Service::Codex => self.settings.reset_toasts.codex = on,
                    Service::Antigravity => self.settings.reset_toasts.antigravity = on,
                }
                self.apply_settings_to_runtime();
                Task::none()
            }
            UiMessage::SetResetToastSound(sound) => {
                self.settings.reset_toasts.sound = sound;
                self.apply_settings_to_runtime();
                Task::none()
            }
            UiMessage::ToggleSource(conn, on) => {
                self.settings.set_connected(conn, on);
                self.apply_settings_to_runtime();
                if on {
                    if let Some(sched) = &self.scheduler {
                        sched.refresh_now();
                    }
                }
                Task::none()
            }
            UiMessage::UnhideAccount(id) => {
                self.settings.set_hidden(&id, false);
                self.apply_settings_to_runtime();
                self.sync_popover_bounds()
            }
            UiMessage::FinishOnboarding => {
                self.ui.page = Page::List;
                self.sync_popover_bounds()
            }
        }
    }

    fn view(&self, window: window::Id) -> Element<'_, Message> {
        let tokens = self.tokens();
        if window == self.pill_id {
            pill::view(&tokens, &self.ui, &self.settings, &self.snapshot).map(Message::Ui)
        } else {
            popover::view(
                &tokens,
                &self.ui,
                &self.settings,
                &self.snapshot,
                self.detected,
                self.update_version.as_deref(),
                self.now_ms,
            )
            .map(Message::Ui)
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        let interaction_sub = iced::event::listen_with(route_raw_event);

        let mut subs = vec![
            window::events().map(|(id, ev)| Message::WindowEvent(id, ev)),
            interaction_sub,
            iced::system::theme_changes().map(Message::SystemThemeChanged),
            tray::subscription().map(Message::Tray),
            Subscription::run_with(self.scheduler_rx_seed.clone(), unbounded_stream)
                .map(Message::Scheduler),
            Subscription::run_with(self.display_rx_seed.clone(), unbounded_stream)
                .map(Message::Display),
            Subscription::run_with(self.update_rx_seed.clone(), watch_stream)
                .map(Message::UpdateAvailable),
            Subscription::run_with(self.toast_rx_seed.clone(), unbounded_stream)
                .map(Message::ToastActivated),
            time::every(Duration::from_secs(2)).map(|_| Message::TopmostTick),
            time::every(notify::RESET_TICK).map(|_| Message::ResetTick),
        ];

        let shimmer = self.popover_visible && self.ui.is_skeleton_loading(&self.snapshot);
        let pulse = self.pill_visible
            && (self.ui.pending.refreshing || self.has_rate_limited_service());
        let animating = self.ui.animations.is_animating(
            self.animations_enabled,
            self.pill_visible,
            self.popover_visible,
            shimmer,
            pulse,
        );
        if animating || self.grip_drag.is_some() {
            subs.push(window::frames().map(Message::Frame));
        }

        Subscription::batch(subs)
    }
}

fn init_window_chrome(
    open_task: Task<window::Id>,
    display_tx: mpsc::UnboundedSender<DisplayEvent>,
    glass_mode: GlassMode,
    dark: bool,
    topmost: bool,
    initial_rect: Option<WindowRect>,
    map_msg: fn(Option<HwndHandle>) -> Message,
) -> Task<Message> {
    open_task.then(move |id| {
        let tx = display_tx.clone();
        window::run(id, move |win| {
            let hwnd = HwndHandle::from_window_handle(win);
            if let Some(h) = hwnd {
                win32::set_tool_window(h);
                win32::round_corners(h);
                win32::install_window_subclass(h, tx);
                win32::apply_glass(h, glass_mode, dark);
                win32::set_topmost(h, topmost);
                if let Some(rect) = initial_rect {
                    win32::mark_active(h);
                    win32::move_and_resize_px(h, rect);
                }
            }
            hwnd
        })
        .map(map_msg)
    })
}

fn route_raw_event(
    event: iced::Event,
    _status: iced::event::Status,
    id: window::Id,
) -> Option<Message> {
    match event {
        iced::Event::Keyboard(keyboard::Event::KeyPressed {
            key: Key::Named(Named::Escape),
            ..
        }) => Some(Message::EscapePressed(id)),
        iced::Event::Mouse(mouse::Event::CursorMoved { position }) => {
            Some(Message::MouseMoveOnWindow(id, position))
        }
        iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
            Some(Message::MouseReleaseOnWindow(id))
        }
        _ => None,
    }
}

fn unbounded_stream<T: Send + 'static>(
    seed: &ChannelSeed<mpsc::UnboundedReceiver<T>>,
) -> impl Stream<Item = T> + use<T> {
    let taken = seed.1.lock().ok().and_then(|mut g| g.take());
    stream::channel(64, async move |mut output| {
        use iced::futures::SinkExt;
        let Some(mut rx) = taken else {
            return;
        };
        while let Some(item) = rx.recv().await {
            if output.send(item).await.is_err() {
                break;
            }
        }
    })
}

fn watch_stream(
    seed: &ChannelSeed<watch::Receiver<Option<String>>>,
) -> impl Stream<Item = Option<String>> {
    let taken = seed.1.lock().ok().and_then(|mut g| g.take());
    stream::channel(8, async move |mut output| {
        use iced::futures::SinkExt;
        let Some(mut rx) = taken else {
            return;
        };
        while rx.changed().await.is_ok() {
            let val = rx.borrow_and_update().clone();
            if output.send(val).await.is_err() {
                break;
            }
        }
    })
}

fn win32_platform_specific() -> PlatformSpecific {
    PlatformSpecific {
        drag_and_drop: false,
        skip_taskbar: true,
        undecorated_shadow: false,
        corner_preference: CornerPreference::Round,
    }
}

fn clamp_popover_height_to_work(raw_logical_h: f32, work: MonitorRect, zoom: f64) -> f32 {
    let z = if zoom.is_finite() && zoom > 0.0 { zoom } else { 1.0 };
    let max_logical = ((f64::from(work.height) / z) - 24.0).max(240.0) as f32;
    raw_logical_h.min(max_logical).max(220.0)
}

fn primary_zoom(monitors: &[DisplayMonitor]) -> f64 {
    monitors
        .iter()
        .find(|m| m.primary)
        .or_else(|| monitors.first())
        .map_or(1.0, DisplayMonitor::scale_factor)
}

fn monitor_for_rect(rect: WindowRect, monitors: &[DisplayMonitor]) -> Option<&DisplayMonitor> {
    let cx = i64::from(rect.x) + i64::from(rect.width) / 2;
    let cy = i64::from(rect.y) + i64::from(rect.height) / 2;
    monitors
        .iter()
        .find(|m| {
            cx >= i64::from(m.full.x)
                && cx < i64::from(m.full.x) + i64::from(m.full.width)
                && cy >= i64::from(m.full.y)
                && cy < i64::from(m.full.y) + i64::from(m.full.height)
        })
        .or_else(|| monitors.iter().find(|m| m.primary))
        .or_else(|| monitors.first())
}

fn resolve_pill_rect(
    settings: &Settings,
    slots: u32,
    monitors: &[DisplayMonitor],
) -> WindowRect {
    let zoom = primary_zoom(monitors);
    let px_size = win32::pill_px_size(slots, settings.pill_scale, zoom);
    let sig = win32::topology_signature(monitors);
    if let Some(anchor) = settings.taskbar_anchor(&sig) {
        if let Some(rect) = win32::resolve_taskbar_anchor(anchor, px_size, monitors) {
            return rect;
        }
    }
    if settings.pill_locked {
        if let Some(pinned) = settings
            .taskbar_anchors
            .values()
            .find(|a| a.on_taskbar_strip)
        {
            if let Some(rect) = win32::resolve_taskbar_anchor(pinned, px_size, monitors) {
                return rect;
            }
        }
    }
    if let Some(pos) = settings.pill_position {
        let sized = WindowRect {
            width: px_size.0,
            height: px_size.1,
            ..pos
        };
        let fulls: Vec<MonitorRect> = monitors.iter().map(|m| m.full).collect();
        return win32::clamp_to_monitors(sized, &fulls);
    }
    let work = monitors
        .iter()
        .find(|m| m.primary)
        .or_else(|| monitors.first())
        .map_or(
            MonitorRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1040,
            },
            |m| m.work,
        );
    win32::default_pill_rect(work, px_size, zoom)
}

fn main() -> iced::Result {
    tracing_subscriber::fmt().init();

    iced::daemon(App::boot, App::update, App::view)
        .title(App::title)
        .default_font(UI_FONT)
        .antialiasing(true)
        .theme(App::theme)
        .style(App::style)
        .subscription(App::subscription)
        .run()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use quotaglass_core::stagger_offset_secs;

    fn test_app(settings: Settings) -> App {
        let (_display_tx, display_rx) = mpsc::unbounded_channel();
        let (_sched_tx, sched_rx) = mpsc::unbounded_channel();
        let (_update_tx, update_rx) = watch::channel(None);
        let (_toast_tx, toast_rx) = mpsc::unbounded_channel();
        App {
            settings_store: None,
            settings,
            snapshot: Snapshot {
                accounts: Vec::new(),
                services: Vec::new(),
                generated_at_ms: 1_800_000_000_000,
            },
            ui: UiState::default(),
            detected: None,
            update_version: None,
            now_ms: 1_800_000_000_000,
            system_dark: true,
            os_glass: true,
            animations_enabled: true,
            monitors: vec![DisplayMonitor {
                full: MonitorRect {
                    x: 0,
                    y: 0,
                    width: 1920,
                    height: 1080,
                },
                work: MonitorRect {
                    x: 0,
                    y: 0,
                    width: 1920,
                    height: 1032,
                },
                primary: true,
                dpi: 96,
            }],
            pill_id: window::Id::unique(),
            popover_id: window::Id::unique(),
            pill_hwnd: None,
            popover_hwnd: None,
            pill_visible: true,
            popover_visible: false,
            user_hidden: false,
            auto_hidden: false,
            popover_opened_at: None,
            suppress_pill_toggle_until: None,
            last_slot_count: 1,
            anchor_service: None,
            open_onboarding_when_ready: false,
            pill_press: None,
            grip_drag: None,
            drag_moved_dirty: false,
            last_frame_at: None,
            scheduler: None,
            warnings: None,
            resets: None,
            toast_cb: None,
            tray: None,
            check_updates_flag: Arc::new(AtomicBool::new(true)),
            update_wake: None,
            _reset_task: None,
            _update_task: None,
            scheduler_rx_seed: ChannelSeed(1, Arc::new(Mutex::new(Some(sched_rx)))),
            display_rx_seed: ChannelSeed(2, Arc::new(Mutex::new(Some(display_rx)))),
            update_rx_seed: ChannelSeed(3, Arc::new(Mutex::new(Some(update_rx)))),
            toast_rx_seed: ChannelSeed(4, Arc::new(Mutex::new(Some(toast_rx)))),
        }
    }

    #[test]
    fn app_style_background_is_always_transparent_for_dwm_glass() {
        let app = test_app(Settings::default());
        let style = app.style(&Theme::Dark);
        assert_eq!(style.background_color, Color::TRANSPARENT);
    }

    #[test]
    fn app_views_render_pill_and_all_popover_pages_without_panicking() {
        let mut app = test_app(Settings::default());
        let _ = app.view(app.pill_id);
        for page in [Page::List, Page::Settings, Page::Faq, Page::Onboarding] {
            app.ui.page = page;
            let _ = app.view(app.popover_id);
        }
    }

    #[test]
    fn pinned_pill_keeps_taskbar_strip_on_new_monitor_topology() {
        let external = [DisplayMonitor {
            full: MonitorRect {
                x: 0,
                y: 0,
                width: 2560,
                height: 1440,
            },
            work: MonitorRect {
                x: 0,
                y: 0,
                width: 2560,
                height: 1380,
            },
            primary: true,
            dpi: 144,
        }];
        let laptop = [DisplayMonitor {
            full: MonitorRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            work: MonitorRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1032,
            },
            primary: true,
            dpi: 96,
        }];

        let mut settings = Settings {
            pill_locked: true,
            ..Settings::default()
        };
        let (sig, anchor) =
            win32::capture_taskbar_anchor(WindowRect { x: 2200, y: 1388, width: 156, height: 48 }, &external)
                .unwrap();
        settings.set_taskbar_anchor(sig, anchor);

        let rect = resolve_pill_rect(&settings, 3, &laptop);
        assert!(rect.y >= 1032 && rect.y + rect.height as i32 <= 1080);
    }

    #[test]
    fn empty_snapshot_round_trips_from_app() {
        let snap = Snapshot {
            accounts: Vec::new(),
            services: Vec::new(),
            generated_at_ms: 1_794_150_000_000,
        };
        let raw = serde_json::to_string(&snap).expect("serialize");
        let restored: Snapshot = serde_json::from_str(&raw).expect("deserialize");
        assert_eq!(restored, snap);
    }

    #[test]
    fn popover_open_grace_prevents_immediate_blur_close() {
        let mut app = test_app(Settings::default());
        app.popover_visible = true;
        app.popover_opened_at = Some(Instant::now());

        let _ = app.update(Message::WindowEvent(app.popover_id, window::Event::Unfocused));
        assert!(app.popover_visible, "within 150ms grace window must stay open");

        app.popover_opened_at = Some(Instant::now() - Duration::from_millis(200));
        let _ = app.update(Message::WindowEvent(app.popover_id, window::Event::Unfocused));
        assert!(!app.popover_visible, "outside grace window must hide cleanly");
    }

    #[test]
    fn tray_quit_and_glass_switching_smoke() {
        let mut app = test_app(Settings::default());
        let _ = app.update(Message::Ui(UiMessage::SetGlassMode(GlassMode::Clear)));
        assert_eq!(app.settings.effective_glass_mode(), GlassMode::Clear);
        let _ = app.update(Message::Ui(UiMessage::SetTheme(
            quotaglass_app::settings::Theme::Light,
        )));
        assert!(!app.is_dark());
        let _ = app.update(Message::Tray(TrayEvent::Quit));
    }

    #[test]
    fn drag_and_grip_release_suppress_spurious_popover_open() {
        let mut app = test_app(Settings::default());

        // Grip drag release followed by MouseArea release must NOT open the popover.
        app.grip_drag = Some(GripDrag {
            start_x_px: 100,
            start_left_x: 100,
            start_center_y: 500,
            start_width_px: 80,
            start_scale: 1.0,
            zoom: 1.0,
            slots: 1,
            monitor_bounds: MonitorRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
        });
        let _ = app.update(Message::MouseReleaseOnWindow(app.pill_id));
        let _ = app.update(Message::Ui(UiMessage::PillSlotReleased(Some(Service::Claude))));
        assert!(
            !app.popover_visible,
            "releasing a resize grip must not open the popover"
        );

        // Dragged pill release followed by MouseArea release must NOT open the popover.
        app.pill_press = Some(PillPress {
            origin_px: Some((100, 100)),
            dragged: true,
        });
        let _ = app.update(Message::MouseReleaseOnWindow(app.pill_id));
        let _ = app.update(Message::Ui(UiMessage::PillSlotReleased(Some(Service::Claude))));
        assert!(
            !app.popover_visible,
            "releasing a dragged pill must not open the popover"
        );

        // Normal click (non-dragged press + release) opens the popover cleanly.
        app.suppress_pill_toggle_until = None;
        app.pill_press = Some(PillPress {
            origin_px: Some((100, 100)),
            dragged: false,
        });
        let _ = app.update(Message::MouseReleaseOnWindow(app.pill_id));
        let _ = app.update(Message::Ui(UiMessage::PillSlotReleased(Some(Service::Claude))));
        assert!(app.popover_visible, "plain click on pill must open the popover");
    }

    #[test]
    fn refresh_now_without_providers_does_not_lock_refreshing() {
        let mut app = test_app(Settings::default());
        let _ = app.update(Message::Ui(UiMessage::RefreshNow));
        assert!(
            !app.ui.pending.refreshing,
            "refreshing flag must stay false when no scheduler/provider tasks exist"
        );
    }

    #[test]
    fn popover_height_clamps_to_compact_and_scaled_work_areas() {
        let compact_work = MonitorRect {
            x: 0,
            y: 0,
            width: 1280,
            height: 720,
        };
        // At 150% DPI (zoom 1.5), 720px physical is 480px logical -> clamped <= 456.0 logical.
        let h_150 = clamp_popover_height_to_work(650.0, compact_work, 1.5);
        assert!((240.0..=456.0).contains(&h_150));

        // At 100% DPI on 1040px work area, 580.0 fits unclipped.
        let desktop_work = MonitorRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1040,
        };
        assert_eq!(clamp_popover_height_to_work(580.0, desktop_work, 1.0), 580.0);

        // Verify flyout placement across narrow (360), target (412), and wide (440) widths.
        let pill = win32::default_pill_rect(desktop_work, win32::pill_px_size(3, 1.0, 1.0), 1.0);
        for w in [360u32, 412u32, 440u32] {
            let (pop, up) = win32::flyout_rect(pill, (pill.width / 2) as i32, w, 580, desktop_work, 1.0);
            assert!(up);
            assert_eq!(pop.width, w);
            assert!(pop.x >= desktop_work.x);
            assert!(pop.x + pop.width as i32 <= desktop_work.x + desktop_work.width as i32);
            assert!(pop.y >= desktop_work.y);
            assert!(pop.y + pop.height as i32 <= pill.y);
        }
    }

    #[test]
    fn drag_threshold_prevents_touch_tap_misfires() {
        const {
            assert!(DRAG_THRESHOLD_PX >= 8);
        }
    }

    proptest! {
        #[test]
        fn stagger_always_stays_within_base(
            index in 0usize..32,
            total in 0usize..32,
            base_secs in 1u64..86_400,
        ) {
            let offset = stagger_offset_secs(index, total, base_secs);
            prop_assert!(offset < base_secs);
            if index == 0 || total <= 1 {
                prop_assert_eq!(offset, 0);
            }
        }
    }
}
