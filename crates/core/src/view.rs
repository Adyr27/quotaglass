//! The token-free view model sent to the UI. Everything here is `Serialize`; nothing here holds a secret.

use crate::model::{Service, SourceKind};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Freshness {
    Live,
    Aged,
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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

#[derive(Debug, Clone, PartialEq, Serialize)]
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

#[derive(Debug, Clone, PartialEq, Serialize)]
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
#[derive(Debug, Clone, PartialEq, Serialize)]
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

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub accounts: Vec<AccountView>,
    /// Only services with at least one visible account, in `Service` order.
    pub services: Vec<ServiceView>,
    pub generated_at_ms: i64,
}
