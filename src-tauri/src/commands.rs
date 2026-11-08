//! The only functions the UI can call. Errors are fixed short strings; nothing secret passes through here.

use crate::connect::{self, CliAction, UrlTarget};
use crate::scheduler::{lock, Detected, Scheduler};
use crate::settings::{AddedAccount, Connection, Settings, SettingsPatch, SettingsStore};
use quotaglass_core::view::{AccountView, Snapshot};
use quotaglass_core::Service;
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

pub fn emit_backdrop(app: &AppHandle, acrylic: bool) {
    let _ = app.emit("backdrop", serde_json::json!({ "acrylic": acrylic }));
}

/// Applies a patch: window and autostart side effects first, then the file, then the scheduler.
/// Used by the `update_settings` command and the tray's Pin item (always on top).
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
    // The scheduler takes them while the settings lock is still held, so the last saved settings are the last applied.
    // Lock order: settings, then the scheduler's own locks. Nothing the scheduler runs takes the settings lock.
    let mut current = lock(&state.settings);
    let mut next = current.clone();
    next.apply(patch);
    state
        .store
        .save(&next)
        .map_err(|_| "Could not save settings.")?;
    *current = next.clone();
    state.scheduler.apply(&next);
    Ok(next)
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

/// Which one-click sources apply on this PC. The UI reads `connected` from the settings for what is on.
#[tauri::command]
pub fn detect(app: AppHandle) -> Detected {
    app.state::<AppState>().scheduler.detected()
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
    connect::open(service, tool.as_deref()).map_err(String::from)
}

/// What a card's account runs on: the CLI's current login, or an added account's profile.
#[derive(Debug, Clone, PartialEq)]
pub enum AccountTask {
    CurrentLogin(Service),
    Profile(AddedAccount),
}

/// Maps a card ID from the UI to its task. The card must be in the current snapshot; an added account with no card yet
/// (sign-in not finished) is found by its own ID. A card read by both an added profile and the current login maps to
/// the profile: that is the login QuotaGlass manages. Any other ID is refused.
pub fn resolve_account(
    account_id: &str,
    cards: &[AccountView],
    added: &[AddedAccount],
) -> Result<AccountTask, &'static str> {
    let profile = |id: &str| {
        added
            .iter()
            .find(|a| a.id == id)
            .cloned()
            .map(AccountTask::Profile)
    };
    match cards.iter().find(|c| c.id == account_id) {
        Some(card) => card
            .profile_id
            .as_deref()
            .and_then(profile)
            .or_else(|| {
                card.current_login
                    .then_some(AccountTask::CurrentLogin(card.service))
            })
            .ok_or("That account has no sign-in on this PC."),
        None => profile(account_id).ok_or("Unknown account."),
    }
}

/// What Remove deletes: an added account only, and whether its card's pin, hidden flag and label go too (not when the
/// current login still shows the card).
#[derive(Debug, Clone, PartialEq)]
pub struct Removal {
    pub account: AddedAccount,
    pub forget_card: bool,
}

pub fn removal_target(
    account_id: &str,
    cards: &[AccountView],
    added: &[AddedAccount],
) -> Result<Removal, &'static str> {
    match resolve_account(account_id, cards, added)? {
        AccountTask::CurrentLogin(_) => Err("Use Hide for your current login"),
        AccountTask::Profile(account) => Ok(Removal {
            account,
            forget_card: !cards.iter().any(|c| c.id == account_id && c.current_login),
        }),
    }
}

/// Every settings change from the account commands: change a copy, save it, then hand it to the scheduler while the
/// settings lock is held, as `update` does.
fn mutate(
    app: &AppHandle,
    change: impl FnOnce(&mut Settings) -> Result<(), String>,
) -> Result<Settings, String> {
    let state = app.state::<AppState>();
    let mut current = lock(&state.settings);
    let mut next = current.clone();
    change(&mut next)?;
    next.clamp();
    state
        .store
        .save(&next)
        .map_err(|_| "Could not save settings.")?;
    *current = next.clone();
    state.scheduler.apply(&next);
    Ok(next)
}

fn snapshot_and_added(app: &AppHandle) -> (Vec<AccountView>, Vec<AddedAccount>) {
    let state = app.state::<AppState>();
    let added = lock(&state.settings).added_accounts.clone();
    (state.scheduler.snapshot().accounts, added)
}

