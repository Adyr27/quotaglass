//! Runs vendor CLIs (`claude`, `codex`) with fixed argument arrays, owns QuotaGlass profile folders,
//! and opens strictly allowlisted HTTPS URLs in the default browser.

use quotaglass_core::view::AccountView;
use quotaglass_core::{Env, Service};
use serde::Deserialize;
use std::collections::hash_map::RandomState;
use std::ffi::OsString;
use std::hash::BuildHasher;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::settings::AddedAccount;

const CONSOLE_TITLE: &str = "QuotaGlass sign-in";
const LOGOUT_TIMEOUT: Duration = Duration::from_secs(30);
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// What CLI action to launch for an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliAction {
    Login,
    Open,
    Logout,
}

/// First matching CLI binary on `PATH` for `service`.
pub fn sign_in_tool(env: &Env, service: Service) -> Option<PathBuf> {
    match service {
        Service::Claude => env.find_on_path(&["claude.cmd", "claude.exe"]),
        Service::Codex => env.find_on_path(&["codex.cmd", "codex.exe"]),
        Service::Antigravity => None,
    }
}

/// `%APPDATA%\QuotaGlass\profiles`, or `None` when APPDATA is unset or relative.
pub fn profiles_root() -> Option<PathBuf> {
    let appdata = PathBuf::from(std::env::var_os("APPDATA")?);
    appdata
        .is_absolute()
        .then(|| appdata.join("QuotaGlass").join("profiles"))
}

static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 32 lowercase hex characters identifying a newly created profile directory.
pub fn new_profile_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let pid = std::process::id();
    let n = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    let a = RandomState::new().hash_one((nanos, pid, n, 0u8));
    let b = RandomState::new().hash_one((nanos, pid, n, 1u8));
    format!("{a:016x}{b:016x}")
}

/// Creates `root\<new id>` atomically (`create_dir`), skipping any ID already in `taken`.
pub fn create_profile_dir(root: &Path, taken: &[String]) -> io::Result<(String, PathBuf)> {
    std::fs::create_dir_all(root)?;
    for _ in 0..16 {
        let id = new_profile_id();
        if taken.contains(&id) {
            continue;
        }
        let dir = root.join(&id);
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok((id, dir)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other("no free profile ID"))
}

/// `Ok(canonical p)` only when `p` exists and, after resolving `..`, symlinks, and junctions,
/// sits strictly inside `root`.
pub fn guard_inside(root: &Path, p: &Path) -> Result<PathBuf, String> {
    let root = root
        .canonicalize()
        .map_err(|_| "The profiles folder is missing.".to_string())?;
    let target = p
        .canonicalize()
        .map_err(|_| "The profile folder is missing.".to_string())?;
    if target != root && target.starts_with(&root) {
        Ok(target)
    } else {
        Err("That profile folder is outside the QuotaGlass profiles.".to_string())
    }
}

/// Strips `\\?\` / `\\?\UNC\` verbatim prefixes after canonicalization for CLI environment vars.
pub fn plain(p: &Path) -> PathBuf {
    if let Some(s) = p.to_str() {
        if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            if rest.as_bytes().get(1) == Some(&b':') {
                return PathBuf::from(rest);
            }
        }
    }
    p.to_path_buf()
}

fn vendor_args(service: Service, action: CliAction) -> &'static [&'static str] {
    match (service, action) {
        (Service::Claude, CliAction::Login) => &["auth", "login"],
        (Service::Claude, CliAction::Logout) => &["auth", "logout"],
        (Service::Codex, CliAction::Login) => &["login"],
        (Service::Codex, CliAction::Logout) => &["logout"],
        (_, CliAction::Open) | (Service::Antigravity, _) => &[],
    }
}

fn profile_var(service: Service) -> Option<&'static str> {
    match service {
        Service::Claude => Some("CLAUDE_CONFIG_DIR"),
        Service::Codex => Some("CODEX_HOME"),
        Service::Antigravity => None,
    }
}

