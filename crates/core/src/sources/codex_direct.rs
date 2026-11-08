//! Codex's own login: `auth.json` plus ChatGPT's usage endpoint.
//!
//! The endpoint is undocumented; the Codex CLI and desktop app share `auth.json`. This source is read-only and
//! never refreshes the token: the refresh token is single-use, so a refresh would log the real CLI out. A token
//! whose `exp` has passed is reported as `Expired` and nothing is sent.

use super::claude_direct::into_secret;
use crate::env::Env;
use crate::model::{checked_percent, AccountKey, Bucket, FetchError, Reading, Service, SourceKind};
use crate::net::{HeaderValueSource, Host};
use crate::provider::{BoxFut, Provider};
use crate::secret::Secret;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::Deserialize;
use serde_json::Value;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

/// The default display name. Never the email (spec §6.3); the user can rename the account in the UI.
const LABEL: &str = "Codex";

/// One Codex login on this PC: `<dir>/auth.json`.
///
/// `dir` is the current login's folder (`Env::default_codex_dir`) or an added account's profile folder.
pub struct CodexDirect {
    pub dir: PathBuf,
    /// The account ID part that identifies this login when it has no email, e.g. "codex-current".
    pub local_id: String,
}

impl Provider for CodexDirect {
    fn kind(&self) -> SourceKind {
        SourceKind::Direct
    }

    fn service(&self) -> Option<Service> {
        Some(Service::Codex)
    }

    fn detect(&self, _env: &Env) -> bool {
        self.dir.join("auth.json").is_file()
    }

    fn fetch<'a>(&'a self, env: &'a Env) -> BoxFut<'a, Result<Vec<Reading>, FetchError>> {
        Box::pin(async move {
            let now = (env.now_ms)();
            let Auth {
                token,
                id_token,
                account_id,
            } = read_auth(&self.dir)?;
            let exp_ms = jwt_exp_ms(token.expose()).ok_or(FetchError::Format("token exp"))?;
            if exp_ms <= now {
                return Err(FetchError::Expired);
            }
            let mut headers = vec![
                ("authorization", HeaderValueSource::Bearer(&token)),
                ("accept", HeaderValueSource::Static("application/json")),
            ];
            if let Some(id) = account_id.as_deref() {
                headers.push(("chatgpt-account-id", HeaderValueSource::Plain(id)));
            }
            let body = env
                .http
                .get_json(Host::ChatGpt, "/backend-api/wham/usage", &headers)
                .await?;
            drop(headers);
            drop(token);
            let email = id_token.as_deref().and_then(email_from_id_token);
            let key = AccountKey::new(Service::Codex, email.as_deref(), self.local_id.as_str());
            parse_usage(&body, key, LABEL.to_string(), now).map(|reading| vec![reading])
        })
    }
}

struct Auth {
    token: Secret,
    id_token: Option<String>,
    account_id: Option<String>,
}

#[derive(Deserialize)]
struct AuthFile {
    tokens: Option<Tokens>,
}

// The refresh token is not listed here, so it is skipped without ever being copied out of the file buffer.
#[derive(Deserialize)]
struct Tokens {
    #[serde(default, deserialize_with = "into_secret")]
    access_token: Option<Secret>,
    id_token: Option<String>,
    account_id: Option<String>,
}

/// A missing file is `NotDetected`. Bad JSON, or a missing `tokens` block or `access_token`, is a `Format` error.
fn read_auth(codex_dir: &Path) -> Result<Auth, FetchError> {
    let raw = Zeroizing::new(std::fs::read(codex_dir.join("auth.json")).map_err(
        |e| match e.kind() {
            ErrorKind::NotFound => FetchError::NotDetected,
            _ => FetchError::Unavailable("Codex login file is not readable"),
        },
    )?);
    let file: AuthFile = serde_json::from_slice(&raw).map_err(|_| FetchError::Format("auth"))?;
    let tokens = file.tokens.ok_or(FetchError::Format("tokens"))?;
    Ok(Auth {
        token: tokens
            .access_token
            .ok_or(FetchError::Format("access_token"))?,
        id_token: tokens.id_token,
        account_id: tokens.account_id.filter(|id| !id.is_empty()),
    })
}

/// The JSON payload (middle segment) of a JWT. The signature is not checked: the value only feeds a label and an
/// expiry guard, and the server judges the token.
fn jwt_payload(jwt: &str) -> Option<Value> {
    let segment = jwt.split('.').nth(1)?.trim_end_matches('=');
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(segment).ok()?).ok()
}

