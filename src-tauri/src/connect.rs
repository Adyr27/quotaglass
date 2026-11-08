//! Runs the vendor CLIs (sign in, open, sign out) and owns the QuotaGlass profile folders. QuotaGlass never handles a
//! login. Every launch uses the absolute tool path found on PATH and a fixed argument list: nothing from the UI is
//! added to an argument. Profile paths come from the settings file, so callers check them with `guard_inside` first.

use quotaglass_core::Service;
use serde::Deserialize;
use std::collections::hash_map::RandomState;
use std::ffi::OsString;
use std::hash::BuildHasher;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const CONSOLE_TITLE: &str = "QuotaGlass sign-in";
const LOGOUT_TIMEOUT: Duration = Duration::from_secs(30);
/// `CREATE_NO_WINDOW`: sign-out and the URL opener run without a console window.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// What to run for an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliAction {
    /// The vendor's sign-in, in a visible console.
    Login,
    /// The CLI itself, in a visible console, so the user can run `/login` there.
    Open,
    /// The vendor's sign-out, hidden, awaited with a timeout.
    Logout,
}

/// `%APPDATA%\QuotaGlass\profiles`, or `None` when APPDATA is not set or not absolute.
pub fn profiles_root() -> Option<PathBuf> {
    let appdata = PathBuf::from(std::env::var_os("APPDATA")?);
    appdata
        .is_absolute()
        .then(|| appdata.join("QuotaGlass").join("profiles"))
}

static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 32 lowercase hex characters: two randomly keyed 64-bit hashes of (time, process, counter). Not a secret; it only
/// has to be unpredictable enough that two profiles never share a folder. `create_profile_dir` makes it unique on disk.
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

/// Creates `root\<new id>` and returns the ID and the folder. `create_dir` fails on a folder that exists, so a folder
/// is never reused; an ID in `taken` (already in the settings) is skipped too.
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

/// `Ok(canonical p)` only when `p` exists and, with every `..`, symlink and junction resolved, is strictly inside
/// `root`. The root itself, a sibling, a parent and a link that points out are refused.
pub fn guard_inside(root: &Path, p: &Path) -> Result<PathBuf, String> {
    let root = root
        .canonicalize()
        .map_err(|_| "The profiles folder is missing.".to_string())?;
    let target = p
        .canonicalize()
        .map_err(|_| "The profile folder is missing.".to_string())?;
    // `starts_with` compares whole components, so `profiles2` is not inside `profiles`.
    if target != root && target.starts_with(&root) {
        Ok(target)
    } else {
        Err("That profile folder is outside the QuotaGlass profiles.".to_string())
    }
}

/// `C:\x` for `\\?\C:\x` and `\\srv\share` for `\\?\UNC\srv\share`. `canonicalize` gives the long form; the CLIs get
/// the short form in their environment.
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

/// The arguments after the tool. Fixed per service and action.
fn vendor_args(service: Service, action: CliAction) -> &'static [&'static str] {
    match (service, action) {
        (Service::Claude, CliAction::Login) => &["auth", "login"],
        (Service::Claude, CliAction::Logout) => &["auth", "logout"],
        (Service::Codex, CliAction::Login) => &["login"],
        (Service::Codex, CliAction::Logout) => &["logout"],
        (_, CliAction::Open) | (Service::Antigravity, _) => &[],
    }
}

/// The variable that points a CLI at a profile folder.
fn profile_var(service: Service) -> Option<&'static str> {
    match service {
        Service::Claude => Some("CLAUDE_CONFIG_DIR"),
        Service::Codex => Some("CODEX_HOME"),
        Service::Antigravity => None,
    }
}

