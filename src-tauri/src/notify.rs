//! Native warnings at 50, 80 and 90% of each limit. Fired levels live in `%APPDATA%\QuotaGlass\warnings.json`.
//! Toast text holds the provider, the account label, the limit name, a percent and a time. No emails, no tokens.

use quotaglass_core::view::Snapshot;
use quotaglass_core::warnings::{self, WarnState};
use quotaglass_core::Service;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::BTreeSet;
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

#[derive(Debug, Clone, PartialEq)]
pub struct Alert {
    pub title: String,
    pub body: String,
    pub level: u8,
}

/// A small JSON file in `%APPDATA%\QuotaGlass`, written atomically. Used for `warnings.json` and `resets.json`.
pub struct JsonStore {
    path: PathBuf,
}

impl JsonStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// `%APPDATA%\QuotaGlass\<name>`, or `None` when APPDATA is not set.
    pub fn in_appdata(name: &str) -> Option<PathBuf> {
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("QuotaGlass").join(name))
    }

    /// A missing or broken file gives an empty state. Worst case, a toast shows once more or not at all.
    pub fn load<T: DeserializeOwned + Default>(&self) -> T {
        std::fs::read(&self.path)
            .ok()
            .and_then(|raw| serde_json::from_slice(&raw).ok())
            .unwrap_or_default()
    }

    /// Writes `<name>.tmp`, then renames it over `<name>`.
    pub fn save<T: Serialize>(&self, state: &T) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(state).map_err(io::Error::other)?)?;
        std::fs::rename(&tmp, &self.path)
    }
}

/// App state: the fired levels and their file.
pub struct Warnings {
    pub state: Mutex<WarnState>,
    pub store: JsonStore,
}

pub fn provider_name(service: Service) -> &'static str {
    match service {
        Service::Claude => "Claude",
        Service::Codex => "Codex",
        Service::Antigravity => "Antigravity",
    }
}

/// The alerts due for this snapshot. Hidden accounts never alert.
/// Prunes only limits that are gone from accounts with readings in this snapshot, plus windows
/// that ended over 7 days ago. An empty snapshot (the first publish, before any fetch) changes nothing.
pub fn check(snapshot: &Snapshot, state: &mut WarnState) -> Vec<Alert> {
    check_at(snapshot, state, warnings::local_offset_secs())
}

