//! QuotaGlass core: data model, sources, merge rules and the HTTPS allowlist.
//!
//! Tokens never leave this crate. Every type that crosses into the app crate's
//! IPC layer lives in `view` and carries numbers and labels only.

pub mod env;
pub mod freshness;
pub mod merge;
pub mod model;
pub mod net;
pub mod provider;
pub mod secret;
pub mod sources;
pub mod view;

#[cfg(test)]
pub(crate) mod testutil;

pub use env::Env;
pub use model::{
    checked_percent, AccountKey, Bucket, FetchError, NetErrorKind, Reading, Service, SourceKind,
};
pub use net::{HeaderValueSource, Host, HttpClient};
pub use provider::{BoxFut, Provider, SourceOutcome};
pub use secret::Secret;
