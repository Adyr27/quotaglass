//! Usage warnings at 50, 80 and 90%: which level fired last for each limit window.
//! Pure state only. The app crate stores it in `warnings.json` and shows the toasts.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The warning levels, in percent, lowest first.
pub const LEVELS: [u8; 3] = [50, 80, 90];

/// The highest level fired for one limit in the window that ends at `reset_ms`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Fired {
    pub reset_ms: Option<i64>,
    pub level: u8,
}

/// `key(account_id, bucket)` to the last fired level.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WarnState(pub BTreeMap<String, Fired>);

pub fn key(account_id: &str, bucket: &str) -> String {
    format!("{account_id}|{bucket}")
}

/// The level to fire now, if any. A new `reset_ms` starts a new window and clears the stored level.
/// A jump past several levels fires only the highest one.
pub fn due(state: &mut WarnState, key: &str, used: f64, reset_ms: Option<i64>) -> Option<u8> {
    if state.0.get(key).is_some_and(|f| f.reset_ms != reset_ms) {
        state.0.remove(key);
    }
    let stored = state.0.get(key).map_or(0, |f| f.level);
    let level = LEVELS
        .iter()
        .rev()
        .copied()
        .find(|&l| used >= f64::from(l))?;
    if level <= stored {
        return None;
    }
    state.0.insert(key.to_string(), Fired { reset_ms, level });
    Some(level)
}

/// A stored window that ended more than this long ago is dropped, even for an account that is not loaded.
pub const STALE_MS: i64 = 7 * 86_400_000;

/// Drops the keys of `present_accounts` that are not in `live_keys` (the limit is gone),
/// and every key whose stored window ended more than [`STALE_MS`] before `now_ms`.
/// Keys of other accounts stay: right after start not every account has a reading yet.
pub fn prune(
    state: &mut WarnState,
    present_accounts: &BTreeSet<String>,
    live_keys: &BTreeSet<String>,
    now_ms: i64,
) {
    state.0.retain(|k, f| {
        if live_keys.contains(k) {
            return true;
        }
        let present = k
            .rsplit_once('|')
            .is_some_and(|(id, _)| present_accounts.contains(id));
        let stale = f
            .reset_ms
            .is_some_and(|r| r < now_ms.saturating_sub(STALE_MS));
        !present && !stale
    });
}

/// The local UTC offset in seconds, or `None` when the OS does not give it.
pub fn local_offset_secs() -> Option<i32> {
    time::UtcOffset::current_local_offset()
        .ok()
        .map(time::UtcOffset::whole_seconds)
}

