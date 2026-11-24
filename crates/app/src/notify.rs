//! Native Windows toast notifications via `tauri-winrt-notification`:
//! 1) Usage warnings at 50, 80, and 90% (`warnings.json`, `quotaglass_core::warnings`).
//! 2) Opt-in reset reminders and Codex banked-reset gains (`resets.json`, `quotaglass_core::resets`).
//!
//! No emails, tokens, or secrets ever enter toast titles or bodies.

use crate::scheduler::lock;
use crate::settings::{ResetToasts, ToastSound};
use crate::state::{now_utc_ms, JsonStore};
use quotaglass_core::resets::{self, Due, Gain, ResetState, Switches};
use quotaglass_core::view::Snapshot;
use quotaglass_core::warnings::{self, WarnState};
use quotaglass_core::Service;
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::task::JoinHandle;

pub const APP_AUMID: &str = "Adyr27.QuotaGlass";
pub const RESET_TICK: Duration = Duration::from_secs(30);

const RESET_WAITING: &str = "A reset is available. Check what it restores before you use it.";

#[derive(Debug, Clone, PartialEq)]
pub struct Alert {
    pub title: String,
    pub body: String,
    pub level: u8,
}

pub fn provider_name(service: Service) -> &'static str {
    match service {
        Service::Claude => "Claude",
        Service::Codex => "Codex",
        Service::Antigravity => "Antigravity",
    }
}

/// Computes due 50/80/90% usage alerts for `snapshot`. Hidden accounts never alert.
pub fn check(snapshot: &Snapshot, state: &mut WarnState) -> Vec<Alert> {
    check_at(snapshot, state, warnings::local_offset_secs())
}

pub fn check_at(snapshot: &Snapshot, state: &mut WarnState, offset_secs: Option<i32>) -> Vec<Alert> {
    if snapshot.accounts.is_empty() {
        return Vec::new();
    }
    let now_ms = snapshot.generated_at_ms;
    let mut present = BTreeSet::new();
    let mut live = BTreeSet::new();
    let mut alerts = Vec::new();
    for account in &snapshot.accounts {
        for bucket in &account.buckets {
            let key = warnings::key(&account.id, &bucket.name);
            if !account.hidden {
                if let Some(level) =
                    warnings::due(state, &key, bucket.used_percent, bucket.resets_at_ms)
                {
                    let resets = bucket
                        .resets_at_ms
                        .and_then(|r| warnings::reset_text(now_ms, r, offset_secs))
                        .map(|t| format!(" — resets {t}"))
                        .unwrap_or_default();
                    let banked = if level >= 90 && account.banked_resets.is_some_and(|n| n > 0) {
                        format!("\n{RESET_WAITING}")
                    } else {
                        String::new()
                    };
                    alerts.push(Alert {
                        title: format!("{} · {}", provider_name(account.service), account.label),
                        body: format!(
                            "{} at {}%{resets}{banked}",
                            bucket.name,
                            bucket.used_percent.floor()
                        ),
                        level,
                    });
                }
            }
            live.insert(key);
        }
        if !account.buckets.is_empty() {
            present.insert(account.id.clone());
        }
    }
    warnings::prune(state, &present, &live, now_ms);
    alerts
}

fn switches(t: &ResetToasts) -> Switches {
    Switches {
        claude: t.claude,
        codex: t.codex,
        antigravity: t.antigravity,
    }
}

/// Tracks future resets from `snapshot`, or pops due resets when `snapshot` is `None`.
/// Writes nothing when `toasts.enabled` is false.
pub fn step(
    state: &mut ResetState,
    toasts: &ResetToasts,
    snapshot: Option<&Snapshot>,
    now_ms: i64,
) -> (Vec<Due>, bool) {
    if !toasts.enabled {
        return (Vec::new(), false);
    }
    let before = state.clone();
    let on = switches(toasts);
    let due = match snapshot {
        Some(s) => {
            resets::track(state, s, &on);
            Vec::new()
        }
        None => {
            state.0.retain(|_, e| on.on(e.service));
            resets::take_due(state, now_ms)
        }
    };
    let changed = *state != before;
    (due, changed)
}

/// Detects Codex banked-reset count increases. Writes nothing when `toasts.enabled` is false.
pub fn step_banked(
    state: &mut ResetState,
    toasts: &ResetToasts,
    snapshot: &Snapshot,
) -> (Vec<Gain>, bool) {
    if !toasts.enabled {
        return (Vec::new(), false);
    }
    let before = state.clone();
    let gains = resets::track_banked(state, snapshot, &switches(toasts));
    (gains, *state != before)
}

pub fn gain_text(gain: &Gain) -> (String, String) {
    (
        format!(
            "{}: a limit reset is available.",
            provider_name(gain.service)
        ),
        gain.label.clone(),
    )
}

pub fn toast_text(due: &Due) -> (String, String) {
    let title = format!("{} · {} reset", provider_name(due.service), due.label);
    let body = match due.buckets.as_slice() {
        [one] => format!("{one} limit reset — start a session"),
        many => format!("{} — start a session", many.join(", ")),
    };
    (title, body)
}

