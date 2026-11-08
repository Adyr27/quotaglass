//! Runs one task per connected source and added account, and merges the results into one snapshot.

use crate::settings::{Connection, Settings};
use quotaglass_core::merge::{self, LastReadings, MergeCtx, TaskMeta, TaskRole};
use quotaglass_core::sources::antigravity_app::AntigravityApp;
use quotaglass_core::sources::claude_direct::ClaudeDirect;
use quotaglass_core::sources::codex_direct::CodexDirect;
use quotaglass_core::sources::orca::Orca;
use quotaglass_core::view::Snapshot;
use quotaglass_core::{Env, FetchError, Provider, Reading, Service, SourceOutcome};
use serde::Serialize;
use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tauri::async_runtime::JoinHandle;
use tokio::sync::Notify;
use tokio::time::{interval_at, Instant, Interval, MissedTickBehavior};

/// Freshness, ages and the headline depend on the clock, so the snapshot is rebuilt this often between fetches.
const REPUBLISH_SECS: u64 = 30;
const BACKOFF_CAP_SECS: u64 = 900;

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What one task reads.
#[derive(Debug, Clone)]
enum Target {
    ClaudeCurrent,
    CodexCurrent,
    AntigravityApp,
    Orca,
    ClaudeProfile(PathBuf),
    CodexProfile(PathBuf),
}

/// One task: its ID keys its outcome, its stored readings and, for an added account, its card's `profile_id`.
#[derive(Debug, Clone)]
struct Entry {
    id: String,
    target: Target,
}

impl Entry {
    fn role(&self) -> TaskRole {
        match self.target {
            Target::ClaudeCurrent | Target::CodexCurrent | Target::AntigravityApp => {
                TaskRole::CurrentLogin
            }
            Target::ClaudeProfile(_) | Target::CodexProfile(_) => TaskRole::AddedAccount,
            Target::Orca => TaskRole::Other,
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
            Target::Orca => Box::new(Orca::new()),
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

/// One entry per `connected` item, then one per Claude or Codex added account. Antigravity accounts are whatever the
/// app is signed into, so an added Antigravity account is skipped. A repeated ID is skipped too: two tasks with one
/// ID would share their stored readings.
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
            Connection::Orca => push("orca", Target::Orca),
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

/// The task IDs `providers_for` gives, in the same order. Needs no `Env`, so `apply` can compare lists cheaply.
pub fn task_ids(settings: &Settings) -> Vec<String> {
    entries(settings).into_iter().map(|e| e.id).collect()
}

/// One provider per task, keyed by task ID.
pub fn providers_for(settings: &Settings, env: &Env) -> Vec<(String, Box<dyn Provider>)> {
    entries(settings)
        .into_iter()
        .map(|e| {
            let p = e.provider(env);
            (e.id, p)
        })
        .collect()
}

fn task_meta(settings: &Settings) -> Vec<TaskMeta> {
    entries(settings)
        .into_iter()
        .map(|e| TaskMeta {
            role: e.role(),
            id: e.id,
        })
        .collect()
}

/// Which one-click sources apply on this PC. Reads files and PATH only: no process starts, nothing blocks for long.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Detected {
    pub claude: bool,
    pub codex: bool,
    /// Always true: finding the app needs a process list, so its fetch decides.
    pub antigravity_app: bool,
    pub orca: bool,
}

pub fn detect(env: &Env) -> Detected {
    Detected {
        claude: env.default_claude_dir.join(".credentials.json").is_file(),
        codex: env.default_codex_dir.join("auth.json").is_file(),
        antigravity_app: true,
        orca: env.find_on_path(&["orca.exe"]).is_some(),
    }
}

/// `NotDetected` from a fetch means the same as `detect() == false`: the source is gone (`None`).
fn settle(result: Result<Vec<Reading>, FetchError>) -> Option<Result<Vec<Reading>, FetchError>> {
    match result {
        Err(FetchError::NotDetected) => None,
        other => Some(other),
    }
}

/// Wait time after each failure: the interval, doubled per failure, capped at 15 minutes.
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
    let period = Duration::from_secs(period_secs);
    let mut t = interval_at(start, period);
    // After a long sleep, run once and carry on; do not replay every missed tick.
    t.set_missed_tick_behavior(MissedTickBehavior::Skip);
    t
}

/// Calls `step` now, then every `base_secs` (longer while it keeps failing), and whenever `wake` fires.
/// `step` returns true on success.
async fn run_loop<F, Fut>(base_secs: u64, wake: Arc<Notify>, mut step: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let mut backoff = Backoff::new(base_secs);
    let mut period = base_secs;
    let mut timer = ticker(Instant::now(), period);
    loop {
        tokio::select! {
            _ = timer.tick() => {}
            _ = wake.notified() => {}
        }
        let next = if step().await {
            backoff.succeeded()
        } else {
            backoff.failed()
        };
        if next != period {
            period = next;
            timer = ticker(Instant::now() + Duration::from_secs(period), period);
        }
    }
}

struct State {
    /// Latest outcome per task ID.
    outcomes: BTreeMap<String, SourceOutcome>,
    last: LastReadings,
    snapshot: Snapshot,
    refresh_minutes: u8,
    /// `None` until the first `apply`.
    tasks: Option<Vec<TaskMeta>>,
    labels: BTreeMap<String, String>,
    pinned: Vec<String>,
    hidden: Vec<String>,
}

impl State {
    fn new(now_ms: i64) -> Self {
        Self {
            outcomes: BTreeMap::new(),
            last: LastReadings::default(),
            snapshot: Snapshot {
                accounts: Vec::new(),
                generated_at_ms: now_ms,
            },
            refresh_minutes: 2,
            tasks: None,
            labels: BTreeMap::new(),
            pinned: Vec::new(),
            hidden: Vec::new(),
        }
    }

