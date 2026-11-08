//! Claude Code's own login: `.credentials.json` plus Anthropic's OAuth usage endpoint.
//!
//! The endpoint is undocumented; Claude Code and several third-party widgets use it. This source is
//! read-only and never refreshes the token: the refresh token is single-use, so a refresh would log
//! the real CLI out. An expired token is reported as `Expired` and nothing is sent.

use crate::env::Env;
use crate::model::{checked_percent, AccountKey, Bucket, FetchError, Reading, Service, SourceKind};
use crate::net::{HeaderValueSource, Host};
use crate::provider::{BoxFut, Provider};
use crate::secret::Secret;
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use zeroize::Zeroizing;

/// The default display name. Never the email (spec §6.3); the user can rename the account in the UI.
const LABEL: &str = "Claude";
const FIVE_HOUR_SECS: u64 = 18_000;
const WEEK_SECS: u64 = 604_800;

/// One Claude Code login on this PC: `<dir>/.credentials.json`.
///
/// `dir` is the current login's folder (`Env::default_claude_dir`) or an added account's profile folder.
pub struct ClaudeDirect {
    pub dir: PathBuf,
    /// The account ID part that identifies this login when it has no email, e.g. "claude-current".
    pub local_id: String,
}

impl Provider for ClaudeDirect {
    fn kind(&self) -> SourceKind {
        SourceKind::Direct
    }

    fn service(&self) -> Option<Service> {
        Some(Service::Claude)
    }

    fn detect(&self, _env: &Env) -> bool {
        self.dir.join(".credentials.json").is_file()
    }

    fn fetch<'a>(&'a self, env: &'a Env) -> BoxFut<'a, Result<Vec<Reading>, FetchError>> {
        Box::pin(async move {
            let now = (env.now_ms)();
            let ClaudeLogin {
                token,
                expires_at_ms,
                plan,
                email,
            } = read_login(&self.dir, &env.home)?;
            if expires_at_ms <= now {
                return Err(FetchError::Expired);
            }
            let body = env
                .http
                .get_json(
                    Host::Anthropic,
                    "/api/oauth/usage",
                    &[
                        ("authorization", HeaderValueSource::Bearer(&token)),
                        (
                            "anthropic-beta",
                            HeaderValueSource::Static("oauth-2025-04-20"),
                        ),
                        ("accept", HeaderValueSource::Static("application/json")),
                    ],
                )
                .await?;
            drop(token);
            let key = AccountKey::new(Service::Claude, email.as_deref(), self.local_id.as_str());
            parse_usage(&body, key, LABEL.to_string(), plan, now).map(|reading| vec![reading])
        })
    }
}

/// What `.credentials.json` and `.claude.json` say about the active login.
#[derive(Debug)]
pub struct ClaudeLogin {
    pub token: Secret,
    pub expires_at_ms: i64,
    /// `subscriptionType` with its first letter capitalised ("pro" becomes "Pro").
    pub plan: Option<String>,
    /// `None` when no config file names the account. Such a login is never merged with another source.
    pub email: Option<String>,
}

#[derive(Deserialize)]
struct CredentialsFile {
    #[serde(rename = "claudeAiOauth")]
    oauth: Option<OauthBlock>,
}

// The refresh token is not listed here, so it is skipped without ever being copied out of the file buffer.
#[derive(Deserialize)]
struct OauthBlock {
    #[serde(rename = "accessToken", default, deserialize_with = "into_secret")]
    access_token: Option<Secret>,
    #[serde(rename = "expiresAt")]
    expires_at: Option<f64>,
    #[serde(rename = "subscriptionType")]
    subscription_type: Option<String>,
}

/// Moves the token straight into a zeroizing `Secret`, so a parse error later in the file cannot leave a plain copy.
pub(super) fn into_secret<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Secret>, D::Error> {
    let token = Option::<String>::deserialize(deserializer)?;
    Ok(token.filter(|t| !t.is_empty()).map(Secret::new))
}

#[derive(Deserialize)]
struct ClaudeJson {
    #[serde(rename = "oauthAccount")]
    oauth_account: Option<OauthAccount>,
}