pub fn cli_command(
    service: Service,
    tool: &Path,
    action: CliAction,
    profile: Option<&Path>,
) -> (Vec<OsString>, Vec<(&'static str, PathBuf)>) {
    let mut args: Vec<OsString> = Vec::new();
    if action != CliAction::Logout {
        args.extend(["/c", "start", CONSOLE_TITLE].map(OsString::from));
        args.push(cmd_exe().into_os_string());
        args.push("/k".into());
        args.push(tool.as_os_str().to_owned());
    }
    args.extend(vendor_args(service, action).iter().map(OsString::from));
    let env = match (profile_var(service), profile) {
        (Some(var), Some(dir)) => vec![(var, dir.to_path_buf())],
        _ => Vec::new(),
    };
    (args, env)
}

const CMD_META: [char; 10] = ['&', '|', '<', '>', '^', '%', '!', '"', '(', ')'];
const UNSAFE_TOOL: &str = "The tool path contains characters cmd.exe cannot run safely.";

fn system32_from(system_root: Option<OsString>) -> PathBuf {
    system_root
        .map(PathBuf::from)
        .filter(|p| p.is_absolute() && !p.to_string_lossy().contains(CMD_META))
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("System32")
}

fn system32() -> PathBuf {
    system32_from(std::env::var_os("SystemRoot"))
}

fn cmd_exe() -> PathBuf {
    system32().join("cmd.exe")
}

fn cmd_launcher() -> Command {
    let mut cmd = Command::new(cmd_exe());
    cmd.current_dir(system32());
    cmd
}

fn hide_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
        let _ = CREATE_NO_WINDOW;
    }
}

fn taskkill_args(pid: u32) -> Vec<OsString> {
    ["/PID", &pid.to_string(), "/T", "/F"]
        .map(OsString::from)
        .to_vec()
}

fn kill_tree(pid: u32) {
    let mut cmd = Command::new(system32().join("taskkill.exe"));
    cmd.args(taskkill_args(pid))
        .current_dir(system32())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hide_window(&mut cmd);
    let _ = cmd.status();
}

fn checked_tool(service: Service, tool: Option<&Path>) -> Result<&Path, &'static str> {
    let tool = tool
        .filter(|t| t.is_absolute())
        .filter(|_| profile_var(service).is_some())
        .ok_or("No sign-in tool for that service.")?;
    if tool.to_string_lossy().contains(CMD_META) {
        return Err(UNSAFE_TOOL);
    }
    Ok(tool)
}

pub fn open_console(
    service: Service,
    tool: Option<&Path>,
    action: CliAction,
    profile: Option<&Path>,
) -> Result<(), &'static str> {
    if action == CliAction::Logout {
        return Err("Sign-out does not open a window.");
    }
    let tool = checked_tool(service, tool)?;
    if profile.is_some_and(|p| !p.is_absolute()) {
        return Err("The profile folder is not valid.");
    }
    let (args, env) = cli_command(service, tool, action, profile);
    cmd_launcher()
        .args(args)
        .envs(env)
        .spawn()
        .map(drop)
        .map_err(|_| "Could not open the sign-in window.")
}

pub fn open(service: Service, tool: Option<&Path>) -> Result<(), &'static str> {
    open_console(service, tool, CliAction::Login, None)
}

pub fn logout(service: Service, tool: Option<&Path>, profile: &Path) -> Result<(), &'static str> {
    let tool = checked_tool(service, tool)?;
    if !profile.is_absolute() {
        return Err("The profile folder is not valid.");
    }
    let (args, env) = cli_command(service, tool, CliAction::Logout, Some(profile));
    let mut cmd = Command::new(tool);
    cmd.args(args)
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hide_window(&mut cmd);
    let mut child = cmd.spawn().map_err(|_| "Could not run the sign-out.")?;
    let deadline = Instant::now() + LOGOUT_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => return Err("The sign-out failed."),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            _ => {
                kill_tree(child.id());
                let _ = child.kill();
                let _ = child.wait();
                return Err("The sign-out timed out.");
            }
        }
    }
}

/// Strictly allowlisted HTTPS pages QuotaGlass may open in the default browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UrlTarget {
    OpenAiUsage,
    ClaudeUsage,
    GitHubReleases,
}