/// The `email` claim, trimmed. `None` when the token is malformed or has no usable email.
pub fn email_from_id_token(jwt: &str) -> Option<String> {
    let payload = jwt_payload(jwt)?;
    let email = payload.get("email")?.as_str()?.trim();
    (!email.is_empty()).then(|| email.to_string())
}

/// The `exp` claim (unix seconds) as milliseconds. `None` when the token is malformed or has no usable `exp`.
pub fn jwt_exp_ms(jwt: &str) -> Option<i64> {
    let exp = jwt_payload(jwt)?.get("exp")?.as_f64()?;
    exp.is_finite().then_some((exp * 1000.0) as i64)
}

/// Turns the body of `GET /backend-api/wham/usage` into one reading.
///
/// Buckets, in order: `rate_limit.primary_window`, `rate_limit.secondary_window`, and
/// `code_review_rate_limit.primary_window` ("Code review"). Null or absent windows and every other key are
/// ignored. No window at all is a `Format` error, as is a window without a usable percent or length.
///
/// `rate_limit_reset_credits.available_count` becomes `banked_resets` (saturated to `u32::MAX`). When it is
/// absent, null or not a whole number from 0 up, `banked_resets` is `None`; it is never an error.
pub fn parse_usage(
    body: &[u8],
    key: AccountKey,
    label: String,
    now_ms: i64,
) -> Result<Reading, FetchError> {
    let root: Value = serde_json::from_slice(body).map_err(|_| FetchError::Format("usage"))?;
    let root = root.as_object().ok_or(FetchError::Format("usage"))?;

    let mut buckets = Vec::new();
    for (limit, field, name) in [
        ("rate_limit", "primary_window", None),
        ("rate_limit", "secondary_window", None),
        (
            "code_review_rate_limit",
            "primary_window",
            Some("Code review"),
        ),
    ] {
        match root.get(limit).and_then(|l| l.get(field)) {
            None | Some(Value::Null) => {}
            Some(window) => buckets.push(bucket(window, name, field)?),
        }
    }
    if buckets.is_empty() {
        return Err(FetchError::Format("no windows"));
    }

    let plan = root
        .get("plan_type")
        .and_then(Value::as_str)
        .and_then(capitalise);
    let banked_resets = root
        .get("rate_limit_reset_credits")
        .and_then(|c| c.get("available_count"))
        .and_then(Value::as_u64)
        .map(|n| u32::try_from(n).unwrap_or(u32::MAX));
    Ok(Reading {
        key,
        label,
        plan,
        buckets,
        source: SourceKind::Direct,
        observed_at_ms: now_ms,
        banked_resets,
    })
}

fn bucket(window: &Value, name: Option<&str>, field: &'static str) -> Result<Bucket, FetchError> {
    let used_percent = checked_percent(window.get("used_percent").and_then(Value::as_f64), field)?;
    let window_secs = window
        .get("limit_window_seconds")
        .and_then(Value::as_u64)
        .filter(|&s| s > 0)
        .ok_or(FetchError::Format("limit_window_seconds"))?;
    let resets_at_ms = match window.get("reset_at") {
        None | Some(Value::Null) => None,
        Some(Value::Number(n)) => {
            let secs = n
                .as_f64()
                .filter(|s| s.is_finite())
                .ok_or(FetchError::Format("reset_at"))?;
            Some((secs * 1000.0) as i64)
        }
        Some(_) => return Err(FetchError::Format("reset_at")),
    };
    let name = name.map_or_else(|| window_name(window_secs), str::to_string);
    Ok(Bucket {
        name,
        used_percent,
        window_secs: Some(window_secs),
        resets_at_ms,
    })
}

/// 18000 is "5 hour", 604800 "Weekly", 2592000 "30 days"; other whole days read "N days", anything else "N h".
fn window_name(secs: u64) -> String {
    match secs {
        18_000 => "5 hour".to_string(),
        604_800 => "Weekly".to_string(),
        s if s % 86_400 == 0 => format!("{} days", s / 86_400),
        s => format!("{} h", s.div_ceil(3600)),
    }
}

