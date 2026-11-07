use crate::model::Bucket;
use crate::view::Freshness;

/// Stale: the window already reset, or the reading is older than the window itself.
/// Aged: older than two refresh intervals. Otherwise live.
pub fn classify(observed_at_ms: i64, refresh_secs: u64, bucket: &Bucket, now_ms: i64) -> Freshness {
    let age_ms = now_ms.saturating_sub(observed_at_ms).max(0);
    if bucket.resets_at_ms.is_some_and(|r| r <= now_ms) {
        return Freshness::Stale;
    }
    if bucket
        .window_secs
        .is_some_and(|w| age_ms > (w as i64).saturating_mul(1000))
    {
        return Freshness::Stale;
    }
    if age_ms > (refresh_secs as i64).saturating_mul(2000) {
        return Freshness::Aged;
    }
    Freshness::Live
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;

    fn bucket(window: u64, resets_in_ms: i64) -> Bucket {
        Bucket {
            name: "b".into(),
            used_percent: 10.0,
            window_secs: Some(window),
            resets_at_ms: Some(NOW + resets_in_ms),
        }
    }

    #[test]
    fn fresh_is_live() {
        assert_eq!(
            classify(NOW - 30_000, 120, &bucket(18_000, 3_600_000), NOW),
            Freshness::Live
        );
    }

    #[test]
    fn reset_in_past_is_stale() {
        assert_eq!(
            classify(NOW, 120, &bucket(18_000, -1), NOW),
            Freshness::Stale
        );
    }

    #[test]
    fn aged_after_two_refreshes() {
        assert_eq!(
            classify(NOW - 300_000, 120, &bucket(18_000, 3_600_000), NOW),
            Freshness::Aged
        );
    }

    #[test]
    fn stale_after_window_length() {
        // A 5 h bucket observed 6 h ago whose reported reset is still ahead (clock skew).
        assert_eq!(
            classify(NOW - 6 * 3_600_000, 120, &bucket(18_000, 60_000), NOW),
            Freshness::Stale
        );
    }

    #[test]
    fn unknown_window_and_reset_only_ages() {
        let b = Bucket {
            name: "b".into(),
            used_percent: 1.0,
            window_secs: None,
            resets_at_ms: None,
        };
        assert_eq!(classify(NOW - 86_400_000, 120, &b, NOW), Freshness::Aged);
    }
}
