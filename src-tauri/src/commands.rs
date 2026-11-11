//! The only functions the UI can call. Errors are fixed short strings; nothing secret passes through here.

use crate::connect::{self, CliAction, UrlTarget};
use crate::flyout::{self, View};
use crate::scheduler::{lock, Detected, Scheduler};
use crate::settings::{AddedAccount, Connection, Settings, SettingsPatch, SettingsStore};
use crate::window;
use quotaglass_core::view::{AccountView, Snapshot};
use quotaglass_core::Service;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};
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
/// Used by the `update_settings` command.
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
    let next = {
        let mut current = lock(&state.settings);
        let mut next = current.clone();
        next.apply(patch);
        state
            .store
            .save(&next)
            .map_err(|_| "Could not save settings.")?;
        *current = next.clone();
        // The scheduler's publish runs the snapshot callbacks on this thread, with the settings lock held:
        // they must never take it. The reset reminders get their copy of the switches first.
        crate::reset_notify::set_toasts(app, &next.reset_toasts);
        state.scheduler.apply(&next);
        next
    };
    // After the settings lock is released: the look is applied on the main thread.
    if (next.theme, next.material) != (before.theme, before.material) {
        window::apply_look(app, next.theme, next.material);
    }
    Ok(next)
}

#[tauri::command]
pub fn get_snapshot(app: AppHandle) -> Snapshot {
    let state = app.state::<AppState>();
    if !state.acrylic.load(Ordering::Relaxed) {
        emit_backdrop(&app, false);
    }
    window::resend_theme(&app);
    state.scheduler.snapshot()
}

/// The pill UI's zoom: device px per CSS px. Windows text size changes it, so the pill follows it.
/// Other windows have their own zoom and never resize the pill.
#[tauri::command]
pub async fn set_zoom(app: AppHandle, window: WebviewWindow, zoom: f64) -> Result<(), String> {
    if window.label() != "main" {
        return Ok(());
    }
    window::set_zoom(&app, zoom).map_err(|_| "Could not resize the window.".to_string())
}

/// The grip's new pill scale, 0.65 to 1.6. Ignored while the pill is locked.
#[tauri::command]
pub async fn set_pill_scale(app: AppHandle, scale: f64) -> Result<(), String> {
    window::set_pill_scale(&app, scale).map_err(|_| "Could not resize the window.".to_string())
}

/// How many service slots the pill shows, 1 to 3.
#[tauri::command]
pub async fn set_pill_slots(app: AppHandle, count: u32) -> Result<(), String> {
    window::set_pill_slots(&app, count).map_err(|_| "Could not resize the window.".to_string())
}

/// Pin (lock) or unpin the pill.
#[tauri::command]
pub async fn set_pill_locked(app: AppHandle, locked: bool) -> Result<(), String> {
    window::set_pill_locked(&app, locked)
}

/// The pill: the pointer or keyboard focus entered or left a slot. `anchor_x` is the slot centre in CSS px.
#[tauri::command]
pub fn flyout_hover(app: AppHandle, service: Service, inside: bool, anchor_x: f64) {
    flyout::hover(&app, service, inside, anchor_x);
}

/// The flyout: the pointer entered or left it.
#[tauri::command]
pub fn flyout_pointer(app: AppHandle, inside: bool) {
    flyout::pointer(&app, inside);
}

/// Click, Enter, tray, toast or first run: open the popover with focus.
#[tauri::command]
pub fn flyout_open_popover(app: AppHandle, service: Option<Service>, view: Option<View>) {
    flyout::open_popover(&app, service, view);
}

#[tauri::command]
pub fn flyout_close(app: AppHandle) {
    flyout::close(&app);
}

/// The flyout's content height in CSS px; clamped to 96-520.
#[tauri::command]
pub fn flyout_size(app: AppHandle, height_css: f64) {
    flyout::set_height(&app, height_css);
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

/// What Remove does with an added account's profile folder.
#[derive(Debug, Clone, PartialEq)]
pub enum ProfileFolder {
    /// Inside the QuotaGlass profiles: sign out, then delete this canonical path.
    Delete(PathBuf),
    /// Nothing at that path: only the settings entry goes.
    Missing,
    /// Outside the QuotaGlass profiles (or a link out of them): the folder is never touched, only the entry goes.
    Outside,
}

impl ProfileFolder {
    pub fn of(root: &Path, dir: &Path) -> Self {
        // `symlink_metadata` does not follow a link, so a dangling link counts as present and goes to the guard.
        if std::fs::symlink_metadata(dir).is_err() {
            return Self::Missing;
        }
        match connect::guard_inside(root, dir) {
            Ok(safe) => Self::Delete(safe),
            Err(_) => Self::Outside,
        }
    }
}

/// What Remove deletes: an added account only, what happens to its folder, and whether its card's pin, hidden flag
/// and label go too (not when the current login still shows the card).
#[derive(Debug, Clone, PartialEq)]
pub struct Removal {
    pub account: AddedAccount,
    pub folder: ProfileFolder,
    pub forget_card: bool,
}

pub fn removal_target(
    account_id: &str,
    cards: &[AccountView],
    added: &[AddedAccount],
    root: &Path,
) -> Result<Removal, &'static str> {
    match resolve_account(account_id, cards, added)? {
        AccountTask::CurrentLogin(_) => Err("Use Hide for your current login"),
        AccountTask::Profile(account) => Ok(Removal {
            folder: ProfileFolder::of(root, &account.profile_dir),
            account,
            forget_card: !cards.iter().any(|c| c.id == account_id && c.current_login),
        }),
    }
}