pub type ToastActivatedCb = Arc<dyn Fn(Option<Service>) + Send + Sync>;

#[derive(Debug, Clone)]
struct PendingToast {
    title: String,
    body: String,
    play_sound: bool,
    service: Option<Service>,
}

#[cfg(windows)]
fn dispatch_toast_blocking(item: PendingToast, on_activated: Option<ToastActivatedCb>) {
    use tauri_winrt_notification::{Sound, Toast};

    let build = |app_id: &str| {
        let mut toast = Toast::new(app_id)
            .title(&item.title)
            .text1(&item.body)
            .sound(item.play_sound.then_some(Sound::Default));
        if let Some(cb) = on_activated.clone() {
            let service = item.service;
            toast = toast.on_activated(move |_| {
                cb(service);
                Ok(())
            });
        }
        toast.show()
    };

    if build(APP_AUMID).is_err() {
        if let Err(e) = build(Toast::POWERSHELL_APP_ID) {
            tracing::warn!(error = ?e, "native Windows toast failed");
        }
    }
}

#[cfg(not(windows))]
fn dispatch_toast_blocking(_item: PendingToast, _on_activated: Option<ToastActivatedCb>) {}

fn dispatch_toasts(items: Vec<PendingToast>, on_activated: Option<ToastActivatedCb>) {
    if items.is_empty() {
        return;
    }
    let work = move || {
        for item in items {
            dispatch_toast_blocking(item, on_activated.clone());
        }
    };
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn_blocking(work);
    } else {
        work();
    }
}

pub struct Warnings {
    pub state: Mutex<WarnState>,
    pub store: JsonStore,
}

impl Warnings {
    pub fn new(store: JsonStore) -> Self {
        Self {
            state: Mutex::new(store.load()),
            store,
        }
    }

    pub fn on_snapshot(&self, snapshot: &Snapshot, on_activated: Option<ToastActivatedCb>) {
        let alerts = {
            let mut state = lock(&self.state);
            let before = state.clone();
            let alerts = check(snapshot, &mut state);
            if *state != before {
                if let Err(e) = self.store.save(&*state) {
                    tracing::warn!(error = %e, "could not save warnings.json");
                }
            }
            alerts
        };
        let batch = alerts
            .into_iter()
            .map(|a| PendingToast {
                title: a.title,
                body: a.body,
                play_sound: a.level >= 90,
                service: None,
            })
            .collect();
        dispatch_toasts(batch, on_activated);
    }
}

pub struct Resets {
    state: Mutex<ResetState>,
    store: JsonStore,
    toasts: Mutex<ResetToasts>,
}

impl Resets {
    pub fn new(store: JsonStore, toasts: ResetToasts) -> Self {
        Self {
            state: Mutex::new(store.load()),
            store,
            toasts: Mutex::new(toasts),
        }
    }

    pub fn set_toasts(&self, toasts: &ResetToasts) {
        *lock(&self.toasts) = toasts.clone();
    }

    pub fn pass(
        &self,
        snapshot: Option<&Snapshot>,
        now_ms: i64,
    ) -> (Vec<Due>, Vec<Gain>, ToastSound) {
        let toasts = lock(&self.toasts).clone();
        let mut state = lock(&self.state);
        let (due, mut changed) = step(&mut state, &toasts, snapshot, now_ms);
        let mut gains = Vec::new();
        if let Some(s) = snapshot {
            let (g, c) = step_banked(&mut state, &toasts, s);
            gains = g;
            changed |= c;
        }
        if changed {
            if let Err(e) = self.store.save(&*state) {
                tracing::warn!(error = %e, "could not save resets.json");
            }
        }
        (due, gains, toasts.sound)
    }

    pub fn on_snapshot(&self, snapshot: &Snapshot, on_activated: Option<ToastActivatedCb>) {
        self.run_pass(Some(snapshot), now_utc_ms(), on_activated);
    }

    pub fn poll_due(&self, on_activated: Option<ToastActivatedCb>) {
        self.run_pass(None, now_utc_ms(), on_activated);
    }

    fn run_pass(
        &self,
        snapshot: Option<&Snapshot>,
        now_ms: i64,
        on_activated: Option<ToastActivatedCb>,
    ) {
        let (due, gains, sound) = self.pass(snapshot, now_ms);
        let play_sound = sound == ToastSound::Default;
        let mut batch = Vec::with_capacity(due.len() + gains.len());
        for d in &due {
            let (title, body) = toast_text(d);
            batch.push(PendingToast {
                title,
                body,
                play_sound,
                service: Some(d.service),
            });
        }
        for g in &gains {
            let (title, body) = gain_text(g);
            batch.push(PendingToast {
                title,
                body,
                play_sound,
                service: Some(g.service),
            });
        }
        dispatch_toasts(batch, on_activated);
    }
}

