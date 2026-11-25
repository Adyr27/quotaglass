//! The token-free view model sent to the UI. Everything here is `Serialize` + `Deserialize`; nothing here holds a secret.

use crate::model::{Service, SourceKind};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Freshness {
    Live,
    Aged,
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CardState {
    Ok,
    NoReadingYet,
    Error,
    NoSource,
    /// The login of a Claude or Codex account ran out. The user opens the CLI to sign in again.
    LoginExpired,
    /// The profile folder of an added account is gone.
    ProfileMissing,
    /// The service answered HTTP 429. The last good reading (if any) stays on the card.
    RateLimited,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BucketView {
    pub name: String,
    pub used_percent: f64,
    pub window_secs: Option<u64>,
    pub resets_at_ms: Option<i64>,
    pub limit_reached: bool,
    pub freshness: Freshness,
    /// Share of the window that has passed, 0..=100. `None` without a window length and a reset time.
    pub passed_percent: Option<f64>,
    /// Usage runs well ahead of the clock (`pace::fast_burn`).
    pub fast_burn: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    /// `AccountKey::store_key()`; stable across refreshes.
    pub id: String,
    pub service: Service,
    /// The user's label when set, else the source's default. Never an email.
    pub label: String,
    pub plan: Option<String>,
    pub source: Option<SourceKind>,
    /// Freshness of the headline bucket; `Stale` when there is no headline.
    pub freshness: Freshness,
    pub age_secs: Option<u64>,
    pub headline: Option<BucketView>,
    pub buckets: Vec<BucketView>,
    pub state: CardState,
    pub reason: Option<String>,
    /// `100 - max(used %)` over the buckets that are not stale.
    pub room_left: Option<f64>,
    /// Minutes until the first limit runs out at this pace (`pace::minutes_to_limit`). Only from a live reading.
    pub forecast_mins: Option<u32>,
    /// The account with the most room left now (`pace::best_now`).
    pub best_now: bool,
    /// Codex only: banked rate-limit resets. `None` when the source does not say.
    pub banked_resets: Option<u32>,
    /// The ID of the added account (its profile) that reads this account.
    pub profile_id: Option<String>,
    /// True when the current CLI or app login reads this account.
    pub current_login: bool,
    pub hidden: bool,
}

/// One pill ring: the best available account of one service (`summary::build`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceView {
    pub service: Service,
    /// The lowest tightest-limit percent among the counted accounts. `None` when no account counts.
    pub used_percent: Option<f64>,
    /// Every counted account has its tightest limit at 100%.
    pub all_exhausted: bool,
    /// Visible (not hidden) accounts of the service.
    pub accounts: u32,
    /// A visible account of the service is in `CardState::RateLimited`.
    pub rate_limited: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub accounts: Vec<AccountView>,
    /// Only services with at least one visible account, in `Service` order.
    pub services: Vec<ServiceView>,
    pub generated_at_ms: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_round_trips_json_without_secrets() {
        let headline = BucketView {
            name: "5-hour".to_string(),
            used_percent: 42.5,
            window_secs: Some(18_000),
            resets_at_ms: Some(1_794_160_000_000),
            limit_reached: false,
            freshness: Freshness::Live,
            passed_percent: Some(50.0),
            fast_burn: false,
        };
        let weekly = BucketView {
            name: "Weekly".to_string(),
            used_percent: 100.0,
            window_secs: Some(604_800),
            resets_at_ms: None,
            limit_reached: true,
            freshness: Freshness::Aged,
            passed_percent: None,
            fast_burn: true,
        };
        let snapshot = Snapshot {
            accounts: vec![
                AccountView {
                    id: "claude:0123456789abcdef".to_string(),
                    service: Service::Claude,
                    label: "Work".to_string(),
                    plan: Some("Max".to_string()),
                    source: Some(SourceKind::Direct),
                    freshness: Freshness::Live,
                    age_secs: Some(12),
                    headline: Some(headline.clone()),
                    buckets: vec![headline, weekly],
                    state: CardState::Ok,
                    reason: None,
                    room_left: Some(0.0),
                    forecast_mins: Some(15),
                    best_now: false,
                    banked_resets: None,
                    profile_id: Some("work".to_string()),
                    current_login: true,
                    hidden: false,
                },
                AccountView {
                    id: "codex:fedcba9876543210".to_string(),
                    service: Service::Codex,
                    label: "Codex".to_string(),
                    plan: Some("Plus".to_string()),
                    source: Some(SourceKind::Direct),
                    freshness: Freshness::Stale,
                    age_secs: Some(900),
                    headline: None,
                    buckets: Vec::new(),
                    state: CardState::RateLimited,
                    reason: Some("Rate limited — retrying in 5m".to_string()),
                    room_left: None,
                    forecast_mins: None,
                    best_now: true,
                    banked_resets: Some(2),
                    profile_id: None,
                    current_login: false,
                    hidden: false,
                },
            ],
            services: vec![
                ServiceView {
                    service: Service::Claude,
                    used_percent: Some(42.5),
                    all_exhausted: false,
                    accounts: 1,
                    rate_limited: false,
                },
                ServiceView {
                    service: Service::Antigravity,
                    used_percent: None,
                    all_exhausted: false,
                    accounts: 1,
                    rate_limited: false,
                },
            ],
            generated_at_ms: 1_794_150_000_000,
        };

        let json = serde_json::to_string(&snapshot).expect("serialize snapshot");
        assert!(json.contains("\"generatedAtMs\""));
        assert!(json.contains("\"rateLimited\""));
        assert!(json.contains("\"bankedResets\":2"));
        assert!(!json.contains("token") && !json.contains("email"));

        let decoded: Snapshot = serde_json::from_str(&json).expect("deserialize snapshot");
        assert_eq!(decoded, snapshot);
    }
}
