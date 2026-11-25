//! Pure-Rust Iced 0.14 UI layer: state machine, animations, formatting, friendly copy, and views.

pub mod account_card;
pub mod faq;
pub mod icons;
pub mod onboarding;
pub mod pill;
pub mod popover;
pub mod settings;
pub mod tokens;

use crate::connect::UrlTarget;
use crate::settings::{Connection, GlassMode, ProviderTab, Settings, Theme, ToastSound};
use crate::ui::tokens::{primitives as p, ui_font, DesignTokens};
use iced::font;
use iced::widget::{button, container, row, text};
use iced::{Element, Length};
use quotaglass_core::view::{AccountView, CardState, Freshness, Snapshot};
use quotaglass_core::Service;
use std::collections::BTreeMap;

const CONVERGE_EPSILON: f32 = 0.002;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Page {
    #[default]
    List,
    Settings,
    Faq,
    Onboarding,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PendingActions {
    pub refreshing: bool,
    pub adding_service: Option<Service>,
    pub removing_account: Option<String>,
    pub signing_in_account: Option<String>,
}

impl PendingActions {
    pub fn can_refresh(&self) -> bool {
        !self.refreshing
    }

    pub fn can_add(&self) -> bool {
        self.adding_service.is_none()
    }

    pub fn is_removing(&self, account_id: &str) -> bool {
        self.removing_account.as_deref() == Some(account_id)
    }

    pub fn is_signing_in(&self, account_id: &str) -> bool {
        self.signing_in_account.as_deref() == Some(account_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameDraft {
    pub account_id: String,
    pub text: String,
    pub error: Option<&'static str>,
}

#[derive(Debug, Clone)]
pub enum UiMessage {
    PillHovered(bool),
    PillSlotHovered(Option<Service>),
    PillGripHovered(bool),
    PillSlotPressed(Option<Service>),
    PillSlotReleased(Option<Service>),
    PillGripPressed,
    SelectTab(ProviderTab),
    RefreshNow,
    ToggleExpand(String),
    StartRename(String),
    RenameDraftChanged(String),
    SubmitRename,
    CancelRename,
    HideAccount(String),
    RequestRemove(String),
    ConfirmRemove(String),
    CancelRemove,
    SignInAccount(String),
    ToggleAddPicker,
    AddAccount(Service),
    TogglePin,
    OpenPage(Page),
    BackToList,
    OpenUrl(UrlTarget),
    DismissProblem,
    SetTheme(Theme),
    SetGlassMode(GlassMode),
    SetRefreshMinutes(u8),
    SetAlwaysOnTop(bool),
    SetAutostart(bool),
    SetCheckUpdates(bool),
    SetResetToastsEnabled(bool),
    SetResetToastService(Service, bool),
    SetResetToastSound(ToastSound),
    ToggleSource(Connection, bool),
    UnhideAccount(String),
    FinishOnboarding,
}

#[derive(Debug, Clone, PartialEq)]
struct AnimatedScalar {
    current: f32,
    target: f32,
}

impl AnimatedScalar {
    fn new(value: f32) -> Self {
        Self {
            current: value,
            target: value,
        }
    }

    fn snap(&mut self) {
        self.current = self.target;
    }

    fn step(&mut self, alpha: f32) -> bool {
        let diff = self.target - self.current;
        if diff.abs() < CONVERGE_EPSILON {
            self.current = self.target;
            false
        } else {
            self.current += diff * alpha;
            if (self.target - self.current).abs() < CONVERGE_EPSILON {
                self.current = self.target;
                false
            } else {
                true
            }
        }
    }

    fn settled(&self) -> bool {
        (self.target - self.current).abs() < CONVERGE_EPSILON
    }
}

/// 60fps `wgpu` animation state that converges cleanly to idle (zero CPU when settled).
#[derive(Debug, Clone)]
pub struct AnimationState {
    ring_fills: BTreeMap<Service, AnimatedScalar>,
    bar_fills: BTreeMap<String, AnimatedScalar>,
    card_expands: BTreeMap<String, AnimatedScalar>,
    tab_indicator: AnimatedScalar,
    pub shimmer_phase: f32,
    pub pulse_phase: f32,
}

impl Default for AnimationState {
    fn default() -> Self {
        Self {
            ring_fills: BTreeMap::new(),
            bar_fills: BTreeMap::new(),
            card_expands: BTreeMap::new(),
            tab_indicator: AnimatedScalar::new(0.0),
            shimmer_phase: 0.0,
            pulse_phase: 0.0,
        }
    }
}

impl AnimationState {
    pub fn sync_targets(
        &mut self,
        snapshot: &Snapshot,
        tab: ProviderTab,
        expanded_id: Option<&str>,
        animate: bool,
    ) {
        let tab_idx = tab_index(tab) as f32;
        if animate {
            self.tab_indicator.target = tab_idx;
        } else {
            self.tab_indicator = AnimatedScalar::new(tab_idx);
        }

        self.ring_fills
            .retain(|svc, _| snapshot.services.iter().any(|s| s.service == *svc));
        for sv in &snapshot.services {
            let target = sv
                .used_percent
                .map_or(0.0, |v| (v as f32 / 100.0).clamp(0.0, 1.0));
            let entry = self
                .ring_fills
                .entry(sv.service)
                .or_insert_with(|| AnimatedScalar::new(if animate { 0.0 } else { target }));
            entry.target = target;
            if !animate {
                entry.snap();
            }
        }

        let mut active_bars = Vec::new();
        for acct in &snapshot.accounts {
            let expand_target = if expanded_id == Some(acct.id.as_str()) {
                1.0
            } else {
                0.0
            };
            let exp_entry = self
                .card_expands
                .entry(acct.id.clone())
                .or_insert_with(|| AnimatedScalar::new(expand_target));
            exp_entry.target = expand_target;
            if !animate {
                exp_entry.snap();
            }

            for bucket in &acct.buckets {
                let key = bar_key(&acct.id, &bucket.name);
                let target = (bucket.used_percent as f32 / 100.0).clamp(0.0, 1.0);
                let entry = self
                    .bar_fills
                    .entry(key.clone())
                    .or_insert_with(|| AnimatedScalar::new(if animate { 0.0 } else { target }));
                entry.target = target;
                if !animate {
                    entry.snap();
                }
                active_bars.push(key);
            }
        }
        self.bar_fills.retain(|k, _| active_bars.contains(k));
        self.card_expands
            .retain(|id, s| snapshot.accounts.iter().any(|a| &a.id == id) && (s.target > 0.0 || !s.settled()));
    }

    pub fn snap_all(&mut self) {
        self.tab_indicator.snap();
        for s in self.ring_fills.values_mut() {
            s.snap();
        }
        for s in self.bar_fills.values_mut() {
            s.snap();
        }
        for s in self.card_expands.values_mut() {
            s.snap();
        }
        self.card_expands.retain(|_, s| s.target > 0.0);
    }

    /// Advances all active transitions and cyclic phases; returns whether another frame is needed.
    pub fn advance(
        &mut self,
        dt_secs: f32,
        animations_enabled: bool,
        shimmer_active: bool,
        pulse_active: bool,
    ) -> bool {
        if !animations_enabled {
            self.snap_all();
            return false;
        }
        let dt = dt_secs.clamp(0.001, 0.05);
        // Smooth exponential ease-out (~250ms settle).
        let alpha = (1.0 - (-dt * 14.0).exp()).clamp(0.05, 1.0);

        let mut moving = self.tab_indicator.step(alpha);
        for s in self.ring_fills.values_mut() {
            moving |= s.step(alpha);
        }
        for s in self.bar_fills.values_mut() {
            moving |= s.step(alpha);
        }
        for s in self.card_expands.values_mut() {
            moving |= s.step(alpha);
        }
        self.card_expands
            .retain(|_, s| s.target > 0.0 || !s.settled());

        if shimmer_active {
            self.shimmer_phase = (self.shimmer_phase + dt * 0.85).fract();
        } else {
            self.shimmer_phase = 0.0;
        }
        if pulse_active {
            self.pulse_phase = (self.pulse_phase + dt * 0.75).fract();
        } else {
            self.pulse_phase = 0.0;
        }

        moving || shimmer_active || pulse_active
    }

    pub fn is_animating(
        &self,
        animations_enabled: bool,
        pill_visible: bool,
        popover_visible: bool,
        shimmer_active: bool,
        pulse_active: bool,
    ) -> bool {
        if !animations_enabled {
            return false;
        }
        let has_ring_motion = pill_visible && self.ring_fills.values().any(|s| !s.settled());
        let has_popover_motion = popover_visible
            && (!self.tab_indicator.settled()
                || self.bar_fills.values().any(|s| !s.settled())
                || self.card_expands.values().any(|s| !s.settled()));
        let shimmer = popover_visible && shimmer_active;
        let pulse = pill_visible && pulse_active;
        has_ring_motion || has_popover_motion || shimmer || pulse
    }

    pub fn ring_fill(&self, service: Service, fallback: f32) -> f32 {
        self.ring_fills
            .get(&service)
            .map_or(fallback, |s| s.current)
    }

    pub fn bar_fill(&self, account_id: &str, bucket_name: &str, fallback: f32) -> f32 {
        let key = bar_key(account_id, bucket_name);
        self.bar_fills.get(&key).map_or(fallback, |s| s.current)
    }

    pub fn expand_progress(&self, account_id: &str, is_expanded: bool) -> f32 {
        self.card_expands
            .get(account_id)
            .map_or(if is_expanded { 1.0 } else { 0.0 }, |s| s.current)
    }

    pub fn tab_pos(&self) -> f32 {
        self.tab_indicator.current
    }
}

fn bar_key(account_id: &str, bucket_name: &str) -> String {
    format!("{account_id}::{bucket_name}")
}

#[derive(Debug, Clone)]
pub struct UiState {
    pub page: Page,
    pub expanded_id: Option<String>,
    pub add_picker_open: bool,
    pub confirm_remove_id: Option<String>,
    pub rename: Option<RenameDraft>,
    pub problem_banner: Option<String>,
    pub has_snapshot: bool,
    pub pending: PendingActions,
    pub animations: AnimationState,
    pub pill_hovered: bool,
    pub hovered_slot: Option<Service>,
    pub grip_hovered: bool,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            page: Page::List,
            expanded_id: None,
            add_picker_open: false,
            confirm_remove_id: None,
            rename: None,
            problem_banner: None,
            has_snapshot: false,
            pending: PendingActions::default(),
            animations: AnimationState::default(),
            pill_hovered: false,
            hovered_slot: None,
            grip_hovered: false,
        }
    }
}

impl UiState {
    pub fn toggle_expand(&mut self, account_id: &str) {
        if self.expanded_id.as_deref() == Some(account_id) {
            self.expanded_id = None;
            self.confirm_remove_id = None;
            self.rename = None;
        } else {
            self.expanded_id = Some(account_id.to_string());
            self.confirm_remove_id = None;
            self.rename = None;
        }
    }

    pub fn request_remove(&mut self, account_id: &str) {
        if self.pending.is_removing(account_id) {
            return;
        }
        self.rename = None;
        self.confirm_remove_id = Some(account_id.to_string());
    }

    pub fn cancel_remove(&mut self) {
        self.confirm_remove_id = None;
    }

    pub fn start_rename(&mut self, account_id: &str, existing_custom_label: Option<&str>) {
        self.confirm_remove_id = None;
        self.rename = Some(RenameDraft {
            account_id: account_id.to_string(),
            text: existing_custom_label.unwrap_or_default().to_string(),
            error: None,
        });
    }

    pub fn update_rename_draft(&mut self, text: String) {
        if let Some(draft) = &mut self.rename {
            draft.text = text;
            draft.error = None;
        }
    }

    pub fn cancel_rename(&mut self) {
        self.rename = None;
    }

    /// Validates and saves the active rename draft into `settings`.
    /// Returns `Ok(true)` when saved, `Ok(false)` when no draft was active, or `Err(msg)` on invalid input.
    pub fn commit_rename(&mut self, settings: &mut Settings) -> Result<bool, &'static str> {
        let Some(draft) = self.rename.as_mut() else {
            return Ok(false);
        };
        match settings.try_set_label(&draft.account_id, &draft.text) {
            Ok(()) => {
                self.rename = None;
                Ok(true)
            }
            Err(err) => {
                draft.error = Some(err);
                Err(err)
            }
        }
    }

    /// Handles `Esc` inside the popover. Returns `true` if consumed by an inner sub-state/page,
    /// or `false` when the popover itself should hide.
    pub fn step_back_on_escape(&mut self) -> bool {
        if self.rename.is_some() {
            self.rename = None;
            return true;
        }
        if self.confirm_remove_id.is_some() {
            self.confirm_remove_id = None;
            return true;
        }
        if self.add_picker_open {
            self.add_picker_open = false;
            return true;
        }
        if matches!(self.page, Page::Settings | Page::Faq) {
            self.page = Page::List;
            return true;
        }
        false
    }

    pub fn is_skeleton_loading(&self, snapshot: &Snapshot) -> bool {
        !self.has_snapshot && self.pending.refreshing && snapshot.accounts.is_empty()
    }

    pub fn reconcile_accounts(&mut self, accounts: &[AccountView]) {
        if let Some(exp) = &self.expanded_id {
            if !accounts.iter().any(|a| !a.hidden && &a.id == exp) {
                self.expanded_id = None;
                self.confirm_remove_id = None;
                self.rename = None;
            }
        }
        if let Some(rem) = &self.confirm_remove_id {
            if !accounts.iter().any(|a| !a.hidden && &a.id == rem) {
                self.confirm_remove_id = None;
            }
        }
    }
}

pub fn service_name(service: Service) -> &'static str {
    match service {
        Service::Claude => "Claude",
        Service::Codex => "Codex",
        Service::Antigravity => "Antigravity",
    }
}

