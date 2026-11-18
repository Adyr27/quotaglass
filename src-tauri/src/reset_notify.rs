//! Reset reminders: one toast per account when its limits reset. Entries live in `%APPDATA%\QuotaGlass\resets.json`.
//! Toast text holds the provider, the account label and bucket names. No emails, no tokens.

use crate::notify::{provider_name, JsonStore};
use crate::settings::{ResetToasts, ToastSound};
use quotaglass_core::resets::{self, Due, Gain, ResetState, Switches};
use quotaglass_core::view::Snapshot;
use quotaglass_core::Service;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

/// How often due resets are checked.
pub const TICK: Duration = Duration::from_secs(30);

/// App state: the tracked resets, their file, and a copy of the reset switches.
///
/// The copy exists because a pass runs inside the scheduler's publish, and `commands::update` calls the scheduler
/// while it holds the settings lock. Reading the settings here would lock them twice on one thread: a deadlock.
/// Lock order: settings, then `toasts`. Nothing holds `toasts` while it takes another lock.
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

    /// Takes the new switches. Called on each settings save, before the scheduler republishes.
    pub fn set_toasts(&self, toasts: &ResetToasts) {
        *crate::scheduler::lock(&self.toasts) = toasts.clone();
    }

    /// One pass: tracks or takes due resets, and with a snapshot also finds new banked resets.
    /// Saves the file when it changed. Takes no settings lock.
    fn pass(&self, snapshot: Option<&Snapshot>, now_ms: i64) -> (Vec<Due>, Vec<Gain>, ToastSound) {
        let toasts = crate::scheduler::lock(&self.toasts).clone();
        let mut state = crate::scheduler::lock(&self.state);
        let (due, mut changed) = step(&mut state, &toasts, snapshot, now_ms);
        let mut gains = Vec::new();
        if let Some(s) = snapshot {
            let (g, c) = step_banked(&mut state, &toasts, s);
            gains = g;
            changed |= c;
        }
        if changed {
            if let Err(e) = self.store.save(&*state) {
                tracing::warn!(error = ?e, "could not save resets.json");
            }
        }
        (due, gains, toasts.sound)
    }
}

/// Copies the saved switches into the reminder state. Call it with the settings lock held, before the scheduler.
pub fn set_toasts(app: &AppHandle, toasts: &ResetToasts) {
    if let Some(resets) = app.try_state::<Resets>() {
        resets.set_toasts(toasts);
    }
}

fn switches(t: &ResetToasts) -> Switches {
    Switches {
        claude: t.claude,
        codex: t.codex,
        antigravity: t.antigravity,
    }
}

/// One pass. With a snapshot it tracks reset times; without one (the timer) it takes what is due.
/// With the master switch off it does nothing, so `resets.json` is never written. Returns the due
/// toasts and whether the state changed (the caller saves only then).
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
            // A service switched off since the last snapshot must not fire before the next one.
            state.0.retain(|_, e| on.on(e.service));
            resets::take_due(state, now_ms)
        }
    };
    let changed = *state != before;
    (due, changed)
}

/// Finds accounts whose banked reset count went up. Same switches as `step`: with the master switch off it
/// does nothing and stores nothing. Returns the gains and whether the state changed.
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

/// Title and body of the toast for a new banked reset. The body is the account label, so two Codex logins differ.
pub fn gain_text(gain: &Gain) -> (String, String) {
    (
        format!(
            "{}: a limit reset is available.",
            provider_name(gain.service)
        ),
        gain.label.clone(),
    )
}

/// Title and body of one reset toast.
pub fn toast_text(due: &Due) -> (String, String) {
    let title = format!("{} · {} reset", provider_name(due.service), due.label);
    let body = match due.buckets.as_slice() {
        [one] => format!("{one} limit reset — start a session"),
        many => format!("{} — start a session", many.join(", ")),
    };
    (title, body)
}

/// The toast id tells a click which service it was for. Usage warnings keep id 0.
fn toast_id(service: Service) -> i32 {
    match service {
        Service::Claude => 1,
        Service::Codex => 2,
        Service::Antigravity => 3,
    }
}

