//! Turns the latest source outcomes into one card per account.

use crate::freshness::classify;
use crate::model::{AccountKey, Bucket, FetchError, Reading, Service, SourceKind};
use crate::pace;
use crate::provider::SourceOutcome;
use crate::view::{AccountView, BucketView, CardState, Freshness};
use std::collections::{BTreeMap, HashMap, HashSet};

const ANTIGRAVITY_HINT: &str = "Open Antigravity to see its limits";
const LOGIN_EXPIRED: &str = "Login expired";
const PROFILE_MISSING: &str = "Profile folder missing";

/// What a scheduler task reads: the current login of a CLI or app, an added account's profile, or neither (Orca).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskRole {
    CurrentLogin,
    AddedAccount,
    Other,
}

/// One scheduler task. Its `id` is the `SourceOutcome::task_id` it produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskMeta {
    pub id: String,
    pub role: TaskRole,
}

/// The user's choices and the task list that `build` applies to the cards. All keys are card IDs.
pub struct MergeCtx<'a> {
    pub labels: &'a BTreeMap<String, String>,
    pub pinned: &'a [String],
    pub hidden: &'a [String],
    pub tasks: &'a [TaskMeta],
}

impl MergeCtx<'_> {
    fn role(&self, task_id: &str) -> TaskRole {
        self.tasks
            .iter()
            .find(|t| t.id == task_id)
            .map_or(TaskRole::Other, |t| t.role)
    }
}

/// Last successful reading per account, then per (source kind, task). Memory only; tasks refill it after a restart.
/// The inner order is the source priority: `SourceKind` (Direct, AntigravityApp, Orca), then task ID.
#[derive(Debug, Default)]
pub struct LastReadings(HashMap<String, BTreeMap<(SourceKind, String), Reading>>);

impl LastReadings {
    /// Drops every stored reading of a task that is gone (no longer detected, or removed), so its old numbers do not
    /// stay on screen. Returns true when it dropped something.
    pub fn forget(&mut self, task_id: &str) -> bool {
        let mut dropped = false;
        for per_task in self.0.values_mut() {
            per_task.retain(|(_, t), _| {
                let keep = t != task_id;
                dropped |= !keep;
                keep
            });
        }
        self.0.retain(|_, m| !m.is_empty());
        dropped
    }

    /// The card ID of a stored reading of this task, if any. The lowest ID wins, so the answer is stable.
    fn key_of(&self, task_id: &str) -> Option<String> {
        self.0
            .iter()
            .filter(|(_, per_task)| per_task.keys().any(|(_, t)| t == task_id))
            .map(|(id, _)| id)
            .min()
            .cloned()
    }
}

