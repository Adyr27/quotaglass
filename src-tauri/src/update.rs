//! Update checks. The window never reaches the updater: it calls `check_update` and `install_update` (commands.rs).

use crate::commands::AppState;
use crate::scheduler::{lock, run_loop, StepResult};
use std::future::Future;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};
use tokio::sync::Notify;

const DAY_SECS: u64 = 24 * 3600;

/// At start and then every 24 h, calls `check` only while `enabled()` is true. With the switch off, no request leaves.
pub async fn run<E, C, Fut>(enabled: E, mut check: C)
where
    E: Fn() -> bool,
    C: FnMut() -> Fut,
    Fut: Future<Output = ()>,
{
    run_loop(DAY_SECS, Arc::new(Notify::new()), move || {
        let pending = enabled().then(&mut check);
        async move {
            if let Some(check) = pending {
                check.await;
            }
            // A failed check is not retried sooner: the next daily tick tries again.
            StepResult::Ok
        }
    })
    .await
}

/// The update the last check found. `install_update` installs this one only: the window never names a URL.
#[derive(Default)]
pub struct Pending(Mutex<Option<Update>>);

fn switched_on(app: &AppHandle) -> bool {
    lock(&app.state::<AppState>().settings).check_updates
}

/// Starts the daily check on the app's async runtime.
pub fn start(app: &AppHandle) {
    let (gate, worker) = (app.clone(), app.clone());
    tauri::async_runtime::spawn(run(
        move || switched_on(&gate),
        move || {
            let app = worker.clone();
            async move { check(&app).await }
        },
    ));
}

/// One request to the update host. A failed request keeps what the last good check found.
pub async fn check(app: &AppHandle) {
    let Ok(updater) = app.updater() else { return };
    let Ok(found) = updater.check().await else {
        tracing::warn!("update check failed");
        return;
    };
    let version = found.as_ref().map(|u| u.version.clone());
    *lock(&app.state::<Pending>().0) = found;
    let _ = app.emit("update", version);
}

/// The version the last check found. Makes no request. `None` while the switch is off.
pub fn pending_version(app: &AppHandle) -> Option<String> {
    if !switched_on(app) {
        return None;
    }
    lock(&app.state::<Pending>().0)
        .as_ref()
        .map(|u| u.version.clone())
}

/// Downloads, verifies the signature (the plugin does, against the key in `tauri.conf.json`), installs, restarts.
pub async fn install(app: &AppHandle) -> Result<(), String> {
    if !switched_on(app) {
        return Err("Update checks are off.".into());
    }
    let update = lock(&app.state::<Pending>().0)
        .clone()
        .ok_or("No update is waiting.")?;
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|_| "Could not install the update.")?;
    app.restart()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    async fn settle() {
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn switch_off_never_calls_the_check() {
        let on = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(AtomicUsize::new(0));
        let (flag, counter) = (on.clone(), calls.clone());
        let task = tokio::spawn(run(
            move || flag.load(Ordering::SeqCst),
            move || {
                let counter = counter.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                }
            },
        ));

        settle().await;
        tokio::time::advance(Duration::from_secs(3 * 24 * 3600)).await;
        settle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 0, "off: no check, ever");

        on.store(true, Ordering::SeqCst);
        tokio::time::advance(Duration::from_secs(24 * 3600)).await;
        settle().await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "on again: next daily tick checks"
        );
        task.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn switch_on_checks_at_start_then_daily() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let task = tokio::spawn(run(
            || true,
            move || {
                let counter = counter.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                }
            },
        ));

        settle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1, "one check at start");
        tokio::time::advance(Duration::from_secs(23 * 3600)).await;
        settle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1, "none before 24 h");
        tokio::time::advance(Duration::from_secs(3600)).await;
        settle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 2, "one more at 24 h");
        task.abort();
    }

    /// The window gets the two app commands and nothing from the updater plugin itself.
    #[test]
    fn capability_has_the_two_commands_and_no_plugin_permission() {
        let caps: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/main.json")).unwrap();
        let perms: Vec<&str> = caps["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|p| p.as_str())
            .collect();
        assert!(perms.contains(&"allow-check-update"), "{perms:?}");
        assert!(perms.contains(&"allow-install-update"), "{perms:?}");
        assert!(
            !perms.iter().any(|p| p.starts_with("updater:")),
            "the window must not get updater permissions: {perms:?}"
        );
    }
}