/// "in 2 h 5 min" when the reset is less than 24 h away, else the local weekday and time ("Mon 14:05").
/// With no known offset (`None`), the time is UTC and the text ends in " UTC".
/// `None` when `reset_ms` is not a valid time.
pub fn reset_text(now_ms: i64, reset_ms: i64, offset_secs: Option<i32>) -> Option<String> {
    let left_min = (reset_ms.saturating_sub(now_ms)).max(0) / 60_000;
    if left_min < 24 * 60 {
        let (h, m) = (left_min / 60, left_min % 60);
        return Some(if h == 0 {
            format!("in {m} min")
        } else {
            format!("in {h} h {m} min")
        });
    }
    let offset = offset_secs.and_then(|o| time::UtcOffset::from_whole_seconds(o).ok());
    let at = time::OffsetDateTime::from_unix_timestamp(reset_ms.div_euclid(1000))
        .ok()?
        .to_offset(offset.unwrap_or(time::UtcOffset::UTC));
    let day = &at.weekday().to_string()[..3];
    let zone = if offset.is_some() { "" } else { " UTC" };
    Some(format!("{day} {:02}:{:02}{zone}", at.hour(), at.minute()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fires_once_per_level() {
        let mut s = WarnState::default();
        let k = key("a", "5h");
        assert_eq!(due(&mut s, &k, 10.0, Some(1)), None);
        assert_eq!(due(&mut s, &k, 50.0, Some(1)), Some(50));
        assert_eq!(due(&mut s, &k, 60.0, Some(1)), None);
        assert_eq!(due(&mut s, &k, 80.5, Some(1)), Some(80));
        assert_eq!(due(&mut s, &k, 85.0, Some(1)), None);
        assert_eq!(due(&mut s, &k, 90.0, Some(1)), Some(90));
        assert_eq!(due(&mut s, &k, 100.0, Some(1)), None);
    }

    #[test]
    fn jump_fires_only_highest() {
        let mut s = WarnState::default();
        let k = key("a", "weekly");
        assert_eq!(due(&mut s, &k, 0.0, None), None);
        assert_eq!(due(&mut s, &k, 92.0, None), Some(90));
        assert_eq!(due(&mut s, &k, 95.0, None), None);
    }

    #[test]
    fn new_reset_clears_levels() {
        let mut s = WarnState::default();
        let k = key("a", "5h");
        assert_eq!(due(&mut s, &k, 85.0, Some(1_000)), Some(80));
        // The old window ended with a lower reading still stored: the entry goes.
        assert_eq!(due(&mut s, &k, 10.0, Some(2_000)), None);
        assert!(s.0.is_empty());
        assert_eq!(due(&mut s, &k, 55.0, Some(2_000)), Some(50));
        assert_eq!(due(&mut s, &k, 55.0, Some(3_000)), Some(50));
    }

    #[test]
    fn nan_never_fires() {
        let mut s = WarnState::default();
        assert_eq!(due(&mut s, "k", f64::NAN, None), None);
    }

    #[test]
    fn state_round_trips_json() {
        let mut s = WarnState::default();
        due(&mut s, &key("a", "5h"), 81.0, Some(1_700_000_000_000));
        due(&mut s, &key("b", "weekly"), 95.0, None);
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<WarnState>(&json).unwrap(), s);
    }

    #[test]
    fn prune_drops_gone_limits_of_present_accounts_only() {
        let now = 1_700_000_000_000_i64;
        let mut s = WarnState::default();
        due(&mut s, &key("a", "5h"), 60.0, None);
        due(&mut s, &key("a", "old"), 60.0, None);
        due(&mut s, &key("b", "5h"), 60.0, None);
        due(&mut s, &key("b", "ended"), 60.0, Some(now - 1));
        due(&mut s, &key("c", "stale"), 60.0, Some(now - STALE_MS - 1));
        prune(
            &mut s,
            &BTreeSet::from(["a".to_string()]),
            &BTreeSet::from([key("a", "5h")]),
            now,
        );
        assert_eq!(
            s.0.keys().cloned().collect::<Vec<_>>(),
            [key("a", "5h"), key("b", "5h"), key("b", "ended")]
        );
    }

    #[test]
    fn reset_text_short_and_long() {
        let now = 1_700_000_000_000_i64;
        assert_eq!(
            reset_text(now, now + (2 * 60 + 5) * 60_000, Some(0)).as_deref(),
            Some("in 2 h 5 min")
        );
        assert_eq!(
            reset_text(now, now + 7 * 60_000, Some(0)).as_deref(),
            Some("in 7 min")
        );
        assert_eq!(
            reset_text(now, now - 1, Some(0)).as_deref(),
            Some("in 0 min")
        );
        // 2023-11-17 00:00:00 UTC is a Friday; +2 h offset gives 02:00 local.
        assert_eq!(
            reset_text(now, 1_700_179_200_000, Some(7_200)).as_deref(),
            Some("Fri 02:00")
        );
        // No known offset: UTC, and the text says so.
        assert_eq!(
            reset_text(now, 1_700_179_200_000, None).as_deref(),
            Some("Fri 00:00 UTC")
        );
        assert_eq!(
            reset_text(now, now + 7 * 60_000, None).as_deref(),
            Some("in 7 min")
        );
    }
}