/// Refuses an ID that is not a card on screen. Used before adding a pin, a hidden flag or a label.
fn require_card(app: &AppHandle, account_id: &str) -> Result<(), String> {
    let (cards, _) = snapshot_and_added(app);
    cards
        .iter()
        .any(|c| c.id == account_id)
        .then_some(())
        .ok_or_else(|| "Unknown account.".to_string())
}

fn profiles_root() -> Result<std::path::PathBuf, String> {
    connect::profiles_root().ok_or_else(|| "APPDATA is not set.".to_string())
}

/// "+ Add account": a new profile folder, its settings entry, then the vendor's sign-in with that folder. When the
/// sign-in window cannot open, the folder and the entry are removed again.
#[tauri::command]
pub async fn add_account(app: AppHandle, service: Service) -> Result<Settings, String> {
    if !matches!(service, Service::Claude | Service::Codex) {
        return Err("Add account works for Claude and Codex.".into());
    }
    let state = app.state::<AppState>();
    let tool = state
        .scheduler
        .sign_in_tool(service)
        .ok_or("No sign-in tool for that service.")?;
    let root = profiles_root()?;
    let taken: Vec<String> = lock(&state.settings)
        .added_accounts
        .iter()
        .map(|a| a.id.clone())
        .collect();
    let (id, dir) = connect::create_profile_dir(&root, &taken)
        .map_err(|_| "Could not create the profile folder.")?;

    let launched = mutate(&app, |s| {
        if s.added_accounts.iter().any(|a| a.id == id) {
            return Err("Could not create the profile folder.".into());
        }
        s.added_accounts.push(AddedAccount {
            id: id.clone(),
            service,
            profile_dir: dir.clone(),
        });
        Ok(())
    })
    .and_then(|settings| {
        let safe = connect::guard_inside(&root, &dir)?;
        connect::open_console(
            service,
            Some(&tool),
            CliAction::Login,
            Some(&connect::plain(&safe)),
        )?;
        Ok(settings)
    });
    if launched.is_err() {
        if let Ok(safe) = connect::guard_inside(&root, &dir) {
            let _ = std::fs::remove_dir_all(safe);
        }
        let _ = mutate(&app, |s| {
            s.added_accounts.retain(|a| a.id != id);
            Ok(())
        });
    }
    launched
}

/// Opens the CLI of a card in a console: the current login with its inherited folder, or an added account with its
/// profile folder (checked to be inside the QuotaGlass profiles).
#[tauri::command]
pub async fn open_cli(app: AppHandle, account_id: String) -> Result<(), String> {
    let (cards, added) = snapshot_and_added(&app);
    let scheduler = &app.state::<AppState>().scheduler;
    match resolve_account(&account_id, &cards, &added)? {
        AccountTask::CurrentLogin(service) => {
            let tool = scheduler.sign_in_tool(service);
            connect::open_console(service, tool.as_deref(), CliAction::Open, None)?;
        }
        AccountTask::Profile(a) => {
            let safe = connect::guard_inside(&profiles_root()?, &a.profile_dir)?;
            let tool = scheduler.sign_in_tool(a.service);
            connect::open_console(
                a.service,
                tool.as_deref(),
                CliAction::Open,
                Some(&connect::plain(&safe)),
            )?;
        }
    }
    Ok(())
}

/// Remove for an added account: the vendor's sign-out with its profile, then the profile folder, then the entry.
/// A failed sign-out does not stop the delete: deleting the folder removes the login from this PC. A folder outside
/// the QuotaGlass profiles is never touched.
#[tauri::command]
pub async fn remove_account(app: AppHandle, account_id: String) -> Result<Settings, String> {
    let (cards, added) = snapshot_and_added(&app);
    let Removal {
        account,
        forget_card,
    } = removal_target(&account_id, &cards, &added)?;
    let root = profiles_root()?;

    // `symlink_metadata` does not follow a link, so a dangling link counts as present and goes to the guard.
    if std::fs::symlink_metadata(&account.profile_dir).is_ok() {
        let safe = connect::guard_inside(&root, &account.profile_dir)?;
        let tool = app
            .state::<AppState>()
            .scheduler
            .sign_in_tool(account.service);
        let service = account.service;
        let profile = connect::plain(&safe);
        let signed_out = tauri::async_runtime::spawn_blocking(move || {
            connect::logout(service, tool.as_deref(), &profile)
        })
        .await;
        if !matches!(signed_out, Ok(Ok(()))) {
            tracing::warn!(?service, "sign-out of a removed profile did not finish");
        }
        std::fs::remove_dir_all(&safe).map_err(|_| {
            "Could not delete the profile folder. Close its console window and try again."
        })?;
    }

    mutate(&app, |s| {
        s.forget_added(&account.id, &account_id, forget_card);
        Ok(())
    })
}

