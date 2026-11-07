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
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    /// `AccountKey::store_key()`; stable across refreshes.
    pub id: String,
    pub service: Service,
    pub label: String,
    pub plan: Option<String>,
    pub source: Option<SourceKind>,
    /// Every source that has a reading for this account, in priority order. Used by the source picker.
    pub sources: Vec<SourceKind>,
    /// Freshness of the headline bucket; `Stale` when there is no headline.
    pub freshness: Freshness,
    pub age_secs: Option<u64>,
    pub headline: Option<BucketView>,
    pub extra_buckets: u32,
    pub buckets: Vec<BucketView>,
    pub state: CardState,
    pub reason: Option<String>,
    /// Set only when a sign-in tool for this service is installed (Claude, Codex).
    pub connect: Option<Service>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub accounts: Vec<AccountView>,
    pub generated_at_ms: i64,
}