pub fn service_order_index(service: Service) -> usize {
    match service {
        Service::Claude => 0,
        Service::Codex => 1,
        Service::Antigravity => 2,
    }
}

pub fn connection_name(conn: Connection) -> &'static str {
    match conn {
        Connection::ClaudeCurrent => "Claude login",
        Connection::CodexCurrent => "Codex login",
        Connection::AntigravityApp => "Antigravity app",
    }
}

pub fn tab_index(tab: ProviderTab) -> usize {
    match tab {
        ProviderTab::All => 0,
        ProviderTab::Claude => 1,
        ProviderTab::Codex => 2,
        ProviderTab::Antigravity => 3,
    }
}

pub fn tab_label(tab: ProviderTab) -> &'static str {
    match tab {
        ProviderTab::All => "All",
        ProviderTab::Claude => "Claude",
        ProviderTab::Codex => "Codex",
        ProviderTab::Antigravity => "Antigravity",
    }
}

pub fn tab_for_service(service: Option<Service>) -> ProviderTab {
    match service {
        None => ProviderTab::All,
        Some(Service::Claude) => ProviderTab::Claude,
        Some(Service::Codex) => ProviderTab::Codex,
        Some(Service::Antigravity) => ProviderTab::Antigravity,
    }
}

pub fn tab_matches(tab: ProviderTab, service: Service) -> bool {
    match tab {
        ProviderTab::All => true,
        ProviderTab::Claude => service == Service::Claude,
        ProviderTab::Codex => service == Service::Codex,
        ProviderTab::Antigravity => service == Service::Antigravity,
    }
}

