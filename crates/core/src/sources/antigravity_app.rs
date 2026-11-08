//! The running Antigravity app: quota from its language server on a loopback port.
//!
//! Method based on the open-source `ai-usagebar` 1.41.0 (`antigravity/fetch.rs`): find the app's language-server
//! processes with one hidden PowerShell CIM query, find their loopback listening ports with `netstat`, read the CSRF
//! token from the server's own page (`GET /`), then `POST` `GetUserStatus` and `RetrieveUserQuotaSummary`. Field
//! names verified against the live Antigravity desktop app on 2026-11-08 (Windows 11). This source only contacts
//! `127.0.0.1`. It never reads a Google login, a token store or Windows Credential Manager, and it never calls a
//! Google endpoint. Response bodies and the CSRF token are never logged.

use crate::env::Env;
use crate::model::{
    checked_percent, AccountKey, Bucket, FetchError, NetErrorKind, Reading, Service, SourceKind,
};
use crate::net::HttpClient;
use crate::provider::{BoxFut, Provider};
use serde_json::Value;
use std::collections::VecDeque;
use std::future::Future;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// Language-server images, compared without case. The app window (`Antigravity.exe`, also used by `agy` sidecars)
/// is not one of them.
const IMAGES: [&str; 2] = ["language_server.exe", "language_server_windows_x64.exe"];
/// Folder that must appear in the executable path (compared without case), so a Windsurf or Codeium language
/// server is never read as Antigravity.
const APP_FOLDER: &str = r"\antigravity\";
/// PIDs and executable paths of language-server processes, as CSV.
const PROCESS_QUERY: &str =
    "Get-CimInstance Win32_Process -Filter \"Name LIKE 'language_server%'\" \
     | Select-Object ProcessId,ExecutablePath | ConvertTo-Csv -NoTypeInformation";
const STATUS_PATH: &str = "/exa.language_server_pb.LanguageServerService/GetUserStatus";
const QUOTA_PATH: &str = "/exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary";
const TOOL_TIMEOUT: Duration = Duration::from_secs(5);
/// Upper bound on one whole `fetch`: process discovery plus every port.
const FETCH_BUDGET: Duration = Duration::from_secs(15);
const MAX_STDOUT: usize = 4 * 1024 * 1024;
/// Upper bound on JSON nodes visited by the best-effort user-status search.
const SEARCH_BUDGET: usize = 10_000;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub struct AntigravityApp;

impl Provider for AntigravityApp {
    fn kind(&self) -> SourceKind {
        SourceKind::AntigravityApp
    }

    fn service(&self) -> Option<Service> {
        Some(Service::Antigravity)
    }

    /// Always true: finding the process spawns PowerShell (up to 5 s), so it runs in `fetch`, off the caller's thread.
    fn detect(&self, _env: &Env) -> bool {
        true
    }

    fn fetch<'a>(&'a self, env: &'a Env) -> BoxFut<'a, Result<Vec<Reading>, FetchError>> {
        Box::pin(within(FETCH_BUDGET, async move {
            let ports = tokio::task::spawn_blocking(|| -> Result<Vec<u16>, FetchError> {
                let pids = app_pids()?;
                if pids.is_empty() {
                    return Err(FetchError::NotDetected);
                }
                let netstat = run_hidden(system_tool("netstat.exe"), &["-ano", "-p", "tcp"])?;
                Ok(parse_netstat(&netstat, &pids))
            })
            .await
            .map_err(|_| FetchError::Unavailable("process list could not be read"))??;
            read_ports(&env.http, &ports, (env.now_ms)()).await
        }))
    }
}

/// `fut`, or `Timeout` once `budget` has passed.
async fn within<T>(
    budget: Duration,
    fut: impl Future<Output = Result<T, FetchError>>,
) -> Result<T, FetchError> {
    tokio::time::timeout(budget, fut)
        .await
        .unwrap_or(Err(FetchError::Timeout))
}

