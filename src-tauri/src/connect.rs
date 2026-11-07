//! "Connect" opens the vendor's own sign-in in a visible console window. QuotaGlass never handles the login.

use quotaglass_core::Service;
use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

/// The sign-in arguments that follow the tool for each service. Nothing from the UI is added to them.
pub fn login_args(service: Service) -> Option<&'static [&'static str]> {
    match service {
        Service::Claude => Some(&["auth", "login"]),
        Service::Codex => Some(&["login"]),
        Service::Antigravity => None,
    }
}

/// The complete `cmd.exe` argument list: a new titled console that runs `tool` with the login arguments.
/// `tool` is the absolute path found on PATH, so cmd.exe never searches the working directory for it.
pub fn command_line(service: Service, tool: &Path) -> Option<Vec<&OsStr>> {
    let login = login_args(service)?;
    let mut args: Vec<&OsStr> = ["/c", "start", "QuotaGlass sign-in", "cmd", "/k"]
        .iter()
        .map(OsStr::new)
        .collect();
    args.push(tool.as_os_str());
    args.extend(login.iter().map(OsStr::new));
    Some(args)
}

/// Opens the sign-in window for `service`. `tool` is its sign-in program as found on PATH; `None` means it is
/// not installed.
pub fn open(service: Service, tool: Option<&Path>) -> Result<(), &'static str> {
    let args = tool
        .filter(|t| t.is_absolute())
        .and_then(|t| command_line(service, t))
        .ok_or("No sign-in tool for that service.")?;
    // The console window is the point: the user signs in there. No CREATE_NO_WINDOW.
    Command::new("cmd.exe")
        .args(args)
        .spawn()
        .map(drop)
        .map_err(|_| "Could not open the sign-in window.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_args_fixed() {
        let tool = Path::new(r"C:\tools\claude.cmd");
        assert_eq!(
            command_line(Service::Claude, tool).unwrap(),
            [
                "/c",
                "start",
                "QuotaGlass sign-in",
                "cmd",
                "/k",
                r"C:\tools\claude.cmd",
                "auth",
                "login"
            ]
            .map(OsStr::new)
        );
        let tool = Path::new(r"C:\tools\codex.cmd");
        assert_eq!(
            command_line(Service::Codex, tool).unwrap(),
            [
                "/c",
                "start",
                "QuotaGlass sign-in",
                "cmd",
                "/k",
                r"C:\tools\codex.cmd",
                "login"
            ]
            .map(OsStr::new)
        );
        assert_eq!(command_line(Service::Antigravity, tool), None);
    }

    #[test]
    fn open_refuses_missing_or_relative_tool() {
        assert!(open(Service::Claude, None).is_err());
        assert!(
            open(Service::Claude, Some(Path::new("claude.cmd"))).is_err(),
            "a bare name would let cmd.exe search the working directory"
        );
        assert!(open(Service::Antigravity, Some(Path::new(r"C:\x\agy.exe"))).is_err());
    }
}
