//! The running Antigravity app: quota from its language server on a loopback port.
//!
//! Method based on the open-source `ai-usagebar` 1.41.0 (`antigravity/fetch.rs`): find the app processes with
//! `tasklist`, find their loopback listening ports with `netstat`, read the CSRF token from the server's own page
//! (`GET /`), then `POST` `GetUserStatus` and `RetrieveUserQuotaSummary`. Field names must be verified against the
//! live IDE. This source only contacts `127.0.0.1`. It never reads a Google login, a token store or Windows
//! Credential Manager, and it never calls a Google endpoint. Response bodies and the CSRF token are never logged.

use crate::env::Env;
use crate::model::{checked_percent, AccountKey, Bucket, FetchError, Reading, Service, SourceKind};
use crate::net::HttpClient;
use crate::provider::{BoxFut, Provider};
use serde_json::Value;
use std::collections::VecDeque;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// Process images of the app and its language server, compared without case.
const IMAGES: [&str; 3] = [
    "Antigravity.exe",
    "language_server.exe",
    "language_server_windows_x64.exe",
];
const STATUS_PATH: &str = "/exa.language_server_pb.LanguageServerService/GetUserStatus";
const QUOTA_PATH: &str = "/exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary";
const TOOL_TIMEOUT: Duration = Duration::from_secs(5);
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

    /// Runs `tasklist` once (hidden, blocking, at most 5 s). Nothing is cached.
    fn detect(&self, _env: &Env) -> bool {
        app_pids().is_ok_and(|pids| !pids.is_empty())
    }

    fn fetch<'a>(&'a self, env: &'a Env) -> BoxFut<'a, Result<Vec<Reading>, FetchError>> {
        Box::pin(async move {
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
        })
    }
}

fn app_pids() -> Result<Vec<u32>, FetchError> {
    let csv = run_hidden(system_tool("tasklist.exe"), &["/fo", "csv", "/nh"])?;
    Ok(parse_tasklist(&csv))
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
/// cannot read is remembered as a `Format` error, so a changed response shape is not reported as "not found".
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
                }]);
            }
            Err(e @ FetchError::Format(_)) => last = e,
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
        Err(
            e @ (FetchError::Timeout | FetchError::Network(crate::model::NetErrorKind::Connect)),
        ) => return Err(e),
        Err(_) => None,
    };
    let (email, plan) = match http
        .post_loopback(port, STATUS_PATH, csrf.as_deref(), b"{}")
        .await
    {
        Ok(body) => parse_user_status(&body),
        Err(_) => (None, None),
    };
    let body = http
        .post_loopback(port, QUOTA_PATH, csrf.as_deref(), b"{}")
        .await?;
    Ok((email, plan, parse_quota_summary(&body)?))
}

/// PIDs from `tasklist /fo csv /nh` whose image is one of `IMAGES`, in order, without repeats.
pub fn parse_tasklist(csv: &str) -> Vec<u32> {
    let mut pids = Vec::new();
    for line in csv.lines() {
        let Some(line) = line.trim().strip_prefix('"') else {
            continue;
        };
        let mut fields = line.splitn(3, "\",\"");
        let (Some(name), Some(pid)) = (fields.next(), fields.next()) else {
            continue;
        };
        let Ok(pid) = pid.trim_end_matches('"').parse::<u32>() else {
            continue;
        };
        if IMAGES.iter().any(|i| name.eq_ignore_ascii_case(i)) && !pids.contains(&pid) {
            pids.push(pid);
        }
    }
    pids
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

/// `groups[]` × `buckets[]` (also under a top-level `response`), camelCase or snake_case.
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
            let bucket_name = field(bucket, "displayName", "display_name")
                .and_then(Value::as_str)
                .ok_or(FetchError::Format("displayName"))?;
            let remaining =
                field(bucket, "remainingFraction", "remaining_fraction").and_then(Value::as_f64);
            let used_percent =
                checked_percent(remaining.map(|r| (1.0 - r) * 100.0), "remainingFraction")?;
            let resets_at_ms = match field(bucket, "resetTime", "reset_time") {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) => Some(iso_to_ms(s)?),
                Some(_) => return Err(FetchError::Format("resetTime")),
            };
            let window_secs = match bucket.get("window").and_then(Value::as_str) {
                Some("5h") => Some(18_000),
                Some("weekly") => Some(604_800),
                _ => None,
            };
            out.push(Bucket {
                name: format!("{group_name} · {bucket_name}"),
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

    const TASKLIST: &str = fixture!("ag_tasklist.csv");
    const NETSTAT: &str = fixture!("ag_netstat.txt");
    const NETSTAT_2: &str = fixture!("ag_netstat_2.txt");
    const QUOTA: &str = fixture!("ag_quota_summary.json");
    const QUOTA_SNAKE: &str = fixture!("ag_quota_summary_snake.json");
    const NOW: i64 = 1_794_150_000_000;

    #[test]
    fn tasklist_finds_ide_pids() {
        assert_eq!(parse_tasklist(TASKLIST), vec![7852, 12000, 956, 10144]);
        assert!(parse_tasklist("").is_empty());
        assert!(
            parse_tasklist("INFO: No tasks are running which match the specified criteria.")
                .is_empty()
        );
    }

    #[test]
    fn netstat_ranks_loopback_ports() {
        let pids = parse_tasklist(TASKLIST);
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
        let pids = parse_tasklist(TASKLIST);
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

        let b = &camel[2];
        assert_eq!(b.name, "Claude and GPT · 5-hour");
        assert_eq!(b.used_percent, 100.0);
        assert_eq!(b.resets_at_ms, Some(1_794_162_600_500));

        let b = &camel[3];
        assert_eq!(b.window_secs, None);
        assert_eq!(b.used_percent, 25.0);
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
}