/// Filters hidden accounts and applies the active `ProviderTab` in stable display order.
pub fn visible_accounts_for_tab(accounts: &[AccountView], tab: ProviderTab) -> Vec<&AccountView> {
    let mut list: Vec<&AccountView> = accounts
        .iter()
        .filter(|a| !a.hidden && tab_matches(tab, a.service))
        .collect();
    list.sort_by(|a, b| {
        service_order_index(a.service)
            .cmp(&service_order_index(b.service))
            .then_with(|| b.current_login.cmp(&a.current_login))
            .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
            .then_with(|| a.id.cmp(&b.id))
    });
    list
}

pub fn select_tab_and_persist(
    settings: &mut Settings,
    ui: &mut UiState,
    tab: ProviderTab,
    accounts: &[AccountView],
) {
    settings.active_tab = tab;
    if let Some(exp) = &ui.expanded_id {
        let visible_in_tab = visible_accounts_for_tab(accounts, tab)
            .iter()
            .any(|a| &a.id == exp);
        if !visible_in_tab {
            ui.expanded_id = None;
            ui.confirm_remove_id = None;
            ui.rename = None;
        }
    }
}

pub fn used_whole(used_percent: f64) -> u32 {
    if !used_percent.is_finite() {
        return 0;
    }
    used_percent.clamp(0.0, 100.0).floor() as u32
}

