//! Turns the latest source outcomes into one card per account.

use crate::freshness::classify;
use crate::model::{FetchError, Reading, Service, SourceKind};
use crate::provider::SourceOutcome;
use crate::view::{AccountView, BucketView, CardState, Freshness};
use std::collections::{BTreeMap, HashMap};

const ANTIGRAVITY_HINT: &str =
    "Sign in with agy, then add the account in Orca Settings > AI Provider Accounts.";
/// Readings from two sources describe the same window when their reset times are this close.
const SAME_RESET_MS: i64 = 60_000;

/// Last successful reading per account and source. Memory only; sources refill it after a restart.
#[derive(Debug, Default)]
pub struct LastReadings(HashMap<String, BTreeMap<SourceKind, Reading>>);

impl LastReadings {
    /// Drops every stored reading from a source that is gone (no longer detected or turned off), so its old
    /// numbers do not stay on screen. `service` limits it to one service for a single-service source.
    pub fn forget(&mut self, kind: SourceKind, service: Option<Service>) {
        for per_source in self.0.values_mut() {
            per_source.retain(|k, r| *k != kind || service.is_some_and(|s| s != r.key.service));
        }
        self.0.retain(|_, m| !m.is_empty());
    }
}

pub fn build(
    outcomes: &[SourceOutcome],
    last: &mut LastReadings,
    connectable: &[Service],
    refresh_secs: u64,
    now_ms: i64,
) -> Vec<AccountView> {
    // Two Direct providers share a kind, so a success only covers its own service.
    let succeeded = |kind: SourceKind, service: Service| {
        outcomes
            .iter()
            .any(|o| o.result.is_ok() && o.kind == kind && o.service.is_none_or(|s| s == service))
    };

    // 1. Forget accounts that a successful source no longer reports (signed out).
    for per_source in last.0.values_mut() {
        per_source.retain(|kind, r| !succeeded(*kind, r.key.service));
    }
    // 2. Store everything that just arrived.
    let mut live: HashMap<String, Vec<SourceKind>> = HashMap::new();
    for o in outcomes {
        if let Ok(readings) = &o.result {
            for r in readings {
                let id = r.key.store_key();
                live.entry(id.clone()).or_default().push(r.source);
                last.0.entry(id).or_default().insert(r.source, r.clone());
            }
        }
    }
    last.0.retain(|_, m| !m.is_empty());

    let mut cards: Vec<AccountView> = Vec::new();
    for (id, per_source) in &last.0 {
        let live_here = live.get(id).map(Vec::as_slice).unwrap_or(&[]);

        // A source that answered this cycle wins over stored readings. Among them, one with numbers beats one
        // without (Orca can list an account it has no limits for). When two readings are otherwise equal, the
        // `SourceKind` order (Direct, AntigravityApp, Orca) breaks the tie: `min_by_key` keeps the first of
        // equal keys, and the map iterates in that order.
        let chosen = per_source
            .iter()
            .filter(|(k, _)| live_here.contains(k))
            .map(|(_, r)| r)
            .min_by_key(|r| r.buckets.is_empty())
            .map(|r| (r, true))
            .or_else(|| {
                per_source
                    .values()
                    .max_by_key(|r| (!r.buckets.is_empty(), r.observed_at_ms))
                    .map(|r| (r, false))
            });
        let Some((reading, fresh)) = chosen else {
            continue;
        };

        let mut reading = reading.clone();
        if reading.source == SourceKind::Orca {
            if let Some(direct) = per_source.get(&SourceKind::Direct) {
                adopt_vendor_windows(&mut reading, direct);
            }
        }
        let failure = (!fresh)
            .then(|| latest_error(outcomes, reading.source, reading.key.service))
            .flatten();
        let mut view = card(&reading, failure, connectable, refresh_secs, now_ms);
        view.sources = per_source.keys().copied().collect();
        cards.push(view);
    }

    // 3. A single-service source failed and nothing else knows that service: show why.
    for o in outcomes {
        let (Some(service), Err(e)) = (o.service, &o.result) else {
            continue;
        };
        if matches!(e, FetchError::NotDetected | FetchError::Disabled)
            || cards.iter().any(|c| c.service == service)
        {
            continue;
        }
        cards.push(AccountView {
            id: format!("{}:{:?}-error", service.as_str(), o.kind).to_lowercase(),
            service,
            label: format!("{} (CLI login)", display_name(service)),
            plan: None,
            source: Some(o.kind),
            sources: vec![],
            freshness: Freshness::Stale,
            age_secs: None,
            headline: None,
            extra_buckets: 0,
            buckets: vec![],
            state: CardState::NoSource,
            reason: Some(e.reason()),
            connect: connectable.contains(&service).then_some(service),
        });
    }

    cards.sort_by(|a, b| {
        a.service
            .cmp(&b.service)
            .then_with(|| a.label.cmp(&b.label))
    });
    cards
}

