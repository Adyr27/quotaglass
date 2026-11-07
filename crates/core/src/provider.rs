use crate::env::Env;
use crate::model::{FetchError, Reading, Service, SourceKind};
use std::future::Future;
use std::pin::Pin;

pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// One data source. Each source module implements this; the app runs them on a schedule.
pub trait Provider: Send + Sync {
    fn kind(&self) -> SourceKind;
    /// The one service this source covers, if it covers exactly one (Claude direct, Codex direct).
    /// Lets the UI show an "expired login" card when the source fails and nothing else knows the account.
    fn service(&self) -> Option<Service> {
        None
    }
    /// True when this source can apply on this machine (its file or tool exists). Must be cheap.
    fn detect(&self, env: &Env) -> bool;
    fn fetch<'a>(&'a self, env: &'a Env) -> BoxFut<'a, Result<Vec<Reading>, FetchError>>;
}

#[derive(Debug, Clone)]
pub struct SourceOutcome {
    pub kind: SourceKind,
    /// Copied from `Provider::service()`.
    pub service: Option<Service>,
    pub attempted_at_ms: i64,
    pub result: Result<Vec<Reading>, FetchError>,
}
