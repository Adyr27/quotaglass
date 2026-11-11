//! How long to wait after a rate-limited (HTTP 429) answer.

/// The longest wait after a 429, whatever `Retry-After` says.
pub const RATE_LIMIT_CAP_SECS: u64 = 900;

/// The wait after a 429: the server's `Retry-After` when it gave one, never shorter than the task's normal interval
/// (`base_secs`), and never longer than the cap (unless the base itself is longer).
pub fn rate_limit_wait_secs(retry_after: Option<u64>, base_secs: u64) -> u64 {
    retry_after
        .unwrap_or(base_secs)
        .clamp(base_secs, RATE_LIMIT_CAP_SECS.max(base_secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wait_clamps() {
        assert_eq!(rate_limit_wait_secs(Some(60), 300), 300);
        assert_eq!(rate_limit_wait_secs(Some(3600), 300), 900);
        assert_eq!(rate_limit_wait_secs(None, 300), 300);
        assert_eq!(rate_limit_wait_secs(Some(120), 60), 120);
    }
}