fn check_at(snapshot: &Snapshot, state: &mut WarnState, offset_secs: Option<i32>) -> Vec<Alert> {
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
                    alerts.push(Alert {
                        title: format!("{} · {}", provider_name(account.service), account.label),
                        body: format!(
                            "{} at {}%{resets}",
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

/// Shows one toast. 50 and 80 are silent; 90 plays the default sound.
pub fn send(app: &AppHandle, alert: &Alert) {
    let mut toast = app
        .notification()
        .builder()
        .title(&alert.title)
        .body(&alert.body);
    // The plugin has no persistent (reminder) toast on Windows, so 90% falls back to a normal toast with sound.
    if alert.level >= 90 {
        toast = toast.sound("Default");
    }
    if let Err(e) = toast.show() {
        tracing::warn!(error = ?e, "warning toast failed");
    }
}

/// Runs after each scheduler publish: finds due alerts, shows them, and saves only when the state changed.
pub fn on_snapshot(app: &AppHandle, warnings: &Warnings, snapshot: &Snapshot) {
    let mut state = crate::scheduler::lock(&warnings.state);
    let before = state.clone();
    for alert in check(snapshot, &mut state) {
        send(app, &alert);
    }
    if *state != before {
        if let Err(e) = warnings.store.save(&*state) {
            tracing::warn!(error = ?e, "could not save warnings.json");
        }
    }
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
            window_secs: None,
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
    fn check_builds_text_and_fires_once() {
        let s = snap(vec![account(
            "claude:x",
            Service::Claude,
            "Claude",
            vec![bucket("5-hour", 81.4, Some(NOW + 125 * 60_000))],
        )]);
        let mut state = WarnState::default();
        let alerts = check_at(&s, &mut state, Some(0));
        assert_eq!(
            alerts,
            [Alert {
                title: "Claude · Claude".into(),
                body: "5-hour at 81% — resets in 2 h 5 min".into(),
                level: 80,
            }]
        );
        assert!(check_at(&s, &mut state, Some(0)).is_empty());
    }

    #[test]
    fn check_skips_hidden_accounts() {
        let mut a = account(
            "codex:x",
            Service::Codex,
            "Codex",
            vec![bucket("weekly", 95.0, None)],
        );
        a.hidden = true;
        let mut state = WarnState::default();
        assert!(check_at(&snap(vec![a]), &mut state, Some(0)).is_empty());
        assert!(state.0.is_empty());
    }

    #[test]
    fn check_prunes_gone_limits_and_stale_windows_only() {
        let mut state = WarnState::default();
        warnings::due(&mut state, "here|old", 60.0, None);
        warnings::due(&mut state, "absent|5h", 60.0, None);
        warnings::due(&mut state, "absent|ended", 60.0, Some(NOW - 1));
        warnings::due(&mut state, "absent|stale", 60.0, Some(NOW - 8 * 86_400_000));
        let s = snap(vec![account(
            "here",
            Service::Claude,
            "Claude",
            vec![bucket("5h", 10.0, None)],
        )]);
        check_at(&s, &mut state, Some(0));
        assert_eq!(
            state.0.keys().collect::<Vec<_>>(),
            ["absent|5h", "absent|ended"]
        );
    }

    #[test]
    fn empty_snapshot_keeps_state() {
        let mut state = WarnState::default();
        warnings::due(&mut state, "claude:x|weekly", 82.0, None);
        warnings::due(&mut state, "codex:x|5h", 60.0, Some(NOW - 8 * 86_400_000));
        let before = state.clone();
        assert!(check_at(&snap(vec![]), &mut state, Some(0)).is_empty());
        // `on_snapshot` saves only when the state changed, so nothing is written.
        assert_eq!(state, before);
    }

    #[test]
    fn restart_does_not_refire() {
        let at = |used| {
            snap(vec![account(
                "claude:x",
                Service::Claude,
                "Claude",
                vec![bucket("weekly", used, None)],
            )])
        };
        let mut state = WarnState::default();
        assert_eq!(check_at(&at(82.0), &mut state, Some(0)).len(), 1);
        // Restart: the first publish comes before any fetch.
        check_at(&snap(vec![]), &mut state, Some(0));
        assert!(check_at(&at(83.0), &mut state, Some(0)).is_empty());
    }

    #[test]
    fn percent_rounds_down() {
        let s = snap(vec![account(
            "claude:x",
            Service::Claude,
            "Claude",
            vec![bucket("5-hour", 89.6, None)],
        )]);
        let alerts = check_at(&s, &mut WarnState::default(), Some(0));
        assert_eq!(alerts[0].level, 80);
        assert_eq!(alerts[0].body, "5-hour at 89%");
    }

    #[test]
    fn alert_text_has_no_email() {
        let s = snap(vec![
            account(
                "claude:me@example.com",
                Service::Claude,
                "Claude",
                vec![bucket("weekly", 91.0, Some(NOW + 3 * 86_400_000))],
            ),
            account(
                "agy:me@example.com",
                Service::Antigravity,
                "Antigravity",
                vec![bucket("Gemini 5h", 55.0, None)],
            ),
        ]);
        let mut state = WarnState::default();
        let alerts = check_at(&s, &mut state, Some(0));
        assert_eq!(alerts.len(), 2);
        assert_eq!(alerts[1].title, "Antigravity · Antigravity");
        assert_eq!(alerts[1].body, "Gemini 5h at 55%");
        for a in alerts {
            assert!(!a.title.contains('@') && !a.body.contains('@'), "{a:?}");
        }
    }

    #[test]
    fn broken_warnings_file_starts_empty() {
        let dir = std::env::temp_dir().join(format!("qg-warn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = JsonStore::new(dir.join("warnings.json"));
        assert_eq!(
            store.load::<WarnState>(),
            WarnState::default(),
            "missing file"
        );

        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("warnings.json"), b"{\"a|5h\":{\"reset_ms\":").unwrap();
        assert_eq!(
            store.load::<WarnState>(),
            WarnState::default(),
            "broken file"
        );

        let mut state = WarnState::default();
        warnings::due(&mut state, "a|5h", 85.0, Some(NOW));
        store.save(&state).unwrap();
        assert_eq!(store.load::<WarnState>(), state);
        assert!(!dir.join("warnings.json.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
