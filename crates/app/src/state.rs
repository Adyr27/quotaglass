//! Atomic JSON persistence (`snapshot_cache.json`, `readings.json`, `warnings.json`, `resets.json`)
//! and UTC-to-local display formatting helpers.

use quotaglass_core::view::Snapshot;
use quotaglass_core::warnings;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use time::{OffsetDateTime, UtcOffset};

pub const SNAPSHOT_CACHE_FILE: &str = "snapshot_cache.json";

/// Small JSON file in `%APPDATA%\QuotaGlass`, written atomically via `<name>.tmp` -> `rename`.
#[derive(Debug, Clone)]
pub struct JsonStore {
    path: PathBuf,
}

impl JsonStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// `%APPDATA%\QuotaGlass\<name>`, or `None` when `APPDATA` is unset, relative, or `name` is unsafe.
    pub fn in_appdata(name: &str) -> Option<PathBuf> {
        if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
            return None;
        }
        let appdata = PathBuf::from(std::env::var_os("APPDATA")?);
        appdata
            .is_absolute()
            .then(|| appdata.join("QuotaGlass").join(name))
    }

    /// Missing or unreadable/corrupt files return `None`.
    pub fn load_opt<T: DeserializeOwned>(&self) -> Option<T> {
        let raw = std::fs::read(&self.path).ok()?;
        serde_json::from_slice(&raw).ok()
    }

    /// Missing or unreadable/corrupt files return `T::default()`.
    pub fn load<T: DeserializeOwned + Default>(&self) -> T {
        self.load_opt().unwrap_or_default()
    }

    /// Writes `<name>.tmp` in the same directory, then atomically renames over `<name>`.
    pub fn save<T: Serialize>(&self, value: &T) -> io::Result<()> {
        self.save_bytes(&serde_json::to_vec(value).map_err(io::Error::other)?)
    }

    /// Pretty-printed variant for human-inspectable files like `settings.json`.
    pub fn save_pretty<T: Serialize>(&self, value: &T) -> io::Result<()> {
        self.save_bytes(&serde_json::to_vec_pretty(value).map_err(io::Error::other)?)
    }

    fn save_bytes(&self, bytes: &[u8]) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = temp_sibling(&self.path);
        if let Err(err) = std::fs::write(&tmp, bytes) {
            let _ = std::fs::remove_file(&tmp);
            return Err(err);
        }
        if let Err(err) = std::fs::rename(&tmp, &self.path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(err);
        }
        Ok(())
    }
}

fn temp_sibling(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| "state".into());
    name.push(".tmp");
    path.with_file_name(name)
}

/// Persists the latest token-free [`Snapshot`] in `%APPDATA%\QuotaGlass\snapshot_cache.json`
/// so the UI renders last known readings immediately on boot.
#[derive(Debug, Clone)]
pub struct SnapshotCache {
    store: JsonStore,
}

impl SnapshotCache {
    pub fn new(path: PathBuf) -> Self {
        Self {
            store: JsonStore::new(path),
        }
    }

    pub fn default_path() -> Option<PathBuf> {
        JsonStore::in_appdata(SNAPSHOT_CACHE_FILE)
    }

    pub fn in_appdata() -> Option<Self> {
        Self::default_path().map(Self::new)
    }

    pub fn path(&self) -> &Path {
        self.store.path()
    }

    pub fn load(&self) -> Option<Snapshot> {
        self.store.load_opt()
    }

    pub fn save(&self, snapshot: &Snapshot) -> io::Result<()> {
        self.store.save(snapshot)
    }
}

/// Current system time as UTC milliseconds since Unix epoch.
pub fn now_utc_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Current local UTC offset, or `None` when unavailable.
pub fn local_offset() -> Option<UtcOffset> {
    UtcOffset::current_local_offset().ok()
}

/// Formats a future reset timestamp ("in 2 h 5 min" under 24 h, else "Fri 14:05" in local time,
/// falling back to UTC with a " UTC" suffix when local offset is unavailable).
pub fn format_reset_time(now_ms: i64, resets_at_ms: i64) -> Option<String> {
    format_reset_time_with_offset(now_ms, resets_at_ms, local_offset())
}

pub fn format_reset_time_with_offset(
    now_ms: i64,
    resets_at_ms: i64,
    offset: Option<UtcOffset>,
) -> Option<String> {
    warnings::reset_text(now_ms, resets_at_ms, offset.map(UtcOffset::whole_seconds))
}

/// Formats a UTC ms timestamp as a local clock string (`"HH:MM"`, or `"HH:MM UTC"` when `offset` is `None`).
pub fn format_local_clock(timestamp_ms: i64) -> Option<String> {
    format_local_clock_with_offset(timestamp_ms, local_offset())
}

pub fn format_local_clock_with_offset(
    timestamp_ms: i64,
    offset: Option<UtcOffset>,
) -> Option<String> {
    let at = OffsetDateTime::from_unix_timestamp(timestamp_ms.div_euclid(1000))
        .ok()?
        .to_offset(offset.unwrap_or(UtcOffset::UTC));
    let zone = if offset.is_some() { "" } else { " UTC" };
    Some(format!("{:02}:{:02}{zone}", at.hour(), at.minute()))
}