pub fn start_reset_timer(
    resets: Arc<Resets>,
    on_activated: Option<ToastActivatedCb>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(RESET_TICK);
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            timer.tick().await;
            resets.poll_due(on_activated.clone());
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use quotaglass_core::view::{AccountView, BucketView, CardState, Freshness};

    const NOW: i64 = 1_700_000_000_000;

    fn bucket(name: &str, used: f64, resets_at_ms: Option<i64>) -> BucketView {
        BucketView {
            name: name.into(),
            used_percent: used,
            window_secs: Some(18_000),
            resets_at_ms,
            limit_reached: false,
            freshness: Freshness::Live,
            passed_percent: None,
            fast_burn: false,
        }
    }

    fn account(id: &str, service: Service, label: &str, buckets: Vec<BucketView>) -> AccountView {
        AccountView {
            id: id.into(),
            service,
            label: label.into(),
            plan: None,
            source: None,
            freshness: Freshness::Live,
            age_secs: None,
            headline: None,
            buckets,
            state: CardState::Ok,
            reason: None,
            room_left: None,
            forecast_mins: None,
            best_now: false,
            banked_resets: None,
            profile_id: None,
            current_login: true,
            hidden: false,
        }
    }

    fn snap(accounts: Vec<AccountView>) -> Snapshot {
        Snapshot {
            services: quotaglass_core::summary::build(&accounts),
            accounts,
            generated_at_ms: NOW,
        }
    }

    #[test]
    fn warnings_fire_once_with_expected_copy_and_no_secrets() {
        let s = snap(vec![
            account(
                "claude:person@example.com",
                Service::Claude,
                "Work",
                vec![bucket("5-hour", 81.4, Some(NOW + 125 * 60_000))],
            ),
            account(
                "codex:other@example.com",
                Service::Codex,
                "Personal",
                vec![bucket("Weekly", 92.0, None)],
            ),
        ]);
        let mut s = s;
        s.accounts[1].banked_resets = Some(2);

        let mut state = WarnState::default();
        let alerts = check_at(&s, &mut state, Some(0));
        assert_eq!(alerts.len(), 2);
        assert_eq!(
            alerts[0],
            Alert {
                title: "Claude · Work".into(),
                body: "5-hour at 81% — resets in 2 h 5 min".into(),
                level: 80,
            }
        );
        assert_eq!(
            alerts[1],
            Alert {
                title: "Codex · Personal".into(),
                body: format!("Weekly at 92%\n{RESET_WAITING}"),
                level: 90,
            }
        );
        for a in &alerts {
            assert!(!a.title.contains('@') && !a.body.contains('@'));
            assert!(!a.body.to_lowercase().contains("token"));
        }
        assert!(check_at(&s, &mut state, Some(0)).is_empty());
    }

    #[test]
    fn warnings_skip_hidden_and_persist_atomically() {
        let dir = std::env::temp_dir().join(format!("qg-app-warn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = JsonStore::new(dir.join("warnings.json"));
        let mut hidden = account(
            "codex:hidden",
            Service::Codex,
            "Hidden",
            vec![bucket("weekly", 95.0, None)],
        );
        hidden.hidden = true;
        let mut state = WarnState::default();
        assert!(check_at(&snap(vec![hidden]), &mut state, Some(0)).is_empty());
        warnings::due(&mut state, "a|5h", 85.0, Some(NOW));
        store.save(&state).unwrap();
        assert_eq!(store.load::<WarnState>(), state);
        assert!(!dir.join("warnings.json.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reset_and_banked_step_respect_opt_in_and_hide_emails() {
        let dir = std::env::temp_dir().join(format!("qg-app-resets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let resets = Resets::new(
            JsonStore::new(dir.join("resets.json")),
            ResetToasts::default(),
        );

        let s = snap(vec![account(
            "claude:person@example.com",
            Service::Claude,
            "Adyr's Claude",
            vec![bucket("5-hour", 40.0, Some(NOW + 60_000))],
        )]);
        assert!(resets.pass(Some(&s), NOW).0.is_empty());
        assert!(!dir.join("resets.json").exists(), "opt-out writes nothing");

        resets.set_toasts(&ResetToasts {
            enabled: true,
            ..ResetToasts::default()
        });
        resets.pass(Some(&s), NOW);
        let (due, _, sound) = resets.pass(None, NOW + 61_000);
        assert_eq!(due.len(), 1);
        assert_eq!(sound, ToastSound::Default);
        let (title, body) = toast_text(&due[0]);
        assert_eq!(title, "Claude · Adyr's Claude reset");
        assert_eq!(body, "5-hour limit reset — start a session");
        assert!(!title.contains('@') && !body.contains('@'));

        let mut codex = snap(vec![account(
            "codex:person@example.com",
            Service::Codex,
            "Work Codex",
            vec![],
        )]);
        codex.accounts[0].banked_resets = Some(0);
        assert!(resets.pass(Some(&codex), NOW).1.is_empty());
        codex.accounts[0].banked_resets = Some(1);
        let (_, gains, _) = resets.pass(Some(&codex), NOW);
        assert_eq!(gains.len(), 1);
        let (g_title, g_body) = gain_text(&gains[0]);
        assert_eq!(g_title, "Codex: a limit reset is available.");
        assert_eq!(g_body, "Work Codex");
        assert!(!g_title.contains('@') && !g_body.contains('@'));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
