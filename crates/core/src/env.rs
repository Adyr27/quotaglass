use crate::model::FetchError;
use crate::net::HttpClient;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn system_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Paths and services every source reads through. Built once at startup.
pub struct Env {
    pub home: PathBuf,
    /// The folder of the current Claude Code login: `CLAUDE_CONFIG_DIR`, else `<home>/.claude`. Added accounts
    /// use their own profile folders, so sources take their folder from their own settings, not from here.
    pub default_claude_dir: PathBuf,
    /// The folder of the current Codex login: `CODEX_HOME`, else `<home>/.codex`.
    pub default_codex_dir: PathBuf,
    pub path_dirs: Vec<PathBuf>,
    pub http: HttpClient,
    pub now_ms: fn() -> i64,
}

/// The path-related part of `Env`, split out so it can be tested without touching process env vars.
#[derive(Debug, PartialEq)]
pub struct Paths {
    pub home: PathBuf,
    pub default_claude_dir: PathBuf,
    pub default_codex_dir: PathBuf,
    pub path_dirs: Vec<PathBuf>,
}

impl Paths {
    pub fn from_vars(get: impl Fn(&str) -> Option<OsString>) -> Result<Self, FetchError> {
        let home = get("USERPROFILE")
            .or_else(|| get("HOME"))
            .map(PathBuf::from)
            .ok_or(FetchError::NotDetected)?;
        let claude_dir = get("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".claude"));
        let codex_dir = get("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".codex"));
        let path_dirs = get("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default();
        Ok(Self {
            home,
            default_claude_dir: claude_dir,
            default_codex_dir: codex_dir,
            path_dirs,
        })
    }
}

impl Env {
    pub fn from_system() -> Result<Self, FetchError> {
        let p = Paths::from_vars(|k| std::env::var_os(k))?;
        Ok(Self {
            home: p.home,
            default_claude_dir: p.default_claude_dir,
            default_codex_dir: p.default_codex_dir,
            path_dirs: p.path_dirs,
            http: HttpClient::new()?,
            now_ms: system_now_ms,
        })
    }

    /// First match of any of `names` (e.g. ["orca.exe"]) in PATH order.
    pub fn find_on_path(&self, names: &[&str]) -> Option<PathBuf> {
        find_in(&self.path_dirs, names)
    }
}

/// A relative PATH entry (`.`, `bin`) is skipped: it names whatever folder the process happens to run in.
pub fn find_in(dirs: &[PathBuf], names: &[&str]) -> Option<PathBuf> {
    dirs.iter()
        .filter(|d| d.is_absolute())
        .flat_map(|d| names.iter().map(move |n| d.join(n)))
        .find(|p| is_file(p))
}

fn is_file(p: &Path) -> bool {
    p.metadata().map(|m| m.is_file()).unwrap_or(false)
}

#[cfg(test)]
pub(crate) fn test_env(home: &Path, http: HttpClient, now_ms: fn() -> i64) -> Env {
    Env {
        home: home.to_path_buf(),
        default_claude_dir: home.join(".claude"),
        default_codex_dir: home.join(".codex"),
        path_dirs: vec![],
        http,
        now_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_honours_overrides() {
        let p = Paths::from_vars(|k| match k {
            "USERPROFILE" => Some("C:/u".into()),
            "CODEX_HOME" => Some("D:/codex".into()),
            "CLAUDE_CONFIG_DIR" => Some("D:/claude".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(p.default_codex_dir, PathBuf::from("D:/codex"));
        assert_eq!(p.default_claude_dir, PathBuf::from("D:/claude"));
    }

    #[test]
    fn find_in_skips_relative_entries() {
        // Tests run in the crate folder, so `src\lib.rs` exists relative to it.
        let rel = PathBuf::from("src");
        assert!(rel.join("lib.rs").is_file());
        assert_eq!(find_in(std::slice::from_ref(&rel), &["lib.rs"]), None);
        let abs = std::env::current_dir().unwrap().join("src");
        assert_eq!(
            find_in(&[rel, abs.clone()], &["lib.rs"]),
            Some(abs.join("lib.rs"))
        );
    }

    #[test]
    fn env_defaults_under_home() {
        let p = Paths::from_vars(|k| (k == "USERPROFILE").then(|| "C:/u".into())).unwrap();
        assert_eq!(p.default_codex_dir, PathBuf::from("C:/u").join(".codex"));
        assert_eq!(p.default_claude_dir, PathBuf::from("C:/u").join(".claude"));
    }
}
