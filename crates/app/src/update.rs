//! Lightweight daily GitHub Releases check (opt-out via `settings.check_updates`).
//! HTTPS-only, no proxy, no redirects, 10s timeout, 256 KiB response cap.
//! Never downloads or runs installers; UI opens `UrlTarget::GitHubReleases` on user click.

use crate::scheduler::run_loop_with;
use serde::Deserialize;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{watch, Notify};
use tokio::task::JoinHandle;

pub const RELEASES_API_URL: &str = "https://api.github.com/repos/Adyr27/quotaglass/releases/latest";
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const USER_AGENT: &str = concat!("QuotaGlass/", env!("CARGO_PKG_VERSION"));
pub const MAX_BODY_BYTES: usize = 256 * 1024;
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
pub const DAILY_SECS: u64 = 24 * 3600;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SemVer {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub is_prerelease: bool,
}

/// Parses `"v0.5.1"` or `"0.5.1"` (ignoring `+build` metadata, tracking `-prerelease`).
pub fn parse_semver(raw: &str) -> Option<SemVer> {
    let trimmed = raw.trim();
    let stripped = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed);
    let without_build = stripped.split('+').next()?;
    let (core, pre) = match without_build.split_once('-') {
        Some((c, p)) if !p.is_empty() => (c, true),
        Some((_, _)) => return None,
        None => (without_build, false),
    };
    let mut parts = core.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next()?.parse::<u64>().ok()?;
    let patch = parts.next()?.parse::<u64>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(SemVer {
        major,
        minor,
        patch,
        is_prerelease: pre,
    })
}

/// True when `candidate` is a valid non-prerelease version strictly newer than `current`.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    let (Some(cand), Some(curr)) = (parse_semver(candidate), parse_semver(current)) else {
        return false;
    };
    if cand.is_prerelease {
        return false;
    }
    (cand.major, cand.minor, cand.patch) > (curr.major, curr.minor, curr.patch)
        || ((cand.major, cand.minor, cand.patch) == (curr.major, curr.minor, curr.patch)
            && curr.is_prerelease)
}

#[derive(Deserialize)]
struct LatestRelease {
    tag_name: String,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
}

/// Extracts a clean release tag from GitHub's `/releases/latest` JSON payload.
pub fn parse_release_tag(body: &[u8]) -> Option<String> {
    let parsed: LatestRelease = serde_json::from_slice(body).ok()?;
    if parsed.draft || parsed.prerelease {
        return None;
    }
    let tag = parsed.tag_name.trim();
    if tag.is_empty() || tag.len() > 64 || tag.chars().any(char::is_control) {
        return None;
    }
    let _ = parse_semver(tag)?;
    Some(
        tag.strip_prefix('v')
            .or_else(|| tag.strip_prefix('V'))
            .unwrap_or(tag)
            .to_string(),
    )
}

pub fn build_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(REQUEST_TIMEOUT)
        .user_agent(USER_AGENT)
        .build()
}

