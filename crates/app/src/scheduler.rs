//! Pure-Tokio scheduler driving `quotaglass_core` providers without Tauri.
//! Supports deterministic multi-account Claude staggering, 5s open debounce,
//! 1-click instant refresh bypassing backoff/429 with per-task in-flight deduplication,
//! and atomic `readings.json` + `snapshot_cache.json` persistence.

use crate::connect::sign_in_tool;
use crate::settings::{Connection, Settings};
use crate::state::{JsonStore, SnapshotCache};
use quotaglass_core::merge::{self, LastReadings, MergeCtx, SavedReading, TaskMeta, TaskRole};
use quotaglass_core::sources::antigravity_app::AntigravityApp;
use quotaglass_core::sources::claude_direct::ClaudeDirect;
use quotaglass_core::sources::codex_direct::CodexDirect;
use quotaglass_core::view::Snapshot;
use quotaglass_core::{
    rate_limit_wait_secs, stagger_offset_secs, Env, FetchError, Provider, Reading, Service,
    SourceKind, SourceOutcome,
};
use serde::Serialize;
use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::{mpsc, Notify};
use tokio::task::JoinHandle;
use tokio::time::{interval_at, Instant, Interval, MissedTickBehavior};

const REPUBLISH_SECS: u64 = 30;
const BACKOFF_CAP_SECS: u64 = 900;
const CLAUDE_FLOOR_SECS: u64 = 300;
pub const OPEN_REFRESH_DEBOUNCE: Duration = Duration::from_secs(5);