/// Deletes a profile folder after its sign-out. The guard runs again first: the sign-out ran for up to 30 s, and the
/// folder may have been swapped for a link meanwhile. Blocks; call it from a blocking task.
fn delete_profile(root: &Path, dir: &Path) -> Result<(), &'static str> {
    match ProfileFolder::of(root, dir) {
        ProfileFolder::Delete(safe) => std::fs::remove_dir_all(safe).map_err(|_| {
            "Could not delete the profile folder. Close its console window and try again."
        }),
        ProfileFolder::Missing => Ok(()),
        ProfileFolder::Outside => {
            tracing::warn!("a removed profile folder left the QuotaGlass profiles; not deleted");
            Ok(())
        }
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
    crate::reset_notify::set_toasts(app, &next.reset_toasts);
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

fn profiles_root() -> Result<PathBuf, String> {
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
/// the QuotaGlass profiles is never touched; its entry is still removed.
#[tauri::command]
pub async fn remove_account(app: AppHandle, account_id: String) -> Result<Settings, String> {
    let (cards, added) = snapshot_and_added(&app);
    let root = profiles_root()?;
    let Removal {
        account,
        folder,
        forget_card,
    } = removal_target(&account_id, &cards, &added, &root)?;

    match folder {
        ProfileFolder::Delete(safe) => {
            let tool = app
                .state::<AppState>()
                .scheduler
                .sign_in_tool(account.service);
            let service = account.service;
            let dir = account.profile_dir.clone();
            let deleted = tauri::async_runtime::spawn_blocking(move || {
                if connect::logout(service, tool.as_deref(), &connect::plain(&safe)).is_err() {
                    tracing::warn!(?service, "sign-out of a removed profile did not finish");
                }
                delete_profile(&root, &dir)
            })
            .await
            .map_err(|_| "Could not delete the profile folder.")?;
            deleted?;
        }
        ProfileFolder::Missing => {}
        ProfileFolder::Outside => {
            // No path in the log: it comes from the settings file and may name a private folder.
            tracing::warn!(
                "a removed account's folder is outside the QuotaGlass profiles; folder left in place"
            );
        }
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
            card("antigravity:d", Service::Antigravity, None, false),
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
        assert!(resolve_account("antigravity:d", &cards, &added).is_err());
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
        let root = Path::new(r"C:\no-such-root\profiles");
        assert_eq!(
            removal_target("claude:a", &cards, &added, root),
            Err("Use Hide for your current login")
        );
        assert_eq!(
            removal_target("codex:b", &cards, &added, root),
            Ok(Removal {
                account: added[0].clone(),
                folder: ProfileFolder::Missing,
                forget_card: true
            })
        );
        assert_eq!(
            removal_target("codex:c", &cards, &added, root),
            Ok(Removal {
                account: added[1].clone(),
                folder: ProfileFolder::Missing,
                forget_card: false
            }),
            "the current login keeps the card, its pin and label"
        );
        assert_eq!(
            removal_target("ghost", &cards, &added, root),
            Err("Unknown account.")
        );
    }

    #[test]
    fn remove_never_touches_a_folder_outside_the_profiles() {
        let base = std::env::temp_dir().join(format!("qg-remove-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("profiles");
        let inside = root.join("p1");
        let outside = base.join("elsewhere");
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let account = |id: &str, dir: &Path| AddedAccount {
            id: id.into(),
            service: Service::Codex,
            profile_dir: dir.to_path_buf(),
        };
        let added = [account("p1", &inside), account("p2", &outside)];

        let r = removal_target("p1", &[], &added, &root).unwrap();
        assert_eq!(
            r.folder,
            ProfileFolder::Delete(inside.canonicalize().unwrap())
        );
        let r = removal_target("p2", &[], &added, &root).unwrap();
        assert_eq!(
            r.folder,
            ProfileFolder::Outside,
            "only the entry goes: the folder is left alone"
        );
        assert_eq!(r.account, added[1]);

        // The delete checks again and leaves an outside folder in place.
        assert_eq!(delete_profile(&root, &outside), Ok(()));
        assert!(outside.is_dir());
        assert_eq!(delete_profile(&root, &inside), Ok(()));
        assert!(!inside.exists());
        assert_eq!(delete_profile(&root, &inside), Ok(()), "already gone");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Tauri refuses a command with no ACL entry. Every handler in main.rs must be in build.rs and allowed in
    /// capabilities/main.json, and nothing else.
    #[test]
    fn acl_lists_every_command() {
        use std::collections::BTreeSet;
        let main_rs = include_str!("main.rs");
        let build_rs = include_str!("../build.rs");
        let caps: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/main.json")).unwrap();

        let between = |text: &'static str, start: &str, end: &str| -> &'static str {
            let from = text.find(start).expect(start) + start.len();
            let len = text[from..].find(end).expect(end);
            &text[from..from + len]
        };
        let handlers: BTreeSet<String> = between(main_rs, "generate_handler![", "]")
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.trim_start_matches("commands::").to_string())
            .collect();
        let manifest: BTreeSet<String> = between(build_rs, "let commands = &[", "];")
            .split(',')
            .map(|s| s.trim().trim_matches('"').to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let allowed: BTreeSet<String> = caps["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|p| p.as_str()?.strip_prefix("allow-"))
            .map(|p| p.replace('-', "_"))
            .collect();

        assert_eq!(handlers.len(), 22, "{handlers:?}");
        assert!(handlers.contains("detect"));
        assert_eq!(manifest, handlers, "build.rs AppManifest::commands");
        assert_eq!(allowed, handlers, "capabilities/main.json allow-* entries");
    }
}