pub fn build(
    outcomes: &[SourceOutcome],
    last: &mut LastReadings,
    ctx: &MergeCtx,
    refresh_secs: u64,
    now_ms: i64,
) -> Vec<AccountView> {
    // 1. Forget accounts that a task which answered no longer reports (signed out). Only that task's readings go:
    //    other logins of the same service are separate tasks.
    let succeeded: HashSet<&str> = outcomes
        .iter()
        .filter(|o| o.result.is_ok())
        .map(|o| o.task_id.as_str())
        .collect();
    for per_task in last.0.values_mut() {
        per_task.retain(|(_, t), _| !succeeded.contains(t.as_str()));
    }
    // 2. Store everything that just arrived.
    let mut live: HashSet<(String, SourceKind, String)> = HashSet::new();
    for o in outcomes {
        if let Ok(readings) = &o.result {
            for r in readings {
                let id = r.key.store_key();
                live.insert((id.clone(), r.source, o.task_id.clone()));
                last.0
                    .entry(id)
                    .or_default()
                    .insert((r.source, o.task_id.clone()), r.clone());
            }
        }
    }
    last.0.retain(|_, m| !m.is_empty());

    let mut cards: Vec<AccountView> = Vec::new();
    let mut covered: HashSet<&str> = HashSet::new();
    for (id, per_task) in &last.0 {
        // A task that answered this cycle wins over stored readings. Among them, one with numbers beats one without
        // (Orca can list an account it has no limits for). Otherwise the map order breaks the tie: `min_by_key`
        // keeps the first of equal keys.
        let chosen = per_task
            .iter()
            .filter(|((kind, task), _)| live.contains(&(id.clone(), *kind, task.clone())))
            .min_by_key(|(_, r)| r.buckets.is_empty())
            .map(|(k, r)| (k, r, true))
            .or_else(|| {
                per_task
                    .iter()
                    .max_by_key(|(_, r)| (!r.buckets.is_empty(), r.observed_at_ms))
                    .map(|(k, r)| (k, r, false))
            });
        let Some(((kind, task), reading, fresh)) = chosen else {
            continue;
        };

        let failure = (!fresh).then(|| latest_error(outcomes, task)).flatten();
        let mut view = card(reading, *kind, failure, refresh_secs, now_ms);
        let tasks = per_task.keys().map(|(_, t)| t.as_str());
        view.current_login = tasks.clone().any(|t| ctx.role(t) == TaskRole::CurrentLogin);
        view.profile_id = tasks
            .clone()
            .find(|t| ctx.role(t) == TaskRole::AddedAccount)
            .map(str::to_string);
        covered.extend(tasks);
        cards.push(view);
    }

    // 3. A task failed and no card shows its reading: show why. A single-account task (Claude, Codex) always gets its
    //    own card; a multi-account task (Antigravity) only when nothing else knows that service.
    for o in outcomes {
        let (Some(service), Err(e)) = (o.service, &o.result) else {
            continue;
        };
        if matches!(e, FetchError::NotDetected | FetchError::Disabled)
            || covered.contains(o.task_id.as_str())
            || (o.kind != SourceKind::Direct && cards.iter().any(|c| c.service == service))
        {
            continue;
        }
        let (state, reason) = failure_state(o.kind, e).unwrap_or((CardState::NoSource, e.reason()));
        let role = ctx.role(&o.task_id);
        // Reuse the card ID of a stored reading of this task, so labels, pins and hidden carry over. A task with a
        // stored reading is normally `covered` above; this keeps the ID right if that ever changes.
        let id = last
            .key_of(&o.task_id)
            .unwrap_or_else(|| AccountKey::new(service, None, o.task_id.as_str()).store_key());
        cards.push(AccountView {
            id,
            service,
            label: display_name(service).to_string(),
            plan: None,
            source: Some(o.kind),
            freshness: Freshness::Stale,
            age_secs: None,
            headline: None,
            buckets: vec![],
            state,
            reason: Some(reason),
            room_left: None,
            best_now: false,
            banked_resets: None,
            profile_id: (role == TaskRole::AddedAccount).then(|| o.task_id.clone()),
            current_login: role == TaskRole::CurrentLogin,
            pinned: false,
            hidden: false,
        });
    }

    // 4. The user's choices.
    for c in &mut cards {
        if let Some(label) = ctx.labels.get(&c.id) {
            c.label = label.clone();
        }
        c.pinned = ctx.pinned.contains(&c.id);
        c.hidden = ctx.hidden.contains(&c.id);
    }

    // 5. Best now, among the accounts the user can switch to.
    let rooms: Vec<(Option<f64>, Option<i64>)> = cards
        .iter()
        .map(|c| {
            let usable = !c.hidden
                && !matches!(c.state, CardState::LoginExpired | CardState::ProfileMissing)
                && !c.buckets.is_empty();
            // A bucket whose reset has passed no longer holds, so its reset does not break a tie.
            let reset = c
                .buckets
                .iter()
                .filter(|b| b.freshness != Freshness::Stale)
                .filter_map(|b| b.resets_at_ms)
                .min();
            (c.room_left.filter(|_| usable), reset)
        })
        .collect();
    if let Some(best) = pace::best_now(&rooms) {
        cards[best].best_now = true;
    }

    // 6. A fixed order: service, the current login first, label, then ID (unique) so equal labels never swap.
    cards.sort_by(|a, b| {
        a.service
            .cmp(&b.service)
            .then_with(|| b.current_login.cmp(&a.current_login))
            .then_with(|| a.label.cmp(&b.label))
            .then_with(|| a.id.cmp(&b.id))
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

/// The latest error of one task, if its last attempt failed.
fn latest_error(outcomes: &[SourceOutcome], task_id: &str) -> Option<FetchError> {
    outcomes
        .iter()
        .filter(|o| o.task_id == task_id)
        .find_map(|o| o.result.as_ref().err().cloned())
}

/// The states with their own wording. An expired login only counts for a single-account source.
fn failure_state(kind: SourceKind, e: &FetchError) -> Option<(CardState, String)> {
    match e {
        FetchError::Expired if kind == SourceKind::Direct => {
            Some((CardState::LoginExpired, LOGIN_EXPIRED.into()))
        }
        FetchError::ProfileMissing => Some((CardState::ProfileMissing, PROFILE_MISSING.into())),
        _ => None,
    }
}

fn card(
    r: &Reading,
    kind: SourceKind,
    failure: Option<FetchError>,
    refresh_secs: u64,
    now_ms: i64,
) -> AccountView {
    let buckets: Vec<BucketView> = r
        .buckets
        .iter()
        .map(|b| {
            let passed = pace::passed_percent(b, now_ms);
            BucketView {
                name: b.name.clone(),
                used_percent: b.used_percent,
                window_secs: b.window_secs,
                resets_at_ms: b.resets_at_ms,
                limit_reached: b.used_percent >= 100.0,
                freshness: classify(r.observed_at_ms, refresh_secs, b, now_ms),
                passed_percent: passed,
                fast_burn: pace::fast_burn(b.used_percent, passed),
            }
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
    // Room left counts only the limits that still hold.
    let current: Vec<Bucket> = r
        .buckets
        .iter()
        .zip(&buckets)
        .filter(|(_, v)| v.freshness != Freshness::Stale)
        .map(|(b, _)| b.clone())
        .collect();
    let (state, reason) = match (&failure, buckets.is_empty()) {
        (Some(e), _) => failure_state(kind, e).unwrap_or((CardState::Error, e.reason())),
        (None, true) if r.key.service == Service::Antigravity => {
            (CardState::NoReadingYet, ANTIGRAVITY_HINT.into())
        }
        (None, true) => (CardState::NoReadingYet, String::new()),
        (None, false) => (CardState::Ok, String::new()),
    };
    AccountView {
        id: r.key.store_key(),
        service: r.key.service,
        label: r.label.clone(),
        plan: r.plan.clone(),
        source: Some(r.source),
        freshness: headline
            .as_ref()
            .map(|h| h.freshness)
            .unwrap_or(Freshness::Stale),
        age_secs: Some((now_ms.saturating_sub(r.observed_at_ms).max(0) / 1000) as u64),
        headline,
        buckets,
        state,
        reason: (!reason.is_empty()).then_some(reason),
        room_left: pace::room_left(&current),
        best_now: false,
        banked_resets: r.banked_resets,
        profile_id: None,
        current_login: false,
        pinned: false,
        hidden: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AccountKey, Bucket};

    const NOW: i64 = 1_800_000_000_000;
    const FIVE_H: u64 = 18_000;

    fn tasks() -> Vec<TaskMeta> {
        [
            ("claude-current", TaskRole::CurrentLogin),
            ("codex-current", TaskRole::CurrentLogin),
            ("antigravity-app", TaskRole::CurrentLogin),
            ("orca", TaskRole::Other),
            ("p1", TaskRole::AddedAccount),
            ("p2", TaskRole::AddedAccount),
        ]
        .into_iter()
        .map(|(id, role)| TaskMeta {
            id: id.into(),
            role,
        })
        .collect()
    }

    fn reading(
        service: Service,
        email: Option<&str>,
        local: &str,
        source: SourceKind,
        pcts: &[f64],
    ) -> Reading {
        Reading {
            key: AccountKey::new(service, email, local),
            label: display_name(service).to_string(),
            plan: None,
            buckets: pcts
                .iter()
                .enumerate()
                .map(|(i, p)| Bucket {
                    name: format!("b{i}"),
                    used_percent: *p,
                    window_secs: Some(FIVE_H),
                    resets_at_ms: Some(NOW + 3_600_000),
                })
                .collect(),
            source,
            observed_at_ms: NOW,
            banked_resets: None,
        }
    }

    fn service_of(kind: SourceKind, rs: &[Reading]) -> Option<Service> {
        match kind {
            SourceKind::Direct => rs.first().map(|r| r.key.service),
            _ => Some(Service::Antigravity),
        }
    }

    fn ok(task: &str, kind: SourceKind, rs: Vec<Reading>) -> SourceOutcome {
        SourceOutcome {
            task_id: task.into(),
            kind,
            service: service_of(kind, &rs),
            attempted_at_ms: NOW,
            result: Ok(rs),
        }
    }

    fn err(task: &str, kind: SourceKind, service: Service, e: FetchError) -> SourceOutcome {
        SourceOutcome {
            task_id: task.into(),
            kind,
            service: Some(service),
            attempted_at_ms: NOW,
            result: Err(e),
        }
    }

    struct Prefs {
        labels: BTreeMap<String, String>,
        pinned: Vec<String>,
        hidden: Vec<String>,
    }

    impl Prefs {
        fn none() -> Self {
            Self {
                labels: BTreeMap::new(),
                pinned: vec![],
                hidden: vec![],
            }
        }
    }

    fn run_with(
        outcomes: &[SourceOutcome],
        last: &mut LastReadings,
        prefs: &Prefs,
    ) -> Vec<AccountView> {
        let tasks = tasks();
        let ctx = MergeCtx {
            labels: &prefs.labels,
            pinned: &prefs.pinned,
            hidden: &prefs.hidden,
            tasks: &tasks,
        };
        build(outcomes, last, &ctx, 120, NOW)
    }

    fn run(outcomes: &[SourceOutcome], last: &mut LastReadings) -> Vec<AccountView> {
        run_with(outcomes, last, &Prefs::none())
    }

    fn claude(task: &str, email: &str, pct: f64) -> SourceOutcome {
        ok(
            task,
            SourceKind::Direct,
            vec![reading(
                Service::Claude,
                Some(email),
                task,
                SourceKind::Direct,
                &[pct],
            )],
        )
    }

    fn id_of(service: Service, email: &str) -> String {
        AccountKey::new(service, Some(email), "x").store_key()
    }

    const E: Option<&str> = Some("user@example.com");

    #[test]
    fn app_beats_orca_when_both_succeed() {
        let mut last = LastReadings::default();
        let cards = run(
            &[
                ok(
                    "orca",
                    SourceKind::Orca,
                    vec![reading(
                        Service::Antigravity,
                        E,
                        "acc-1",
                        SourceKind::Orca,
                        &[50.0],
                    )],
                ),
                ok(
                    "antigravity-app",
                    SourceKind::AntigravityApp,
                    vec![reading(
                        Service::Antigravity,
                        E,
                        "antigravity-app",
                        SourceKind::AntigravityApp,
                        &[10.0],
                    )],
                ),
            ],
            &mut last,
        );
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].source, Some(SourceKind::AntigravityApp));
        assert_eq!(cards[0].headline.as_ref().unwrap().used_percent, 10.0);
    }

    #[test]
    fn orca_and_app_same_email_one_card() {
        let mut last = LastReadings::default();
        let cards = run(
            &[
                ok(
                    "antigravity-app",
                    SourceKind::AntigravityApp,
                    vec![reading(
                        Service::Antigravity,
                        Some("User@Example.com"),
                        "antigravity-app",
                        SourceKind::AntigravityApp,
                        &[10.0],
                    )],
                ),
                ok(
                    "orca",
                    SourceKind::Orca,
                    vec![reading(
                        Service::Antigravity,
                        E,
                        "acc-1",
                        SourceKind::Orca,
                        &[50.0],
                    )],
                ),
            ],
            &mut last,
        );
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].id, id_of(Service::Antigravity, "user@example.com"));
        assert!(cards[0].current_login, "the app login reads it");
        assert_eq!(cards[0].profile_id, None);
    }

    #[test]
    fn orca_used_when_app_fails_this_cycle() {
        let mut last = LastReadings::default();
        run(
            &[ok(
                "antigravity-app",
                SourceKind::AntigravityApp,
                vec![reading(
                    Service::Antigravity,
                    E,
                    "antigravity-app",
                    SourceKind::AntigravityApp,
                    &[10.0],
                )],
            )],
            &mut last,
        );
        let cards = run(
            &[
                err(
                    "antigravity-app",
                    SourceKind::AntigravityApp,
                    Service::Antigravity,
                    FetchError::Timeout,
                ),
                ok(
                    "orca",
                    SourceKind::Orca,
                    vec![reading(
                        Service::Antigravity,
                        E,
                        "acc-1",
                        SourceKind::Orca,
                        &[50.0],
                    )],
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
                    "antigravity-app",
                    SourceKind::AntigravityApp,
                    vec![reading(
                        Service::Antigravity,
                        None,
                        "antigravity-app",
                        SourceKind::AntigravityApp,
                        &[10.0],
                    )],
                ),
                ok(
                    "orca",
                    SourceKind::Orca,
                    vec![reading(
                        Service::Antigravity,
                        E,
                        "acc-1",
                        SourceKind::Orca,
                        &[50.0],
                    )],
                ),
            ],
            &mut last,
        );
        assert_eq!(cards.len(), 2);
    }

    #[test]
    fn one_task_success_keeps_other_task_readings() {
        let mut last = LastReadings::default();
        run(
            &[
                claude("claude-current", "a@example.com", 10.0),
                claude("p1", "b@example.com", 20.0),
            ],
            &mut last,
        );
        // Only the current login answered this cycle; the profile's reading must stay.
        let cards = run(
            &[claude("claude-current", "a@example.com", 11.0)],
            &mut last,
        );
        assert_eq!(cards.len(), 2);
        let profile = cards
            .iter()
            .find(|c| c.id == id_of(Service::Claude, "b@example.com"))
            .unwrap();
        assert_eq!(profile.profile_id.as_deref(), Some("p1"));
        assert!(!profile.current_login);
    }

    #[test]
    fn headline_is_tightest() {
        let mut last = LastReadings::default();
        let cards = run(
            &[ok(
                "orca",
                SourceKind::Orca,
                vec![reading(
                    Service::Antigravity,
                    E,
                    "acc-1",
                    SourceKind::Orca,
                    &[10.0, 100.0, 15.0],
                )],
            )],
            &mut last,
        );
        let h = cards[0].headline.as_ref().unwrap();
        assert_eq!(h.used_percent, 100.0);
        assert!(h.limit_reached);
        assert_eq!(cards[0].room_left, Some(0.0));
    }

    #[test]
    fn headline_skips_a_bucket_whose_reset_passed() {
        let mut last = LastReadings::default();
        let mut r = reading(
            Service::Claude,
            E,
            "claude-current",
            SourceKind::Direct,
            &[90.0, 20.0],
        );
        r.buckets[0].resets_at_ms = Some(NOW - 1);
        let cards = run(
            &[ok("claude-current", SourceKind::Direct, vec![r])],
            &mut last,
        );
        assert_eq!(cards[0].headline.as_ref().unwrap().used_percent, 20.0);
        assert_eq!(cards[0].freshness, Freshness::Live);
        assert_eq!(
            cards[0].room_left,
            Some(80.0),
            "a stale bucket gives no room"
        );
    }

    #[test]
    fn reading_with_numbers_beats_higher_priority_empty_one() {
        let mut last = LastReadings::default();
        let cards = run(
            &[
                ok(
                    "antigravity-app",
                    SourceKind::AntigravityApp,
                    vec![reading(
                        Service::Antigravity,
                        E,
                        "antigravity-app",
                        SourceKind::AntigravityApp,
                        &[],
                    )],
                ),
                ok(
                    "orca",
                    SourceKind::Orca,
                    vec![reading(
                        Service::Antigravity,
                        E,
                        "acc-1",
                        SourceKind::Orca,
                        &[40.0],
                    )],
                ),
            ],
            &mut last,
        );
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].source, Some(SourceKind::Orca));
        assert_eq!(cards[0].state, CardState::Ok);
    }

    #[test]
    fn forget_drops_only_that_task() {
        let mut last = LastReadings::default();
        run(
            &[
                ok(
                    "orca",
                    SourceKind::Orca,
                    vec![reading(
                        Service::Antigravity,
                        E,
                        "acc-1",
                        SourceKind::Orca,
                        &[5.0],
                    )],
                ),
                claude("claude-current", "a@example.com", 7.0),
            ],
            &mut last,
        );
        assert!(last.forget("orca"));
        assert!(!last.forget("orca"), "nothing left to drop");
        let cards = run(&[], &mut last);
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].service, Service::Claude);
    }

    #[test]
    fn fallback_to_last_reading_marks_error() {
        let mut last = LastReadings::default();
        run(
            &[ok(
                "orca",
                SourceKind::Orca,
                vec![reading(
                    Service::Antigravity,
                    E,
                    "acc-1",
                    SourceKind::Orca,
                    &[20.0],
                )],
            )],
            &mut last,
        );
        let cards = run(
            &[err(
                "orca",
                SourceKind::Orca,
                Service::Antigravity,
                FetchError::Timeout,
            )],
            &mut last,
        );
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].state, CardState::Error);
        assert_eq!(
            cards[0].reason.as_deref(),
            Some(FetchError::Timeout.reason().as_str())
        );
    }

    #[test]
    fn signed_out_account_disappears() {
        let mut last = LastReadings::default();
        run(
            &[ok(
                "orca",
                SourceKind::Orca,
                vec![reading(
                    Service::Antigravity,
                    E,
                    "acc-1",
                    SourceKind::Orca,
                    &[20.0],
                )],
            )],
            &mut last,
        );
        let cards = run(&[ok("orca", SourceKind::Orca, vec![])], &mut last);
        assert!(cards.is_empty());
    }

    #[test]
    fn empty_antigravity_reading_is_no_reading_yet_with_hint() {
        let mut last = LastReadings::default();
        let cards = run(
            &[ok(
                "orca",
                SourceKind::Orca,
                vec![reading(
                    Service::Antigravity,
                    E,
                    "acc-1",
                    SourceKind::Orca,
                    &[],
                )],
            )],
            &mut last,
        );
        assert_eq!(cards[0].state, CardState::NoReadingYet);
        assert_eq!(
            cards[0].reason.as_deref(),
            Some("Open Antigravity to see its limits")
        );
        assert!(cards[0].headline.is_none());
        assert_eq!(cards[0].room_left, None);
    }

    #[test]
    fn expired_login_state() {
        let mut last = LastReadings::default();
        let cards = run(
            &[
                claude("p1", "b@example.com", 20.0),
                err(
                    "claude-current",
                    SourceKind::Direct,
                    Service::Claude,
                    FetchError::Expired,
                ),
            ],
            &mut last,
        );
        assert_eq!(
            cards.len(),
            2,
            "another Claude card does not hide the expired one"
        );
        let expired = cards
            .iter()
            .find(|c| c.state == CardState::LoginExpired)
            .unwrap();
        assert_eq!(expired.id, "claude:claude-current");
        assert_eq!(expired.label, "Claude");
        assert_eq!(expired.reason.as_deref(), Some("Login expired"));
        assert!(expired.current_login);
        assert_eq!(expired.room_left, None);

        // A stored reading whose login then expires keeps its numbers but changes state.
        let cards = run(
            &[err(
                "p1",
                SourceKind::Direct,
                Service::Claude,
                FetchError::Expired,
            )],
            &mut last,
        );
        let profile = cards
            .iter()
            .find(|c| c.id == id_of(Service::Claude, "b@example.com"))
            .unwrap();
        assert_eq!(profile.state, CardState::LoginExpired);
        assert_eq!(profile.reason.as_deref(), Some("Login expired"));
        assert_eq!(profile.profile_id.as_deref(), Some("p1"));
    }

    #[test]
    fn profile_missing_state() {
        let mut last = LastReadings::default();
        let cards = run(
            &[err(
                "p2",
                SourceKind::Direct,
                Service::Codex,
                FetchError::ProfileMissing,
            )],
            &mut last,
        );
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].state, CardState::ProfileMissing);
        assert_eq!(cards[0].id, "codex:p2");
        assert_eq!(cards[0].label, "Codex");
        assert_eq!(cards[0].reason.as_deref(), Some("Profile folder missing"));
        assert_eq!(cards[0].profile_id.as_deref(), Some("p2"));
        assert!(!cards[0].current_login);
    }

    #[test]
    fn not_detected_adds_no_card() {
        let mut last = LastReadings::default();
        let cards = run(
            &[err(
                "codex-current",
                SourceKind::Direct,
                Service::Codex,
                FetchError::NotDetected,
            )],
            &mut last,
        );
        assert!(cards.is_empty());
    }

    #[test]
    fn fast_burn_flag_in_view() {
        let mut last = LastReadings::default();
        let mut r = reading(
            Service::Claude,
            E,
            "claude-current",
            SourceKind::Direct,
            &[62.0, 44.0],
        );
        // 30 % of a 5 h window has passed: the reset is 70 % of the window away.
        for b in &mut r.buckets {
            b.resets_at_ms = Some(NOW + 12_600_000);
        }
        let cards = run(
            &[ok("claude-current", SourceKind::Direct, vec![r])],
            &mut last,
        );
        let b = &cards[0].buckets;
        assert!((b[0].passed_percent.unwrap() - 30.0).abs() < 1e-9);
        assert!(b[0].fast_burn);
        assert!(!b[1].fast_burn);
        assert!(cards[0].headline.as_ref().unwrap().fast_burn);
    }

    #[test]
    fn best_now_skips_hidden_and_expired() {
        let mut last = LastReadings::default();
        let mut prefs = Prefs::none();
        prefs.hidden = vec![id_of(Service::Claude, "a@example.com")];
        let all = [
            claude("claude-current", "a@example.com", 10.0),
            claude("p1", "b@example.com", 20.0),
            claude("p2", "c@example.com", 50.0),
            ok(
                "codex-current",
                SourceKind::Direct,
                vec![reading(
                    Service::Codex,
                    Some("d@example.com"),
                    "codex-current",
                    SourceKind::Direct,
                    &[60.0],
                )],
            ),
        ];
        run_with(&all, &mut last, &prefs);
        let cards = run_with(
            &[
                all[0].clone(),
                err(
                    "p1",
                    SourceKind::Direct,
                    Service::Claude,
                    FetchError::Expired,
                ),
                all[2].clone(),
                all[3].clone(),
            ],
            &mut last,
            &prefs,
        );
        let best: Vec<&str> = cards
            .iter()
            .filter(|c| c.best_now)
            .map(|c| c.id.as_str())
            .collect();
        assert_eq!(best, [id_of(Service::Claude, "c@example.com").as_str()]);
    }

    #[test]
    fn best_now_tie_break_ignores_stale_resets() {
        let mut last = LastReadings::default();
        // Same room (80) on both. A's earliest reset is on a bucket whose reset passed, so it must not count.
        let mut a = reading(
            Service::Claude,
            Some("a@example.com"),
            "claude-current",
            SourceKind::Direct,
            &[90.0, 20.0],
        );
        a.buckets[0].resets_at_ms = Some(NOW - 1);
        a.buckets[1].resets_at_ms = Some(NOW + 7_200_000);
        let mut b = reading(
            Service::Claude,
            Some("b@example.com"),
            "p1",
            SourceKind::Direct,
            &[20.0],
        );
        b.buckets[0].resets_at_ms = Some(NOW + 3_600_000);
        let cards = run(
            &[
                ok("claude-current", SourceKind::Direct, vec![a]),
                ok("p1", SourceKind::Direct, vec![b]),
            ],
            &mut last,
        );
        let best: Vec<&str> = cards
            .iter()
            .filter(|c| c.best_now)
            .map(|c| c.id.as_str())
            .collect();
        assert_eq!(best, [id_of(Service::Claude, "b@example.com").as_str()]);
    }

    #[test]
    fn all_stale_account_is_never_best_now() {
        let mut last = LastReadings::default();
        let mut a = reading(
            Service::Claude,
            Some("a@example.com"),
            "claude-current",
            SourceKind::Direct,
            &[1.0, 2.0],
        );
        for bucket in &mut a.buckets {
            bucket.resets_at_ms = Some(NOW - 1);
        }
        let cards = run(
            &[
                ok("claude-current", SourceKind::Direct, vec![a]),
                claude("p1", "b@example.com", 50.0),
                claude("p2", "c@example.com", 60.0),
            ],
            &mut last,
        );
        let stale = cards
            .iter()
            .find(|c| c.id == id_of(Service::Claude, "a@example.com"))
            .unwrap();
        assert!(!stale.best_now);
        assert_eq!(stale.room_left, None);
        let best: Vec<&str> = cards
            .iter()
            .filter(|c| c.best_now)
            .map(|c| c.id.as_str())
            .collect();
        assert_eq!(best, [id_of(Service::Claude, "b@example.com").as_str()]);
    }

    #[test]
    fn hidden_current_login_stays_hidden_when_its_task_errors() {
        let mut last = LastReadings::default();
        let mut prefs = Prefs::none();
        let id = id_of(Service::Claude, "a@example.com");
        prefs.hidden = vec![id.clone()];
        prefs.pinned = vec![id.clone()];
        prefs.labels.insert(id.clone(), "Work".into());
        run_with(
            &[claude("claude-current", "a@example.com", 10.0)],
            &mut last,
            &prefs,
        );
        for e in [FetchError::Expired, FetchError::Timeout] {
            let cards = run_with(
                &[err(
                    "claude-current",
                    SourceKind::Direct,
                    Service::Claude,
                    e,
                )],
                &mut last,
                &prefs,
            );
            assert_eq!(cards.len(), 1, "no second card under the task's own ID");
            assert_eq!(cards[0].id, id);
            assert!(cards[0].hidden && cards[0].pinned);
            assert_eq!(cards[0].label, "Work");
            assert_ne!(cards[0].state, CardState::Ok);
        }
    }

    #[test]
    fn error_card_reuses_remembered_key() {
        let mut last = LastReadings::default();
        let mut prefs = Prefs::none();
        let id = id_of(Service::Claude, "a@example.com");
        prefs.hidden = vec![id.clone()];
        run_with(
            &[claude("claude-current", "a@example.com", 10.0)],
            &mut last,
            &prefs,
        );
        assert_eq!(last.key_of("claude-current").as_deref(), Some(id.as_str()));
        assert_eq!(last.key_of("p1"), None);
        // Without a remembered key the error card falls back to the task's own ID.
        let cards = run_with(
            &[err(
                "p1",
                SourceKind::Direct,
                Service::Claude,
                FetchError::Expired,
            )],
            &mut last,
            &prefs,
        );
        assert!(cards.iter().any(|c| c.id == "claude:p1" && !c.hidden));
    }

    #[test]
    fn label_override_applied() {
        let mut last = LastReadings::default();
        let mut prefs = Prefs::none();
        let id = id_of(Service::Claude, "a@example.com");
        prefs.labels.insert(id.clone(), "Work".into());
        let cards = run_with(
            &[claude("claude-current", "a@example.com", 10.0)],
            &mut last,
            &prefs,
        );
        assert_eq!(cards[0].id, id);
        assert_eq!(cards[0].label, "Work");
    }

    #[test]
    fn pinned_and_hidden_flags() {
        let mut last = LastReadings::default();
        let mut prefs = Prefs::none();
        let a = id_of(Service::Claude, "a@example.com");
        let b = id_of(Service::Claude, "b@example.com");
        prefs.pinned = vec![a.clone()];
        prefs.hidden = vec![b.clone()];
        let cards = run_with(
            &[
                claude("claude-current", "a@example.com", 10.0),
                claude("p1", "b@example.com", 10.0),
            ],
            &mut last,
            &prefs,
        );
        let find = |id: &str| cards.iter().find(|c| c.id == id).unwrap();
        assert!(find(&a).pinned && !find(&a).hidden);
        assert!(find(&b).hidden && !find(&b).pinned);
    }

    #[test]
    fn cards_sorted_deterministically() {
        let mut prefs = Prefs::none();
        // A profile label that sorts before "Claude" still comes after the current login.
        prefs
            .labels
            .insert(id_of(Service::Claude, "b@example.com"), "A team".into());
        let outcomes = vec![
            ok(
                "antigravity-app",
                SourceKind::AntigravityApp,
                vec![reading(
                    Service::Antigravity,
                    Some("z@example.com"),
                    "antigravity-app",
                    SourceKind::AntigravityApp,
                    &[5.0],
                )],
            ),
            ok(
                "p2",
                SourceKind::Direct,
                vec![reading(
                    Service::Codex,
                    None,
                    "p2",
                    SourceKind::Direct,
                    &[5.0],
                )],
            ),
            ok(
                "p1",
                SourceKind::Direct,
                vec![reading(
                    Service::Codex,
                    None,
                    "p1",
                    SourceKind::Direct,
                    &[5.0],
                )],
            ),
            claude("p1", "b@example.com", 5.0),
            claude("claude-current", "a@example.com", 5.0),
            ok(
                "codex-current",
                SourceKind::Direct,
                vec![reading(
                    Service::Codex,
                    None,
                    "codex-current",
                    SourceKind::Direct,
                    &[5.0],
                )],
            ),
        ];
        let ids = |cards: Vec<AccountView>| cards.into_iter().map(|c| c.id).collect::<Vec<_>>();
        let forward = ids(run_with(&outcomes, &mut LastReadings::default(), &prefs));
        let mut reversed = outcomes.clone();
        reversed.reverse();
        let backward = ids(run_with(&reversed, &mut LastReadings::default(), &prefs));
        assert_eq!(forward, backward);
        assert_eq!(
            forward,
            [
                id_of(Service::Claude, "a@example.com"),
                id_of(Service::Claude, "b@example.com"),
                "codex:codex-current".to_string(),
                "codex:p1".to_string(),
                "codex:p2".to_string(),
                id_of(Service::Antigravity, "z@example.com"),
            ]
        );
    }
}