fn display_name(s: Service) -> &'static str {
    match s {
        Service::Claude => "Claude",
        Service::Codex => "Codex",
        Service::Antigravity => "Antigravity",
    }
}

fn latest_error(
    outcomes: &[SourceOutcome],
    kind: SourceKind,
    service: Service,
) -> Option<FetchError> {
    outcomes
        .iter()
        .filter(|o| o.kind == kind && o.service.is_none_or(|s| s == service))
        .find_map(|o| o.result.as_ref().err().cloned())
}

/// Orca's window lengths can be wrong (it labels Codex Free's 30-day window as 5 h).
/// A Direct bucket with the same reset time is the vendor's own answer, for both the length and the name.
fn adopt_vendor_windows(reading: &mut Reading, direct: &Reading) {
    for b in &mut reading.buckets {
        let Some(reset) = b.resets_at_ms else {
            continue;
        };
        if let Some(d) = direct.buckets.iter().find(|d| {
            d.resets_at_ms
                .is_some_and(|r| (r - reset).abs() <= SAME_RESET_MS)
        }) {
            if d.window_secs.is_some() {
                b.window_secs = d.window_secs;
                b.name = d.name.clone();
            }
        }
    }
}

fn card(
    r: &Reading,
    failure: Option<FetchError>,
    connectable: &[Service],
    refresh_secs: u64,
    now_ms: i64,
) -> AccountView {
    let buckets: Vec<BucketView> = r
        .buckets
        .iter()
        .map(|b| BucketView {
            name: b.name.clone(),
            used_percent: b.used_percent,
            window_secs: b.window_secs,
            resets_at_ms: b.resets_at_ms,
            limit_reached: b.used_percent >= 100.0,
            freshness: classify(r.observed_at_ms, refresh_secs, b, now_ms),
        })
        .collect();
    // The tightest bucket that is still current. A bucket whose reset has passed only leads when all have.
    let tightest = |current_only: bool| {
        buckets
            .iter()
            .filter(|b| !current_only || b.freshness != Freshness::Stale)
            .max_by(|a, b| a.used_percent.total_cmp(&b.used_percent))
            .cloned()
    };
    let headline = tightest(true).or_else(|| tightest(false));
    let (state, reason) = match (&failure, buckets.is_empty()) {
        (Some(e), _) => (CardState::Error, Some(e.reason())),
        (None, true) if r.key.service == Service::Antigravity => {
            (CardState::NoReadingYet, Some(ANTIGRAVITY_HINT.into()))
        }
        (None, true) => (CardState::NoReadingYet, None),
        (None, false) => (CardState::Ok, None),
    };
    let connect = (failure.is_some()
        && r.key.service != Service::Antigravity
        && connectable.contains(&r.key.service))
    .then_some(r.key.service);
    AccountView {
        id: r.key.store_key(),
        service: r.key.service,
        label: r.label.clone(),
        plan: r.plan.clone(),
        source: Some(r.source),
        sources: vec![],
        freshness: headline
            .as_ref()
            .map(|h| h.freshness)
            .unwrap_or(Freshness::Stale),
        age_secs: Some((now_ms.saturating_sub(r.observed_at_ms).max(0) / 1000) as u64),
        extra_buckets: buckets.len().saturating_sub(1) as u32,
        headline,
        buckets,
        state,
        reason,
        connect,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AccountKey, Bucket};

    const NOW: i64 = 1_800_000_000_000;

    fn reading(
        service: Service,
        email: Option<&str>,
        local: &str,
        source: SourceKind,
        pcts: &[f64],
    ) -> Reading {
        Reading {
            key: AccountKey::new(service, email, local),
            label: email.unwrap_or(local).to_string(),
            plan: None,
            buckets: pcts
                .iter()
                .enumerate()
                .map(|(i, p)| Bucket {
                    name: format!("b{i}"),
                    used_percent: *p,
                    window_secs: Some(18_000),
                    resets_at_ms: Some(NOW + 3_600_000),
                })
                .collect(),
            source,
            observed_at_ms: NOW,
            banked_resets: None,
        }
    }

    fn ok(kind: SourceKind, rs: Vec<Reading>) -> SourceOutcome {
        SourceOutcome {
            kind,
            service: None,
            attempted_at_ms: NOW,
            result: Ok(rs),
        }
    }

    fn err(kind: SourceKind, service: Option<Service>, e: FetchError) -> SourceOutcome {
        SourceOutcome {
            kind,
            service,
            attempted_at_ms: NOW,
            result: Err(e),
        }
    }

    fn run(outcomes: &[SourceOutcome], last: &mut LastReadings) -> Vec<AccountView> {
        build(outcomes, last, &[Service::Claude, Service::Codex], 120, NOW)
    }

    const E: Option<&str> = Some("user@example.com");

    #[test]
    fn direct_beats_orca_when_both_succeed() {
        let mut last = LastReadings::default();
        let cards = run(
            &[
                ok(
                    SourceKind::Orca,
                    vec![reading(Service::Claude, E, "o", SourceKind::Orca, &[50.0])],
                ),
                ok(
                    SourceKind::Direct,
                    vec![reading(
                        Service::Claude,
                        E,
                        "d",
                        SourceKind::Direct,
                        &[10.0],
                    )],
                ),
            ],
            &mut last,
        );
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].source, Some(SourceKind::Direct));
        assert_eq!(cards[0].headline.as_ref().unwrap().used_percent, 10.0);
    }

    #[test]
    fn orca_used_when_direct_fails_this_cycle() {
        let mut last = LastReadings::default();
        run(
            &[ok(
                SourceKind::Direct,
                vec![reading(
                    Service::Claude,
                    E,
                    "d",
                    SourceKind::Direct,
                    &[10.0],
                )],
            )],
            &mut last,
        );
        let cards = run(
            &[
                err(
                    SourceKind::Direct,
                    Some(Service::Claude),
                    FetchError::Timeout,
                ),
                ok(
                    SourceKind::Orca,
                    vec![reading(Service::Claude, E, "o", SourceKind::Orca, &[50.0])],
                ),
            ],
            &mut last,
        );
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].source, Some(SourceKind::Orca));
        assert_eq!(cards[0].state, CardState::Ok);
    }

    #[test]
    fn email_less_readings_never_merged() {
        let mut last = LastReadings::default();
        let cards = run(
            &[
                ok(
                    SourceKind::Direct,
                    vec![reading(
                        Service::Claude,
                        None,
                        "claude-direct",
                        SourceKind::Direct,
                        &[10.0],
                    )],
                ),
                ok(
                    SourceKind::Orca,
                    vec![reading(Service::Claude, E, "o", SourceKind::Orca, &[50.0])],
                ),
            ],
            &mut last,
        );
        assert_eq!(cards.len(), 2);
    }

    #[test]
    fn emails_merge_case_insensitive() {
        let mut last = LastReadings::default();
        let cards = run(
            &[
                ok(
                    SourceKind::Direct,
                    vec![reading(
                        Service::Codex,
                        Some("User@Example.com"),
                        "d",
                        SourceKind::Direct,
                        &[10.0],
                    )],
                ),
                ok(
                    SourceKind::Orca,
                    vec![reading(Service::Codex, E, "o", SourceKind::Orca, &[50.0])],
                ),
            ],
            &mut last,
        );
        assert_eq!(cards.len(), 1);
    }

    #[test]
    fn vendor_window_wins() {
        let mut last = LastReadings::default();
        let mut direct = reading(Service::Codex, E, "d", SourceKind::Direct, &[52.0]);
        direct.buckets[0].window_secs = Some(2_592_000);
        direct.buckets[0].name = "30 days".into();
        run(&[ok(SourceKind::Direct, vec![direct])], &mut last);
        let cards = run(
            &[
                err(
                    SourceKind::Direct,
                    Some(Service::Codex),
                    FetchError::Timeout,
                ),
                ok(
                    SourceKind::Orca,
                    vec![reading(Service::Codex, E, "o", SourceKind::Orca, &[52.0])],
                ),
            ],
            &mut last,
        );
        assert_eq!(cards[0].source, Some(SourceKind::Orca));
        assert_eq!(cards[0].buckets[0].window_secs, Some(2_592_000));
        assert_eq!(cards[0].buckets[0].name, "30 days");
    }

    #[test]
    fn headline_is_tightest_and_extra_count() {
        let mut last = LastReadings::default();
        let cards = run(
            &[ok(
                SourceKind::Orca,
                vec![reading(
                    Service::Antigravity,
                    E,
                    "o",
                    SourceKind::Orca,
                    &[10.0, 100.0, 15.0],
                )],
            )],
            &mut last,
        );
        let h = cards[0].headline.as_ref().unwrap();
        assert_eq!(h.used_percent, 100.0);
        assert!(h.limit_reached);
        assert_eq!(cards[0].extra_buckets, 2);
    }

    #[test]
    fn headline_skips_a_bucket_whose_reset_passed() {
        let mut last = LastReadings::default();
        let mut r = reading(Service::Claude, E, "d", SourceKind::Direct, &[90.0, 20.0]);
        r.buckets[0].resets_at_ms = Some(NOW - 1);
        let cards = run(&[ok(SourceKind::Direct, vec![r])], &mut last);
        assert_eq!(cards[0].headline.as_ref().unwrap().used_percent, 20.0);
        assert_eq!(cards[0].freshness, Freshness::Live);
    }

    #[test]
    fn reading_with_numbers_beats_higher_priority_empty_one() {
        let mut last = LastReadings::default();
        let cards = run(
            &[
                ok(
                    SourceKind::Direct,
                    vec![reading(Service::Claude, E, "d", SourceKind::Direct, &[])],
                ),
                ok(
                    SourceKind::Orca,
                    vec![reading(Service::Claude, E, "o", SourceKind::Orca, &[40.0])],
                ),
            ],
            &mut last,
        );
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].source, Some(SourceKind::Orca));
        assert_eq!(cards[0].state, CardState::Ok);
    }

    #[test]
    fn forget_drops_only_that_source() {
        let mut last = LastReadings::default();
        run(
            &[
                ok(
                    SourceKind::Orca,
                    vec![reading(Service::Codex, E, "o", SourceKind::Orca, &[5.0])],
                ),
                ok(
                    SourceKind::Direct,
                    vec![reading(Service::Claude, E, "d", SourceKind::Direct, &[7.0])],
                ),
            ],
            &mut last,
        );
        last.forget(SourceKind::Orca, None);
        let cards = run(&[], &mut last);
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].source, Some(SourceKind::Direct));
    }

    #[test]
    fn fallback_to_last_reading_marks_error() {
        let mut last = LastReadings::default();
        run(
            &[ok(
                SourceKind::Orca,
                vec![reading(
                    Service::Antigravity,
                    E,
                    "o",
                    SourceKind::Orca,
                    &[20.0],
                )],
            )],
            &mut last,
        );
        let cards = run(
            &[err(SourceKind::Orca, None, FetchError::Timeout)],
            &mut last,
        );
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].state, CardState::Error);
        assert_eq!(
            cards[0].reason.as_deref(),
            Some(FetchError::Timeout.reason().as_str())
        );
        assert_eq!(cards[0].connect, None);
    }

    #[test]
    fn signed_out_account_disappears() {
        let mut last = LastReadings::default();
        run(
            &[ok(
                SourceKind::Orca,
                vec![reading(
                    Service::Antigravity,
                    E,
                    "o",
                    SourceKind::Orca,
                    &[20.0],
                )],
            )],
            &mut last,
        );
        let cards = run(&[ok(SourceKind::Orca, vec![])], &mut last);
        assert!(cards.is_empty());
    }

    #[test]
    fn empty_antigravity_reading_is_no_reading_yet_with_hint() {
        let mut last = LastReadings::default();
        let cards = run(
            &[ok(
                SourceKind::Orca,
                vec![reading(Service::Antigravity, E, "o", SourceKind::Orca, &[])],
            )],
            &mut last,
        );
        assert_eq!(cards[0].state, CardState::NoReadingYet);
        assert_eq!(cards[0].reason.as_deref(), Some(ANTIGRAVITY_HINT));
        assert!(cards[0].headline.is_none());
    }

    #[test]
    fn expired_direct_with_no_other_source_shows_sign_in_card() {
        let mut last = LastReadings::default();
        let cards = run(
            &[err(
                SourceKind::Direct,
                Some(Service::Claude),
                FetchError::Expired,
            )],
            &mut last,
        );
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].state, CardState::NoSource);
        assert_eq!(cards[0].connect, Some(Service::Claude));
        assert_eq!(cards[0].reason, Some(FetchError::Expired.reason()));
    }

    #[test]
    fn not_detected_direct_adds_no_card() {
        let mut last = LastReadings::default();
        let cards = run(
            &[err(
                SourceKind::Direct,
                Some(Service::Codex),
                FetchError::NotDetected,
            )],
            &mut last,
        );
        assert!(cards.is_empty());
    }
}