/// Arguments and environment for one action.
/// `Login` and `Open`: the `cmd.exe` arguments for a new titled console that runs `tool`.
/// `Logout`: the arguments for `tool` itself.
/// The profile goes in the environment only, never in the arguments. With no profile the CLI keeps the folder it
/// inherits, which is the folder QuotaGlass reads for the current login.
pub fn cli_command(
    service: Service,
    tool: &Path,
    action: CliAction,
    profile: Option<&Path>,
) -> (Vec<OsString>, Vec<(&'static str, PathBuf)>) {
    let mut args: Vec<OsString> = Vec::new();
    if action != CliAction::Logout {
        // `start` runs the absolute cmd.exe, never a bare `cmd` that a search could resolve to another file.
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

/// Characters that `cmd.exe` reads as syntax. `start` and `/k` parse the tool path again, so a path with one of them
/// could run a second command.
const CMD_META: [char; 10] = ['&', '|', '<', '>', '^', '%', '!', '"', '(', ')'];
const UNSAFE_TOOL: &str = "The tool path contains characters cmd.exe cannot run safely.";

/// `<SystemRoot>\System32`, or `C:\Windows\System32` when SystemRoot is not set, not absolute or not plain.
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

/// The absolute `cmd.exe`: a bare name would let Windows search the working directory first.
fn cmd_exe() -> PathBuf {
    system32().join("cmd.exe")
}

/// `cmd.exe` with System32 as its working directory: no folder a user can write to is searched for a program.
fn cmd_launcher() -> Command {
    let mut cmd = Command::new(cmd_exe());
    cmd.current_dir(system32());
    cmd
}

/// No console window for a helper process.
fn hide_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

fn taskkill_args(pid: u32) -> Vec<OsString> {
    ["/PID", &pid.to_string(), "/T", "/F"]
        .map(OsString::from)
        .to_vec()
}

/// Ends a process and every process it started (a `.cmd` tool runs node under cmd.exe). The result is ignored: the
/// caller kills the direct child afterwards anyway.
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

/// Only an absolute tool for Claude or Codex: a bare name would let `cmd.exe` search the working directory. A path
/// with a `cmd.exe` metacharacter is refused.
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

/// Opens `Login` or `Open` in a visible console. The environment is set on `cmd.exe`, so the console and the CLI in
/// it inherit it.
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
    // The console window is the point: the user signs in there. No CREATE_NO_WINDOW.
    cmd_launcher()
        .args(args)
        .envs(env)
        .spawn()
        .map(drop)
        .map_err(|_| "Could not open the sign-in window.")
}

/// The vendor's own sign-in for the current login (the `connect` command).
pub fn open(service: Service, tool: Option<&Path>) -> Result<(), &'static str> {
    open_console(service, tool, CliAction::Login, None)
}

/// Runs the vendor's sign-out for one profile: hidden, no input, output discarded, killed after 30 s. Blocks; call it
/// from a blocking task.
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
                // `kill` ends only the direct child; the CLI it started would keep the profile folder open.
                kill_tree(child.id());
                let _ = child.kill();
                let _ = child.wait();
                return Err("The sign-out timed out.");
            }
        }
    }
}

/// A page QuotaGlass may open in the default browser. The UI names the page; it never sends a URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UrlTarget {
    OpenAiUsage,
    Orca,
}

impl UrlTarget {
    pub fn url(self) -> &'static str {
        match self {
            // verify before release: the page the Codex app links to for usage and banked resets.
            UrlTarget::OpenAiUsage => "https://chatgpt.com/codex/settings/usage",
            // verify before release: the repo names no Orca homepage, so this is the project's GitHub page.
            UrlTarget::Orca => "https://github.com/stablyai/orca",
        }
    }
}