pub async fn fetch_latest_from(
    client: &reqwest::Client,
    url: &str,
    current_version: &str,
) -> Option<String> {
    let mut resp = client
        .get(url)
        .header("accept", "application/vnd.github+json")
        .send()
        .await
        .ok()?;
    let status = resp.status();
    if status.is_redirection() || !status.is_success() {
        return None;
    }
    if resp
        .content_length()
        .is_some_and(|n| n as usize > MAX_BODY_BYTES)
    {
        return None;
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.ok()? {
        if body.len() + chunk.len() > MAX_BODY_BYTES {
            return None;
        }
        body.extend_from_slice(&chunk);
    }
    let version = parse_release_tag(&body)?;
    is_newer(&version, current_version).then_some(version)
}

pub async fn check_once() -> Option<String> {
    let client = build_client().ok()?;
    fetch_latest_from(&client, RELEASES_API_URL, CURRENT_VERSION).await
}

/// Runs `check` at start and every 24h ONLY while `enabled()` returns true.
pub async fn run_daily<E, C, Fut>(enabled: E, wake: Arc<Notify>, mut check: C)
where
    E: Fn() -> bool,
    C: FnMut() -> Fut,
    Fut: Future<Output = ()>,
{
    run_loop_with(DAILY_SECS, wake, move || {
        let pending = enabled().then(&mut check);
        async move {
            if let Some(fut) = pending {
                fut.await;
            }
        }
    })
    .await;
}

/// Spawns the daily release check task controlled by `enabled` and an optional wake trigger
/// (e.g. when the user toggles `check_updates` back on in Settings).
pub fn spawn_daily(
    enabled: Arc<AtomicBool>,
) -> (watch::Receiver<Option<String>>, Arc<Notify>, JoinHandle<()>) {
    let (tx, rx) = watch::channel(None);
    let wake = Arc::new(Notify::new());
    let wake_task = wake.clone();
    let handle = tokio::spawn(run_daily(
        move || enabled.load(Ordering::Acquire),
        wake_task,
        move || {
            let tx = tx.clone();
            async move {
                if let Some(found) = check_once().await {
                    let _ = tx.send(Some(found));
                }
            }
        },
    ));
    (rx, wake, handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    async fn settle() {
        for _ in 0..6 {
            tokio::task::yield_now().await;
        }
    }

    #[test]
    fn semver_parsing_and_comparison() {
        assert!(is_newer("0.5.1", "0.5.0"));
        assert!(is_newer("v0.5.1", "0.5.0"));
        assert!(is_newer("V1.0.0", "0.9.9"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("0.5.0", "0.5.0-rc.1"));

        assert!(!is_newer("v0.5.0", "0.5.0"));
        assert!(!is_newer("0.4.9", "0.5.0"));
        assert!(!is_newer("0.5.1-beta.1", "0.5.0"));
        assert!(!is_newer("not-a-version", "0.5.0"));
        assert!(!is_newer("0.5", "0.5.0"));
        assert_eq!(parse_semver("v0.5.1+sha.abc").unwrap().patch, 1);
    }

    #[test]
    fn parse_release_tag_handles_valid_draft_prerelease_and_malformed() {
        assert_eq!(
            parse_release_tag(br#"{"tag_name":"v0.5.2","draft":false,"prerelease":false}"#),
            Some("0.5.2".to_string())
        );
        assert_eq!(
            parse_release_tag(br#"{"tag_name":"v0.6.0","draft":true,"prerelease":false}"#),
            None
        );
        assert_eq!(
            parse_release_tag(br#"{"tag_name":"v0.6.0-rc.1","draft":false,"prerelease":true}"#),
            None
        );
        assert_eq!(parse_release_tag(br#"{"tag_name":"bad\n"}"#), None);
        assert_eq!(parse_release_tag(br#"{"other":1}"#), None);
    }

    #[test]
    fn releases_url_is_https_and_user_agent_matches_version() {
        assert!(RELEASES_API_URL.starts_with("https://api.github.com/repos/Adyr27/quotaglass/"));
        assert_eq!(USER_AGENT, "QuotaGlass/0.5.0");
        assert!(build_client().is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn opt_out_silences_all_update_checks() {
        let on = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(AtomicUsize::new(0));
        let wake = Arc::new(Notify::new());
        let (flag, counter, w) = (on.clone(), calls.clone(), wake.clone());

        let task = tokio::spawn(run_daily(
            move || flag.load(Ordering::SeqCst),
            w,
            move || {
                let counter = counter.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                }
            },
        ));

        settle().await;
        tokio::time::advance(Duration::from_secs(3 * DAILY_SECS)).await;
        settle().await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "opt-out makes zero requests"
        );

        on.store(true, Ordering::SeqCst);
        wake.notify_one();
        settle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        tokio::time::advance(Duration::from_secs(DAILY_SECS)).await;
        settle().await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        task.abort();
    }
}
