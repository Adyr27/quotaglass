//! Pace math for the widget: how much of a window has passed, fast burn, room left and best account now.

use crate::model::Bucket;

/// Share of the bucket's window that has elapsed, 0.0..=100.0. Needs both the window length and the reset time.
pub fn passed_percent(b: &Bucket, now_ms: i64) -> Option<f64> {
    let window_ms = b.window_secs? as f64 * 1000.0;
    let reset = b.resets_at_ms? as f64;
    if window_ms <= 0.0 {
        return None;
    }
    let start = reset - window_ms;
    Some(((now_ms as f64 - start) / window_ms * 100.0).clamp(0.0, 100.0))
}

/// Usage runs well ahead of the clock. The 20 % floor keeps an early burst in a fresh window quiet.
pub fn fast_burn(used: f64, passed: Option<f64>) -> bool {
    passed.is_some_and(|p| used >= 1.5 * p) && used >= 20.0
}

/// The fullest bucket limits the account, so its headroom is the account's headroom.
pub fn room_left(buckets: &[Bucket]) -> Option<f64> {
    buckets
        .iter()
        .map(|b| b.used_percent)
        .reduce(f64::max)
        .map(|m| 100.0 - m)
}

/// Index of the account with the most room. On a tie the sooner reset wins, since that account recovers first.
/// A pick among fewer than two candidates says nothing, so it returns `None`.
pub fn best_now(rooms: &[(Option<f64>, Option<i64>)]) -> Option<usize> {
    let mut best: Option<(usize, f64, Option<i64>)> = None;
    let mut candidates = 0;
    for (i, &(room, reset)) in rooms.iter().enumerate() {
        let Some(room) = room else { continue };
        candidates += 1;
        let better = match best {
            None => true,
            Some((_, br, breset)) => {
                room > br
                    || (room == br
                        && match (reset, breset) {
                            (Some(r), Some(b)) => r < b,
                            (Some(_), None) => true,
                            _ => false,
                        })
            }
        };
        if better {
            best = Some((i, room, reset));
        }
    }
    if candidates < 2 {
        return None;
    }
    best.map(|(i, _, _)| i)
}

/// FNV-1a 64-bit over UTF-8 bytes. Stable across builds and platforms, unlike `std`'s randomised hasher.
pub fn fnv1a64(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AccountKey, Service};

    #[test]
    fn passed_percent_edges() {
        let b = |w, r| Bucket {
            name: "5-hour".into(),
            used_percent: 50.0,
            window_secs: w,
            resets_at_ms: r,
        };
        assert_eq!(
            passed_percent(&b(Some(18_000), Some(18_000_000)), 9_000_000),
            Some(50.0)
        );
        assert_eq!(
            passed_percent(&b(Some(18_000), Some(1_000)), 9_000_000),
            Some(100.0)
        );
        assert_eq!(passed_percent(&b(None, Some(18_000_000)), 9_000_000), None);
        assert_eq!(passed_percent(&b(Some(18_000), None), 9_000_000), None);
    }

    #[test]
    fn fast_burn_rule() {
        assert!(fast_burn(62.0, Some(30.0)));
        assert!(!fast_burn(44.0, Some(30.0)));
        assert!(!fast_burn(19.0, Some(5.0)));
        assert!(!fast_burn(90.0, None));
    }

    #[test]
    fn room_left_uses_the_fullest_bucket() {
        let b = |u| Bucket {
            name: "w".into(),
            used_percent: u,
            window_secs: None,
            resets_at_ms: None,
        };
        assert_eq!(room_left(&[]), None);
        assert_eq!(room_left(&[b(20.0), b(70.0)]), Some(30.0));
    }

    #[test]
    fn best_now_needs_two_and_breaks_ties_by_reset() {
        assert_eq!(best_now(&[(Some(80.0), Some(5))]), None);
        assert_eq!(
            best_now(&[(Some(40.0), Some(1)), (Some(80.0), Some(9))]),
            Some(1)
        );
        assert_eq!(
            best_now(&[(Some(80.0), Some(9)), (Some(80.0), Some(3))]),
            Some(1)
        );
        assert_eq!(
            best_now(&[(None, None), (Some(10.0), None), (Some(20.0), None)]),
            Some(2)
        );
        assert_eq!(
            best_now(&[(Some(80.0), None), (Some(80.0), Some(3))]),
            Some(1)
        );
    }

    #[test]
    fn store_key_has_no_email() {
        let k = AccountKey::new(Service::Claude, Some("User@Example.com"), "x");
        assert!(!k.store_key().contains('@'));
        assert_eq!(
            k.store_key(),
            format!("claude:{:016x}", fnv1a64("user@example.com"))
        );
        assert_eq!(fnv1a64(""), 0xcbf29ce484222325);
    }
}
