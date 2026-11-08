//! Orca's own account list: `orca account list --json`.
//!
//! Orca is an optional third-party tool. QuotaGlass uses it for Antigravity only: Claude and Codex always come from
//! their direct sources. The command needs the Orca app to be running and its output shape is undocumented, so every
//! field that is read is checked and an unexpected shape is a `Format` error. This source runs a program and reads
//! its stdout; it never touches a token.

use crate::env::Env;
use crate::model::{checked_percent, AccountKey, Bucket, FetchError, Reading, Service, SourceKind};
use crate::provider::{BoxFut, Provider};
use serde_json::Value;
use std::ffi::OsString;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

const ARGS: [&str; 3] = ["account", "list", "--json"];
const MAX_STDOUT: usize = 4 * 1024 * 1024;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The `orca.exe` found on PATH.
pub struct Orca {
    pub timeout: Duration,
    /// Replaces `orca.exe` in tests.
    #[cfg(test)]
    program: Option<(OsString, Vec<OsString>)>,
}

impl Orca {
    pub fn new() -> Self {
        Self {
            timeout: Duration::from_secs(20),
            #[cfg(test)]
            program: None,
        }
    }

    fn command(&self, env: &Env) -> Option<(OsString, Vec<OsString>)> {
        #[cfg(test)]
        if let Some((program, args)) = &self.program {
            return Some((program.clone(), args.clone()));
        }
        let exe = env.find_on_path(&["orca.exe"])?;
        Some((
            exe.into_os_string(),
            ARGS.iter().map(OsString::from).collect(),
        ))
    }
}

impl Default for Orca {
    fn default() -> Self {
        Self::new()
    }
}

impl Provider for Orca {
    fn kind(&self) -> SourceKind {
        SourceKind::Orca
    }

    fn service(&self) -> Option<Service> {
        Some(Service::Antigravity)
    }

    fn detect(&self, env: &Env) -> bool {
        env.find_on_path(&["orca.exe"]).is_some()
    }

    fn fetch<'a>(&'a self, env: &'a Env) -> BoxFut<'a, Result<Vec<Reading>, FetchError>> {
        Box::pin(async move {
            let (program, args) = self.command(env).ok_or(FetchError::NotDetected)?;
            let mut cmd = Command::new(program);
            cmd.args(args)
                .kill_on_drop(true)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            #[cfg(windows)]
            cmd.creation_flags(CREATE_NO_WINDOW);
            let mut child = cmd
                .spawn()
                .map_err(|_| FetchError::Unavailable("Orca could not be started"))?;
            // Dropping `child` on an error or a timeout ends the process through `kill_on_drop`.
            let (status, out) = tokio::time::timeout(self.timeout, read_capped(&mut child))
                .await
                .map_err(|_| FetchError::Timeout)?
                .map_err(|_| FetchError::Unavailable("Orca is not running"))?;
            let status = status.ok_or(FetchError::Format("orca"))?;
            if !status.success() {
                return Err(FetchError::Unavailable("Orca is not running"));
            }
            parse_list(&out, (env.now_ms)())
        })
    }
}

/// Reads stdout to the end, but at most one byte past `MAX_STDOUT`, so a runaway output is never held in full.
/// Over the cap, the exit status is `None` and the process is not waited for.
async fn read_capped(
    child: &mut tokio::process::Child,
) -> std::io::Result<(Option<std::process::ExitStatus>, Vec<u8>)> {
    let mut out = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        stdout
            .take(MAX_STDOUT as u64 + 1)
            .read_to_end(&mut out)
            .await?;
    }
    if out.len() > MAX_STDOUT {
        return Ok((None, out));
    }
    Ok((Some(child.wait().await?), out))
}

/// Turns the stdout of `orca account list --json` into Antigravity readings: every entry of `antigravityAccounts`;
/// an account without limits becomes a reading with no buckets. `rateLimits.antigravity` repeats the active
/// Antigravity account and is skipped. Claude, Codex and other providers are ignored, even when their data is bad.
/// `ok` not true, or a missing `result` or `rateLimits`, is a `Format` error.
pub fn parse_list(stdout: &[u8], now_ms: i64) -> Result<Vec<Reading>, FetchError> {
    let root: Value = serde_json::from_slice(stdout).map_err(|_| FetchError::Format("orca"))?;
    if root.get("ok") != Some(&Value::Bool(true)) {
        return Err(FetchError::Format("orca"));
    }
    let limits = root
        .get("result")
        .filter(|v| v.is_object())
        .ok_or(FetchError::Format("orca"))?
        .get("rateLimits")
        .filter(|v| v.is_object())
        .ok_or(FetchError::Format("orca"))?;
    let mut out = Vec::new();
    for account in limits
        .get("antigravityAccounts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        out.extend(antigravity_reading(account, now_ms)?);
    }
    Ok(out)
}