fn app_pids() -> Result<Vec<u32>, FetchError> {
    let csv = run_hidden(
        system_tool(r"WindowsPowerShell\v1.0\powershell.exe"),
        &["-NoProfile", "-NonInteractive", "-Command", PROCESS_QUERY],
    )?;
    Ok(parse_process_csv(&csv))
}

/// `%SystemRoot%\System32\<name>`, so a program of the same name on PATH is never run instead.
fn system_tool(name: &str) -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    PathBuf::from(root).join("System32").join(name)
}

/// Runs a program without a console window and returns its stdout. Over 5 s it is killed; over 4 MiB of output
/// is an error.
fn run_hidden(program: PathBuf, args: &[&str]) -> Result<String, FetchError> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd
        .spawn()
        .map_err(|_| FetchError::Unavailable("process list could not be read"))?;
    let stdout = child.stdout.take().ok_or(FetchError::Format("stdout"))?;
    // A separate reader keeps the pipe drained, so a large output cannot block the child.
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.take(MAX_STDOUT as u64 + 1).read_to_end(&mut out);
        out
    });
    let deadline = Instant::now() + TOOL_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let out = reader.join().map_err(|_| FetchError::Format("stdout"))?;
                if !status.success() || out.len() > MAX_STDOUT {
                    return Err(FetchError::Unavailable("process list could not be read"));
                }
                return Ok(String::from_utf8_lossy(&out).into_owned());
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(FetchError::Timeout);
            }
        }
    }
}

/// Tries each port in order; the first that returns buckets wins. A port that answers with a quota body this parser
/// cannot read (`Format`) or that refuses a call (HTTP 401 or 403) is remembered, so a changed response shape or a
/// signed-out app is not reported as "not found".
async fn read_ports(
    http: &HttpClient,
    ports: &[u16],
    now_ms: i64,
) -> Result<Vec<Reading>, FetchError> {
    let mut last = FetchError::NotDetected;
    for &port in ports {
        match read_port(http, port).await {
            Ok((email, plan, buckets)) => {
                return Ok(vec![Reading {
                    key: AccountKey::new(Service::Antigravity, email.as_deref(), "antigravity-app"),
                    label: "Antigravity".to_string(),
                    plan,
                    buckets,
                    source: SourceKind::AntigravityApp,
                    observed_at_ms: now_ms,
                    banked_resets: None,
                }]);
            }
            Err(
                e @ (FetchError::Format(_) | FetchError::Network(NetErrorKind::Status(401 | 403))),
            ) => last = e,
            Err(_) => {}
        }
    }
    Err(last)
}

type PortReading = (Option<String>, Option<String>, Vec<Bucket>);

async fn read_port(http: &HttpClient, port: u16) -> Result<PortReading, FetchError> {
    // A port that refuses or stalls on `GET /` is not the language server: skip it before two more requests.
    let csrf = match http.get_loopback(port, "/").await {
        Ok(page) => parse_csrf(&String::from_utf8_lossy(&page)),
        Err(e @ (FetchError::Timeout | FetchError::Network(NetErrorKind::Connect))) => {
            return Err(e)
        }
        Err(_) => None,
    };
    let (email, plan) = match http
        .post_loopback(port, STATUS_PATH, csrf.as_deref(), b"{}")
        .await
    {
        Ok(body) => parse_user_status(&body),
        Err(e @ FetchError::Network(NetErrorKind::Status(401 | 403))) => return Err(e),
        Err(_) => (None, None),
    };
    let body = http
        .post_loopback(port, QUOTA_PATH, csrf.as_deref(), b"{}")
        .await?;
    Ok((email, plan, parse_quota_summary(&body)?))
}

/// PIDs from the `PROCESS_QUERY` CSV (`"ProcessId","ExecutablePath"`) whose file name is one of `IMAGES` and whose
/// path contains `APP_FOLDER`, in order, without repeats. The header and rows without a path are skipped.
pub fn parse_process_csv(csv: &str) -> Vec<u32> {
    let mut pids = Vec::new();
    for line in csv.lines() {
        let Some((pid, path)) = line
            .trim()
            .strip_prefix('"')
            .and_then(|l| l.split_once("\",\""))
        else {
            continue;
        };
        let Ok(pid) = pid.parse::<u32>() else {
            continue;
        };
        let path = path.strip_suffix('"').unwrap_or(path);
        if is_app_server(path) && !pids.contains(&pid) {
            pids.push(pid);
        }
    }
    pids
}