    fn has_task(&self, id: &str) -> bool {
        self.tasks.iter().flatten().any(|t| t.id == id)
    }

    /// Takes the settings the scheduler cares about. Returns true when the timers must restart: the task list or the
    /// refresh interval changed. Labels, pins and hidden accounts only need a republish.
    fn take_settings(&mut self, settings: &Settings) -> bool {
        let ids = task_ids(settings);
        let tasks_changed = self
            .tasks
            .as_ref()
            .is_none_or(|t| !t.iter().map(|m| &m.id).eq(ids.iter()));
        let restart = tasks_changed || self.refresh_minutes != settings.refresh_minutes;
        self.refresh_minutes = settings.refresh_minutes;
        self.labels = settings.labels.clone();
        self.pinned = settings.pinned_accounts.clone();
        self.hidden = settings.hidden_accounts.clone();
        if tasks_changed {
            let next = task_meta(settings);
            // A removed task leaves nothing behind. Kept tasks keep their readings until they answer again.
            for old in self.tasks.take().unwrap_or_default() {
                if !next.iter().any(|t| t.id == old.id) {
                    self.outcomes.remove(&old.id);
                    self.last.forget(&old.id);
                }
            }
            self.tasks = Some(next);
        }
        restart
    }

    /// Stores what one run of a task found; `None` means its source is gone. Returns true when the snapshot changed.
    fn record(&mut self, task_id: &str, found: Option<SourceOutcome>) -> bool {
        // The task may have been removed while its fetch ran.
        if !self.has_task(task_id) {
            return false;
        }
        match found {
            Some(outcome) => {
                self.outcomes.insert(task_id.to_string(), outcome);
                true
            }
            None => {
                // The login or program is gone: drop its old readings too, or they stay on screen as if current.
                let had = self.outcomes.remove(task_id).is_some();
                self.last.forget(task_id) || had
            }
        }
    }