pub fn sentence_case(value: &str) -> String {
    let spaced = value.replace('_', " ");
    let trimmed = spaced.trim();
    let mut chars = trimmed.chars();
    match chars.next() {
        Some(first) => {
            let mut out: String = first.to_uppercase().collect();
            out.push_str(chars.as_str());
            out
        }
        None => String::new(),
    }
}

pub fn forecast_line(mins: Option<u32>) -> Option<String> {
    let mins = mins?;
    if mins == 0 {
        return None;
    }
    if mins < 60 {
        Some(format!("At this pace, limit in about {mins} min."))
    } else {
        let hours = ((f64::from(mins)) / 60.0).round().max(1.0) as u32;
        Some(format!("At this pace, limit in about {hours} h."))
    }
}

pub fn resets_chip_text(count: u32) -> String {
    match count {
        0 => "No resets".to_string(),
        1 => "1 reset".to_string(),
        n => format!("{n} resets"),
    }
}

pub fn sign_in_button_label(account: &AccountView) -> Option<String> {
    if account.service == Service::Antigravity {
        return None;
    }
    let needs = account.state == CardState::LoginExpired
        || (account.state == CardState::NoReadingYet && account.profile_id.is_some());
    if !needs {
        return None;
    }
    if account.profile_id.is_none() {
        Some("Sign in again".to_string())
    } else {
        Some(format!("Open {}", service_name(account.service)))
    }
}

