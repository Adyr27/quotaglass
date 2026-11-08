//! Runs each enabled source on its own timer and merges the results into one snapshot.

use crate::settings::{Connection, Settings};
use quotaglass_core::merge::{self, LastReadings};
use quotaglass_core::sources::claude_direct::ClaudeDirect;
use quotaglass_core::sources::codex_direct::CodexDirect;
use quotaglass_core::sources::orca::Orca;
use quotaglass_core::view::Snapshot;
use quotaglass_core::{Env, Provider, Service, SourceOutcome};
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tauri::async_runtime::JoinHandle;
use tokio::sync::Notify;
use tokio::time::{interval_at, Instant, Interval, MissedTickBehavior};

const CLAUDE: usize = 0;
const CODEX: usize = 1;
const ORCA: usize = 2;
const PROVIDERS: usize = 3;

/// Freshness, ages and the headline depend on the clock, so the snapshot is rebuilt this often between fetches.
const REPUBLISH_SECS: u64 = 30;
const BACKOFF_CAP_SECS: u64 = 900;

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// TODO(v0.2-task7): remove this `connected` -> switches mapping when the scheduler builds providers per connection.
/// The v0.1.0 source switches, derived from `connected` until the scheduler builds providers per connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Switches {
    pub direct: bool,
    pub orca: bool,
}

pub fn switches(settings: &Settings) -> Switches {
    let has = |c: Connection| settings.connected.contains(&c);
    Switches {
        direct: has(Connection::ClaudeCurrent) || has(Connection::CodexCurrent),
        orca: has(Connection::Orca),
    }
}

fn enabled(provider: usize, sources: &Switches) -> bool {
    match provider {
        CLAUDE | CODEX => sources.direct,
        _ => sources.orca,
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
    outcomes: Vec<Option<SourceOutcome>>,
    last: LastReadings,
    snapshot: Snapshot,
    refresh_minutes: u8,
    applied_sources: Option<Switches>,
}

struct Shared {
    env: Env,
    providers: Vec<Box<dyn Provider>>,
    emit: Box<dyn Fn(&Snapshot) + Send + Sync>,
    state: Mutex<State>,
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

fn connectable(env: &Env) -> Vec<Service> {
    [Service::Claude, Service::Codex]
        .into_iter()
        .filter(|s| sign_in_tool(env, *s).is_some())
        .collect()
}

impl Shared {
    /// Rebuilds the snapshot from the stored outcomes, keeps it for `get_snapshot` and emits "snapshot".
    fn publish(&self, st: &mut State) {
        let outcomes: Vec<SourceOutcome> = st.outcomes.iter().flatten().cloned().collect();
        let now = (self.env.now_ms)();
        let refresh_secs = u64::from(st.refresh_minutes) * 60;
        let accounts = merge::build(
            &outcomes,
            &mut st.last,
            &connectable(&self.env),
            refresh_secs,
            now,
        );
        st.snapshot = Snapshot {
            accounts,
            generated_at_ms: now,
        };
        (self.emit)(&st.snapshot);
    }

    /// One fetch of one provider. Returns false when the fetch failed.
    async fn run_one(&self, i: usize) -> bool {
        let provider = &self.providers[i];
        if !provider.detect(&self.env) {
            // The login or program is gone: drop its old readings too, or they stay on screen as if current.
            let mut st = lock(&self.state);
            if st.outcomes[i].take().is_some() {
                st.last.forget(provider.kind(), provider.service());
                self.publish(&mut st);
            }
            return true;
        }

        let result = provider.fetch(&self.env).await;
        let ok = result.is_ok();
        if let Err(e) = &result {
            tracing::warn!(source = ?provider.kind(), error = ?e, "source fetch failed");
        }
        let outcome = SourceOutcome {
            kind: provider.kind(),
            service: provider.service(),
            attempted_at_ms: (self.env.now_ms)(),
            result,
        };

        let mut st = lock(&self.state);
        // The source may have been switched off while this fetch was running.
        if st.applied_sources.is_some_and(|s| enabled(i, &s)) {
            st.outcomes[i] = Some(outcome);
            self.publish(&mut st);
        }
        ok
    }
}

impl Scheduler {
    pub fn new(env: Env, emit: impl Fn(&Snapshot) + Send + Sync + 'static) -> Self {
        let providers: Vec<Box<dyn Provider>> = vec![
            Box::new(ClaudeDirect {
                dir: env.default_claude_dir.clone(),
                local_id: "claude-current".into(),
            }),
            Box::new(CodexDirect {
                dir: env.default_codex_dir.clone(),
                local_id: "codex-current".into(),
            }),
            Box::new(Orca::new()),
        ];
        debug_assert_eq!(providers.len(), PROVIDERS);
        let snapshot = Snapshot {
            accounts: Vec::new(),
            generated_at_ms: (env.now_ms)(),
        };
        let state = State {
            outcomes: (0..PROVIDERS).map(|_| None).collect(),
            last: LastReadings::default(),
            snapshot,
            refresh_minutes: 2,
            applied_sources: None,
        };
        Self {
            shared: Arc::new(Shared {
                env,
                providers,
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

    /// (direct, orca): whether each source can apply on this PC.
    pub fn detected(&self) -> (bool, bool) {
        let (env, p) = (&self.shared.env, &self.shared.providers);
        (
            p[CLAUDE].detect(env) || p[CODEX].detect(env),
            p[ORCA].detect(env),
        )
    }

    /// Runs every running provider now.
    pub fn refresh_now(&self) {
        for t in lock(&self.tasks).iter() {
            t.wake.notify_one();
        }
    }

    /// Takes the settings the scheduler cares about.
    /// A change of sources or refresh interval restarts the timers, which also fetches right away.
    pub fn apply(&self, settings: &Settings) {
        let restart = {
            let mut st = lock(&self.shared.state);
            let sources = switches(settings);
            let sources_changed = st.applied_sources != Some(sources);
            let restart = sources_changed || st.refresh_minutes != settings.refresh_minutes;
            st.refresh_minutes = settings.refresh_minutes;
            if sources_changed {
                for i in 0..PROVIDERS {
                    if !enabled(i, &sources) {
                        st.outcomes[i] = None;
                    }
                }
                st.last = LastReadings::default();
                st.applied_sources = Some(sources);
            }
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
        let sources = switches(settings);
        for i in 0..PROVIDERS {
            if !enabled(i, &sources) {
                continue;
            }
            let base = u64::from(settings.refresh_minutes) * 60;
            let wake = Arc::new(Notify::new());
            let shared = self.shared.clone();
            let handle = tauri::async_runtime::spawn(run_loop(base, wake.clone(), move || {
                let shared = shared.clone();
                async move { shared.run_one(i).await }
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
}