fn antigravity_reading(account: &Value, now_ms: i64) -> Result<Option<Reading>, FetchError> {
    if !account.is_object() {
        return Ok(None);
    }
    let email = account
        .get("email")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|e| !e.is_empty());
    let local_id = account.get("accountId").and_then(Value::as_str);
    let limits = match account.get("rateLimits") {
        None | Some(Value::Null) => {
            return Ok(reading(email, local_id, Vec::new(), account, now_ms));
        }
        Some(l) if l.is_object() => l,
        Some(_) => return Err(FetchError::Format("orca rateLimits")),
    };
    let mut buckets = Vec::new();
    match limits
        .get("buckets")
        .and_then(Value::as_array)
        .filter(|b| !b.is_empty())
    {
        Some(items) => {
            for item in items {
                let group = item
                    .get("group")
                    .and_then(Value::as_str)
                    .or_else(|| item.get("name").and_then(Value::as_str))
                    .ok_or(FetchError::Format("orca bucket"))?;
                let name = match item.get("windowName").and_then(Value::as_str) {
                    Some(window) if window != group => format!("{group} · {window}"),
                    _ => group.to_string(),
                };
                let (used_percent, window_secs, resets_at_ms) = read_window(item)?;
                buckets.push(Bucket {
                    name,
                    used_percent,
                    window_secs,
                    resets_at_ms,
                });
            }
        }
        None => {
            for (slot, fallback_name) in [("session", "Session"), ("weekly", "Weekly")] {
                buckets.extend(window_bucket(limits.get(slot), fallback_name)?);
            }
        }
    }
    Ok(reading(email, local_id, buckets, limits, now_ms))
}

/// `observed_at` is `updatedAt` on `stamped`. Without a numeric `updatedAt`, a reading with buckets is dropped (its
/// age is unknown, so its numbers could be shown as fresh when they are not); a reading without buckets shows no
/// number and uses `now_ms`.
fn reading(
    email: Option<&str>,
    local_id: Option<&str>,
    buckets: Vec<Bucket>,
    stamped: &Value,
    now_ms: i64,
) -> Option<Reading> {
    let observed_at_ms = match stamped.get("updatedAt").and_then(Value::as_i64) {
        Some(at) => at,
        None if buckets.is_empty() => now_ms,
        None => return None,
    };
    let service = Service::Antigravity;
    let local_id = local_id
        .map(str::to_string)
        .unwrap_or_else(|| format!("orca-{}", service.as_str()));
    Some(Reading {
        key: AccountKey::new(service, email, local_id),
        label: email
            .map(str::to_string)
            .unwrap_or_else(|| "Antigravity (Orca)".to_string()),
        plan: None,
        buckets,
        source: SourceKind::Orca,
        observed_at_ms,
        banked_resets: None,
    })
}

/// A `session` or `weekly` object. Null or missing means the window does not exist.
fn window_bucket(
    window: Option<&Value>,
    fallback_name: &str,
) -> Result<Option<Bucket>, FetchError> {
    let window = match window {
        None | Some(Value::Null) => return Ok(None),
        Some(w) if w.is_object() => w,
        Some(_) => return Err(FetchError::Format("orca window")),
    };
    let (used_percent, window_secs, resets_at_ms) = read_window(window)?;
    let name = match window_secs {
        Some(secs) => window_name(secs / 60),
        None => fallback_name.to_string(),
    };
    Ok(Some(Bucket {
        name,
        used_percent,
        window_secs,
        resets_at_ms,
    }))
}

/// `usedPercent` (required), `windowMinutes` as seconds, `resetsAt` in milliseconds.
fn read_window(window: &Value) -> Result<(f64, Option<u64>, Option<i64>), FetchError> {
    let used_percent = checked_percent(
        window.get("usedPercent").and_then(Value::as_f64),
        "orca usedPercent",
    )?;
    let window_secs = match window.get("windowMinutes") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_u64()
                .and_then(|m| m.checked_mul(60))
                .ok_or(FetchError::Format("orca windowMinutes"))?,
        ),
    };
    Ok((
        used_percent,
        window_secs,
        window.get("resetsAt").and_then(Value::as_i64),
    ))
}