impl UrlTarget {
    pub fn url(self) -> &'static str {
        match self {
            UrlTarget::OpenAiUsage => "https://chatgpt.com/codex/settings/usage",
            UrlTarget::ClaudeUsage => "https://claude.ai/settings/usage",
            UrlTarget::GitHubReleases => "https://github.com/Adyr27/quotaglass/releases/latest",
        }
    }
}

pub fn open_url(target: UrlTarget) -> Result<(), &'static str> {
    let mut cmd = cmd_launcher();
    cmd.args(["/c", "start", "", target.url()]);
    hide_window(&mut cmd);
    cmd.spawn()
        .map(drop)
        .map_err(|_| "Could not open the browser.")
}

#[derive(Debug, Clone, PartialEq)]
pub enum AccountTask {
    CurrentLogin(Service),
    Profile(AddedAccount),
}

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

#[derive(Debug, Clone, PartialEq)]
pub enum ProfileFolder {
    Delete(PathBuf),
    Missing,
    Outside,
}

impl ProfileFolder {
    pub fn of(root: &Path, dir: &Path) -> Self {
        if std::fs::symlink_metadata(dir).is_err() {
            return Self::Missing;
        }
        match guard_inside(root, dir) {
            Ok(safe) => Self::Delete(safe),
            Err(_) => Self::Outside,
        }
    }
}

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