/// "plus" becomes "Plus". An empty string has no plan name.
fn capitalise(s: &str) -> Option<String> {
    let mut chars = s.chars();
    let first = chars.next()?;
    Some(first.to_uppercase().chain(chars).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::test_env;
    use crate::net::HttpClient;
    use crate::testutil::MockServer;
    use std::path::PathBuf;

    const AUTH: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/codex_auth.json"
    ));
    const USAGE_FREE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/codex_usage_free.json"
    ));
    const USAGE_REVIEW: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/codex_usage_code_review.json"
    ));

    // Unsigned synthetic JWTs, built outside this crate: header {"alg":"none","typ":"JWT"} and the payloads below.
    // The id token (and the access token in codex_auth.json) carry exp 1893456000 (2030-01-01T00:00:00Z).
    const ID_TOKEN: &str = "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.eyJlbWFpbCI6InVzZXJAZXhhbXBsZS5jb20iLCJleHAiOjE4OTM0NTYwMDAsInN1YiI6InVzZXItU1lOVEhFVElDMDAwMSJ9.sig";
    // {"sub":"x"}: no exp.
    const NO_EXP_TOKEN: &str = "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.eyJzdWIiOiJ4In0.sig";
    // {"exp":1893456000,"sub":"user-SYNTHETIC0001"}: the access token in codex_auth.json.
    const ACCESS_TOKEN: &str = "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.eyJleHAiOjE4OTM0NTYwMDAsInN1YiI6InVzZXItU1lOVEhFVElDMDAwMSJ9.sig";
    const ACCOUNT_ID: &str = "00000000-0000-0000-0000-000000000003";
    const EXP_MS: i64 = 1_893_456_000_000;

    fn now() -> i64 {
        1_794_063_600_000 // 2026-11-07T15:00:00Z, before the token expires
    }

    fn after_expiry() -> i64 {
        1_924_992_000_000 // 2031-01-01T00:00:00Z
    }

    /// A throwaway home folder with an empty `.codex` inside. Removed on drop, so a failed assertion cannot leave it behind.
    struct TempHome(PathBuf);

    impl TempHome {
        fn new(name: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("qg-test-{}-{}", std::process::id(), name));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join(".codex")).unwrap();
            Self(root)
        }

        fn write(&self, relative: &str, content: &str) {
            std::fs::write(self.0.join(relative), content).unwrap();
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

    const LOCAL_ID: &str = "codex-current";

    /// The source for the default `.codex` folder inside `home`.
    fn source(home: &TempHome) -> CodexDirect {
        CodexDirect {
            dir: home.0.join(".codex"),
            local_id: LOCAL_ID.into(),
        }
    }

    fn key() -> AccountKey {
        AccountKey::new(Service::Codex, Some("user@example.com"), LOCAL_ID)
    }

    fn parse(body: &str) -> Result<Reading, FetchError> {
        parse_usage(body.as_bytes(), key(), "Codex".into(), now())
    }

    const USAGE_RESETS: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/codex_usage_with_resets.json"
    ));

    #[test]
    fn banked_resets_read_when_present() {
        let reading = parse(USAGE_RESETS).unwrap();
        assert_eq!(reading.banked_resets, Some(1));
        assert_eq!(reading.buckets.len(), 3);
        // A count beyond u32 saturates instead of failing.
        let big = parse(
            r#"{"rate_limit":{"primary_window":{"used_percent":5,"limit_window_seconds":18000}},"rate_limit_reset_credits":{"available_count":99999999999}}"#,
        )
        .unwrap();
        assert_eq!(big.banked_resets, Some(u32::MAX));
    }

    #[test]
    fn banked_resets_absent_is_none() {
        assert_eq!(parse(USAGE_REVIEW).unwrap().banked_resets, None);
        for credits in [
            "null",
            "{}",
            r#"{"available_count":null}"#,
            r#"{"available_count":"3"}"#,
            r#"{"available_count":-1}"#,
            r#"{"available_count":1.5}"#,
            r#""x""#,
            "[]",
        ] {
            let body = format!(
                r#"{{"rate_limit":{{"primary_window":{{"used_percent":5,"limit_window_seconds":18000}}}},"rate_limit_reset_credits":{credits}}}"#
            );
            assert_eq!(parse(&body).unwrap().banked_resets, None, "{credits}");
        }
    }

    #[tokio::test]
    async fn label_is_never_email() {
        let home = TempHome::new("codex-label");
        home.write(".codex/auth.json", AUTH);
        let server = MockServer::start(vec![(200, "", USAGE_FREE.to_string())]);
        let env = env_for(&home, &server.base, now);

        let readings = source(&home).fetch(&env).await.unwrap();

        assert_eq!(readings[0].label, "Codex");
        assert!(!readings[0].label.contains('@'));
        // The email still identifies the account.
        assert_eq!(readings[0].key, key());
    }

    #[tokio::test]
    async fn profile_dir_is_used() {
        let home = TempHome::new("codex-profile");
        let profile = home.0.join("profile-a");
        let empty = home.0.join("profile-b");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::create_dir_all(&empty).unwrap();
        std::fs::write(profile.join("auth.json"), AUTH).unwrap();
        let server = MockServer::start(vec![(200, "", USAGE_FREE.to_string())]);
        let env = env_for(&home, &server.base, now);
        let in_profile = CodexDirect {
            dir: profile,
            local_id: "profile-a".into(),
        };
        let in_empty = CodexDirect {
            dir: empty,
            local_id: "profile-b".into(),
        };

        assert!(in_profile.detect(&env));
        assert!(!in_empty.detect(&env));
        // The default `.codex` folder is empty, so a reading proves the profile folder was read.
        assert!(!source(&home).detect(&env));
        let readings = in_profile.fetch(&env).await.unwrap();
        assert_eq!(readings[0].key.local_id, "profile-a");
        assert_eq!(in_empty.fetch(&env).await, Err(FetchError::NotDetected));
    }

    fn expected(name: &str, used_percent: f64, window_secs: u64, resets_at_ms: i64) -> Bucket {
        Bucket {
            name: name.into(),
            used_percent,
            window_secs: Some(window_secs),
            resets_at_ms: Some(resets_at_ms),
        }
    }

    #[tokio::test]
    async fn codex_free_thirty_day_window() {
        let home = TempHome::new("codex-free");
        home.write(".codex/auth.json", AUTH);
        let server = MockServer::start(vec![(200, "", USAGE_FREE.to_string())]);
        let env = env_for(&home, &server.base, now);

        let readings = source(&home).fetch(&env).await.unwrap();

        assert_eq!(readings.len(), 1);
        let reading = &readings[0];
        assert_eq!(
            reading.buckets,
            vec![expected("30 days", 52.0, 2_592_000, 1_796_040_000_000)]
        );
        assert_eq!(reading.plan.as_deref(), Some("Free"));
        assert_eq!(reading.key, key());
        assert_eq!(reading.label, "Codex");
        assert_eq!(reading.source, SourceKind::Direct);
        assert_eq!(reading.observed_at_ms, now());
    }

    #[test]
    fn codex_code_review_bucket() {
        let reading = parse(USAGE_REVIEW).unwrap();
        assert_eq!(
            reading.buckets,
            vec![
                expected("5 hour", 18.0, 18_000, 1_794_076_200_000),
                expected("Weekly", 40.0, 604_800, 1_794_474_000_000),
                expected("Code review", 7.5, 604_800, 1_794_560_400_000),
            ]
        );
        assert_eq!(reading.plan.as_deref(), Some("Plus"));
    }

    #[test]
    fn codex_window_names() {
        assert_eq!(window_name(18_000), "5 hour");
        assert_eq!(window_name(604_800), "Weekly");
        assert_eq!(window_name(2_592_000), "30 days");
        assert_eq!(window_name(172_800), "2 days");
        assert_eq!(window_name(7_200), "2 h");
        assert_eq!(window_name(5_400), "2 h");
    }

    #[test]
    fn codex_unusable_bodies_are_format_errors() {
        for (body, field) in [
            ("not json", "usage"),
            ("[]", "usage"),
            ("{}", "no windows"),
            (
                r#"{"rate_limit":{"primary_window":null,"secondary_window":null},"code_review_rate_limit":null}"#,
                "no windows",
            ),
            (
                r#"{"rate_limit":{"primary_window":{"used_percent":-1,"limit_window_seconds":18000}}}"#,
                "primary_window",
            ),
            (
                r#"{"rate_limit":{"primary_window":{"limit_window_seconds":18000}}}"#,
                "primary_window",
            ),
            (
                r#"{"rate_limit":{"primary_window":{"used_percent":5}}}"#,
                "limit_window_seconds",
            ),
            (
                r#"{"rate_limit":{"primary_window":{"used_percent":5,"limit_window_seconds":0}}}"#,
                "limit_window_seconds",
            ),
            (
                r#"{"rate_limit":{"primary_window":{"used_percent":5,"limit_window_seconds":18000,"reset_at":"soon"}}}"#,
                "reset_at",
            ),
        ] {
            assert_eq!(parse(body), Err(FetchError::Format(field)), "{body}");
        }
    }

    #[test]
    fn codex_over_100_clamps_and_null_reset_is_none() {
        let reading =
            parse(r#"{"rate_limit":{"primary_window":{"used_percent":130,"limit_window_seconds":18000,"reset_at":null}}}"#)
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
        assert_eq!(reading.plan, None);
    }

    #[test]
    fn codex_email_from_id_token() {
        assert_eq!(
            super::email_from_id_token(ID_TOKEN).as_deref(),
            Some("user@example.com")
        );
        assert_eq!(super::email_from_id_token(NO_EXP_TOKEN), None);
        assert_eq!(super::email_from_id_token("not-a-jwt"), None);
        assert_eq!(super::email_from_id_token("a.%%%.c"), None);
        // Padded base64 is accepted as well.
        assert_eq!(
            super::email_from_id_token("h.eyJlbWFpbCI6ImFAYi5jIn0=.s").as_deref(),
            Some("a@b.c")
        );
        assert_eq!(jwt_exp_ms(ID_TOKEN), Some(EXP_MS));
        assert_eq!(jwt_exp_ms(NO_EXP_TOKEN), None);
    }

    #[tokio::test]
    async fn codex_expired_never_calls_network() {
        let home = TempHome::new("codex-expired");
        home.write(".codex/auth.json", AUTH);
        let server = MockServer::start(vec![(200, "", USAGE_FREE.to_string())]);
        let env = env_for(&home, &server.base, after_expiry);

        let result = source(&home).fetch(&env).await;

        assert_eq!(result, Err(FetchError::Expired));
        assert_eq!(server.hits(), 0);
    }

    #[tokio::test]
    async fn codex_token_without_exp_is_format_error_and_never_calls_network() {
        let home = TempHome::new("codex-no-exp");
        home.write(
            ".codex/auth.json",
            &format!(r#"{{"tokens":{{"access_token":"{NO_EXP_TOKEN}"}}}}"#),
        );
        let server = MockServer::start(vec![(200, "", USAGE_FREE.to_string())]);
        let env = env_for(&home, &server.base, now);

        let result = source(&home).fetch(&env).await;

        assert_eq!(result, Err(FetchError::Format("token exp")));
        assert_eq!(server.hits(), 0);
    }

    #[tokio::test]
    async fn codex_missing_auth_json_is_not_detected() {
        let home = TempHome::new("codex-missing");
        let env = env_for(&home, "http://127.0.0.1:9", now);
        assert!(!source(&home).detect(&env));
        assert_eq!(
            source(&home).fetch(&env).await,
            Err(FetchError::NotDetected)
        );

        home.write(".codex/auth.json", AUTH);
        assert!(source(&home).detect(&env));
    }

    #[test]
    fn codex_incomplete_auth_json_is_format_error() {
        let home = TempHome::new("codex-incomplete");
        for (content, field) in [
            (r#"{"tokens":{"access_to"#, "auth"),
            (r#"{}"#, "tokens"),
            (r#"{"tokens":{"id_token":"x"}}"#, "access_token"),
            (r#"{"tokens":{"access_token":""}}"#, "access_token"),
        ] {
            home.write(".codex/auth.json", content);
            assert_eq!(
                read_auth(&home.0.join(".codex")).err(),
                Some(FetchError::Format(field)),
                "{content}"
            );
        }
    }

    #[tokio::test]
    async fn codex_sends_account_id_header() {
        let home = TempHome::new("codex-headers");
        home.write(".codex/auth.json", AUTH);
        let server = MockServer::start(vec![(200, "", USAGE_REVIEW.to_string())]);
        let env = env_for(&home, &server.base, now);

        source(&home).fetch(&env).await.unwrap();

        let request = server.last_request.lock().unwrap().to_lowercase();
        assert!(request.starts_with("get /backend-api/wham/usage "));
        assert!(request.contains(&format!("chatgpt-account-id: {ACCOUNT_ID}")));
        assert!(request.contains(&format!(
            "authorization: bearer {}",
            ACCESS_TOKEN.to_lowercase()
        )));
        assert!(request.contains("accept: application/json"));
        assert_eq!(server.hits(), 1);
    }

    #[tokio::test]
    async fn codex_without_account_id_or_id_token_still_fetches() {
        let home = TempHome::new("codex-bare");
        home.write(
            ".codex/auth.json",
            &format!(r#"{{"tokens":{{"access_token":"{ACCESS_TOKEN}"}}}}"#),
        );
        let server = MockServer::start(vec![(200, "", USAGE_FREE.to_string())]);
        let env = env_for(&home, &server.base, now);

        let readings = source(&home).fetch(&env).await.unwrap();

        let request = server.last_request.lock().unwrap().to_lowercase();
        assert!(!request.contains("chatgpt-account-id"));
        assert_eq!(readings[0].label, "Codex");
        assert_eq!(readings[0].key.email, None);
    }
}