/// Opens a fixed page in the default browser: `<System32>\cmd.exe /c start "" <url>`, the URL one fixed argument with
/// no character that `cmd.exe` reads (see `url_targets_are_https_allowlisted`).
pub fn open_url(target: UrlTarget) -> Result<(), &'static str> {
    let mut cmd = cmd_launcher();
    cmd.args(["/c", "start", "", target.url()]);
    hide_window(&mut cmd);
    cmd.spawn()
        .map(drop)
        .map_err(|_| "Could not open the browser.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("qg-connect-{name}-{}", std::process::id()));
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

        assert!(guard_inside(&root, &root).is_err(), "the root itself");
        assert!(
            guard_inside(&root, &root.join(".")).is_err(),
            "the root itself, spelled with ."
        );
        assert!(
            guard_inside(&root, &child.join("..")).is_err(),
            "a .. back to the root"
        );
        assert!(
            guard_inside(&root, &child.join("..").join("..").join("profiles2")).is_err(),
            "a .. escape to a sibling"
        );
        assert!(
            guard_inside(&root, &sibling).is_err(),
            "a sibling with the root as a name prefix"
        );
        assert!(guard_inside(&root, &base).is_err(), "the parent");
        assert!(
            guard_inside(&root, &root.join("missing")).is_err(),
            "a folder that does not exist"
        );
        assert!(
            guard_inside(&root, Path::new("abc")).is_err(),
            "a relative path"
        );

        // A link inside the root that points out of it. Creating one can need a privilege; skip when refused.
        #[cfg(windows)]
        {
            let link = root.join("link-out");
            if std::os::windows::fs::symlink_dir(&sibling, &link).is_ok() {
                assert!(guard_inside(&root, &link).is_err(), "a symlink out");
            }
            let junction = root.join("junction-out");
            let made = Command::new("cmd.exe")
                .args([OsStr::new("/c"), OsStr::new("mklink"), OsStr::new("/J")])
                .arg(&junction)
                .arg(&sibling)
                .output()
                .is_ok_and(|o| o.status.success());
            if made {
                assert!(guard_inside(&root, &junction).is_err(), "a junction out");
                std::fs::remove_dir(&junction).unwrap();
                assert!(sibling.is_dir(), "removing the junction leaves its target");
            }
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn cli_command_sets_profile_env() {
        let tool = Path::new(r"C:\tools\claude.cmd");
        let profile = Path::new(r"C:\Users\u\AppData\Roaming\QuotaGlass\profiles\abc");
        for action in [CliAction::Login, CliAction::Open, CliAction::Logout] {
            let (_, env) = cli_command(Service::Claude, tool, action, Some(profile));
            assert_eq!(env, [("CLAUDE_CONFIG_DIR", profile.to_path_buf())]);
            let (_, env) = cli_command(Service::Codex, tool, action, Some(profile));
            assert_eq!(env, [("CODEX_HOME", profile.to_path_buf())]);
            let (_, env) = cli_command(Service::Claude, tool, action, None);
            assert!(
                env.is_empty(),
                "the current login keeps the inherited folder"
            );
            let (_, env) = cli_command(Service::Antigravity, tool, action, Some(profile));
            assert!(env.is_empty());
        }
    }

    #[test]
    fn cli_command_args_are_fixed() {
        let tool = Path::new(r"C:\Program Files\nodejs\claude.cmd");
        let console = |rest: &[&str]| {
            let mut v = os(&["/c", "start", "QuotaGlass sign-in"]);
            v.push(cmd_exe().into_os_string());
            v.push("/k".into());
            v.push(tool.as_os_str().to_owned());
            v.extend(os(rest));
            v
        };
        let profile = Some(Path::new(r"C:\p\abc"));
        let cases: [(Service, CliAction, Vec<OsString>); 6] = [
            (
                Service::Claude,
                CliAction::Login,
                console(&["auth", "login"]),
            ),
            (Service::Claude, CliAction::Open, console(&[])),
            (Service::Claude, CliAction::Logout, os(&["auth", "logout"])),
            (Service::Codex, CliAction::Login, console(&["login"])),
            (Service::Codex, CliAction::Open, console(&[])),
            (Service::Codex, CliAction::Logout, os(&["logout"])),
        ];
        for (service, action, want) in cases {
            for p in [None, profile] {
                let (args, _) = cli_command(service, tool, action, p);
                assert_eq!(args, want, "{service:?} {action:?}");
                assert!(
                    !args.iter().any(|a| a == OsStr::new(r"C:\p\abc")),
                    "the profile goes in the environment, never in the arguments"
                );
            }
        }
    }

    #[test]
    fn console_runs_absolute_cmd() {
        let tool = Path::new(r"C:\tools\claude.cmd");
        let (args, _) = cli_command(Service::Claude, tool, CliAction::Open, None);
        let cmd = cmd_exe();
        assert!(cmd.is_absolute(), "{}", cmd.display());
        assert!(cmd.ends_with(r"System32\cmd.exe"));
        assert_eq!(args[3], cmd.as_os_str(), "start runs the absolute cmd.exe");
        assert!(
            !args.iter().any(|a| a == OsStr::new("cmd")),
            "never a bare cmd"
        );
    }

    #[test]
    fn system32_falls_back_to_windows() {
        assert_eq!(
            system32_from(Some(r"D:\Win".into())),
            PathBuf::from(r"D:\Win\System32")
        );
        for bad in [None, Some("Windows".into()), Some(r"C:\a&b".into())] {
            assert_eq!(system32_from(bad), PathBuf::from(r"C:\Windows\System32"));
        }
    }

    #[test]
    fn taskkill_kills_the_tree() {
        assert_eq!(taskkill_args(42), os(&["/PID", "42", "/T", "/F"]));
    }

    #[test]
    fn tool_with_cmd_metacharacters_is_refused() {
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
                Err(UNSAFE_TOOL),
                "{bad}"
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
        let ok = Path::new(r"C:\Program Files\nodejs\claude.cmd");
        assert_eq!(checked_tool(Service::Claude, Some(ok)), Ok(ok));
    }

    #[test]
    fn new_profile_id_is_hex_32_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..2000 {
            let id = new_profile_id();
            assert_eq!(id.len(), 32);
            assert!(id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
            assert!(seen.insert(id), "an ID repeated");
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
    fn profiles_root_is_under_appdata() {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            assert_eq!(
                profiles_root(),
                Some(PathBuf::from(appdata).join("QuotaGlass").join("profiles"))
            );
        }
    }

    #[test]
    fn url_targets_are_https_allowlisted() {
        for t in [UrlTarget::OpenAiUsage, UrlTarget::Orca] {
            let url = t.url();
            assert!(url.starts_with("https://"), "{url}");
            assert!(
                !url.chars()
                    .any(|c| c.is_whitespace() || "&|<>^%\"()!`'".contains(c)),
                "{url} would be read by cmd.exe"
            );
        }
        assert_eq!(
            UrlTarget::OpenAiUsage.url(),
            "https://chatgpt.com/codex/settings/usage"
        );
        let t: UrlTarget = serde_json::from_str(r#""openAiUsage""#).unwrap();
        assert_eq!(t, UrlTarget::OpenAiUsage);
        assert!(serde_json::from_str::<UrlTarget>(r#""https://evil.example""#).is_err());
    }

    #[test]
    fn launch_refuses_missing_or_relative_tool() {
        let claude = Some(Path::new(r"C:\x\claude.cmd"));
        assert!(open(Service::Claude, None).is_err());
        assert!(
            open(Service::Claude, Some(Path::new("claude.cmd"))).is_err(),
            "a bare name would let cmd.exe search the working directory"
        );
        assert!(open_console(
            Service::Antigravity,
            Some(Path::new(r"C:\x\agy.exe")),
            CliAction::Open,
            None
        )
        .is_err());
        assert!(
            open_console(Service::Claude, claude, CliAction::Logout, None).is_err(),
            "sign-out never opens a console"
        );
        assert!(
            open_console(
                Service::Claude,
                claude,
                CliAction::Open,
                Some(Path::new("rel"))
            )
            .is_err(),
            "a relative profile"
        );
        assert!(logout(Service::Claude, None, Path::new(r"C:\p\abc")).is_err());
        assert!(logout(Service::Claude, claude, Path::new("rel")).is_err());
        assert!(logout(
            Service::Antigravity,
            Some(Path::new(r"C:\x\agy.exe")),
            Path::new(r"C:\p\abc")
        )
        .is_err());
    }

    #[test]
    fn plain_strips_verbatim_disk_prefix() {
        assert_eq!(
            plain(Path::new(r"\\?\C:\Users\u\p")),
            PathBuf::from(r"C:\Users\u\p")
        );
        assert_eq!(
            plain(Path::new(r"\\?\UNC\srv\share\p")),
            PathBuf::from(r"\\srv\share\p")
        );
        assert_eq!(plain(Path::new(r"C:\x")), PathBuf::from(r"C:\x"));
    }
}