    /// Rebuilds the snapshot from the stored outcomes.
    fn rebuild(&mut self, now_ms: i64) {
        let outcomes: Vec<SourceOutcome> = self.outcomes.values().cloned().collect();
        let ctx = MergeCtx {
            labels: &self.labels,
            pinned: &self.pinned,
            hidden: &self.hidden,
            tasks: self.tasks.as_deref().unwrap_or(&[]),
        };
        let refresh_secs = u64::from(self.refresh_minutes) * 60;
        let accounts = merge::build(&outcomes, &mut self.last, &ctx, refresh_secs, now_ms);
        self.snapshot = Snapshot {
            accounts,
            generated_at_ms: now_ms,
        };
    }
}

struct Shared {
    env: Env,
    emit: Box<dyn Fn(&Snapshot) + Send + Sync>,
    state: Mutex<State>,
}

/// One running provider.
struct TaskSpec {
    id: String,
    provider: Box<dyn Provider>,
    /// Set for an added account: a missing folder is its own card state, checked before each fetch.
    profile_dir: Option<PathBuf>,
}

struct Task {
    wake: Arc<Notify>,
    handle: JoinHandle<()>,
}

pub struct Scheduler {
    shared: Arc<Shared>,
    tasks: Mutex<Vec<Task>>,
}

/// The sign-in tool for a service, as found on PATH. `connect` runs this exact file.
fn sign_in_tool(env: &Env, service: Service) -> Option<PathBuf> {
    match service {
        Service::Claude => env.find_on_path(&["claude.cmd", "claude.exe"]),
        Service::Codex => env.find_on_path(&["codex.cmd", "codex.exe"]),
        Service::Antigravity => None,
    }
}

impl Shared {
    /// Rebuilds the snapshot, keeps it for `get_snapshot` and emits "snapshot".
    fn publish(&self, st: &mut State) {
        st.rebuild((self.env.now_ms)());
        (self.emit)(&st.snapshot);
    }

    /// One fetch of one task. Returns false when the fetch failed. A gone source counts as success for the backoff.
    async fn run_one(&self, task: &TaskSpec) -> bool {
        let provider = &task.provider;
        let result = if task.profile_dir.as_ref().is_some_and(|d| !d.is_dir()) {
            Some(Err(FetchError::ProfileMissing))
        } else if !provider.detect(&self.env) {
            None
        } else {
            settle(provider.fetch(&self.env).await)
        };
        let ok = !matches!(result, Some(Err(_)));
        if let Some(Err(e)) = &result {
            tracing::warn!(source = ?provider.kind(), service = ?provider.service(), error = ?e, "source fetch failed");
        }
        let outcome = result.map(|result| SourceOutcome {
            task_id: task.id.clone(),
            kind: provider.kind(),
            service: provider.service(),
            attempted_at_ms: (self.env.now_ms)(),
            result,
        });

        let mut st = lock(&self.state);
        if st.record(&task.id, outcome) {
            self.publish(&mut st);
        }
        ok
    }
}

impl Scheduler {
    pub fn new(env: Env, emit: impl Fn(&Snapshot) + Send + Sync + 'static) -> Self {
        let state = State::new((env.now_ms)());
        Self {
            shared: Arc::new(Shared {
                env,
                emit: Box::new(emit),
                state: Mutex::new(state),
            }),
            tasks: Mutex::new(Vec::new()),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        lock(&self.shared.state).snapshot.clone()
    }

    pub fn sign_in_tool(&self, service: Service) -> Option<PathBuf> {
        sign_in_tool(&self.shared.env, service)
    }

    /// Which one-click sources apply on this PC. Cheap: no process starts.
    pub fn detected(&self) -> Detected {
        detect(&self.shared.env)
    }

    /// Runs every running provider now.
    pub fn refresh_now(&self) {
        for t in lock(&self.tasks).iter() {
            t.wake.notify_one();
        }
    }

    /// Takes the settings the scheduler cares about and republishes. A change of the task list or of the refresh
    /// interval restarts the timers, which also fetches right away.
    pub fn apply(&self, settings: &Settings) {
        let restart = {
            let mut st = lock(&self.shared.state);
            let restart = st.take_settings(settings);
            self.shared.publish(&mut st);
            restart
        };
        if restart {
            self.respawn(settings);
        }
    }

    fn respawn(&self, settings: &Settings) {
        let mut tasks = lock(&self.tasks);
        for t in tasks.drain(..) {
            t.handle.abort();
        }
        let base = u64::from(settings.refresh_minutes) * 60;
        let dirs = entries(settings).into_iter().map(|e| e.profile_dir());
        for ((id, provider), profile_dir) in providers_for(settings, &self.shared.env)
            .into_iter()
            .zip(dirs)
        {
            let spec = Arc::new(TaskSpec {
                id,
                provider,
                profile_dir,
            });
            let wake = Arc::new(Notify::new());
            let shared = self.shared.clone();
            let handle = tauri::async_runtime::spawn(run_loop(base, wake.clone(), move || {
                let shared = shared.clone();
                let spec = spec.clone();
                async move { shared.run_one(&spec).await }
            }));
            tasks.push(Task { wake, handle });
        }
        // Rebuild from the stored outcomes, so a quiet spell (backoff, sleep) does not leave "live" numbers on
        // screen. `merge::build` gives the same cards for the same outcomes, apart from the clock.
        let wake = Arc::new(Notify::new());
        let shared = self.shared.clone();
        let handle =
            tauri::async_runtime::spawn(run_loop(REPUBLISH_SECS, wake.clone(), move || {
                let shared = shared.clone();
                async move {
                    shared.publish(&mut lock(&shared.state));
                    true
                }
            }));
        tasks.push(Task { wake, handle });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::AddedAccount;
    use quotaglass_core::{AccountKey, HttpClient, SourceKind};
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn backoff_doubles_and_caps() {
        let mut b = Backoff::new(120);
        assert_eq!(b.failed(), 240);
        assert_eq!(b.failed(), 480);
        assert_eq!(b.failed(), 900);
        assert_eq!(b.failed(), 900);
        assert_eq!(b.succeeded(), 120);
        assert_eq!(b.failed(), 240);
    }

    #[tokio::test(start_paused = true)]
    async fn sleep_resume_single_tick() {
        let runs = Arc::new(AtomicUsize::new(0));
        let counter = runs.clone();
        let task = tokio::spawn(run_loop(120, Arc::new(Notify::new()), move || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                true
            }
        }));

        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        assert_eq!(runs.load(Ordering::SeqCst), 1, "one run at start");

        tokio::time::advance(Duration::from_secs(6 * 3600)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            runs.load(Ordering::SeqCst),
            2,
            "one run after the long sleep, not one per missed tick"
        );
        task.abort();
    }