fn is_app_server(path: &str) -> bool {
    let name = path.rsplit('\\').next().unwrap_or(path);
    IMAGES.iter().any(|i| name.eq_ignore_ascii_case(i))
        && path.to_ascii_lowercase().contains(APP_FOLDER)
}

/// Loopback (`127.0.0.1`) listening ports of `pids` from `netstat -ano -p tcp`, highest first, without repeats.
/// The state column is localised on some Windows languages, so a remote address of `0.0.0.0:0` also counts as
/// listening.
pub fn parse_netstat(text: &str, pids: &[u32]) -> Vec<u16> {
    let mut ports: Vec<u16> = text
        .lines()
        .filter_map(|line| {
            let cols: Vec<&str> = line.split_whitespace().collect();
            let [proto, local, remote, state, pid] = cols[..] else {
                return None;
            };
            if !proto.eq_ignore_ascii_case("TCP") || !pids.contains(&pid.parse().ok()?) {
                return None;
            }
            if !state.eq_ignore_ascii_case("LISTENING") && remote != "0.0.0.0:0" {
                return None;
            }
            let port: u16 = local.strip_prefix("127.0.0.1:")?.parse().ok()?;
            (port != 0).then_some(port)
        })
        .collect();
    ports.sort_unstable_by(|a, b| b.cmp(a));
    ports.dedup();
    ports
}

/// The value after `csrfToken":"`: 1 to 128 characters of `[A-Za-z0-9_-]`, else `None`.
pub fn parse_csrf(html: &str) -> Option<String> {
    const KEY: &str = "csrfToken\":\"";
    let rest = &html[html.find(KEY)? + KEY.len()..];
    let token = &rest[..rest.find('"')?];
    let valid = (1..=128).contains(&token.len())
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    valid.then(|| token.to_string())
}

/// `groups[]` × `buckets[]` (also under a top-level `response`), camelCase or snake_case. Disabled buckets are
/// skipped. A bucket is named `"<group> · 5-hour"`, `"<group> · Weekly"` or `"<group> · <bucket displayName>"`.
pub fn parse_quota_summary(body: &[u8]) -> Result<Vec<Bucket>, FetchError> {
    let root: Value =
        serde_json::from_slice(body).map_err(|_| FetchError::Format("quota summary"))?;
    let groups = root
        .get("groups")
        .or_else(|| root.pointer("/response/groups"))
        .and_then(Value::as_array);
    let mut out = Vec::new();
    for group in groups.into_iter().flatten() {
        let group_name = field(group, "displayName", "display_name").and_then(Value::as_str);
        let group_name = group_name.ok_or(FetchError::Format("displayName"))?;
        for bucket in group
            .get("buckets")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if bucket.get("disabled").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            // proto3 JSON omits a zero value, so a missing fraction means nothing is left.
            let remaining = match field(bucket, "remainingFraction", "remaining_fraction") {
                None | Some(Value::Null) => Some(0.0),
                Some(v) => v.as_f64(),
            };
            let used_percent =
                checked_percent(remaining.map(|r| (1.0 - r) * 100.0), "remainingFraction")?;
            let resets_at_ms = match field(bucket, "resetTime", "reset_time") {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) => Some(iso_to_ms(s)?),
                Some(_) => return Err(FetchError::Format("resetTime")),
            };
            let (window_secs, name) = match bucket.get("window").and_then(Value::as_str) {
                Some("5h") => (Some(18_000), format!("{group_name} · 5-hour")),
                Some("weekly") => (Some(604_800), format!("{group_name} · Weekly")),
                _ => {
                    let bucket_name = field(bucket, "displayName", "display_name")
                        .and_then(Value::as_str)
                        .ok_or(FetchError::Format("displayName"))?;
                    (None, format!("{group_name} · {bucket_name}"))
                }
            };
            out.push(Bucket {
                name,
                used_percent,
                window_secs,
                resets_at_ms,
            });
        }
    }
    if out.is_empty() {
        return Err(FetchError::Format("no buckets"));
    }
    Ok(out)
}