/// The service of a clicked toast, or `None` for a usage warning.
pub fn service_of_toast(id: i32) -> Option<Service> {
    match id {
        1 => Some(Service::Claude),
        2 => Some(Service::Codex),
        3 => Some(Service::Antigravity),
        _ => None,
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn run(app: &AppHandle, snapshot: Option<&Snapshot>) {
    let Some(resets) = app.try_state::<Resets>() else {
        return;
    };
    let (due, gains, sound) = resets.pass(snapshot, now_ms());
    for d in &due {
        send(app, toast_text(d), d.service, sound);
    }
    for g in &gains {
        send(app, gain_text(g), g.service, sound);
    }
}

/// Runs after each scheduler publish: tracks the reset times in the snapshot.
pub fn on_snapshot(app: &AppHandle, snapshot: &Snapshot) {
    run(app, Some(snapshot));
}

fn send(app: &AppHandle, (title, body): (String, String), service: Service, sound: ToastSound) {
    let mut toast = app
        .notification()
        .builder()
        .id(toast_id(service))
        .title(title)
        .body(body);
    if sound == ToastSound::Default {
        toast = toast.sound("Default");
    }
    if let Err(e) = toast.show() {
        tracing::warn!(error = ?e, "reset toast failed");
    }
}

/// Registers the toast click handler, then checks due resets every [`TICK`]. The first tick runs at once,
/// so a reset missed while the app was closed (within the grace) fires at startup.
pub fn start(app: &AppHandle) {
    let handle = app.clone();
    // Windows reports a click only while the toast is on screen. Registered before the first toast.
    let _ = app.notification().on_action(move |performed| {
        if performed.action_id() != "tap" {
            return;
        }
        let service = performed
            .notification()
            .and_then(|n| service_of_toast(n.id()));
        let h = handle.clone();
        let _ = handle.run_on_main_thread(move || {
            crate::window::show(&h);
            crate::flyout::open_popover(&h, service, None);
        });
    });
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut timer = tokio::time::interval(TICK);
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            timer.tick().await;
            run(&handle, None);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use quotaglass_core::view::{AccountView, BucketView, CardState, Freshness};

    const NOW: i64 = 1_800_000_000_000;

    fn due(buckets: &[&str]) -> Due {
        Due {
            service: Service::Claude,
            account_id: "claude:0123456789abcdef".into(),
            label: "Work".into(),
            buckets: buckets.iter().map(|b| (*b).to_string()).collect(),
        }
    }

    fn snap(resets_at_ms: i64) -> Snapshot {
        let bucket = BucketView {
            name: "5-hour".into(),
            used_percent: 40.0,
            window_secs: Some(18_000),
            resets_at_ms: Some(resets_at_ms),
            limit_reached: false,
            freshness: Freshness::Live,
            passed_percent: None,
            fast_burn: false,
        };
        let accounts = vec![AccountView {
            id: "claude:x".into(),
            service: Service::Claude,
            label: "Claude".into(),
            plan: None,
            source: None,
            freshness: Freshness::Live,
            age_secs: None,
            headline: Some(bucket.clone()),
            buckets: vec![bucket],
            state: CardState::Ok,
            reason: None,
            room_left: None,
            forecast_mins: None,
            best_now: false,
            banked_resets: None,
            profile_id: None,
            current_login: true,
            hidden: false,
        }];
        Snapshot {
            services: quotaglass_core::summary::build(&accounts),
            accounts,
            generated_at_ms: NOW,
        }
    }

    fn on() -> ResetToasts {
        ResetToasts {
            enabled: true,
            ..ResetToasts::default()
        }
    }

    #[test]
    fn toast_text_single_5h() {
        assert_eq!(
            toast_text(&due(&["5-hour"])),
            (
                "Claude · Work reset".to_string(),
                "5-hour limit reset — start a session".to_string()
            )
        );
    }

    #[test]
    fn toast_text_grouped() {
        let (title, body) = toast_text(&due(&["5-hour", "Weekly"]));
        assert_eq!(title, "Claude · Work reset");
        assert_eq!(body, "5-hour, Weekly — start a session");
    }

    #[test]
    fn toast_text_has_no_email() {
        let mut d = due(&["Weekly"]);
        d.account_id = "claude:me@example.com".into();
        d.service = Service::Antigravity;
        let (title, body) = toast_text(&d);
        assert_eq!(title, "Antigravity · Work reset");
        assert!(!title.contains('@') && !body.contains('@'));
    }

    #[test]
    fn toast_id_round_trips_service() {
        for s in [Service::Claude, Service::Codex, Service::Antigravity] {
            assert_eq!(service_of_toast(toast_id(s)), Some(s));
        }
        assert_eq!(service_of_toast(0), None, "usage warnings");
    }

    #[test]
    fn store_roundtrip_and_broken_file() {
        let dir = std::env::temp_dir().join(format!("qg-resets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = JsonStore::new(dir.join("resets.json"));
        assert_eq!(store.load::<ResetState>(), ResetState::default(), "missing");

        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("resets.json"), b"{\"a|5h\":{\"reset_ms\":").unwrap();
        assert_eq!(store.load::<ResetState>(), ResetState::default(), "broken");

        let mut state = ResetState::default();
        step(&mut state, &on(), Some(&snap(NOW + 60_000)), NOW);
        assert_eq!(state.0.len(), 1);
        store.save(&state).unwrap();
        assert_eq!(store.load::<ResetState>(), state);
        assert!(!dir.join("resets.json.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn disabled_writes_nothing() {
        let mut state = ResetState::default();
        let off = ResetToasts::default();
        assert_eq!(
            step(&mut state, &off, Some(&snap(NOW + 60_000)), NOW),
            (vec![], false)
        );
        assert!(state.0.is_empty());
    }

    #[test]
    fn enabled_tracks_then_fires_once() {
        let mut state = ResetState::default();
        let (due_now, changed) = step(&mut state, &on(), Some(&snap(NOW + 60_000)), NOW);
        assert!(due_now.is_empty() && changed);
        assert_eq!(step(&mut state, &on(), None, NOW), (vec![], false));
        let (fired, changed) = step(&mut state, &on(), None, NOW + 61_000);
        assert_eq!(fired.len(), 1);
        assert!(changed);
        assert_eq!(
            step(&mut state, &on(), None, NOW + 120_000),
            (vec![], false)
        );
    }

    /// The pass reads only its own copy of the switches, so it can run inside the scheduler's publish
    /// while `commands::update` holds the settings lock.
    #[test]
    fn pass_uses_its_own_switches() {
        let dir = std::env::temp_dir().join(format!("qg-resets-pass-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let resets = Resets::new(
            JsonStore::new(dir.join("resets.json")),
            ResetToasts::default(),
        );
        assert_eq!(resets.pass(Some(&snap(NOW + 60_000)), NOW).0, vec![]);
        assert!(!dir.join("resets.json").exists(), "off: nothing written");

        resets.set_toasts(&on());
        resets.pass(Some(&snap(NOW + 60_000)), NOW);
        assert!(dir.join("resets.json").exists(), "on: tracked and saved");
        let (fired, _, sound) = resets.pass(None, NOW + 61_000);
        assert_eq!(fired.len(), 1);
        assert_eq!(sound, ToastSound::Default);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn codex_snap(banked: Option<u32>) -> Snapshot {
        let mut s = snap(NOW + 60_000);
        s.accounts[0].id = "codex:0123456789abcdef".into();
        s.accounts[0].service = Service::Codex;
        s.accounts[0].label = "Work".into();
        s.accounts[0].banked_resets = banked;
        s.accounts[0].buckets.clear();
        s
    }

    fn gained(state: &mut ResetState, banked: Option<u32>) -> usize {
        step_banked(state, &on(), &codex_snap(banked)).0.len()
    }

    #[test]
    fn gain_text_names_provider_and_account_only() {
        let gain = Gain {
            service: Service::Codex,
            account_id: "codex:me@example.com".into(),
            label: "Work".into(),
        };
        assert_eq!(
            gain_text(&gain),
            (
                "Codex: a limit reset is available.".to_string(),
                "Work".to_string()
            )
        );
    }

    #[test]
    fn new_reset_sends_one_toast_per_increase() {
        let mut state = ResetState::default();
        assert_eq!(gained(&mut state, Some(0)), 0, "0 stored");
        assert_eq!(gained(&mut state, Some(1)), 1, "0 to 1");
        assert_eq!(gained(&mut state, Some(1)), 0, "1 to 1");
        assert_eq!(gained(&mut state, Some(2)), 1, "1 to 2");
    }

    #[test]
    fn restart_with_the_same_count_sends_none() {
        let dir = std::env::temp_dir().join(format!("qg-resets-banked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = JsonStore::new(dir.join("resets.json"));
        let mut state = ResetState::default();
        assert_eq!(gained(&mut state, Some(0)), 0);
        assert_eq!(gained(&mut state, Some(1)), 1);
        store.save(&state).unwrap();

        let mut restarted: ResetState = store.load();
        assert_eq!(gained(&mut restarted, Some(1)), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn banked_follows_the_switches() {
        let mut state = ResetState::default();
        step_banked(&mut state, &on(), &codex_snap(Some(0)));
        let off = ResetToasts::default();
        assert_eq!(
            step_banked(&mut state, &off, &codex_snap(Some(1))),
            (vec![], false),
            "master switch off: nothing stored, nothing sent"
        );
        let codex_off = ResetToasts {
            codex: false,
            ..on()
        };
        let (g, changed) = step_banked(&mut state, &codex_off, &codex_snap(Some(1)));
        assert!(g.is_empty() && changed && state.1.is_empty());
    }

    /// 5a found no readable expiry date, so time passing alone never sends a toast.
    #[test]
    fn no_expiry_toast_without_an_expiry_date() {
        let mut state = ResetState::default();
        gained(&mut state, Some(0));
        gained(&mut state, Some(1));
        let month = 30 * 86_400_000;
        assert_eq!(step(&mut state, &on(), None, NOW + month), (vec![], false));
        assert_eq!(state.1.len(), 1, "the count stays");
    }

    #[test]
    fn pass_returns_the_gain_and_saves() {
        let dir = std::env::temp_dir().join(format!("qg-resets-gain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let resets = Resets::new(JsonStore::new(dir.join("resets.json")), on());
        assert!(resets.pass(Some(&codex_snap(Some(0))), NOW).1.is_empty());
        let (_, gains, _) = resets.pass(Some(&codex_snap(Some(1))), NOW);
        assert_eq!(gains.len(), 1);
        let saved: ResetState = JsonStore::new(dir.join("resets.json")).load();
        assert_eq!(saved.1.values().copied().collect::<Vec<_>>(), vec![1]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn service_switched_off_never_fires() {
        let mut state = ResetState::default();
        step(&mut state, &on(), Some(&snap(NOW + 60_000)), NOW);
        let claude_off = ResetToasts {
            claude: false,
            ..on()
        };
        let (fired, changed) = step(&mut state, &claude_off, None, NOW + 61_000);
        assert!(fired.is_empty());
        assert!(changed && state.0.is_empty());
    }
}