    const NOW: i64 = 1_800_000_000_000;

    fn temp_home(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("quotaglass-sched-{name}-{}", std::process::id()));
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

    fn touch(path: PathBuf) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"{}").unwrap();
    }

    fn added(id: &str, service: Service, dir: PathBuf) -> AddedAccount {
        AddedAccount {
            id: id.into(),
            service,
            profile_dir: dir,
        }
    }

    #[test]
    fn providers_follow_connected_and_added() {
        let home = temp_home("providers");
        let settings = Settings {
            connected: vec![
                Connection::ClaudeCurrent,
                Connection::Orca,
                Connection::AntigravityApp,
                Connection::CodexCurrent,
            ],
            added_accounts: vec![
                added("p1", Service::Claude, home.join("p1")),
                added("p2", Service::Codex, home.join("p2")),
                added("p3", Service::Antigravity, home.join("p3")),
            ],
            ..Settings::default()
        };
        let env = env_at(&home);
        let providers = providers_for(&settings, &env);
        let ids: Vec<&str> = providers.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "claude-current",
                "orca",
                "antigravity-app",
                "codex-current",
                "p1",
                "p2"
            ]
        );
        assert_eq!(task_ids(&settings), ids);
        let shapes: Vec<(SourceKind, Option<Service>)> = providers
            .iter()
            .map(|(_, p)| (p.kind(), p.service()))
            .collect();
        assert_eq!(
            shapes,
            [
                (SourceKind::Direct, Some(Service::Claude)),
                (SourceKind::Orca, Some(Service::Antigravity)),
                (SourceKind::AntigravityApp, Some(Service::Antigravity)),
                (SourceKind::Direct, Some(Service::Codex)),
                (SourceKind::Direct, Some(Service::Claude)),
                (SourceKind::Direct, Some(Service::Codex)),
            ]
        );

        // Each Direct task reads its own folder.
        let detect = |i: usize| providers[i].1.detect(&env);
        assert!(!detect(0) && !detect(3) && !detect(4) && !detect(5));
        touch(home.join(".claude").join(".credentials.json"));
        touch(home.join("p2").join("auth.json"));
        assert!(detect(0), "current Claude login reads the default folder");
        assert!(
            !detect(4),
            "the Claude profile does not read the default folder"
        );
        assert!(detect(5), "the Codex profile reads its own folder");
        assert!(!detect(3));

        assert!(providers_for(&Settings::default(), &env).is_empty());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn respawn_only_on_task_list_change() {
        let mut st = State::new(NOW);
        let mut s = Settings {
            connected: vec![Connection::ClaudeCurrent],
            ..Settings::default()
        };
        assert!(st.take_settings(&s), "the first apply starts the tasks");
        assert!(!st.take_settings(&s));

        s.labels.insert("claude:x".into(), "Work".into());
        s.pinned_accounts = vec!["claude:x".into()];
        s.hidden_accounts = vec!["claude:y".into()];
        assert!(
            !st.take_settings(&s),
            "labels, pins and hidden only republish"
        );
        assert_eq!(st.labels.get("claude:x").map(String::as_str), Some("Work"));
        assert_eq!(st.pinned, ["claude:x"]);
        assert_eq!(st.hidden, ["claude:y"]);

        s.refresh_minutes = 5;
        assert!(st.take_settings(&s), "a new interval restarts the timers");

        s.connected.push(Connection::Orca);
        assert!(st.take_settings(&s), "a new task restarts the timers");
        s.added_accounts
            .push(added("p1", Service::Codex, PathBuf::from("C:/x")));
        assert!(st.take_settings(&s));
        assert!(!st.take_settings(&s));
    }

    fn reading(local: &str) -> Reading {
        Reading {
            key: AccountKey::new(Service::Claude, None, local),
            label: "Claude".into(),
            plan: None,
            buckets: vec![],
            source: SourceKind::Direct,
            observed_at_ms: NOW,
            banked_resets: None,
        }
    }

    fn outcome(task: &str, result: Result<Vec<Reading>, FetchError>) -> SourceOutcome {
        SourceOutcome {
            task_id: task.into(),
            kind: SourceKind::Direct,
            service: Some(Service::Claude),
            attempted_at_ms: NOW,
            result,
        }
    }

    #[test]
    fn not_detected_forgets_readings() {
        assert!(settle(Err(FetchError::NotDetected)).is_none());
        assert_eq!(
            settle(Err(FetchError::Timeout)),
            Some(Err(FetchError::Timeout))
        );
        assert_eq!(settle(Ok(vec![])), Some(Ok(vec![])));

        let mut st = State::new(NOW);
        st.take_settings(&Settings {
            connected: vec![Connection::ClaudeCurrent],
            ..Settings::default()
        });
        let task = "claude-current";
        assert!(st.record(task, Some(outcome(task, Ok(vec![reading(task)])))));
        st.rebuild(NOW);
        assert_eq!(st.snapshot.accounts.len(), 1);

        assert!(st.record(task, None), "a gone source drops its reading");
        st.rebuild(NOW);
        assert!(st.snapshot.accounts.is_empty());
        assert!(!st.record(task, None), "nothing left to drop, no republish");

        assert!(
            !st.record(
                "removed",
                Some(outcome("removed", Ok(vec![reading("removed")])))
            ),
            "a task removed while its fetch ran is ignored"
        );
    }

    #[test]
    fn removed_task_readings_are_dropped_on_apply() {
        let mut st = State::new(NOW);
        let mut s = Settings {
            connected: vec![Connection::ClaudeCurrent, Connection::CodexCurrent],
            ..Settings::default()
        };
        st.take_settings(&s);
        st.record(
            "claude-current",
            Some(outcome(
                "claude-current",
                Ok(vec![reading("claude-current")]),
            )),
        );
        st.rebuild(NOW);
        assert_eq!(st.snapshot.accounts.len(), 1);
        s.connected = vec![Connection::CodexCurrent];
        st.take_settings(&s);
        st.rebuild(NOW);
        assert!(st.snapshot.accounts.is_empty());
    }

    #[test]
    fn detected_checks_files_only() {
        let home = temp_home("detected");
        let env = env_at(&home);
        assert_eq!(
            detect(&env),
            Detected {
                claude: false,
                codex: false,
                antigravity_app: true,
                orca: false
            }
        );
        touch(home.join(".claude").join(".credentials.json"));
        touch(home.join(".codex").join("auth.json"));
        touch(home.join("bin").join("orca.exe"));
        assert_eq!(
            detect(&env),
            Detected {
                claude: true,
                codex: true,
                antigravity_app: true,
                orca: true
            }
        );
        assert_eq!(
            serde_json::to_value(detect(&env)).unwrap(),
            serde_json::json!({"claude": true, "codex": true, "antigravityApp": true, "orca": true})
        );
        let _ = std::fs::remove_dir_all(&home);
    }
}