/// Best effort: the first `email` string and the first plan or tier name in the user status. Never fails.
pub fn parse_user_status(body: &[u8]) -> (Option<String>, Option<String>) {
    let Ok(root) = serde_json::from_slice::<Value>(body) else {
        return (None, None);
    };
    let start = [
        "/userStatus",
        "/user_status",
        "/response/userStatus",
        "/response/user_status",
    ]
    .iter()
    .find_map(|p| root.pointer(p))
    .unwrap_or(&root);
    let email = find_str(start, &["email"], 254).filter(|e| e.contains('@'));
    let plan = find_str(
        start,
        &["planName", "plan_name", "tierName", "tier_name"],
        64,
    );
    (email, plan)
}

fn field<'a>(v: &'a Value, camel: &str, snake: &str) -> Option<&'a Value> {
    v.get(camel).or_else(|| v.get(snake))
}

/// Breadth-first search for the first non-empty string under one of `keys`, at most `max_len` bytes.
fn find_str(root: &Value, keys: &[&str], max_len: usize) -> Option<String> {
    let mut queue = VecDeque::from([root]);
    let mut visited = 0;
    while let Some(node) = queue.pop_front() {
        visited += 1;
        if visited > SEARCH_BUDGET {
            return None;
        }
        match node {
            Value::Object(map) => {
                for key in keys {
                    if let Some(s) = map.get(*key).and_then(Value::as_str) {
                        let s = s.trim();
                        if !s.is_empty() && s.len() <= max_len {
                            return Some(s.to_string());
                        }
                    }
                }
                queue.extend(map.values());
            }
            Value::Array(items) => queue.extend(items),
            _ => {}
        }
    }
    None
}