#[derive(Deserialize)]
struct OauthAccount {
    #[serde(rename = "emailAddress")]
    email_address: Option<String>,
}

/// Reads the access token, its expiry and the plan from `<claude_dir>/.credentials.json`, and the account email
/// as `read_email` describes. Both files are only read, never locked or written.
///
/// A missing credentials file is `NotDetected`. Truncated or mistyped JSON, or a missing `accessToken` or
/// `expiresAt`, is a `Format` error. A missing email is not an error.
pub fn read_login(claude_dir: &Path, home: &Path) -> Result<ClaudeLogin, FetchError> {
    let raw = Zeroizing::new(
        std::fs::read(claude_dir.join(".credentials.json")).map_err(|e| match e.kind() {
            ErrorKind::NotFound => FetchError::NotDetected,
            _ => FetchError::Unavailable("Claude login file is not readable"),
        })?,
    );
    let file: CredentialsFile =
        serde_json::from_slice(&raw).map_err(|_| FetchError::Format("credentials"))?;
    let oauth = file.oauth.ok_or(FetchError::Format("claudeAiOauth"))?;
    let token = oauth
        .access_token
        .ok_or(FetchError::Format("accessToken"))?;
    let expires_at = oauth
        .expires_at
        .filter(|ms| ms.is_finite())
        .ok_or(FetchError::Format("expiresAt"))?;
    Ok(ClaudeLogin {
        token,
        expires_at_ms: expires_at as i64,
        plan: oauth.subscription_type.as_deref().and_then(capitalise),
        email: read_email(claude_dir, home),
    })
}

/// Best effort: any missing, unreadable or unexpected file just means no email. Claude Code keeps its config in
/// `<home>/.claude.json`, or in `<claude_dir>/.claude.json` when `CLAUDE_CONFIG_DIR` moves it.
///
/// For the default folder (`<home>/.claude`) both files are read, home first. For any other folder (a moved
/// default or an added profile) only `<claude_dir>/.claude.json` is read: `<home>/.claude.json` names the
/// default login, and borrowing its email would give the profile the wrong account ID.
pub(crate) fn read_email(claude_dir: &Path, home: &Path) -> Option<String> {
    let paths = if claude_dir == home.join(".claude") {
        vec![home.join(".claude.json"), claude_dir.join(".claude.json")]
    } else {
        vec![claude_dir.join(".claude.json")]
    };
    paths.iter().find_map(|path| {
        let raw = std::fs::read(path).ok()?;
        let file: ClaudeJson = serde_json::from_slice(&raw).ok()?;
        let email = file.oauth_account?.email_address?;
        let email = email.trim();
        (!email.is_empty()).then(|| email.to_string())
    })
}

/// "pro" becomes "Pro". An empty string has no plan name.
fn capitalise(s: &str) -> Option<String> {
    let mut chars = s.chars();
    let first = chars.next()?;
    Some(first.to_uppercase().chain(chars).collect())
}