const FLIGHT_IDLE: u8 = 0;
const FLIGHT_RUNNING: u8 = 1;
const FLIGHT_FORCED: u8 = 2;

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn update_atomic_u8(atom: &AtomicU8, mut f: impl FnMut(u8) -> Option<u8>) -> Result<u8, u8> {
    let mut cur = atom.load(Ordering::Acquire);
    loop {
        let Some(next) = f(cur) else {
            return Err(cur);
        };
        match atom.compare_exchange_weak(cur, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(prev) => return Ok(prev),
            Err(actual) => cur = actual,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SchedulerEvent {
    Snapshot(Snapshot),
    Refreshing(bool),
}

#[derive(Debug, Clone)]
enum Target {
    ClaudeCurrent,
    CodexCurrent,
    AntigravityApp,
    ClaudeProfile(PathBuf),
    CodexProfile(PathBuf),
}

#[derive(Debug, Clone)]
struct Entry {
    id: String,
    target: Target,
}

impl Entry {
    fn is_claude(&self) -> bool {
        matches!(
            self.target,
            Target::ClaudeCurrent | Target::ClaudeProfile(_)
        )
    }

    fn role(&self) -> TaskRole {
        match self.target {
            Target::ClaudeCurrent | Target::CodexCurrent | Target::AntigravityApp => {
                TaskRole::CurrentLogin
            }
            Target::ClaudeProfile(_) | Target::CodexProfile(_) => TaskRole::AddedAccount,
        }
    }

    fn base_secs(&self, refresh_minutes: u8) -> u64 {
        let refresh = u64::from(refresh_minutes) * 60;
        if self.is_claude() {
            refresh.max(CLAUDE_FLOOR_SECS)
        } else {
            refresh
        }
    }

    fn profile_dir(&self) -> Option<PathBuf> {
        match &self.target {
            Target::ClaudeProfile(d) | Target::CodexProfile(d) => Some(d.clone()),
            _ => None,
        }
    }

    fn provider(&self, env: &Env) -> Box<dyn Provider> {
        match &self.target {
            Target::ClaudeCurrent => Box::new(ClaudeDirect {
                dir: env.default_claude_dir.clone(),
                local_id: self.id.clone(),
            }),
            Target::CodexCurrent => Box::new(CodexDirect {
                dir: env.default_codex_dir.clone(),
                local_id: self.id.clone(),
            }),
            Target::AntigravityApp => Box::new(AntigravityApp),
            Target::ClaudeProfile(dir) => Box::new(ClaudeDirect {
                dir: dir.clone(),
                local_id: self.id.clone(),
            }),
            Target::CodexProfile(dir) => Box::new(CodexDirect {
                dir: dir.clone(),
                local_id: self.id.clone(),
            }),
        }
    }
}

fn entries(settings: &Settings) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    let mut push = |id: &str, target: Target| {
        if !out.iter().any(|e| e.id == id) {
            out.push(Entry {
                id: id.to_string(),
                target,
            });
        }
    };
    for c in &settings.connected {
        match c {
            Connection::ClaudeCurrent => push("claude-current", Target::ClaudeCurrent),
            Connection::CodexCurrent => push("codex-current", Target::CodexCurrent),
            Connection::AntigravityApp => push("antigravity-app", Target::AntigravityApp),
        }
    }
    for a in &settings.added_accounts {
        let dir = a.profile_dir.clone();
        match a.service {
            Service::Claude => push(&a.id, Target::ClaudeProfile(dir)),
            Service::Codex => push(&a.id, Target::CodexProfile(dir)),
            Service::Antigravity => {}
        }
    }
    out
}

/// Which one-click sources apply on this PC. File checks only; no child process starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Detected {
    pub claude: bool,
    pub codex: bool,
    pub antigravity_app: bool,
}

pub fn detect(env: &Env) -> Detected {
    Detected {
        claude: env.default_claude_dir.join(".credentials.json").is_file(),
        codex: env.default_codex_dir.join("auth.json").is_file(),
        antigravity_app: true,
    }
}

fn settle(
    kind: SourceKind,
    result: Result<Vec<Reading>, FetchError>,
) -> Option<Result<Vec<Reading>, FetchError>> {
    match result {
        Err(FetchError::NotDetected) if kind != SourceKind::AntigravityApp => None,
        other => Some(other),
    }
}

/// Exponential failure backoff capped at 15 minutes.
pub struct Backoff {
    base: u64,
    next: u64,
}

impl Backoff {
    pub fn new(base_secs: u64) -> Self {
        Self {
            base: base_secs,
            next: base_secs,
        }
    }

    pub fn failed(&mut self) -> u64 {
        self.next = (self.next * 2).min(BACKOFF_CAP_SECS);
        self.next
    }

    pub fn succeeded(&mut self) -> u64 {
        self.next = self.base;
        self.next
    }
}

fn ticker(start: Instant, period_secs: u64) -> Interval {
    let period = Duration::from_secs(period_secs.max(1));
    let mut t = interval_at(start, period);
    t.set_missed_tick_behavior(MissedTickBehavior::Skip);
    t
}

/// Periodic `ticker` + `wake` loop shared with daily update checks.
pub(crate) async fn run_loop_with<F, Fut>(period_secs: u64, wake: Arc<Notify>, mut step: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = ()>,
{
    let period = period_secs.max(1);
    let mut timer = ticker(Instant::now(), period);
    loop {
        let woken = tokio::select! {
            biased;
            _ = wake.notified() => true,
            _ = timer.tick() => false,
        };
        step().await;
        if woken {
            timer = ticker(Instant::now() + Duration::from_secs(period), period);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepResult {
    Ok,
    Failed,
    RateLimited(Option<u64>),
}

struct TaskCleanupGuard {
    flight: Arc<AtomicU8>,
    shared: Arc<Shared>,
}

impl Drop for TaskCleanupGuard {
    fn drop(&mut self) {
        let prev = self.flight.swap(FLIGHT_IDLE, Ordering::AcqRel);
        if prev & FLIGHT_FORCED != 0 {
            self.shared.finish_forced_one();
        }
    }
}

async fn run_loop<F, Fut>(
    base_secs: u64,
    initial_delay: Duration,
    wake: Arc<Notify>,
    flight: Arc<AtomicU8>,
    shared: Arc<Shared>,
    mut step: F,
) where
    F: FnMut() -> Fut,
    Fut: Future<Output = StepResult>,
{
    let _cleanup = TaskCleanupGuard {
        flight: flight.clone(),
        shared: shared.clone(),
    };
    let mut backoff = Backoff::new(base_secs);
    let mut period = base_secs;
    let mut timer = ticker(Instant::now() + initial_delay, period);

    loop {
        let woken = tokio::select! {
            biased;
            _ = wake.notified() => true,
            _ = timer.tick() => false,
        };
        let entered = update_atomic_u8(&flight, |s| {
            if s & FLIGHT_RUNNING != 0 {
                None
            } else {
                Some(s | FLIGHT_RUNNING)
            }
        })
        .is_ok();
        if !entered {
            continue;
        }

        let result = step().await;
        let prev = flight.swap(FLIGHT_IDLE, Ordering::AcqRel);
        if prev & FLIGHT_FORCED != 0 {
            shared.finish_forced_one();
        }

        let next = match result {
            StepResult::Ok => backoff.succeeded(),
            StepResult::Failed => backoff.failed(),
            StepResult::RateLimited(retry_after) => rate_limit_wait_secs(retry_after, base_secs),
        };
        if woken || next != period || matches!(result, StepResult::RateLimited(_)) {
            period = next;
            let stagger = if woken && matches!(result, StepResult::Ok) {
                initial_delay
            } else {
                Duration::ZERO
            };
            timer = ticker(
                Instant::now() + Duration::from_secs(period) + stagger,
                period,
            );
        }
    }
}

struct State {
    outcomes: BTreeMap<String, SourceOutcome>,
    last: LastReadings,
    snapshot: Snapshot,
    refresh_minutes: u8,
    tasks: Option<Vec<TaskMeta>>,
    labels: BTreeMap<String, String>,
    hidden: Vec<String>,
    store: Option<JsonStore>,
    saved: Vec<SavedReading>,
    cache: Option<SnapshotCache>,
    has_cached_snapshot: bool,
}

impl State {
    fn new(now_ms: i64) -> Self {
        Self {
            outcomes: BTreeMap::new(),
            last: LastReadings::default(),
            snapshot: Snapshot {
                accounts: Vec::new(),
                services: Vec::new(),
                generated_at_ms: now_ms,
            },
            refresh_minutes: 2,
            tasks: None,
            labels: BTreeMap::new(),
            hidden: Vec::new(),
            store: None,
            saved: Vec::new(),
            cache: None,
            has_cached_snapshot: false,
        }
    }

    fn keep_readings(&mut self, store: JsonStore) {
        self.last = LastReadings::from_saved(store.load());
        self.saved = self.last.to_saved();
        self.store = Some(store);
    }

    fn keep_snapshot_cache(&mut self, cache: SnapshotCache) -> Option<Snapshot> {
        let loaded = cache.load();
        if let Some(snap) = &loaded {
            self.snapshot = snap.clone();
            self.has_cached_snapshot = true;
        }
        self.cache = Some(cache);
        loaded
    }

    fn has_task(&self, id: &str) -> bool {
        self.tasks.iter().flatten().any(|t| t.id == id)
    }

    fn take_settings(&mut self, settings: &Settings) -> Option<Vec<Entry>> {
        let plan = entries(settings);
        let tasks_changed = self
            .tasks
            .as_ref()
            .is_none_or(|t| !t.iter().map(|m| &m.id).eq(plan.iter().map(|e| &e.id)));
        let restart = tasks_changed || self.refresh_minutes != settings.refresh_minutes;
        self.refresh_minutes = settings.refresh_minutes;
        self.labels = settings.labels.clone();
        self.hidden = settings.hidden_accounts.clone();
        if restart {
            let next: Vec<TaskMeta> = plan
                .iter()
                .map(|e| TaskMeta {
                    id: e.id.clone(),
                    role: e.role(),
                    base_secs: e.base_secs(settings.refresh_minutes),
                })
                .collect();
            let kept = |id: &str| next.iter().any(|t| t.id == id);
            self.outcomes.retain(|id, _| kept(id));
            self.last.retain_tasks(kept);
            self.tasks = Some(next);
        }
        restart.then_some(plan)
    }

    fn record(&mut self, task_id: &str, found: Option<SourceOutcome>) -> bool {
        if !self.has_task(task_id) {
            return false;
        }
        self.has_cached_snapshot = false;
        match found {
            Some(outcome) => {
                self.outcomes.insert(task_id.to_string(), outcome);
                true
            }
            None => {
                let had = self.outcomes.remove(task_id).is_some();
                self.last.forget(task_id) || had
            }
        }
    }

    fn rebuild(&mut self, now_ms: i64, save_snapshot_cache: bool) {
        let no_tasks = self.tasks.as_ref().is_some_and(Vec::is_empty);
        if self.has_cached_snapshot
            && self.last.to_saved().is_empty()
            && self.outcomes.is_empty()
            && !no_tasks
        {
            return;
        }
        let outcomes: Vec<SourceOutcome> = self.outcomes.values().cloned().collect();
        let ctx = MergeCtx {
            labels: &self.labels,
            hidden: &self.hidden,
            tasks: self.tasks.as_deref().unwrap_or(&[]),
        };
        let refresh_secs = u64::from(self.refresh_minutes) * 60;
        let accounts = merge::build(&outcomes, &mut self.last, &ctx, refresh_secs, now_ms);
        self.snapshot = Snapshot {
            services: quotaglass_core::summary::build(&accounts),
            accounts,
            generated_at_ms: now_ms,
        };
        let readings_changed = self.save_readings();
        if save_snapshot_cache || readings_changed {
            if let Some(cache) = &self.cache {
                if let Err(e) = cache.save(&self.snapshot) {
                    tracing::warn!(error = %e, "could not save snapshot_cache.json");
                }
            }
        }
    }

    fn save_readings(&mut self) -> bool {
        let Some(store) = &self.store else {
            return false;
        };
        let now = self.last.to_saved();
        if now == self.saved {
            return false;
        }
        match store.save(&now) {
            Ok(()) => {
                self.saved = now;
                true
            }
            Err(e) => {
                tracing::warn!(error = %e, "could not save readings.json");
                false
            }
        }
    }
}

struct Shared {
    env: Env,
    events_tx: mpsc::UnboundedSender<SchedulerEvent>,
    state: Mutex<State>,
    active_forced: AtomicUsize,
}

impl Shared {
    fn finish_forced_one(&self) {
        let prev = self.active_forced.fetch_sub(1, Ordering::AcqRel);
        if prev == 1 {
            let _ = self.events_tx.send(SchedulerEvent::Refreshing(false));
        }
    }

    fn publish(&self, st: &mut State, save_snapshot_cache: bool) {
        st.rebuild((self.env.now_ms)(), save_snapshot_cache);
        let _ = self
            .events_tx
            .send(SchedulerEvent::Snapshot(st.snapshot.clone()));
    }

    async fn run_one(&self, task: &TaskSpec) -> StepResult {
        let provider = &task.provider;
        let result = if task.profile_dir.as_ref().is_some_and(|d| !d.is_dir()) {
            Some(Err(FetchError::ProfileMissing))
        } else if !provider.detect(&self.env) {
            None
        } else {
            settle(provider.kind(), provider.fetch(&self.env).await)
        };
        let step = match &result {
            Some(Err(FetchError::RateLimited { retry_after_secs })) => {
                StepResult::RateLimited(*retry_after_secs)
            }
            Some(Err(FetchError::NotDetected)) => StepResult::Ok,
            Some(Err(_)) => StepResult::Failed,
            _ => StepResult::Ok,
        };
        if let Some(Err(e)) = &result {
            if *e != FetchError::NotDetected {
                tracing::warn!(
                    source = ?provider.kind(),
                    service = ?provider.service(),
                    error = ?e,
                    "source fetch failed"
                );
            }
        }
        let fetched_ok = matches!(&result, Some(Ok(_)));
        let outcome = result.map(|result| SourceOutcome {
            task_id: task.id.clone(),
            kind: provider.kind(),
            service: provider.service(),
            attempted_at_ms: (self.env.now_ms)(),
            result,
        });

        let mut st = lock(&self.state);
        if st.record(&task.id, outcome) {
            self.publish(&mut st, fetched_ok);
        }
        step
    }
}

struct TaskSpec {
    id: String,
    provider: Box<dyn Provider>,
    profile_dir: Option<PathBuf>,
}

struct Task {
    wake: Arc<Notify>,
    flight: Arc<AtomicU8>,
    is_provider: bool,
    handle: JoinHandle<()>,
}

/// Lock order: `tasks`, then `shared.state`.
pub struct Scheduler {
    shared: Arc<Shared>,
    tasks: Mutex<Vec<Task>>,
    last_open_refresh: Mutex<Option<Instant>>,
}

impl Scheduler {
    pub fn new(env: Env) -> (Self, mpsc::UnboundedReceiver<SchedulerEvent>) {
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let state = State::new((env.now_ms)());
        let scheduler = Self {
            shared: Arc::new(Shared {
                env,
                events_tx,
                state: Mutex::new(state),
                active_forced: AtomicUsize::new(0),
            }),
            tasks: Mutex::new(Vec::new()),
            last_open_refresh: Mutex::new(None),
        };
        (scheduler, events_rx)
    }

    pub fn keep_readings(&self, store: JsonStore) {
        lock(&self.shared.state).keep_readings(store);
    }

    /// Loads `snapshot_cache.json` immediately on boot and emits it so the UI renders frame 1.
    pub fn keep_snapshot_cache(&self, cache: SnapshotCache) -> Option<Snapshot> {
        let loaded = lock(&self.shared.state).keep_snapshot_cache(cache);
        if let Some(snap) = &loaded {
            let _ = self
                .shared
                .events_tx
                .send(SchedulerEvent::Snapshot(snap.clone()));
        }
        loaded
    }

    pub fn snapshot(&self) -> Snapshot {
        lock(&self.shared.state).snapshot.clone()
    }

    pub fn is_refreshing(&self) -> bool {
        self.shared.active_forced.load(Ordering::Acquire) > 0
    }

    pub fn sign_in_tool(&self, service: Service) -> Option<PathBuf> {
        sign_in_tool(&self.shared.env, service)
    }

    pub fn detected(&self) -> Detected {
        detect(&self.shared.env)
    }

    /// 1-click instant Refresh: bypasses backoff & 429 immediately, coalescing any in-flight fetch
    /// per task and tracking `Refreshing(true/false)` until all forced fetches finish.
    /// Returns `true` when at least one provider task is scheduled to refresh, `false` if none are connected.
    pub fn refresh_now(&self) -> bool {
        let tasks = lock(&self.tasks);
        let provider_tasks: Vec<&Task> = tasks.iter().filter(|t| t.is_provider).collect();
        if provider_tasks.is_empty() {
            return false;
        }
        let prev = self.shared.active_forced.fetch_add(1, Ordering::AcqRel);
        if prev == 0 {
            let _ = self.shared.events_tx.send(SchedulerEvent::Refreshing(true));
        }
        for t in provider_tasks {
            let transitioned = update_atomic_u8(&t.flight, |s| {
                if s & FLIGHT_FORCED != 0 {
                    None
                } else {
                    Some(s | FLIGHT_FORCED)
                }
            });
            if let Ok(before) = transitioned {
                self.shared.active_forced.fetch_add(1, Ordering::AcqRel);
                if before == FLIGHT_IDLE {
                    t.wake.notify_one();
                }
            }
        }
        self.shared.finish_forced_one();
        true
    }

    /// Triggers `refresh_now()` when opening the popover, debounced by `OPEN_REFRESH_DEBOUNCE` (5s).
    /// Returns `true` if a refresh was triggered, `false` if debounced or no providers are connected.
    pub fn refresh_on_open(&self) -> bool {
        if !lock(&self.tasks).iter().any(|t| t.is_provider) {
            return false;
        }
        let now = Instant::now();
        {
            let mut last = lock(&self.last_open_refresh);
            if last.is_some_and(|prev| now.saturating_duration_since(prev) < OPEN_REFRESH_DEBOUNCE)
            {
                return false;
            }
            *last = Some(now);
        }
        self.refresh_now()
    }

    pub fn apply(&self, settings: &Settings) {
        let mut tasks = lock(&self.tasks);
        let plan = {
            let mut st = lock(&self.shared.state);
            let plan = st.take_settings(settings);
            self.shared.publish(&mut st, false);
            plan
        };
        if let Some(plan) = plan {
            self.respawn(&mut tasks, plan, settings.refresh_minutes);
        }
    }

    fn respawn(&self, tasks: &mut Vec<Task>, plan: Vec<Entry>, refresh_minutes: u8) {
        for t in tasks.drain(..) {
            t.handle.abort();
        }
        let claude_total = plan.iter().filter(|e| e.is_claude()).count();
        let mut claude_idx = 0usize;

        for e in plan {
            let base = e.base_secs(refresh_minutes);
            let delay_secs = if e.is_claude() {
                let d = stagger_offset_secs(claude_idx, claude_total, base);
                claude_idx += 1;
                d
            } else {
                0
            };
            let spec = Arc::new(TaskSpec {
                provider: e.provider(&self.shared.env),
                profile_dir: e.profile_dir(),
                id: e.id,
            });
            self.spawn_task(tasks, spec, base, Duration::from_secs(delay_secs), true);
        }

        let wake = Arc::new(Notify::new());
        let flight = Arc::new(AtomicU8::new(FLIGHT_IDLE));
        let shared = self.shared.clone();
        let handle = tokio::spawn(run_loop(
            REPUBLISH_SECS,
            Duration::from_secs(REPUBLISH_SECS),
            wake.clone(),
            flight.clone(),
            shared.clone(),
            move || {
                let shared = shared.clone();
                async move {
                    shared.publish(&mut lock(&shared.state), false);
                    StepResult::Ok
                }
            },
        ));
        tasks.push(Task {
            wake,
            flight,
            is_provider: false,
            handle,
        });
    }

    fn spawn_task(
        &self,
        tasks: &mut Vec<Task>,
        spec: Arc<TaskSpec>,
        base_secs: u64,
        initial_delay: Duration,
        is_provider: bool,
    ) {
        let wake = Arc::new(Notify::new());
        let flight = Arc::new(AtomicU8::new(FLIGHT_IDLE));
        let shared = self.shared.clone();
        let handle = tokio::spawn(run_loop(
            base_secs,
            initial_delay,
            wake.clone(),
            flight.clone(),
            shared.clone(),
            move || {
                let shared = shared.clone();
                let spec = spec.clone();
                async move { shared.run_one(&spec).await }
            },
        ));
        tasks.push(Task {
            wake,
            flight,
            is_provider,
            handle,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::AddedAccount;
    use quotaglass_core::{AccountKey, BoxFut, Bucket, HttpClient};
    use std::path::Path;
    use std::sync::atomic::AtomicBool;

    const NOW: i64 = 1_800_000_000_000;

    fn temp_home(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("qg-app-sched-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn env_at(home: &Path) -> Env {
        Env {
            home: home.to_path_buf(),
            default_claude_dir: home.join(".claude"),
            default_codex_dir: home.join(".codex"),
            path_dirs: vec![home.join("bin")],
            http: HttpClient::new().unwrap(),
            now_ms: || NOW,
        }
    }

    async fn settle_tasks() {
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
    }

    struct MockProvider {
        id: String,
        service: Service,
        calls: Arc<AtomicUsize>,
        timestamps_ms: Arc<Mutex<Vec<u64>>>,
        start: Instant,
        gate: Option<Arc<Notify>>,
        fail_with: Arc<Mutex<Option<FetchError>>>,
    }

    impl MockProvider {
        fn new(id: &str, service: Service, start: Instant) -> Self {
            Self {
                id: id.to_string(),
                service,
                calls: Arc::new(AtomicUsize::new(0)),
                timestamps_ms: Arc::new(Mutex::new(Vec::new())),
                start,
                gate: None,
                fail_with: Arc::new(Mutex::new(None)),
            }
        }
    }

    impl Provider for MockProvider {
        fn kind(&self) -> SourceKind {
            SourceKind::Direct
        }

        fn service(&self) -> Option<Service> {
            Some(self.service)
        }

        fn detect(&self, _env: &Env) -> bool {
            true
        }

        fn fetch<'a>(&'a self, _env: &'a Env) -> BoxFut<'a, Result<Vec<Reading>, FetchError>> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                let elapsed = Instant::now().duration_since(self.start).as_millis() as u64;
                lock(&self.timestamps_ms).push(elapsed);
                if let Some(gate) = &self.gate {
                    gate.notified().await;
                }
                if let Some(err) = lock(&self.fail_with).clone() {
                    return Err(err);
                }
                Ok(vec![Reading {
                    key: AccountKey::new(self.service, None, &self.id),
                    label: self.id.clone(),
                    plan: Some("Max".into()),
                    buckets: vec![Bucket {
                        name: "5-hour".into(),
                        used_percent: 33.0,
                        window_secs: Some(18_000),
                        resets_at_ms: Some(NOW + 3_600_000),
                    }],
                    source: SourceKind::Direct,
                    observed_at_ms: NOW,
                    banked_resets: None,
                }])
            })
        }
    }

    type CustomTaskSpec = (String, TaskRole, u64, Duration, Box<dyn Provider>);

    fn setup_custom_tasks(sched: &Scheduler, specs: Vec<CustomTaskSpec>) {
        let metas: Vec<TaskMeta> = specs
            .iter()
            .map(|(id, role, base, _, _)| TaskMeta {
                id: id.clone(),
                role: *role,
                base_secs: *base,
            })
            .collect();
        lock(&sched.shared.state).tasks = Some(metas);
        let mut tasks = lock(&sched.tasks);
        for (id, _, base, delay, provider) in specs {
            let spec = Arc::new(TaskSpec {
                id,
                provider,
                profile_dir: None,
            });
            sched.spawn_task(&mut tasks, spec, base, delay, true);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn claude_accounts_stagger_deterministically_without_burst() {
        let home = temp_home("stagger");
        let (sched, _rx) = Scheduler::new(env_at(&home));
        let start = Instant::now();

        let p0 = MockProvider::new("c0", Service::Claude, start);
        let p1 = MockProvider::new("c1", Service::Claude, start);
        let p2 = MockProvider::new("c2", Service::Claude, start);
        let t0 = p0.timestamps_ms.clone();
        let t1 = p1.timestamps_ms.clone();
        let t2 = p2.timestamps_ms.clone();

        let plan = entries(&Settings {
            connected: vec![Connection::ClaudeCurrent],
            added_accounts: vec![
                AddedAccount {
                    id: "c1".into(),
                    service: Service::Claude,
                    profile_dir: home.join("c1"),
                },
                AddedAccount {
                    id: "c2".into(),
                    service: Service::Claude,
                    profile_dir: home.join("c2"),
                },
            ],
            ..Settings::default()
        });
        assert_eq!(plan.len(), 3);
        let total = plan.iter().filter(|e| e.is_claude()).count();
        assert_eq!(total, 3);
        let d0 = stagger_offset_secs(0, total, 300);
        let d1 = stagger_offset_secs(1, total, 300);
        let d2 = stagger_offset_secs(2, total, 300);
        assert_eq!((d0, d1, d2), (0, 100, 200));

        setup_custom_tasks(
            &sched,
            vec![
                (
                    "claude-current".into(),
                    TaskRole::CurrentLogin,
                    300,
                    Duration::from_secs(d0),
                    Box::new(p0),
                ),
                (
                    "c1".into(),
                    TaskRole::AddedAccount,
                    300,
                    Duration::from_secs(d1),
                    Box::new(p1),
                ),
                (
                    "c2".into(),
                    TaskRole::AddedAccount,
                    300,
                    Duration::from_secs(d2),
                    Box::new(p2),
                ),
            ],
        );

        settle_tasks().await;
        assert_eq!(lock(&t0).as_slice(), &[0]);
        assert!(lock(&t1).is_empty());
        assert!(lock(&t2).is_empty());

        tokio::time::advance(Duration::from_secs(100)).await;
        settle_tasks().await;
        assert_eq!(lock(&t1).as_slice(), &[100_000]);
        assert!(lock(&t2).is_empty());

        tokio::time::advance(Duration::from_secs(100)).await;
        settle_tasks().await;
        assert_eq!(lock(&t2).as_slice(), &[200_000]);

        lock(&sched.tasks).drain(..).for_each(|t| t.handle.abort());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[tokio::test(start_paused = true)]
    async fn refresh_on_open_debounces_five_seconds() {
        let home = temp_home("open-debounce");
        let (sched, _rx) = Scheduler::new(env_at(&home));
        let start = Instant::now();
        let p = MockProvider::new("codex", Service::Codex, start);
        let calls = p.calls.clone();

        setup_custom_tasks(
            &sched,
            vec![(
                "codex-current".into(),
                TaskRole::CurrentLogin,
                120,
                Duration::from_secs(120),
                Box::new(p),
            )],
        );
        settle_tasks().await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        // First open refreshes immediately.
        assert!(sched.refresh_on_open());
        settle_tasks().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Re-open within 5 seconds is debounced.
        tokio::time::advance(Duration::from_secs(2)).await;
        assert!(!sched.refresh_on_open());
        settle_tasks().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // At 5+ seconds from first open, refreshes again.
        tokio::time::advance(Duration::from_secs(3)).await;
        assert!(sched.refresh_on_open());
        settle_tasks().await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        lock(&sched.tasks).drain(..).for_each(|t| t.handle.abort());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[tokio::test(start_paused = true)]
    async fn instant_refresh_bypasses_backoff_and_429_and_preserves_readings() {
        let home = temp_home("bypass-429");
        let (sched, mut rx) = Scheduler::new(env_at(&home));
        let start = Instant::now();
        let p = MockProvider::new("claude", Service::Claude, start);
        let calls = p.calls.clone();
        let fail_with = p.fail_with.clone();

        setup_custom_tasks(
            &sched,
            vec![(
                "claude-current".into(),
                TaskRole::CurrentLogin,
                300,
                Duration::ZERO,
                Box::new(p),
            )],
        );
        settle_tasks().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(sched.snapshot().accounts.len(), 1);

        // Next background poll hits 429 with Retry-After=600s; last good reading must stay.
        *lock(&fail_with) = Some(FetchError::RateLimited {
            retry_after_secs: Some(600),
        });
        tokio::time::advance(Duration::from_secs(300)).await;
        settle_tasks().await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let card = &sched.snapshot().accounts[0];
        assert_eq!(card.state, quotaglass_core::view::CardState::RateLimited);
        assert_eq!(card.buckets.len(), 1, "last reading preserved during 429");

        // 10s later (long before 600s cooldown), clear error and click instant refresh.
        *lock(&fail_with) = None;
        tokio::time::advance(Duration::from_secs(10)).await;
        while rx.try_recv().is_ok() {}
        sched.refresh_now();
        settle_tasks().await;
        assert_eq!(calls.load(Ordering::SeqCst), 3, "bypassed 429 cooldown");
        assert_eq!(
            sched.snapshot().accounts[0].state,
            quotaglass_core::view::CardState::Ok
        );
        assert!(!sched.is_refreshing());

        let mut events = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }
        assert!(events.contains(&SchedulerEvent::Refreshing(true)));
        assert!(events.contains(&SchedulerEvent::Refreshing(false)));

        // Also verify bypassing exponential backoff after error.
        *lock(&fail_with) = Some(FetchError::Timeout);
        tokio::time::advance(Duration::from_secs(300)).await;
        settle_tasks().await;
        assert_eq!(calls.load(Ordering::SeqCst), 4);
        *lock(&fail_with) = None;
        tokio::time::advance(Duration::from_secs(5)).await;
        sched.refresh_now();
        settle_tasks().await;
        assert_eq!(calls.load(Ordering::SeqCst), 5, "bypassed backoff wait");

        lock(&sched.tasks).drain(..).for_each(|t| t.handle.abort());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[tokio::test(start_paused = true)]
    async fn refresh_now_deduplicates_in_flight_fetches_and_tracks_refreshing() {
        let home = temp_home("dedup");
        let (sched, mut rx) = Scheduler::new(env_at(&home));
        let gate = Arc::new(Notify::new());
        let mut p = MockProvider::new("codex", Service::Codex, Instant::now());
        p.gate = Some(gate.clone());
        let calls = p.calls.clone();

        setup_custom_tasks(
            &sched,
            vec![(
                "codex-current".into(),
                TaskRole::CurrentLogin,
                120,
                Duration::ZERO,
                Box::new(p),
            )],
        );
        settle_tasks().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1, "fetch is blocked on gate");

        // Trigger multiple concurrent refreshes while first fetch is still in flight.
        sched.refresh_now();
        assert!(sched.is_refreshing());
        sched.refresh_now();
        sched.refresh_now();
        settle_tasks().await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "concurrent triggers coalesced into single in-flight fetch"
        );
        assert!(sched.is_refreshing());

        gate.notify_waiters();
        settle_tasks().await;
        assert!(!sched.is_refreshing());
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let mut refreshing_seq = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if let SchedulerEvent::Refreshing(b) = ev {
                refreshing_seq.push(b);
            }
        }
        assert_eq!(refreshing_seq, vec![true, false]);

        lock(&sched.tasks).drain(..).for_each(|t| t.handle.abort());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[tokio::test(start_paused = true)]
    async fn snapshot_cache_and_readings_save_atomically_and_load_on_boot() {
        let home = temp_home("snap-cache");
        let readings_path = home.join("readings.json");
        let cache_path = home.join("snapshot_cache.json");

        {
            let (sched, _rx) = Scheduler::new(env_at(&home));
            sched.keep_readings(JsonStore::new(readings_path.clone()));
            assert!(sched
                .keep_snapshot_cache(SnapshotCache::new(cache_path.clone()))
                .is_none());

            let p = MockProvider::new("codex", Service::Codex, Instant::now());
            setup_custom_tasks(
                &sched,
                vec![(
                    "codex-current".into(),
                    TaskRole::CurrentLogin,
                    120,
                    Duration::ZERO,
                    Box::new(p),
                )],
            );
            settle_tasks().await;
            assert!(readings_path.exists());
            assert!(cache_path.exists());
            assert!(!home.join("snapshot_cache.json.tmp").exists());
            lock(&sched.tasks).drain(..).for_each(|t| t.handle.abort());
        }

        // Reboot: cache loads immediately before any fetch runs.
        let (rebooted, mut rx) = Scheduler::new(env_at(&home));
        let loaded = rebooted
            .keep_snapshot_cache(SnapshotCache::new(cache_path.clone()))
            .expect("cached snapshot available immediately on boot");
        assert_eq!(loaded.accounts.len(), 1);
        assert_eq!(rebooted.snapshot().accounts.len(), 1);
        assert!(matches!(
            rx.try_recv(),
            Ok(SchedulerEvent::Snapshot(s)) if s.accounts.len() == 1
        ));

        let _ = std::fs::remove_dir_all(&home);
        let _ = AtomicBool::new(false);
    }

    #[tokio::test(start_paused = true)]
    async fn refresh_returns_false_with_zero_provider_tasks() {
        let home = temp_home("zero-tasks");
        let (sched, mut rx) = Scheduler::new(env_at(&home));
        assert!(!sched.refresh_now());
        assert!(!sched.refresh_on_open());
        assert!(!sched.is_refreshing());
        assert!(
            rx.try_recv().is_err(),
            "no Refreshing event emitted when 0 providers"
        );
        let _ = std::fs::remove_dir_all(&home);
    }
}