/// Maps card state and core error reasons into friendly, plain-English UI copy.
/// Never surfaces raw HTTP status codes (`HTTP 429`), OS error tokens (`ECONNREFUSED`), or parser details.
pub fn friendly_state_notice(account: &AccountView) -> Option<String> {
    let raw = account.reason.as_deref().unwrap_or("");
    match account.state {
        CardState::Ok => (account.freshness == Freshness::Stale)
            .then(|| "Last check is out of date. Refresh to update.".to_string()),
        CardState::LoginExpired => Some(match account.service {
            Service::Antigravity => "Open Antigravity or T3 Code to refresh.".to_string(),
            _ => "Sign-in expired. Sign in again to refresh.".to_string(),
        }),
        CardState::ProfileMissing => Some(
            "This profile folder could not be found. Remove and add it again.".to_string(),
        ),
        CardState::NoReadingYet => {
            if account.profile_id.is_some() {
                Some("Finish signing in inside the sign-in window.".to_string())
            } else if account.service == Service::Antigravity {
                Some("Open Antigravity or T3 Code to see live limits.".to_string())
            } else {
                Some("Waiting for the first reading.".to_string())
            }
        }
        CardState::NoSource => Some(match account.service {
            Service::Antigravity => {
                "Antigravity is closed. Open Antigravity or T3 Code for live limits.".to_string()
            }
            _ => "No active login found on this PC.".to_string(),
        }),
        CardState::RateLimited => Some(friendly_rate_limit_text(raw, !account.buckets.is_empty())),
        CardState::Error => Some(scrub_raw_error_text(raw)),
    }
}

fn contains_raw_tech_code(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("http")
        || lower.contains("econn")
        || lower.contains("etimedout")
        || lower.contains("429")
        || lower.contains("500")
        || lower.contains("502")
        || lower.contains("503")
        || lower.contains("401")
        || lower.contains("403")
        || lower.contains("404")
        || lower.contains("unexpected data")
}

fn friendly_rate_limit_text(raw: &str, has_reading: bool) -> String {
    if !raw.is_empty() && !contains_raw_tech_code(raw) && raw.starts_with("Rate limited.") {
        return raw.to_string();
    }
    if has_reading {
        "Too many checks right now. Showing the last reading.".to_string()
    } else {
        "Too many checks right now. Try again shortly.".to_string()
    }
}

fn scrub_raw_error_text(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    if lower.contains("401") || lower.contains("403") || lower.contains("expired") {
        "Sign-in could not be verified. Try signing in again.".to_string()
    } else if lower.contains("429") || lower.contains("rate limit") {
        "Too many checks right now. Try again shortly.".to_string()
    } else if lower.contains("reach")
        || lower.contains("econn")
        || lower.contains("timeout")
        || lower.contains("timed out")
        || lower.contains("took too long")
    {
        "Could not reach the service right now.".to_string()
    } else if lower.contains("redirect") || lower.contains("blocked") {
        "Connection was blocked for safety. Check your network.".to_string()
    } else if lower.contains("unexpected data") || lower.contains("format") {
        "Received an unexpected reply from the service. Try refreshing.".to_string()
    } else if !raw.is_empty() && !contains_raw_tech_code(raw) {
        raw.to_string()
    } else {
        "Could not refresh right now. Please try again shortly.".to_string()
    }
}

/// Computes compact logical popover height so `win32::flyout_rect` anchors neatly without clipping.
pub fn popover_logical_height(
    ui: &UiState,
    settings: &Settings,
    snapshot: &Snapshot,
    update_version: Option<&str>,
) -> f32 {
    match ui.page {
        Page::Onboarding => 396.0,
        Page::Faq => 520.0,
        Page::Settings => 580.0,
        Page::List => {
            let mut list_h = if ui.is_skeleton_loading(snapshot) {
                206.0
            } else {
                let visible = visible_accounts_for_tab(&snapshot.accounts, settings.active_tab);
                if visible.is_empty() {
                    98.0
                } else {
                    visible
                        .iter()
                        .map(|a| {
                            let is_exp = ui.expanded_id.as_deref() == Some(a.id.as_str());
                            let notice_h = if friendly_state_notice(a).is_some() {
                                24.0
                            } else {
                                0.0
                            };
                            if is_exp {
                                let buckets = (a.buckets.len().max(1) as f32) * 52.0;
                                let forecast_h = if forecast_line(a.forecast_mins).is_some() {
                                    26.0
                                } else {
                                    0.0
                                };
                                let extra = if ui.rename.as_ref().is_some_and(|r| r.account_id == a.id) {
                                    68.0
                                } else if ui.confirm_remove_id.as_deref() == Some(a.id.as_str()) {
                                    86.0
                                } else {
                                    44.0
                                };
                                let reset_extra = match a.service {
                                    Service::Codex => match a.banked_resets {
                                        Some(n) if n > 0 => 76.0,
                                        Some(_) => 44.0,
                                        None => 0.0,
                                    },
                                    Service::Claude => 46.0,
                                    Service::Antigravity => 0.0,
                                };
                                84.0 + notice_h + buckets + forecast_h + reset_extra + extra
                            } else {
                                92.0 + notice_h
                            }
                        })
                        .sum::<f32>()
                        + (visible.len().saturating_sub(1) as f32 * 10.0)
                        + 10.0
                }
            };
            list_h = list_h.min(p::ACCOUNT_LIST_MAX_H);
            let banner = if ui.problem_banner.is_some() { 48.0 } else { 0.0 };
            let add_picker = if ui.add_picker_open { 92.0 } else { 0.0 };
            let update = if settings.check_updates && update_version.is_some() {
                48.0
            } else {
                0.0
            };
            let header = 74.0;
            let footer = 50.0;
            let outer_pad = 24.0;
            (outer_pad + banner + header + list_h + add_picker + update + footer).clamp(230.0, 650.0)
        }
    }
}