/// Turns the body of `GET /api/oauth/usage` into one reading.
///
/// Buckets: `five_hour` ("5 hour"), `seven_day` ("Weekly"), and every `seven_day_<model>` object that carries a
/// `utilization` ("Weekly · Sonnet"). Null windows and every other key are ignored. Both headline windows
/// missing or null is a `Format` error, as is any utilization that is not a number from 0 up.
pub fn parse_usage(
    body: &[u8],
    key: AccountKey,
    label: String,
    plan: Option<String>,
    now_ms: i64,
) -> Result<Reading, FetchError> {
    let root: Value = serde_json::from_slice(body).map_err(|_| FetchError::Format("usage"))?;
    let root = root.as_object().ok_or(FetchError::Format("usage"))?;

    let mut buckets = Vec::new();
    for (field, name, window_secs) in [
        ("five_hour", "5 hour", FIVE_HOUR_SECS),
        ("seven_day", "Weekly", WEEK_SECS),
    ] {
        match root.get(field) {
            None | Some(Value::Null) => {}
            Some(window) => buckets.push(bucket(window, name.to_string(), window_secs, field)?),
        }
    }
    if buckets.is_empty() {
        return Err(FetchError::Format("no windows"));
    }

    // A null or utilization-less model entry is a model the plan does not meter. Sorted by name so the
    // order does not depend on whether serde_json keeps JSON key order.
    let mut models = Vec::new();
    for (field, window) in root {
        let Some(model) = field.strip_prefix("seven_day_") else {
            continue;
        };
        if window.get("utilization").is_none_or(Value::is_null) {
            continue;
        }
        let name = format!(
            "Weekly · {}",
            capitalise(&model.replace('_', " ")).unwrap_or_default()
        );
        models.push(bucket(window, name, WEEK_SECS, "seven_day_model")?);
    }
    models.sort_by(|a, b| a.name.cmp(&b.name));
    buckets.extend(models);

    Ok(Reading {
        key,
        label,
        plan,
        buckets,
        source: SourceKind::Direct,
        observed_at_ms: now_ms,
        banked_resets: None,
    })
}

fn bucket(
    window: &Value,
    name: String,
    window_secs: u64,
    field: &'static str,
) -> Result<Bucket, FetchError> {
    let used_percent = checked_percent(window.get("utilization").and_then(Value::as_f64), field)?;
    let resets_at_ms = match window.get("resets_at") {
        None | Some(Value::Null) => None,
        Some(Value::String(iso)) => Some(iso_to_ms(iso)?),
        Some(_) => return Err(FetchError::Format("resets_at")),
    };
    Ok(Bucket {
        name,
        used_percent,
        window_secs: Some(window_secs),
        resets_at_ms,
    })
}

