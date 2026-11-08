//! The only functions the UI can call. Errors are fixed short strings; nothing secret passes through here.

use crate::scheduler::{lock, Scheduler};
use crate::settings::{Settings, SettingsPatch, SettingsStore};
use quotaglass_core::view::Snapshot;
use quotaglass_core::{Service, SourceKind};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_autostart::ManagerExt;

pub struct AppState {
    pub settings: Mutex<Settings>,
    pub store: SettingsStore,
    pub scheduler: Scheduler,
    /// False once the acrylic backdrop failed to apply; the UI then draws its own solid background.
    pub acrylic: AtomicBool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceStatus {
    kind: SourceKind,
    detected: bool,
    enabled: bool,
    unofficial: bool,
}

pub fn emit_backdrop(app: &AppHandle, acrylic: bool) {
    let _ = app.emit("backdrop", serde_json::json!({ "acrylic": acrylic }));
}

/// Applies a patch: window and autostart side effects first, then the file, then the scheduler.
/// Used by the `update_settings` command and the tray's Pin item.
pub fn update(app: &AppHandle, patch: SettingsPatch) -> Result<Settings, String> {
    let state = app.state::<AppState>();
    let (before, after) = {
        let current = lock(&state.settings);
        let mut next = current.clone();
        next.apply(patch.clone());
        (current.clone(), next)
    };

    if after.always_on_top != before.always_on_top {
        if let Some(window) = app.get_webview_window("main") {
            window
                .set_always_on_top(after.always_on_top)
                .map_err(|_| "Could not change always on top.")?;
        }
    }
    if after.autostart != before.autostart {
        let launch = app.autolaunch();
        let result = if after.autostart {
            launch.enable()
        } else {
            launch.disable()
        };
        result.map_err(|_| "Could not change start with Windows.")?;
    }

    // Apply to the latest settings, not the copy above: the window rect may have been saved meanwhile.
    let saved = {
        let mut current = lock(&state.settings);
        let mut next = current.clone();
        next.apply(patch);
        state
            .store
            .save(&next)
            .map_err(|_| "Could not save settings.")?;
        *current = next.clone();
        next
    };
    state.scheduler.apply(&saved);
    Ok(saved)
}

#[tauri::command]
pub fn get_snapshot(app: AppHandle) -> Snapshot {
    let state = app.state::<AppState>();
    if !state.acrylic.load(Ordering::Relaxed) {
        emit_backdrop(&app, false);
    }
    state.scheduler.snapshot()
}

#[tauri::command]
pub fn refresh_now(app: AppHandle) {
    app.state::<AppState>().scheduler.refresh_now();
}

#[tauri::command]
pub fn detect_sources(app: AppHandle) -> Vec<SourceStatus> {
    let state = app.state::<AppState>();
    let (direct, orca) = state.scheduler.detected();
    let on = lock(&state.settings).sources;
    vec![
        SourceStatus {
            kind: SourceKind::Direct,
            detected: direct,
            enabled: on.direct,
            unofficial: true,
        },
        SourceStatus {
            kind: SourceKind::Orca,
            detected: orca,
            enabled: on.orca,
            unofficial: false,
        },
    ]
}

#[tauri::command]
pub fn get_settings(app: AppHandle) -> Settings {
    lock(&app.state::<AppState>().settings).clone()
}

#[tauri::command]
pub async fn update_settings(app: AppHandle, patch: SettingsPatch) -> Result<Settings, String> {
    update(&app, patch)
}

#[tauri::command]
pub async fn connect(app: AppHandle, service: Service) -> Result<(), String> {
    let tool = app.state::<AppState>().scheduler.sign_in_tool(service);
    crate::connect::open(service, tool.as_deref()).map_err(String::from)
}