/// Shared segmented option pill row used across Settings.
pub fn segmented_row<'a, T: Copy + PartialEq + 'a>(
    tokens: DesignTokens,
    choices: &[(T, &'static str)],
    selected: T,
    on_pick: impl Fn(T) -> UiMessage,
) -> Element<'a, UiMessage> {
    let mut r = row![].spacing(4.0);
    for &(val, label) in choices {
        let active = val == selected;
        r = r.push(
            button(
                container(
                    text(label)
                        .size(p::TEXT_SM)
                        .font(ui_font(if active {
                            font::Weight::Semibold
                        } else {
                            font::Weight::Normal
                        })),
                )
                .center_y(Length::Fill),
            )
            .padding([0.0, 10.0])
            .height(Length::Fixed(36.0))
            .style(move |_, status| tokens.segmented_button_style(active, status))
            .on_press(on_pick(val)),
        );
    }
    r.into()
}

pub fn format_first_run_flag(settings: &Settings) -> bool {
    settings.connected.is_empty() && settings.added_accounts.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use quotaglass_core::view::BucketView;
    use quotaglass_core::SourceKind;

    fn sample_bucket(name: &str, used: f64, fast_burn: bool) -> BucketView {
        BucketView {
            name: name.to_string(),
            used_percent: used,
            window_secs: Some(18_000),
            resets_at_ms: Some(1_800_005_000_000),
            limit_reached: used >= 100.0,
            freshness: Freshness::Live,
            passed_percent: Some(30.0),
            fast_burn,
        }
    }

    fn sample_account(
        id: &str,
        service: Service,
        label: &str,
        state: CardState,
        reason: Option<&str>,
        profile_id: Option<&str>,
    ) -> AccountView {
        let b = sample_bucket("5-hour", 42.0, false);
        AccountView {
            id: id.to_string(),
            service,
            label: label.to_string(),
            plan: Some("max_5x".to_string()),
            source: Some(SourceKind::Direct),
            freshness: Freshness::Live,
            age_secs: Some(12),
            headline: Some(b.clone()),
            buckets: vec![b],
            state,
            reason: reason.map(str::to_string),
            room_left: Some(58.0),
            forecast_mins: Some(40),
            best_now: false,
            banked_resets: (service == Service::Codex).then_some(2),
            profile_id: profile_id.map(str::to_string),
            current_login: profile_id.is_none(),
            hidden: false,
        }
    }

    #[test]
    fn friendly_error_mapping_never_leaks_raw_http_or_socket_codes() {
        let cases = [
            (CardState::Error, Some("The service answered HTTP 429")),
            (CardState::Error, Some("The service answered HTTP 502")),
            (CardState::Error, Some("The service answered HTTP 401")),
            (CardState::Error, Some("The service answered HTTP 403")),
            (CardState::Error, Some("ECONNREFUSED 127.0.0.1:4200")),
            (CardState::Error, Some("ETIMEDOUT connecting to host")),
            (CardState::Error, Some("Unexpected data from the source (usage)")),
            (CardState::RateLimited, Some("HTTP 429 Too Many Requests")),
            (CardState::RateLimited, Some("Rate limited. Retrying in 5 min.")),
            (CardState::LoginExpired, Some("HTTP 401")),
            (CardState::NoSource, Some("Not found")),
            (CardState::ProfileMissing, Some("missing")),
        ];

        for (state, raw) in cases {
            let acct = sample_account("claude:a", Service::Claude, "Work", state, raw, None);
            let notice = friendly_state_notice(&acct).expect("notice present");
            let lower = notice.to_ascii_lowercase();
            assert!(!lower.contains("http"), "leaked HTTP in '{notice}'");
            assert!(!lower.contains("429"), "leaked 429 in '{notice}'");
            assert!(!lower.contains("502"), "leaked 502 in '{notice}'");
            assert!(!lower.contains("401"), "leaked 401 in '{notice}'");
            assert!(!lower.contains("403"), "leaked 403 in '{notice}'");
            assert!(!lower.contains("econnrefused"), "leaked ECONNREFUSED in '{notice}'");
            assert!(!lower.contains("etimedout"), "leaked ETIMEDOUT in '{notice}'");
        }
    }

    #[test]
    fn tab_filtering_and_immediate_persistence() {
        let accounts = vec![
            sample_account("c1", Service::Claude, "Work", CardState::Ok, None, None),
            sample_account("x1", Service::Codex, "Personal", CardState::Ok, None, None),
            sample_account("a1", Service::Antigravity, "IDE", CardState::Ok, None, None),
            {
                let mut h = sample_account("c2", Service::Claude, "Hidden", CardState::Ok, None, None);
                h.hidden = true;
                h
            },
        ];
        let mut settings = Settings::default();
        let mut ui = UiState {
            expanded_id: Some("c1".to_string()),
            ..Default::default()
        };

        assert_eq!(visible_accounts_for_tab(&accounts, ProviderTab::All).len(), 3);

        select_tab_and_persist(&mut settings, &mut ui, ProviderTab::Codex, &accounts);
        assert_eq!(settings.active_tab, ProviderTab::Codex);
        let codex = visible_accounts_for_tab(&accounts, settings.active_tab);
        assert_eq!(codex.len(), 1);
        assert_eq!(codex[0].service, Service::Codex);
        assert_eq!(ui.expanded_id, None, "expanded card outside new tab collapses");

        select_tab_and_persist(&mut settings, &mut ui, ProviderTab::Antigravity, &accounts);
        assert_eq!(settings.active_tab, ProviderTab::Antigravity);
        assert_eq!(
            visible_accounts_for_tab(&accounts, settings.active_tab)[0].service,
            Service::Antigravity
        );

        select_tab_and_persist(&mut settings, &mut ui, ProviderTab::Claude, &accounts);
        assert_eq!(visible_accounts_for_tab(&accounts, settings.active_tab).len(), 1);
    }

    #[test]
    fn rename_validation_in_ui_state_matches_settings_rules() {
        let mut settings = Settings::default();
        let mut ui = UiState::default();

        ui.start_rename("acct-1", None);
        ui.update_rename_draft("  Adyr's Claude  ".to_string());
        assert_eq!(ui.commit_rename(&mut settings), Ok(true));
        assert_eq!(
            settings.labels.get("acct-1").map(String::as_str),
            Some("Adyr's Claude")
        );
        assert!(ui.rename.is_none());

        // Over 40 chars rejected with inline error, keeping editor open.
        ui.start_rename("acct-1", Some("Adyr's Claude"));
        ui.update_rename_draft("x".repeat(41));
        assert!(ui.commit_rename(&mut settings).is_err());
        assert!(ui.rename.as_ref().and_then(|r| r.error).is_some());

        // Control chars rejected.
        ui.update_rename_draft("bad\nname".to_string());
        assert!(ui.commit_rename(&mut settings).is_err());
        assert!(ui.rename.as_ref().and_then(|r| r.error).is_some());

        // Blank clears custom label.
        ui.update_rename_draft("   ".to_string());
        assert_eq!(ui.commit_rename(&mut settings), Ok(true));
        assert!(!settings.labels.contains_key("acct-1"));
    }

    #[test]
    fn two_step_remove_confirmation_and_action_locking() {
        let mut ui = UiState::default();
        ui.request_remove("prof-1");
        assert_eq!(ui.confirm_remove_id.as_deref(), Some("prof-1"));

        // Esc cancels confirmation before hiding popover.
        assert!(ui.step_back_on_escape());
        assert_eq!(ui.confirm_remove_id, None);
        assert!(!ui.step_back_on_escape());

        // Action locking prevents duplicate refresh/add/remove/sign-in.
        assert!(ui.pending.can_refresh());
        ui.pending.refreshing = true;
        assert!(!ui.pending.can_refresh());

        assert!(ui.pending.can_add());
        ui.pending.adding_service = Some(Service::Claude);
        assert!(!ui.pending.can_add());

        ui.pending.removing_account = Some("prof-1".to_string());
        assert!(ui.pending.is_removing("prof-1"));
        ui.request_remove("prof-1");
        assert_eq!(ui.confirm_remove_id, None, "locked while removal runs");

        ui.pending.signing_in_account = Some("prof-2".to_string());
        assert!(ui.pending.is_signing_in("prof-2"));
    }

    #[test]
    fn animations_converge_to_idle_with_zero_cpu_and_respect_os_setting() {
        let mut anim = AnimationState::default();
        let snap = Snapshot {
            accounts: vec![sample_account(
                "c1",
                Service::Claude,
                "Work",
                CardState::Ok,
                None,
                None,
            )],
            services: vec![quotaglass_core::view::ServiceView {
                service: Service::Claude,
                used_percent: Some(42.0),
                all_exhausted: false,
                accounts: 1,
                rate_limited: false,
            }],
            generated_at_ms: 1_800_000_000_000,
        };

        anim.sync_targets(&snap, ProviderTab::Codex, Some("c1"), true);
        assert!(anim.is_animating(true, true, true, false, false));

        for _ in 0..80 {
            if !anim.advance(1.0 / 60.0, true, false, false) {
                break;
            }
        }
        assert!(
            !anim.is_animating(true, true, true, false, false),
            "animation targets must settle below epsilon"
        );
        assert!((anim.ring_fill(Service::Claude, 0.0) - 0.42).abs() < 0.002);
        assert!((anim.bar_fill("c1", "5-hour", 0.0) - 0.42).abs() < 0.002);
        assert!((anim.expand_progress("c1", true) - 1.0).abs() < 0.002);
        assert!((anim.tab_pos() - 2.0).abs() < 0.002);

        // Disabled animations snap immediately -> never animate.
        anim.sync_targets(&snap, ProviderTab::Antigravity, None, false);
        assert!(!anim.is_animating(false, true, true, true, true));
        assert_eq!(anim.tab_pos(), 3.0);
    }

    #[test]
    fn skeleton_loading_only_before_first_snapshot() {
        let mut ui = UiState::default();
        let empty = Snapshot {
            accounts: Vec::new(),
            services: Vec::new(),
            generated_at_ms: 0,
        };
        assert!(!ui.is_skeleton_loading(&empty));
        ui.pending.refreshing = true;
        assert!(ui.is_skeleton_loading(&empty));

        ui.has_snapshot = true;
        assert!(!ui.is_skeleton_loading(&empty));
    }

    #[test]
    fn formatting_helpers_match_ux_copy() {
        assert_eq!(used_whole(79.9), 79);
        assert_eq!(used_whole(105.0), 100);
        assert_eq!(used_whole(-2.0), 0);
        assert_eq!(sentence_case("max_5x"), "Max 5x");
        assert_eq!(
            forecast_line(Some(40)).as_deref(),
            Some("At this pace, limit in about 40 min.")
        );
        assert_eq!(
            forecast_line(Some(120)).as_deref(),
            Some("At this pace, limit in about 2 h.")
        );
        assert_eq!(resets_chip_text(0), "No resets");
        assert_eq!(resets_chip_text(1), "1 reset");
        assert_eq!(resets_chip_text(2), "2 resets");
    }

    #[test]
    fn popover_height_accounts_for_expanded_buckets_notices_and_forecasts() {
        let mut antigravity = sample_account(
            "ag-1",
            Service::Antigravity,
            "Antigravity",
            CardState::Ok,
            None,
            None,
        );
        antigravity.buckets = vec![
            sample_bucket("Gemini Weekly", 49.0, true),
            sample_bucket("Gemini 5-hour", 96.0, true),
            sample_bucket("Claude Weekly", 99.0, false),
            sample_bucket("Claude 5-hour", 0.0, false),
        ];
        antigravity.forecast_mins = Some(5);

        let mut claude = sample_account(
            "cl-1",
            Service::Claude,
            "Claude",
            CardState::RateLimited,
            Some("Rate limited. Retrying in 5 min."),
            None,
        );
        claude.buckets = vec![
            sample_bucket("5 hour", 7.0, false),
            sample_bucket("Weekly", 84.0, true),
        ];

        let snap = Snapshot {
            accounts: vec![claude, antigravity],
            services: Vec::new(),
            generated_at_ms: 1_800_000_000_000,
        };
        let settings = Settings::default();
        let mut ui = UiState::default();

        let collapsed_h = popover_logical_height(&ui, &settings, &snap, None);
        ui.expanded_id = Some("cl-1".to_string());
        let claude_exp_h = popover_logical_height(&ui, &settings, &snap, None);
        assert!(claude_exp_h > collapsed_h + 120.0);

        ui.expanded_id = Some("ag-1".to_string());
        let ag_exp_h = popover_logical_height(&ui, &settings, &snap, None);
        assert!(ag_exp_h >= 500.0);
    }
}