#[tauri::command]
pub async fn set_hidden(
    app: AppHandle,
    account_id: String,
    hidden: bool,
) -> Result<Settings, String> {
    if hidden {
        require_card(&app, &account_id)?;
    }
    mutate(&app, |s| {
        s.set_hidden(&account_id, hidden);
        Ok(())
    })
}

#[tauri::command]
pub async fn set_pinned(
    app: AppHandle,
    account_id: String,
    pinned: bool,
) -> Result<Settings, String> {
    if pinned {
        require_card(&app, &account_id)?;
    }
    mutate(&app, |s| {
        s.set_pinned(&account_id, pinned).map_err(String::from)
    })
}

#[tauri::command]
pub async fn set_label(
    app: AppHandle,
    account_id: String,
    label: Option<String>,
) -> Result<Settings, String> {
    if label.is_some() {
        require_card(&app, &account_id)?;
    }
    mutate(&app, |s| {
        s.set_label(&account_id, label);
        Ok(())
    })
}

#[tauri::command]
pub async fn connect_source(
    app: AppHandle,
    conn: Connection,
    on: bool,
) -> Result<Settings, String> {
    mutate(&app, |s| {
        s.set_connected(conn, on);
        Ok(())
    })
}

#[tauri::command]
pub async fn open_url(target: UrlTarget) -> Result<(), String> {
    connect::open_url(target).map_err(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use quotaglass_core::view::{CardState, Freshness};
    use std::path::PathBuf;

    fn card(id: &str, service: Service, profile: Option<&str>, current: bool) -> AccountView {
        AccountView {
            id: id.into(),
            service,
            label: "x".into(),
            plan: None,
            source: None,
            freshness: Freshness::Stale,
            age_secs: None,
            headline: None,
            buckets: vec![],
            state: CardState::LoginExpired,
            reason: None,
            room_left: None,
            best_now: false,
            banked_resets: None,
            profile_id: profile.map(String::from),
            current_login: current,
            pinned: false,
            hidden: false,
        }
    }

    fn added(id: &str) -> AddedAccount {
        AddedAccount {
            id: id.into(),
            service: Service::Codex,
            profile_dir: PathBuf::from(format!(r"C:\p\{id}")),
        }
    }

    #[test]
    fn resolve_maps_cards_to_tasks() {
        let cards = [
            card("claude:a", Service::Claude, None, true),
            card("codex:b", Service::Codex, Some("p1"), false),
            card("codex:c", Service::Codex, Some("p2"), true),
            card("orca:d", Service::Antigravity, None, false),
        ];
        let added = [added("p1"), added("p2"), added("p3")];
        assert_eq!(
            resolve_account("claude:a", &cards, &added),
            Ok(AccountTask::CurrentLogin(Service::Claude))
        );
        assert_eq!(
            resolve_account("codex:b", &cards, &added),
            Ok(AccountTask::Profile(added[0].clone()))
        );
        assert_eq!(
            resolve_account("codex:c", &cards, &added),
            Ok(AccountTask::Profile(added[1].clone()))
        );
        assert!(resolve_account("orca:d", &cards, &added).is_err());
        assert_eq!(
            resolve_account("p3", &cards, &added),
            Ok(AccountTask::Profile(added[2].clone())),
            "an added account with no card yet"
        );
        assert_eq!(
            resolve_account("nope", &cards, &added),
            Err("Unknown account.")
        );
        assert_eq!(
            resolve_account(r"..\..\Windows", &cards, &added),
            Err("Unknown account.")
        );
    }

    #[test]
    fn remove_rejects_current_login() {
        let cards = [
            card("claude:a", Service::Claude, None, true),
            card("codex:b", Service::Codex, Some("p1"), false),
            card("codex:c", Service::Codex, Some("p2"), true),
        ];
        let added = [added("p1"), added("p2")];
        assert_eq!(
            removal_target("claude:a", &cards, &added),
            Err("Use Hide for your current login")
        );
        assert_eq!(
            removal_target("codex:b", &cards, &added),
            Ok(Removal {
                account: added[0].clone(),
                forget_card: true
            })
        );
        assert_eq!(
            removal_target("codex:c", &cards, &added),
            Ok(Removal {
                account: added[1].clone(),
                forget_card: false
            }),
            "the current login keeps the card, its pin and label"
        );
        assert_eq!(
            removal_target("ghost", &cards, &added),
            Err("Unknown account.")
        );
    }
}
