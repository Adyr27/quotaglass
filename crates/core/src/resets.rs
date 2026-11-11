//! Reset reminders: the next reset time seen for each limit, and which resets are due now.
//! Pure state only. The app crate stores it in `resets.json` and shows the toasts.

use crate::model::Service;
use crate::view::Snapshot;
use crate::warnings::{key, SAME_WINDOW_MS};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Resets of one account this close together share one toast.
pub const GROUP_MS: i64 = 2 * 60_000;
/// A reset missed by more than this (the app was not running) is dropped without a toast.
pub const LATE_GRACE_MS: i64 = 15 * 60_000;

/// `warnings::key(account_id, bucket)` to the next reset seen for that limit.
#[derive(Serialize, Deserialize, Clone, PartialEq, Default, Debug)]
pub struct ResetState(pub BTreeMap<String, ResetEntry>);

/// Holds ids, labels, bucket names and times only. Never an email or a token.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct ResetEntry {
    pub reset_ms: i64,
    pub service: Service,
    pub account_id: String,
    pub label: String,
    pub bucket: String,
}

/// The per-service switches. The master switch is the caller's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Switches {
    pub claude: bool,
    pub codex: bool,
    pub antigravity: bool,
}

impl Switches {
    pub fn on(&self, service: Service) -> bool {
        match service {
            Service::Claude => self.claude,
            Service::Codex => self.codex,
            Service::Antigravity => self.antigravity,
        }
    }
}

/// One toast: the resets of one account that fell within [`GROUP_MS`].
#[derive(Debug, Clone, PartialEq)]
pub struct Due {
    pub service: Service,
    pub account_id: String,
    pub label: String,
    pub buckets: Vec<String>,
}

/// Updates the entries from a snapshot. Hidden accounts and switched-off services drop their entries.
/// An account missing from the snapshot drops its entries that are not yet due. A due entry stays for
/// `take_due`: right after start an account has no reading yet, and its reset may have passed meanwhile.
/// Only `take_due` removes a due entry, so a new window is tracked after the old one fired.
pub fn track(state: &mut ResetState, snapshot: &Snapshot, on: &Switches) {
    let now = snapshot.generated_at_ms;
    state.0.retain(
        |_, e| match snapshot.accounts.iter().find(|a| a.id == e.account_id) {
            Some(a) => !a.hidden && on.on(a.service),
            None => e.reset_ms <= now && on.on(e.service),
        },
    );
    for a in snapshot
        .accounts
        .iter()
        .filter(|a| !a.hidden && on.on(a.service))
    {
        for b in &a.buckets {
            // A bucket without a reset time (Claude when idle) keeps its entry until due.
            let Some(reset_ms) = b.resets_at_ms else {
                continue;
            };
            let k = key(&a.id, &b.name);
            if let Some(e) = state.0.get_mut(&k) {
                e.label.clone_from(&a.label);
                let due = e.reset_ms <= now;
                let jitter = e.reset_ms.abs_diff(reset_ms) < SAME_WINDOW_MS.unsigned_abs();
                if due || jitter || reset_ms <= now {
                    continue;
                }
            } else if reset_ms <= now {
                continue;
            }
            state.0.insert(
                k,
                ResetEntry {
                    reset_ms,
                    service: a.service,
                    account_id: a.id.clone(),
                    label: a.label.clone(),
                    bucket: b.name.clone(),
                },
            );
        }
    }
}