/// Relative age text for "Updated Xm ago" indicators.
pub fn format_updated_ago(now_ms: i64, observed_at_ms: i64) -> String {
    let age_secs = now_ms.saturating_sub(observed_at_ms).max(0) / 1000;
    if age_secs < 60 {
        "just now".to_string()
    } else if age_secs < 3600 {
        format!("{} min ago", age_secs / 60)
    } else if age_secs < 86_400 {
        format!("{} h ago", age_secs / 3600)
    } else {
        format!("{} d ago", age_secs / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quotaglass_core::view::{AccountView, BucketView, CardState, Freshness, ServiceView};
    use quotaglass_core::{Service, SourceKind};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("qg-state-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn sample_snapshot() -> Snapshot {
        let bucket = BucketView {
            name: "5-hour".to_string(),
            used_percent: 38.0,
            window_secs: Some(18_000),
            resets_at_ms: Some(1_794_160_000_000),
            limit_reached: false,
            freshness: Freshness::Live,
            passed_percent: Some(45.0),
            fast_burn: false,
        };
        Snapshot {
            accounts: vec![AccountView {
                id: "claude:0123456789abcdef".to_string(),
                service: Service::Claude,
                label: "Work".to_string(),
                plan: Some("Max".to_string()),
                source: Some(SourceKind::Direct),
                freshness: Freshness::Live,
                age_secs: Some(10),
                headline: Some(bucket.clone()),
                buckets: vec![bucket],
                state: CardState::Ok,
                reason: None,
                room_left: Some(62.0),
                forecast_mins: Some(90),
                best_now: true,
                banked_resets: None,
                profile_id: None,
                current_login: true,
                hidden: false,
            }],
            services: vec![ServiceView {
                service: Service::Claude,
                used_percent: Some(38.0),
                all_exhausted: false,
                accounts: 1,
                rate_limited: false,
            }],
            generated_at_ms: 1_794_150_000_000,
        }
    }

    #[test]
    fn json_store_atomic_save_and_corrupt_load() {
        let dir = temp_dir("atomic");
        let path = dir.join("nested").join("cache.json");
        let store = JsonStore::new(path.clone());

        assert_eq!(store.load_opt::<Snapshot>(), None);

        let snap = sample_snapshot();
        store.save(&snap).unwrap();
        assert!(path.exists());
        assert!(!dir.join("nested").join("cache.json.tmp").exists());
        assert_eq!(store.load_opt::<Snapshot>(), Some(snap));

        std::fs::write(&path, b"{corrupt").unwrap();
        assert_eq!(store.load_opt::<Snapshot>(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn snapshot_cache_roundtrips_without_temp_leftover() {
        let dir = temp_dir("snap-cache");
        let cache = SnapshotCache::new(dir.join(SNAPSHOT_CACHE_FILE));
        assert_eq!(cache.load(), None);

        let snap = sample_snapshot();
        cache.save(&snap).unwrap();
        assert!(!dir.join("snapshot_cache.json.tmp").exists());
        assert_eq!(cache.load(), Some(snap));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn in_appdata_rejects_traversal_and_separators() {
        assert_eq!(JsonStore::in_appdata(""), None);
        assert_eq!(JsonStore::in_appdata("."), None);
        assert_eq!(JsonStore::in_appdata(".."), None);
        assert_eq!(JsonStore::in_appdata("../evil.json"), None);
        assert_eq!(JsonStore::in_appdata(r"sub\file.json"), None);
        assert_eq!(JsonStore::in_appdata("a\0b"), None);
    }

    #[test]
    fn local_time_formatting_uses_offset_or_utc_fallback() {
        let now = 1_700_000_000_000_i64;
        let plus_8 = UtcOffset::from_hms(8, 0, 0).unwrap();

        assert_eq!(
            format_reset_time_with_offset(now, now + 95 * 60_000, Some(plus_8)).as_deref(),
            Some("in 1 h 35 min")
        );
        // 2023-11-17 00:00:00 UTC -> +08:00 is Fri 08:00.
        assert_eq!(
            format_reset_time_with_offset(now, 1_700_179_200_000, Some(plus_8)).as_deref(),
            Some("Fri 08:00")
        );
        assert_eq!(
            format_reset_time_with_offset(now, 1_700_179_200_000, None).as_deref(),
            Some("Fri 00:00 UTC")
        );

        assert_eq!(
            format_local_clock_with_offset(1_700_179_200_000, Some(plus_8)).as_deref(),
            Some("08:00")
        );
        assert_eq!(
            format_local_clock_with_offset(1_700_179_200_000, None).as_deref(),
            Some("00:00 UTC")
        );
    }

    #[test]
    fn updated_ago_formats_expected_units() {
        let now = 1_800_000_000_000_i64;
        assert_eq!(format_updated_ago(now, now - 25_000), "just now");
        assert_eq!(format_updated_ago(now, now + 5_000), "just now");
        assert_eq!(format_updated_ago(now, now - 4 * 60_000), "4 min ago");
        assert_eq!(format_updated_ago(now, now - 2 * 3_600_000), "2 h ago");
        assert_eq!(format_updated_ago(now, now - 3 * 86_400_000), "3 d ago");
    }
}