fn iso_to_ms(iso: &str) -> Result<i64, FetchError> {
    let at = OffsetDateTime::parse(iso, &Rfc3339).map_err(|_| FetchError::Format("resetTime"))?;
    i64::try_from(at.unix_timestamp_nanos() / 1_000_000)
        .map_err(|_| FetchError::Format("resetTime"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::MockServer;

    macro_rules! fixture {
        ($name:literal) => {
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/",
                $name
            ))
        };
    }

    const PROCESSES: &str = fixture!("ag_processes.csv");
    const NETSTAT: &str = fixture!("ag_netstat.txt");
    const NETSTAT_2: &str = fixture!("ag_netstat_2.txt");
    const QUOTA: &str = fixture!("ag_quota_summary.json");
    const QUOTA_SNAKE: &str = fixture!("ag_quota_summary_snake.json");
    const NOW: i64 = 1_794_150_000_000;

    #[test]
    fn process_csv_filters_by_name_and_path() {
        let csv = r#""ProcessId","ExecutablePath"
"12000","C:\Users\u\AppData\Local\Programs\antigravity\resources\bin\language_server.exe"
"23000","C:\Users\u\AppData\Local\Programs\Windsurf\resources\bin\language_server_windows_x64.exe"
"956","C:\Users\u\AppData\Local\Programs\antigravity\Antigravity.exe"
"#;
        assert_eq!(parse_process_csv(csv), vec![12000]);
        // Case-insensitive name and folder; empty, renamed and look-alike paths are rejected; no repeats.
        assert_eq!(parse_process_csv(PROCESSES), vec![12000, 7852]);
        assert!(parse_process_csv("").is_empty());
        assert!(parse_process_csv(r#""ProcessId","ExecutablePath""#).is_empty());
    }

    #[test]
    fn netstat_ranks_loopback_ports() {
        let pids = parse_process_csv(PROCESSES);
        assert_eq!(parse_netstat(NETSTAT, &pids), vec![62294, 54366]);
        assert_eq!(parse_netstat(NETSTAT, &[19328]), vec![6463]);
        assert!(parse_netstat(NETSTAT, &[]).is_empty());
    }

    #[test]
    fn netstat_accepts_localised_state() {
        let row = "  TCP    127.0.0.1:50000        0.0.0.0:0              ABHÖREN         42";
        assert_eq!(parse_netstat(row, &[42]), vec![50000]);
    }

    #[test]
    fn ports_change_between_calls_is_rediscovered() {
        let pids = parse_process_csv(PROCESSES);
        assert_eq!(parse_netstat(NETSTAT, &pids)[0], 62294);
        assert_eq!(parse_netstat(NETSTAT_2, &pids), vec![63001, 62294]);
    }

    #[test]
    fn csrf_extracted_and_validated() {
        let page = r#"<script>window.__cfg={"csrfToken":"aB3_x-9","port":1}</script>"#;
        assert_eq!(parse_csrf(page).as_deref(), Some("aB3_x-9"));
        assert_eq!(parse_csrf(r#"{"csrfToken":"<script>"}"#), None);
        assert_eq!(parse_csrf(r#"{"csrfToken":""}"#), None);
        assert_eq!(parse_csrf(r#"{"csrfToken":"abc"#), None);
        assert_eq!(parse_csrf("<html></html>"), None);
        let long = format!(r#"{{"csrfToken":"{}"}}"#, "a".repeat(129));
        assert_eq!(parse_csrf(&long), None);
        let max = format!(r#"{{"csrfToken":"{}"}}"#, "a".repeat(128));
        assert_eq!(parse_csrf(&max).map(|t| t.len()), Some(128));
    }

    #[test]
    fn quota_summary_parses_both_casings() {
        let camel = parse_quota_summary(QUOTA.as_bytes()).unwrap();
        let snake = parse_quota_summary(QUOTA_SNAKE.as_bytes()).unwrap();
        assert_eq!(camel, snake);
        assert_eq!(camel.len(), 4);

        let b = &camel[0];
        assert_eq!(b.name, "Gemini · 5-hour");
        assert!((b.used_percent - 62.0).abs() < 1e-9);
        assert_eq!(b.window_secs, Some(18_000));
        assert_eq!(b.resets_at_ms, Some(1_794_157_200_000));

        let b = &camel[1];
        assert_eq!(b.name, "Gemini · Weekly");
        assert_eq!(b.used_percent, 0.0);
        assert_eq!(b.window_secs, Some(604_800));
        assert_eq!(b.resets_at_ms, None);

        // The disabled bucket is skipped; this one omits `remainingFraction` (proto3 drops a zero).
        let b = &camel[2];
        assert_eq!(b.name, "Claude and GPT · 5-hour");
        assert_eq!(b.used_percent, 100.0);
        assert_eq!(b.resets_at_ms, Some(1_794_162_600_500));

        let b = &camel[3];
        assert_eq!(b.name, "Claude and GPT · Pool");
        assert_eq!(b.window_secs, None);
        assert_eq!(b.used_percent, 25.0);
    }

    fn one_bucket(bucket: &str) -> Result<Vec<Bucket>, FetchError> {
        let body =
            format!(r#"{{"groups":[{{"displayName":"Gemini 4 Argon","buckets":[{bucket}]}}]}}"#);
        parse_quota_summary(body.as_bytes())
    }

    #[test]
    fn missing_remaining_fraction_is_fully_used() {
        for bucket in [
            r#"{"displayName":"X","window":"5h"}"#,
            r#"{"displayName":"X","window":"5h","remainingFraction":null}"#,
            r#"{"displayName":"X","window":"5h","remaining_fraction":0}"#,
        ] {
            assert_eq!(
                one_bucket(bucket).unwrap()[0].used_percent,
                100.0,
                "{bucket}"
            );
        }
        assert_eq!(
            one_bucket(r#"{"displayName":"X","remainingFraction":"half"}"#),
            Err(FetchError::Format("remainingFraction"))
        );
    }

    #[test]
    fn disabled_buckets_are_skipped() {
        let both = r#"{"displayName":"A","disabled":true,"remainingFraction":0.1},
            {"displayName":"B","disabled":false,"remainingFraction":0.9}"#;
        let buckets = one_bucket(both).unwrap();
        assert_eq!(buckets.len(), 1);
        assert_eq!(buckets[0].name, "Gemini 4 Argon · B");
        assert_eq!(
            one_bucket(r#"{"displayName":"A","disabled":true}"#),
            Err(FetchError::Format("no buckets"))
        );
    }

    #[test]
    fn bucket_names_use_the_window() {
        let name = |bucket: &str| one_bucket(bucket).unwrap()[0].name.clone();
        assert_eq!(
            name(
                r#"{"displayName":"Gemini 4 Argon 5-Hour Limit","window":"5h","remainingFraction":1}"#
            ),
            "Gemini 4 Argon · 5-hour"
        );
        assert_eq!(
            name(
                r#"{"displayName":"Gemini 4 Argon Weekly Limit","window":"weekly","remainingFraction":1}"#
            ),
            "Gemini 4 Argon · Weekly"
        );
        assert_eq!(
            name(r#"{"displayName":"Monthly Pool","window":"monthly","remainingFraction":1}"#),
            "Gemini 4 Argon · Monthly Pool"
        );
        assert_eq!(
            one_bucket(r#"{"window":"monthly","remainingFraction":1}"#),
            Err(FetchError::Format("displayName"))
        );
    }

    #[test]
    fn quota_summary_empty_is_format_error() {
        let no_buckets = FetchError::Format("no buckets");
        for body in [
            r#"{}"#,
            r#"{"groups":[]}"#,
            r#"{"groups":[{"displayName":"Gemini","buckets":[]}]}"#,
            r#"{"response":{"groups":[]}}"#,
        ] {
            assert_eq!(
                parse_quota_summary(body.as_bytes()),
                Err(no_buckets.clone()),
                "{body}"
            );
        }
        assert!(parse_quota_summary(b"not json").is_err());
        let bad = r#"{"groups":[{"displayName":"G","buckets":[{"displayName":"B","remainingFraction":1.5}]}]}"#;
        assert_eq!(
            parse_quota_summary(bad.as_bytes()),
            Err(FetchError::Format("remainingFraction"))
        );
    }

    #[test]
    fn user_status_is_best_effort() {
        let body = br#"{"userStatus":{"name":"A","email":"Someone@Example.com",
            "planStatus":{"planInfo":{"planName":"Pro","teamsTier":"TEAMS_TIER_PRO"}}}}"#;
        assert_eq!(
            parse_user_status(body),
            (Some("Someone@Example.com".into()), Some("Pro".into()))
        );
        assert_eq!(parse_user_status(b"garbage"), (None, None));
        assert_eq!(parse_user_status(br#"{"email":42}"#), (None, None));
        assert_eq!(
            parse_user_status(
                br#"{"response":{"user_status":{"email":"a@b.c","tier_name":"Free"}}}"#
            ),
            (Some("a@b.c".into()), Some("Free".into()))
        );
    }

    #[test]
    fn provider_identity() {
        assert_eq!(AntigravityApp.kind(), SourceKind::AntigravityApp);
        assert_eq!(AntigravityApp.service(), Some(Service::Antigravity));
    }

    fn port_of(server: &MockServer) -> u16 {
        server.base.rsplit(':').next().unwrap().parse().unwrap()
    }

    fn closed_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    #[tokio::test]
    async fn first_port_with_buckets_wins() {
        let server = MockServer::start(vec![
            (200, "", r#"<script>{"csrfToken":"tok-1"}</script>"#.to_string()),
            (
                200,
                "",
                r#"{"userStatus":{"email":"Me@Example.com","planStatus":{"planInfo":{"planName":"Pro"}}}}"#
                    .to_string(),
            ),
            (200, "", QUOTA.to_string()),
        ]);
        let http = HttpClient::new().unwrap();
        let readings = read_ports(&http, &[closed_port(), port_of(&server)], NOW)
            .await
            .unwrap();
        assert_eq!(readings.len(), 1);
        let r = &readings[0];
        assert_eq!(
            r.key,
            AccountKey::new(
                Service::Antigravity,
                Some("me@example.com"),
                "antigravity-app"
            )
        );
        assert_eq!(r.label, "Antigravity");
        assert_eq!(r.plan.as_deref(), Some("Pro"));
        assert_eq!(r.source, SourceKind::AntigravityApp);
        assert_eq!(r.observed_at_ms, NOW);
        assert_eq!(r.buckets.len(), 4);
        assert_eq!(server.hits(), 3);
        let req = server.last_request.lock().unwrap().to_ascii_lowercase();
        assert!(req.starts_with(&format!("post {} ", QUOTA_PATH.to_ascii_lowercase())));
        assert!(req.contains("x-codeium-csrf-token: tok-1"));
    }

    #[tokio::test]
    async fn no_answering_port_is_not_detected() {
        let http = HttpClient::new().unwrap();
        assert_eq!(
            read_ports(&http, &[], NOW).await,
            Err(FetchError::NotDetected)
        );
        assert_eq!(
            read_ports(&http, &[closed_port()], NOW).await,
            Err(FetchError::NotDetected)
        );
        // A server that is not the language server: every call is a 404.
        let server = MockServer::start(vec![(404, "", String::new())]);
        assert_eq!(
            read_ports(&http, &[port_of(&server)], NOW).await,
            Err(FetchError::NotDetected)
        );
    }

    #[tokio::test]
    async fn unreadable_quota_is_format_error() {
        let server = MockServer::start(vec![
            (200, "", "<html></html>".to_string()),
            (200, "", "{}".to_string()),
            (200, "", r#"{"groups":[]}"#.to_string()),
        ]);
        let http = HttpClient::new().unwrap();
        assert_eq!(
            read_ports(&http, &[port_of(&server)], NOW).await,
            Err(FetchError::Format("no buckets"))
        );
        let req = server.last_request.lock().unwrap().to_ascii_lowercase();
        assert!(!req.contains("x-codeium-csrf-token"));
    }

    #[tokio::test]
    async fn auth_failure_is_kept_as_last_error() {
        let page = || {
            (
                200,
                "",
                r#"<script>{"csrfToken":"tok-1"}</script>"#.to_string(),
            )
        };
        let http = HttpClient::new().unwrap();
        // 401 on the status call.
        let server = MockServer::start(vec![page(), (401, "", String::new())]);
        assert_eq!(
            read_ports(&http, &[port_of(&server)], NOW).await,
            Err(FetchError::Network(NetErrorKind::Status(401)))
        );
        // 403 on the quota call, and a later closed port does not replace it.
        let server = MockServer::start(vec![
            page(),
            (200, "", "{}".to_string()),
            (403, "", String::new()),
        ]);
        assert_eq!(
            read_ports(&http, &[port_of(&server), closed_port()], NOW).await,
            Err(FetchError::Network(NetErrorKind::Status(403)))
        );
    }

    #[tokio::test]
    async fn fetch_budget_ends_in_timeout() {
        let stalled = std::future::pending::<Result<(), FetchError>>();
        assert_eq!(
            within(Duration::from_millis(50), stalled).await,
            Err(FetchError::Timeout)
        );
        assert_eq!(within(Duration::from_secs(1), async { Ok(7) }).await, Ok(7));
    }

    #[test]
    fn detect_is_cheap() {
        let env = crate::env::test_env(
            &std::env::temp_dir(),
            HttpClient::for_test("http://127.0.0.1:9"),
            || NOW,
        );
        let started = Instant::now();
        assert!(AntigravityApp.detect(&env));
        assert!(started.elapsed() < Duration::from_millis(50));
    }
}