/// "2026-11-07T20:10:00.102452+00:00" to unix milliseconds (truncated).
fn iso_to_ms(iso: &str) -> Result<i64, FetchError> {
    OffsetDateTime::parse(iso, &Rfc3339)
        .map(|t| (t.unix_timestamp_nanos() / 1_000_000) as i64)
        .map_err(|_| FetchError::Format("resets_at"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::test_env;
    use crate::net::HttpClient;
    use crate::testutil::MockServer;
    use std::path::PathBuf;

    const CREDENTIALS: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/claude_credentials.json"
    ));
    const CONFIG: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/claude_config.json"
    ));
    const USAGE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/claude_usage.json"
    ));
    const USAGE_NOISY: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/claude_usage_noisy.json"
    ));
    const USAGE_MODEL: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/claude_usage_model.json"
    ));

    // Token and expiry in claude_credentials.json.
    const TOKEN: &str = "sk-ant-oat01-SYNTHETIC-access-token";
    const EXPIRES_AT_MS: i64 = 1_893_456_000_000; // 2030-01-01T00:00:00Z
                                                  // Reset times in claude_usage*.json, computed outside this crate and truncated to whole milliseconds.
    const FIVE_HOUR_RESET_MS: i64 = 1_794_082_200_102; // 2026-11-07T20:10:00.102452Z
    const WEEKLY_RESET_MS: i64 = 1_794_477_600_102; // 2026-11-12T10:00:00.102452Z

    fn now() -> i64 {
        1_794_063_600_000 // 2026-11-07T15:00:00Z, before the token expires
    }

    fn after_expiry() -> i64 {
        1_924_992_000_000 // 2031-01-01T00:00:00Z
    }

    /// A throwaway home folder with an empty `.claude` inside. Removed on drop, so a failed assertion cannot leave it behind.
    struct TempHome(PathBuf);

    impl TempHome {
        fn new(name: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("qg-test-{}-{}", std::process::id(), name));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join(".claude")).unwrap();
            Self(root)
        }

        fn write(&self, relative: &str, content: &str) {
            std::fs::write(self.0.join(relative), content).unwrap();
        }

        fn claude_dir(&self) -> PathBuf {
            self.0.join(".claude")
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn env_for(home: &TempHome, base: &str, now_ms: fn() -> i64) -> Env {
        test_env(&home.0, HttpClient::for_test(base), now_ms)
    }

    const LOCAL_ID: &str = "claude-current";

    /// The source for the default `.claude` folder inside `home`.
    fn source(home: &TempHome) -> ClaudeDirect {
        ClaudeDirect {
            dir: home.claude_dir(),
            local_id: LOCAL_ID.into(),
        }
    }

    fn key() -> AccountKey {
        AccountKey::new(Service::Claude, Some("user@example.com"), LOCAL_ID)
    }

    fn parse(body: &str) -> Result<Reading, FetchError> {
        parse_usage(
            body.as_bytes(),
            key(),
            "Claude".into(),
            Some("Pro".into()),
            now(),
        )
    }

    #[tokio::test]
    async fn label_is_never_email() {
        let home = TempHome::new("claude-label");
        home.write(".claude/.credentials.json", CREDENTIALS);
        home.write(".claude.json", CONFIG);
        let server = MockServer::start(vec![(200, "", USAGE.to_string())]);
        let env = env_for(&home, &server.base, now);

        let readings = source(&home).fetch(&env).await.unwrap();

        assert_eq!(readings[0].label, "Claude");
        assert!(!readings[0].label.contains('@'));
        assert_eq!(readings[0].banked_resets, None);
        // The email still identifies the account.
        assert_eq!(readings[0].key, key());
    }

    #[tokio::test]
    async fn profile_dir_is_used() {
        let home = TempHome::new("claude-profile");
        let profile = home.0.join("profile-a");
        let empty = home.0.join("profile-b");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::create_dir_all(&empty).unwrap();
        std::fs::write(profile.join(".credentials.json"), CREDENTIALS).unwrap();
        // The default login's config names an account. A profile must not borrow it.
        home.write(".claude.json", CONFIG);
        let server = MockServer::start(vec![
            (200, "", USAGE.to_string()),
            (200, "", USAGE.to_string()),
        ]);
        let env = env_for(&home, &server.base, now);
        let in_profile = ClaudeDirect {
            dir: profile.clone(),
            local_id: "profile-a".into(),
        };
        let in_empty = ClaudeDirect {
            dir: empty,
            local_id: "profile-b".into(),
        };

        assert!(in_profile.detect(&env));
        assert!(!in_empty.detect(&env));
        assert!(!source(&home).detect(&env));
        let readings = in_profile.fetch(&env).await.unwrap();
        assert_eq!(readings[0].key.local_id, "profile-a");
        assert_eq!(readings[0].key.email, None);
        assert_eq!(readings[0].label, "Claude");
        assert_eq!(in_empty.fetch(&env).await, Err(FetchError::NotDetected));

        // The profile's own config names its account.
        std::fs::write(profile.join(".claude.json"), CONFIG).unwrap();
        let readings = in_profile.fetch(&env).await.unwrap();
        assert_eq!(readings[0].key.email.as_deref(), Some("user@example.com"));
    }

    fn expected(name: &str, used_percent: f64, window_secs: u64, resets_at_ms: i64) -> Bucket {
        Bucket {
            name: name.into(),
            used_percent,
            window_secs: Some(window_secs),
            resets_at_ms: Some(resets_at_ms),
        }
    }

    #[test]
    fn parses_five_hour_and_weekly() {
        let reading = parse(USAGE).unwrap();
        assert_eq!(
            reading.buckets,
            vec![
                expected("5 hour", 10.0, 18_000, FIVE_HOUR_RESET_MS),
                expected("Weekly", 21.0, 604_800, WEEKLY_RESET_MS),
            ]
        );
        assert_eq!(reading.key, key());
        assert_eq!(reading.label, "Claude");
        assert_eq!(reading.plan.as_deref(), Some("Pro"));
        assert_eq!(reading.source, SourceKind::Direct);
        assert_eq!(reading.observed_at_ms, now());
    }

    #[test]
    fn ignores_null_and_unknown_fields() {
        // Unknown keys of every JSON type, null model windows, a window-shaped key that is not five_hour or
        // seven_day*, and extra keys inside the real windows.
        let reading = parse(USAGE_NOISY).unwrap();
        assert_eq!(
            reading.buckets,
            vec![
                expected("5 hour", 10.0, 18_000, FIVE_HOUR_RESET_MS),
                expected("Weekly", 21.0, 604_800, WEEKLY_RESET_MS),
            ]
        );
    }

    #[test]
    fn includes_non_null_model_bucket() {
        let reading = parse(USAGE_MODEL).unwrap();
        assert_eq!(
            reading.buckets,
            vec![
                expected("5 hour", 10.0, 18_000, FIVE_HOUR_RESET_MS),
                expected("Weekly", 21.0, 604_800, WEEKLY_RESET_MS),
                expected("Weekly · Sonnet", 3.5, 604_800, WEEKLY_RESET_MS),
            ]
        );
    }

    #[test]
    fn over_100_clamps_to_100() {
        let reading =
            parse(r#"{"five_hour":{"utilization":112.5,"resets_at":null},"seven_day":null}"#)
                .unwrap();
        assert_eq!(
            reading.buckets,
            vec![Bucket {
                name: "5 hour".into(),
                used_percent: 100.0,
                window_secs: Some(18_000),
                resets_at_ms: None
            }]
        );
    }

    #[test]
    fn negative_or_missing_is_format_error() {
        for (body, field) in [
            (
                r#"{"five_hour":{"utilization":-1,"resets_at":null}}"#,
                "five_hour",
            ),
            (r#"{"five_hour":{"resets_at":null}}"#, "five_hour"),
            (
                r#"{"five_hour":{"utilization":null,"resets_at":null}}"#,
                "five_hour",
            ),
            (
                r#"{"five_hour":{"utilization":"10","resets_at":null}}"#,
                "five_hour",
            ),
            (
                r#"{"five_hour":null,"seven_day":{"utilization":-0.5}}"#,
                "seven_day",
            ),
            (
                r#"{"five_hour":{"utilization":5},"seven_day_sonnet":{"utilization":-2}}"#,
                "seven_day_model",
            ),
        ] {
            assert_eq!(parse(body), Err(FetchError::Format(field)), "{body}");
        }
    }

    #[test]
    fn unusable_bodies_are_format_errors() {
        for (body, field) in [
            ("not json", "usage"),
            ("[]", "usage"),
            ("{}", "no windows"),
            (
                r#"{"five_hour":null,"seven_day":null,"seven_day_sonnet":{"utilization":3}}"#,
                "no windows",
            ),
            (
                r#"{"five_hour":{"utilization":1,"resets_at":"tomorrow"}}"#,
                "resets_at",
            ),
            (
                r#"{"five_hour":{"utilization":1,"resets_at":12345}}"#,
                "resets_at",
            ),
        ] {
            assert_eq!(parse(body), Err(FetchError::Format(field)), "{body}");
        }
    }

    #[tokio::test]
    async fn expired_token_never_calls_network() {
        let home = TempHome::new("claude-expired");
        home.write(".claude/.credentials.json", CREDENTIALS);
        let server = MockServer::start(vec![(200, "", USAGE.to_string())]);
        let env = env_for(&home, &server.base, after_expiry);

        let result = source(&home).fetch(&env).await;

        assert_eq!(result, Err(FetchError::Expired));
        assert_eq!(server.hits(), 0);
    }

    #[test]
    fn truncated_credentials_is_format_error() {
        let home = TempHome::new("claude-truncated");
        home.write(
            ".claude/.credentials.json",
            r#"{"claudeAiOauth":{"accessTo"#,
        );

        let err = read_login(&home.claude_dir(), &home.0).unwrap_err();

        assert!(matches!(err, FetchError::Format(_)), "{err:?}");
    }

    #[test]
    fn credentials_without_token_or_expiry_are_format_errors() {
        let home = TempHome::new("claude-incomplete");
        for (content, field) in [
            (r#"{}"#, "claudeAiOauth"),
            (
                r#"{"claudeAiOauth":{"expiresAt":1893456000000}}"#,
                "accessToken",
            ),
            (
                r#"{"claudeAiOauth":{"accessToken":"","expiresAt":1893456000000}}"#,
                "accessToken",
            ),
            (r#"{"claudeAiOauth":{"accessToken":"t"}}"#, "expiresAt"),
        ] {
            home.write(".claude/.credentials.json", content);
            assert_eq!(
                read_login(&home.claude_dir(), &home.0).unwrap_err(),
                FetchError::Format(field),
                "{content}"
            );
        }
    }

    #[test]
    fn missing_credentials_is_not_detected() {
        let home = TempHome::new("claude-missing");
        let env = env_for(&home, "http://127.0.0.1:9", now);
        assert!(!source(&home).detect(&env));
        assert_eq!(
            read_login(&home.claude_dir(), &home.0).unwrap_err(),
            FetchError::NotDetected
        );

        home.write(".claude/.credentials.json", CREDENTIALS);
        assert!(source(&home).detect(&env));
    }

    #[test]
    fn reads_login_plan_and_email() {
        let home = TempHome::new("claude-login");
        home.write(".claude/.credentials.json", CREDENTIALS);
        home.write(".claude.json", CONFIG);

        let login = read_login(&home.claude_dir(), &home.0).unwrap();

        assert_eq!(login.token.expose(), TOKEN);
        assert_eq!(login.expires_at_ms, EXPIRES_AT_MS);
        assert_eq!(login.plan.as_deref(), Some("Pro"));
        assert_eq!(login.email.as_deref(), Some("user@example.com"));
    }

    #[test]
    fn email_falls_back_to_the_claude_dir_config() {
        let home = TempHome::new("claude-email-fallback");
        home.write(".claude/.credentials.json", CREDENTIALS);
        home.write(".claude/.claude.json", CONFIG);

        let login = read_login(&home.claude_dir(), &home.0).unwrap();

        assert_eq!(login.email.as_deref(), Some("user@example.com"));
    }

    #[tokio::test]
    async fn missing_email_gives_none() {
        let home = TempHome::new("claude-no-email");
        home.write(".claude/.credentials.json", CREDENTIALS);
        assert_eq!(read_login(&home.claude_dir(), &home.0).unwrap().email, None);
        let server = MockServer::start(vec![(200, "", USAGE.to_string())]);
        let env = env_for(&home, &server.base, now);

        let readings = source(&home).fetch(&env).await.unwrap();

        assert_eq!(readings.len(), 1);
        assert_eq!(readings[0].label, "Claude");
        assert_eq!(readings[0].key.email, None);
        assert_eq!(readings[0].key.merge_key(), None);
    }

    #[tokio::test]
    async fn fetch_sends_required_headers() {
        let home = TempHome::new("claude-headers");
        home.write(".claude/.credentials.json", CREDENTIALS);
        home.write(".claude.json", CONFIG);
        let server = MockServer::start(vec![(200, "", USAGE.to_string())]);
        let env = env_for(&home, &server.base, now);

        let readings = source(&home).fetch(&env).await.unwrap();

        let request = server.last_request.lock().unwrap().to_lowercase();
        assert!(request.starts_with("get /api/oauth/usage "));
        assert!(request.contains("anthropic-beta: oauth-2025-04-20"));
        assert!(request.contains(&format!("authorization: bearer {}", TOKEN.to_lowercase())));
        assert!(request.contains("accept: application/json"));
        assert_eq!(server.hits(), 1);

        assert_eq!(readings.len(), 1);
        let reading = &readings[0];
        assert_eq!(reading.key, key());
        assert_eq!(reading.label, "Claude");
        assert_eq!(reading.plan.as_deref(), Some("Pro"));
        assert_eq!(reading.source, SourceKind::Direct);
        assert_eq!(reading.observed_at_ms, now());
        assert_eq!(
            reading.buckets,
            vec![
                expected("5 hour", 10.0, 18_000, FIVE_HOUR_RESET_MS),
                expected("Weekly", 21.0, 604_800, WEEKLY_RESET_MS),
            ]
        );
    }
}