/// Re-verifies `guard_inside` after logout before removing the folder.
pub fn delete_profile(root: &Path, dir: &Path) -> Result<(), &'static str> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use quotaglass_core::view::{CardState, Freshness};
    use std::ffi::OsStr;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("qg-app-conn-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn os(v: &[&str]) -> Vec<OsString> {
        v.iter().map(OsString::from).collect()
    }

    #[test]
    fn guard_accepts_child() {
        let base = temp_dir("guard-ok");
        let root = base.join("profiles");
        let child = root.join("abc");
        std::fs::create_dir_all(child.join("deeper")).unwrap();
        let ok = guard_inside(&root, &child).unwrap();
        assert_eq!(ok, child.canonicalize().unwrap());
        assert!(guard_inside(&root, &child.join("deeper")).is_ok());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn guard_rejects_outside_paths() {
        let base = temp_dir("guard-bad");
        let root = base.join("profiles");
        let child = root.join("abc");
        let sibling = base.join("profiles2");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();

        assert!(guard_inside(&root, &root).is_err());
        assert!(guard_inside(&root, &root.join(".")).is_err());
        assert!(guard_inside(&root, &child.join("..")).is_err());
        assert!(guard_inside(&root, &child.join("..").join("..").join("profiles2")).is_err());
        assert!(guard_inside(&root, &sibling).is_err());
        assert!(guard_inside(&root, &base).is_err());
        assert!(guard_inside(&root, &root.join("missing")).is_err());
        assert!(guard_inside(&root, Path::new("abc")).is_err());

        #[cfg(windows)]
        {
            let link = root.join("link-out");
            if std::os::windows::fs::symlink_dir(&sibling, &link).is_ok() {
                assert!(guard_inside(&root, &link).is_err());
            }
            let junction = root.join("junction-out");
            let made = Command::new("cmd.exe")
                .args([OsStr::new("/c"), OsStr::new("mklink"), OsStr::new("/J")])
                .arg(&junction)
                .arg(&sibling)
                .output()
                .is_ok_and(|o| o.status.success());
            if made {
                assert!(guard_inside(&root, &junction).is_err());
                std::fs::remove_dir(&junction).unwrap();
                assert!(sibling.is_dir());
            }
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn cli_command_sets_profile_env_and_fixed_args() {
        let tool = Path::new(r"C:\Program Files\nodejs\claude.cmd");
        let profile = Path::new(r"C:\appdata\QuotaGlass\profiles\abc");
        for action in [CliAction::Login, CliAction::Open, CliAction::Logout] {
            let (_, env) = cli_command(Service::Claude, tool, action, Some(profile));
            assert_eq!(env, [("CLAUDE_CONFIG_DIR", profile.to_path_buf())]);
            let (_, env) = cli_command(Service::Codex, tool, action, Some(profile));
            assert_eq!(env, [("CODEX_HOME", profile.to_path_buf())]);
            let (_, env) = cli_command(Service::Claude, tool, action, None);
            assert!(env.is_empty());
            let (_, env) = cli_command(Service::Antigravity, tool, action, Some(profile));
            assert!(env.is_empty());
        }

        let console = |rest: &[&str]| {
            let mut v = os(&["/c", "start", "QuotaGlass sign-in"]);
            v.push(cmd_exe().into_os_string());
            v.push("/k".into());
            v.push(tool.as_os_str().to_owned());
            v.extend(os(rest));
            v
        };
        let cases: [(Service, CliAction, Vec<OsString>); 6] = [
            (Service::Claude, CliAction::Login, console(&["auth", "login"])),
            (Service::Claude, CliAction::Open, console(&[])),
            (Service::Claude, CliAction::Logout, os(&["auth", "logout"])),
            (Service::Codex, CliAction::Login, console(&["login"])),
            (Service::Codex, CliAction::Open, console(&[])),
            (Service::Codex, CliAction::Logout, os(&["logout"])),
        ];
        for (service, action, want) in cases {
            for p in [None, Some(profile)] {
                let (args, _) = cli_command(service, tool, action, p);
                assert_eq!(args, want, "{service:?} {action:?}");
                assert!(!args.iter().any(|a| a == profile.as_os_str()));
            }
        }
    }

    #[test]
    fn console_runs_absolute_cmd_and_system32_fallback() {
        let tool = Path::new(r"C:\tools\claude.cmd");
        let (args, _) = cli_command(Service::Claude, tool, CliAction::Open, None);
        let cmd = cmd_exe();
        assert!(cmd.is_absolute());
        assert!(cmd.ends_with(r"System32\cmd.exe"));
        assert_eq!(args[3], cmd.as_os_str());
        assert!(!args.iter().any(|a| a == OsStr::new("cmd")));

        assert_eq!(
            system32_from(Some(r"D:\Win".into())),
            PathBuf::from(r"D:\Win\System32")
        );
        for bad in [None, Some("Windows".into()), Some(r"C:\a&b".into())] {
            assert_eq!(system32_from(bad), PathBuf::from(r"C:\Windows\System32"));
        }
        assert_eq!(taskkill_args(42), os(&["/PID", "42", "/T", "/F"]));
    }

    #[test]
    fn tool_with_cmd_metacharacters_and_relative_paths_refused() {
        for bad in [
            r"C:\a&b\claude.cmd",
            r"C:\a^b\claude.cmd",
            r"C:\100%\claude.cmd",
            r"C:\a!b\claude.cmd",
            r"C:\Program Files (x86)\claude.cmd",
            r"C:\a|b\claude.cmd",
            r"C:\a<b>\claude.cmd",
            r#"C:\a"b\claude.cmd"#,
        ] {
            assert_eq!(
                checked_tool(Service::Claude, Some(Path::new(bad))),
                Err(UNSAFE_TOOL)
            );
            assert!(
                open_console(Service::Claude, Some(Path::new(bad)), CliAction::Open, None).is_err()
            );
            assert!(logout(
                Service::Claude,
                Some(Path::new(bad)),
                Path::new(r"C:\p\abc")
            )
            .is_err());
        }
        let claude = Some(Path::new(r"C:\x\claude.cmd"));
        assert!(open(Service::Claude, None).is_err());
        assert!(open(Service::Claude, Some(Path::new("claude.cmd"))).is_err());
        assert!(open_console(
            Service::Antigravity,
            Some(Path::new(r"C:\x\agy.exe")),
            CliAction::Open,
            None
        )
        .is_err());
        assert!(open_console(Service::Claude, claude, CliAction::Logout, None).is_err());
        assert!(open_console(
            Service::Claude,
            claude,
            CliAction::Open,
            Some(Path::new("rel"))
        )
        .is_err());
        assert!(logout(Service::Claude, None, Path::new(r"C:\p\abc")).is_err());
        assert!(logout(Service::Claude, claude, Path::new("rel")).is_err());
    }

    #[test]
    fn new_profile_id_is_hex_32_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..500 {
            let id = new_profile_id();
            assert_eq!(id.len(), 32);
            assert!(id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
            assert!(seen.insert(id));
        }

        let base = temp_dir("create");
        let root = base.join("profiles");
        let (a, dir_a) = create_profile_dir(&root, &[]).unwrap();
        let (b, dir_b) = create_profile_dir(&root, std::slice::from_ref(&a)).unwrap();
        assert_ne!(a, b);
        assert_eq!(dir_a, root.join(&a));
        assert!(dir_a.is_dir() && dir_b.is_dir());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn url_targets_are_https_allowlisted() {
        for target in [
            UrlTarget::OpenAiUsage,
            UrlTarget::ClaudeUsage,
            UrlTarget::GitHubReleases,
        ] {
            let url = target.url();
            assert!(url.starts_with("https://"), "{url}");
            assert!(!url
                .chars()
                .any(|c| c.is_whitespace() || "&|<>^%\"()!`'".contains(c)));
        }
        assert_eq!(
            UrlTarget::OpenAiUsage.url(),
            "https://chatgpt.com/codex/settings/usage"
        );
        assert_eq!(
            UrlTarget::ClaudeUsage.url(),
            "https://claude.ai/settings/usage"
        );
        assert_eq!(
            UrlTarget::GitHubReleases.url(),
            "https://github.com/Adyr27/quotaglass/releases/latest"
        );
        assert_eq!(
            serde_json::from_str::<UrlTarget>(r#""openAiUsage""#).unwrap(),
            UrlTarget::OpenAiUsage
        );
        assert_eq!(
            serde_json::from_str::<UrlTarget>(r#""claudeUsage""#).unwrap(),
            UrlTarget::ClaudeUsage
        );
        assert_eq!(
            serde_json::from_str::<UrlTarget>(r#""gitHubReleases""#).unwrap(),
            UrlTarget::GitHubReleases
        );
        assert!(serde_json::from_str::<UrlTarget>(r#""https://evil.example""#).is_err());
    }

    #[test]
    fn plain_strips_verbatim_disk_prefix() {
        assert_eq!(plain(Path::new(r"\\?\C:\p\u")), PathBuf::from(r"C:\p\u"));
        assert_eq!(
            plain(Path::new(r"\\?\UNC\srv\share\p")),
            PathBuf::from(r"\\srv\share\p")
        );
        assert_eq!(plain(Path::new(r"C:\x")), PathBuf::from(r"C:\x"));
    }

    #[test]
    fn remove_never_touches_outside_folder() {
        let base = temp_dir("remove");
        let root = base.join("profiles");
        let inside = root.join("p1");
        let outside = base.join("elsewhere");
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::create_dir_all(&outside).unwrap();

        let added = [
            AddedAccount {
                id: "p1".into(),
                service: Service::Codex,
                profile_dir: inside.clone(),
            },
            AddedAccount {
                id: "p2".into(),
                service: Service::Codex,
                profile_dir: outside.clone(),
            },
        ];
        let card = AccountView {
            id: "claude:a".into(),
            service: Service::Claude,
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
            forecast_mins: None,
            best_now: false,
            banked_resets: None,
            profile_id: None,
            current_login: true,
            hidden: false,
        };
        assert!(removal_target("claude:a", &[card], &added, &root).is_err());
        let r1 = removal_target("p1", &[], &added, &root).unwrap();
        assert_eq!(r1.folder, ProfileFolder::Delete(inside.canonicalize().unwrap()));
        let r2 = removal_target("p2", &[], &added, &root).unwrap();
        assert_eq!(r2.folder, ProfileFolder::Outside);

        assert_eq!(delete_profile(&root, &outside), Ok(()));
        assert!(outside.is_dir());
        assert_eq!(delete_profile(&root, &inside), Ok(()));
        assert!(!inside.exists());
        let _ = std::fs::remove_dir_all(&base);
    }
}