fn window_name(minutes: u64) -> String {
    match minutes {
        300 => "5 hour".into(),
        10_080 => "Weekly".into(),
        43_200 => "30 days".into(),
        m if m >= 1_440 && m % 1_440 == 0 => format!("{} days", m / 1_440),
        m if m >= 60 && m % 60 == 0 => format!("{} h", m / 60),
        m => format!("{m} min"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::test_env;
    use crate::net::HttpClient;
    use std::time::Instant;

    const LIST: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/orca_list.json"
    ));
    const UNAVAILABLE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/orca_unavailable.json"
    ));
    const NOT_OK: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/orca_not_ok.json"
    ));
    const NOW: i64 = 1_794_070_000_000;

    fn now() -> i64 {
        NOW
    }

    fn by_label<'a>(readings: &'a [Reading], label: &str) -> &'a Reading {
        readings
            .iter()
            .find(|r| r.label == label)
            .unwrap_or_else(|| panic!("no reading labelled {label}"))
    }

    #[test]
    fn orca_ignores_claude_and_codex() {
        let readings = parse_list(LIST.as_bytes(), NOW).unwrap();
        assert_eq!(readings.len(), 2);
        assert!(readings
            .iter()
            .all(|r| r.key.service == Service::Antigravity));
        // A broken Claude entry no longer fails the whole list.
        let body =
            br#"{"ok":true,"result":{"rateLimits":{"claude":{"session":{"usedPercent":"x"}}}}}"#;
        assert_eq!(parse_list(body, NOW), Ok(Vec::new()));
    }

    #[test]
    fn orca_service_is_antigravity() {
        assert_eq!(Orca::new().service(), Some(Service::Antigravity));
    }

    #[test]
    fn orca_skips_unavailable() {
        let readings = parse_list(UNAVAILABLE.as_bytes(), NOW).unwrap();
        assert!(readings.is_empty());
    }

    #[test]
    fn orca_null_ratelimits_gives_empty_reading() {
        let readings = parse_list(LIST.as_bytes(), NOW).unwrap();
        let empty = by_label(&readings, "c@example.com");
        assert!(empty.buckets.is_empty());
        assert_eq!(
            empty.key,
            AccountKey::new(Service::Antigravity, Some("c@example.com"), "acc-3")
        );
        assert_eq!(empty.source, SourceKind::Orca);
        assert_eq!(empty.observed_at_ms, NOW);
    }

    #[test]
    fn orca_antigravity_bucket_names() {
        let readings = parse_list(LIST.as_bytes(), NOW).unwrap();
        let acc = by_label(&readings, "a@example.com");
        let names: Vec<&str> = acc.buckets.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Gemini Models · Weekly Limit Remaining",
                "Claude and GPT models",
                "Flash"
            ]
        );
        assert_eq!(acc.buckets[0].used_percent, 10.0);
        assert_eq!(acc.buckets[0].window_secs, Some(604_800));
        assert_eq!(acc.buckets[0].resets_at_ms, Some(1_794_651_395_000));
        assert_eq!(acc.buckets[2].window_secs, None);
        assert_eq!(acc.observed_at_ms, 1_794_066_861_198);
        assert_eq!(
            readings
                .iter()
                .filter(|r| r.key.service == Service::Antigravity)
                .count(),
            2
        );
    }

    #[test]
    fn orca_drops_numbers_without_updated_at() {
        let body = br#"{"ok":true,"result":{"rateLimits":{"antigravityAccounts":[{"accountId":"a","rateLimits":{"weekly":{"usedPercent":35,"windowMinutes":10080},"status":"ok"}}]}}}"#;
        assert!(parse_list(body, NOW).unwrap().is_empty());
    }

    #[test]
    fn orca_not_ok_is_format_error() {
        assert_eq!(
            parse_list(NOT_OK.as_bytes(), NOW),
            Err(FetchError::Format("orca"))
        );
        assert_eq!(
            parse_list(b"{\"ok\":true}", NOW),
            Err(FetchError::Format("orca"))
        );
        assert_eq!(
            parse_list(b"not json", NOW),
            Err(FetchError::Format("orca"))
        );
    }

    #[test]
    fn orca_bad_percent_in_antigravity_account_is_format_error() {
        let body = br#"{"ok":true,"result":{"rateLimits":{"antigravityAccounts":[{"rateLimits":{"session":{"usedPercent":"x"}}}]}}}"#;
        assert_eq!(
            parse_list(body, NOW),
            Err(FetchError::Format("orca usedPercent"))
        );
    }

    #[test]
    fn window_names() {
        assert_eq!(window_name(300), "5 hour");
        assert_eq!(window_name(10_080), "Weekly");
        assert_eq!(window_name(43_200), "30 days");
        assert_eq!(window_name(2_880), "2 days");
        assert_eq!(window_name(120), "2 h");
        assert_eq!(window_name(45), "45 min");
    }

    #[tokio::test]
    async fn orca_timeout() {
        let orca = Orca {
            timeout: Duration::from_millis(300),
            program: Some((
                "ping".into(),
                ["-n", "6", "127.0.0.1"].map(OsString::from).to_vec(),
            )),
        };
        let env = test_env(
            &std::env::temp_dir(),
            HttpClient::for_test("http://127.0.0.1:9"),
            now,
        );
        let started = Instant::now();
        let result = orca.fetch(&env).await;
        assert_eq!(result, Err(FetchError::Timeout));
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
