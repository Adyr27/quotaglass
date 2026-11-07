use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Service {
    Claude,
    Codex,
    Antigravity,
}

impl Service {
    pub fn as_str(self) -> &'static str {
        match self {
            Service::Claude => "claude",
            Service::Codex => "codex",
            Service::Antigravity => "antigravity",
        }
    }
}

/// Declaration order is the merge priority: Direct beats Orca beats Omp beats SessionKey.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SourceKind {
    Direct,
    Orca,
    Omp,
    SessionKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AccountKey {
    pub service: Service,
    /// Lowercased and trimmed. `None` means the account is never merged with another reading.
    pub email: Option<String>,
    /// Distinguishes email-less accounts, e.g. "claude-direct" or an Orca account id.
    pub local_id: String,
}

impl AccountKey {
    pub fn new(service: Service, email: Option<&str>, local_id: impl Into<String>) -> Self {
        let email = email
            .map(|e| e.trim().to_lowercase())
            .filter(|e| !e.is_empty());
        Self {
            service,
            email,
            local_id: local_id.into(),
        }
    }

    /// Key used to merge readings across sources. Only accounts with an email can merge.
    pub fn merge_key(&self) -> Option<String> {
        self.email
            .as_ref()
            .map(|e| format!("{}:{}", self.service.as_str(), e))
    }

    /// Key used for the in-memory last-reading store and for the UI card id.
    pub fn store_key(&self) -> String {
        match &self.email {
            Some(e) => format!("{}:{}", self.service.as_str(), e),
            None => format!("{}:{}", self.service.as_str(), self.local_id),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Bucket {
    pub name: String,
    /// 0.0..=100.0, already validated by `checked_percent`.
    pub used_percent: f64,
    pub window_secs: Option<u64>,
    pub resets_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub key: AccountKey,
    pub label: String,
    pub plan: Option<String>,
    /// Empty means the source knows the account but has no reading yet.
    pub buckets: Vec<Bucket>,
    pub source: SourceKind,
    pub observed_at_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetErrorKind {
    Status(u16),
    Connect,
    /// A redirect or a non-allowlisted target. Never followed.
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    NotDetected,
    Expired,
    Disabled,
    Unavailable(&'static str),
    Format(&'static str),
    Network(NetErrorKind),
    Timeout,
}

impl FetchError {
    /// Fixed user-facing text. Never includes response bodies, headers or tokens.
    pub fn reason(&self) -> String {
        match self {
            FetchError::NotDetected => "Not found on this PC".into(),
            FetchError::Expired => "Login expired. Open the CLI once to refresh it.".into(),
            FetchError::Disabled => "Source turned off in settings".into(),
            FetchError::Unavailable(why) => (*why).into(),
            FetchError::Format(what) => format!("Unexpected data from the source ({what})"),
            FetchError::Network(NetErrorKind::Status(code)) => {
                format!("The service answered HTTP {code}")
            }
            FetchError::Network(NetErrorKind::Connect) => "Could not reach the service".into(),
            FetchError::Network(NetErrorKind::Blocked) => {
                "Blocked a redirect away from the service".into()
            }
            FetchError::Timeout => "The source took too long to answer".into(),
        }
    }
}

/// Validates a usage percentage from any source.
/// Missing, non-finite or negative values are a format error; values over 100 clamp to 100.
pub fn checked_percent(value: Option<f64>, field: &'static str) -> Result<f64, FetchError> {
    match value {
        Some(v) if v.is_finite() && v >= 0.0 => Ok(v.min(100.0)),
        _ => Err(FetchError::Format(field)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_kind_priority() {
        assert!(SourceKind::Direct < SourceKind::Orca);
        assert!(SourceKind::Orca < SourceKind::Omp);
        assert!(SourceKind::Omp < SourceKind::SessionKey);
    }

    #[test]
    fn email_is_normalised_and_empty_is_none() {
        let k = AccountKey::new(Service::Claude, Some("  User@Example.com "), "x");
        assert_eq!(k.merge_key().as_deref(), Some("claude:user@example.com"));
        let k = AccountKey::new(Service::Codex, Some(""), "codex-direct");
        assert_eq!(k.merge_key(), None);
        assert_eq!(k.store_key(), "codex:codex-direct");
    }

    #[test]
    fn percent_rules() {
        assert_eq!(checked_percent(Some(42.0), "p"), Ok(42.0));
        assert_eq!(checked_percent(Some(112.0), "p"), Ok(100.0));
        assert_eq!(
            checked_percent(Some(-1.0), "p"),
            Err(FetchError::Format("p"))
        );
        assert_eq!(checked_percent(None, "p"), Err(FetchError::Format("p")));
        assert_eq!(
            checked_percent(Some(f64::NAN), "p"),
            Err(FetchError::Format("p"))
        );
    }
}
