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

/// Deterministic startup delay in seconds so multi-account polls do not fire in a simultaneous burst.
/// Returns `0` for the first slot (`index == 0`), single-account configs (`total <= 1`), or `base_secs == 0`.
/// Clamps out-of-range indices and guarantees the result is strictly `< base_secs` whenever `base_secs > 0`.
pub fn stagger_offset_secs(index: usize, total: usize, base_secs: u64) -> u64 {
    if index == 0 || total <= 1 || base_secs == 0 {
        return 0;
    }
    let slot = index.min(total - 1) as u128;
    let offset = (slot * u128::from(base_secs)) / (total as u128);
    (offset as u64).min(base_secs - 1)
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

    #[test]
    fn stagger_edge_cases_return_zero() {
        assert_eq!(stagger_offset_secs(0, 0, 300), 0);
        assert_eq!(stagger_offset_secs(2, 0, 300), 0);
        assert_eq!(stagger_offset_secs(0, 1, 300), 0);
        assert_eq!(stagger_offset_secs(1, 1, 300), 0);
        assert_eq!(stagger_offset_secs(0, 5, 300), 0);
        assert_eq!(stagger_offset_secs(1, 3, 0), 0);
        assert_eq!(stagger_offset_secs(4, 5, 0), 0);
    }

    #[test]
    fn stagger_spreads_accounts_evenly_across_base() {
        assert_eq!(
            (0..2)
                .map(|i| stagger_offset_secs(i, 2, 300))
                .collect::<Vec<_>>(),
            vec![0, 150]
        );
        assert_eq!(
            (0..3)
                .map(|i| stagger_offset_secs(i, 3, 300))
                .collect::<Vec<_>>(),
            vec![0, 100, 200]
        );
        assert_eq!(
            (0..5)
                .map(|i| stagger_offset_secs(i, 5, 300))
                .collect::<Vec<_>>(),
            vec![0, 60, 120, 180, 240]
        );
    }

    #[test]
    fn stagger_is_monotonic_and_strictly_below_base() {
        for total in [2usize, 3, 5, 8] {
            for base_secs in [1u64, 2, 10, 300] {
                let mut prev = 0u64;
                for index in 0..=(total + 2) {
                    let offset = stagger_offset_secs(index, total, base_secs);
                    assert!(
                        offset < base_secs,
                        "offset {offset} must be < {base_secs} (index={index}, total={total})"
                    );
                    assert!(
                        offset >= prev,
                        "offset {offset} regressed below {prev} (index={index}, total={total}, base={base_secs})"
                    );
                    prev = offset;
                }
            }
        }
    }
}