/// Removes and returns what is due now, grouped per account. An entry later than [`LATE_GRACE_MS`] is dropped.
/// A group starts at an account's earliest entry and holds its entries within [`GROUP_MS`] of it. The group
/// fires once all of them are due, so two resets a minute apart give one toast, not two.
pub fn take_due(state: &mut ResetState, now_ms: i64) -> Vec<Due> {
    state
        .0
        .retain(|_, e| e.reset_ms >= now_ms.saturating_sub(LATE_GRACE_MS));
    let mut by_account: BTreeMap<String, Vec<(String, ResetEntry)>> = BTreeMap::new();
    for (k, e) in &state.0 {
        by_account
            .entry(e.account_id.clone())
            .or_default()
            .push((k.clone(), e.clone()));
    }
    let mut out: Vec<(i64, Due)> = Vec::new();
    for mut entries in by_account.into_values() {
        entries.sort_by(|a, b| (a.1.reset_ms, &a.1.bucket).cmp(&(b.1.reset_ms, &b.1.bucket)));
        while let Some((_, first)) = entries.first() {
            let start = first.reset_ms;
            if start > now_ms {
                break;
            }
            let n = entries
                .iter()
                .take_while(|(_, e)| e.reset_ms.saturating_sub(start) < GROUP_MS)
                .count();
            if entries[n - 1].1.reset_ms > now_ms {
                break;
            }
            let group: Vec<(String, ResetEntry)> = entries.drain(..n).collect();
            for (k, _) in &group {
                state.0.remove(k);
            }
            let head = &group[0].1;
            out.push((
                start,
                Due {
                    service: head.service,
                    account_id: head.account_id.clone(),
                    label: head.label.clone(),
                    buckets: group.iter().map(|(_, e)| e.bucket.clone()).collect(),
                },
            ));
        }
    }
    out.sort_by(|a, b| (a.0, &a.1.account_id).cmp(&(b.0, &b.1.account_id)));
    out.into_iter().map(|(_, d)| d).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{AccountView, BucketView, CardState, Freshness};

    const NOW: i64 = 1_800_000_000_000;
    const MIN: i64 = 60_000;
    const H: i64 = 60 * MIN;
    const ALL: Switches = Switches {
        claude: true,
        codex: true,
        antigravity: true,
    };

    fn bucket(name: &str, resets_at_ms: Option<i64>) -> BucketView {
        BucketView {
            name: name.into(),
            used_percent: 40.0,
            window_secs: Some(18_000),
            resets_at_ms,
            limit_reached: false,
            freshness: Freshness::Live,
            passed_percent: None,
            fast_burn: false,
        }
    }

    fn acct(id: &str, service: Service, buckets: Vec<BucketView>) -> AccountView {
        AccountView {
            id: id.into(),
            service,
            label: format!("{id} label"),
            plan: None,
            source: None,
            freshness: Freshness::Live,
            age_secs: None,
            headline: buckets.first().cloned(),
            buckets,
            state: CardState::Ok,
            reason: None,
            room_left: None,
            best_now: false,
            banked_resets: None,
            profile_id: None,
            current_login: false,
            hidden: false,
        }
    }

    fn snap(at: i64, accounts: Vec<AccountView>) -> Snapshot {
        Snapshot {
            accounts,
            services: Vec::new(),
            generated_at_ms: at,
        }
    }

    fn claude(buckets: Vec<BucketView>) -> AccountView {
        acct("claude:a", Service::Claude, buckets)
    }

    #[test]
    fn new_future_reset_creates_entry() {
        let mut s = ResetState::default();
        track(
            &mut s,
            &snap(NOW, vec![claude(vec![bucket("5-hour", Some(NOW + H))])]),
            &ALL,
        );
        let e = &s.0[&key("claude:a", "5-hour")];
        assert_eq!(e.reset_ms, NOW + H);
        assert_eq!(e.service, Service::Claude);
        assert_eq!(e.label, "claude:a label");
        assert_eq!(e.bucket, "5-hour");
    }

    #[test]
    fn past_reset_creates_nothing() {
        let mut s = ResetState::default();
        let a = claude(vec![
            bucket("5-hour", Some(NOW - 1)),
            bucket("Weekly", None),
        ]);
        track(&mut s, &snap(NOW, vec![a]), &ALL);
        assert!(s.0.is_empty());
    }

    #[test]
    fn jitter_keeps_one_entry() {
        let mut s = ResetState::default();
        track(
            &mut s,
            &snap(NOW, vec![claude(vec![bucket("5-hour", Some(NOW + H))])]),
            &ALL,
        );
        track(
            &mut s,
            &snap(
                NOW + MIN,
                vec![claude(vec![bucket("5-hour", Some(NOW + H + 9))])],
            ),
            &ALL,
        );
        assert_eq!(s.0.len(), 1);
        assert_eq!(s.0[&key("claude:a", "5-hour")].reset_ms, NOW + H);
    }

    #[test]
    fn vendor_moves_window_replaces_entry() {
        let mut s = ResetState::default();
        track(
            &mut s,
            &snap(NOW, vec![claude(vec![bucket("5-hour", Some(NOW + H))])]),
            &ALL,
        );
        track(
            &mut s,
            &snap(
                NOW + MIN,
                vec![claude(vec![bucket("5-hour", Some(NOW + 3 * H))])],
            ),
            &ALL,
        );
        assert_eq!(s.0[&key("claude:a", "5-hour")].reset_ms, NOW + 3 * H);
    }

    #[test]
    fn vanished_bucket_still_fires() {
        let mut s = ResetState::default();
        track(
            &mut s,
            &snap(NOW, vec![claude(vec![bucket("5-hour", Some(NOW + H))])]),
            &ALL,
        );
        // Idle after the reset: Claude reports no reset time, then no bucket at all.
        track(
            &mut s,
            &snap(NOW + 30 * MIN, vec![claude(vec![bucket("5-hour", None)])]),
            &ALL,
        );
        track(&mut s, &snap(NOW + 50 * MIN, vec![claude(vec![])]), &ALL);
        let due = take_due(&mut s, NOW + H + 10_000);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].buckets, vec!["5-hour".to_string()]);
    }

    #[test]
    fn due_removes_entry_once() {
        let mut s = ResetState::default();
        track(
            &mut s,
            &snap(NOW, vec![claude(vec![bucket("5-hour", Some(NOW + H))])]),
            &ALL,
        );
        assert!(take_due(&mut s, NOW + H - 1).is_empty(), "not due yet");
        assert_eq!(take_due(&mut s, NOW + H).len(), 1);
        assert!(take_due(&mut s, NOW + H + 30_000).is_empty());
        assert!(s.0.is_empty());
    }

    #[test]
    fn new_window_waits_for_due_entry_then_tracks() {
        let mut s = ResetState::default();
        track(
            &mut s,
            &snap(NOW, vec![claude(vec![bucket("5-hour", Some(NOW + H))])]),
            &ALL,
        );
        // The vendor already shows the next window, but the old one is due and not yet taken.
        let next = vec![claude(vec![bucket("5-hour", Some(NOW + 6 * H))])];
        track(&mut s, &snap(NOW + H + 1, next.clone()), &ALL);
        assert_eq!(s.0[&key("claude:a", "5-hour")].reset_ms, NOW + H);
        assert_eq!(take_due(&mut s, NOW + H + 2).len(), 1);
        track(&mut s, &snap(NOW + H + 3, next), &ALL);
        assert_eq!(s.0[&key("claude:a", "5-hour")].reset_ms, NOW + 6 * H);
    }

    #[test]
    fn two_buckets_two_minutes_apart_share_one_due() {
        let mut s = ResetState::default();
        let a = claude(vec![
            bucket("5-hour", Some(NOW + H)),
            bucket("Weekly", Some(NOW + H + 2 * MIN - 1)),
        ]);
        track(&mut s, &snap(NOW, vec![a]), &ALL);
        // The first is due, the second is not yet: wait for it rather than send two toasts.
        assert!(take_due(&mut s, NOW + H + MIN).is_empty());
        let due = take_due(&mut s, NOW + H + 2 * MIN);
        assert_eq!(due.len(), 1);
        assert_eq!(
            due[0].buckets,
            vec!["5-hour".to_string(), "Weekly".to_string()]
        );
        assert!(s.0.is_empty());
    }

    #[test]
    fn two_buckets_far_apart_give_two_dues() {
        let mut s = ResetState::default();
        let a = claude(vec![
            bucket("5-hour", Some(NOW + H)),
            bucket("Weekly", Some(NOW + H + 5 * MIN)),
        ]);
        track(&mut s, &snap(NOW, vec![a]), &ALL);
        let first = take_due(&mut s, NOW + H);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].buckets, vec!["5-hour".to_string()]);
        let second = take_due(&mut s, NOW + H + 5 * MIN);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].buckets, vec!["Weekly".to_string()]);

        // Both already due at once (after a restart): still two toasts.
        let mut s = ResetState::default();
        let a = claude(vec![
            bucket("5-hour", Some(NOW + H)),
            bucket("Weekly", Some(NOW + H + 5 * MIN)),
        ]);
        track(&mut s, &snap(NOW, vec![a]), &ALL);
        assert_eq!(take_due(&mut s, NOW + H + 6 * MIN).len(), 2);
    }

    #[test]
    fn different_accounts_never_group() {
        let mut s = ResetState::default();
        let a = claude(vec![bucket("5-hour", Some(NOW + H))]);
        let b = acct(
            "claude:b",
            Service::Claude,
            vec![bucket("5-hour", Some(NOW + H))],
        );
        track(&mut s, &snap(NOW, vec![a, b]), &ALL);
        let due = take_due(&mut s, NOW + H);
        assert_eq!(due.len(), 2);
        assert_ne!(due[0].account_id, due[1].account_id);
    }

    #[test]
    fn late_over_15_min_is_dropped_silently() {
        let mut s = ResetState::default();
        track(
            &mut s,
            &snap(NOW, vec![claude(vec![bucket("5-hour", Some(NOW + H))])]),
            &ALL,
        );
        assert!(take_due(&mut s, NOW + H + 15 * MIN + 1).is_empty());
        assert!(s.0.is_empty(), "the late entry is gone, not kept for later");
    }

    #[test]
    fn late_within_15_min_fires() {
        let mut s = ResetState::default();
        track(
            &mut s,
            &snap(NOW, vec![claude(vec![bucket("5-hour", Some(NOW + H))])]),
            &ALL,
        );
        assert_eq!(take_due(&mut s, NOW + H + 15 * MIN).len(), 1);
    }

    #[test]
    fn hidden_account_drops_entries() {
        let mut s = ResetState::default();
        track(
            &mut s,
            &snap(NOW, vec![claude(vec![bucket("5-hour", Some(NOW + H))])]),
            &ALL,
        );
        let mut hidden = claude(vec![bucket("5-hour", Some(NOW + H))]);
        hidden.hidden = true;
        track(&mut s, &snap(NOW + MIN, vec![hidden.clone()]), &ALL);
        assert!(s.0.is_empty());
        // A hidden account never creates one either.
        track(&mut s, &snap(NOW + 2 * MIN, vec![hidden]), &ALL);
        assert!(s.0.is_empty());
    }

    #[test]
    fn switch_off_drops_entries() {
        let mut s = ResetState::default();
        let accounts = vec![
            claude(vec![bucket("5-hour", Some(NOW + H))]),
            acct(
                "codex:a",
                Service::Codex,
                vec![bucket("5-hour", Some(NOW + H))],
            ),
        ];
        track(&mut s, &snap(NOW, accounts.clone()), &ALL);
        assert_eq!(s.0.len(), 2);
        let no_claude = Switches {
            claude: false,
            ..ALL
        };
        track(&mut s, &snap(NOW + MIN, accounts), &no_claude);
        assert_eq!(
            s.0.values().map(|e| e.service).collect::<Vec<_>>(),
            vec![Service::Codex]
        );
    }

    #[test]
    fn account_gone_drops_entries() {
        let mut s = ResetState::default();
        track(
            &mut s,
            &snap(NOW, vec![claude(vec![bucket("5-hour", Some(NOW + H))])]),
            &ALL,
        );
        track(&mut s, &snap(NOW + MIN, vec![]), &ALL);
        assert!(s.0.is_empty());
    }

    #[test]
    fn state_round_trips_json() {
        let mut s = ResetState::default();
        let a = claude(vec![
            bucket("5-hour", Some(NOW + H)),
            bucket("Weekly", Some(NOW + 9 * H)),
        ]);
        track(&mut s, &snap(NOW, vec![a]), &ALL);
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<ResetState>(&json).unwrap(), s);
    }

    #[test]
    fn entry_has_no_email_or_token() {
        let mut s = ResetState::default();
        track(
            &mut s,
            &snap(NOW, vec![claude(vec![bucket("5-hour", Some(NOW + H))])]),
            &ALL,
        );
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains('@'));
        assert!(!json.to_lowercase().contains("token"));
    }
}
