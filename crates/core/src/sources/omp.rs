//! OMP's own usage history: the `usage_history` table in `~/.omp/agent/agent.db`.
//!
//! OMP records a quota snapshot every time it talks to Claude or Antigravity, so this is a local, token-free
//! fallback. The database belongs to another running app: it is opened read-only (`mode=ro`, never `immutable=1`,
//! which would ignore the WAL) and the reader never blocks OMP's writer.

use crate::env::Env;
use crate::model::{checked_percent, AccountKey, Bucket, FetchError, Reading, Service, SourceKind};
use crate::provider::{BoxFut, Provider};
use rusqlite::{Connection, OpenFlags};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

const FIVE_HOUR_SECS: u64 = 18_000;
const WEEK_SECS: u64 = 604_800;

/// Latest row per (provider, account, limit). The window function keeps NULL account or limit ids together.
const LATEST_SQL: &str = "\
SELECT provider, account_key, email, label, window_label, used_fraction, resets_at, recorded_at \
FROM (SELECT *, ROW_NUMBER() OVER (PARTITION BY provider, account_key, limit_id ORDER BY recorded_at DESC, id DESC) AS rn \
      FROM usage_history) \
WHERE rn = 1 AND provider IN ('anthropic', 'google-antigravity') \
ORDER BY provider, account_key, limit_id";

/// OMP's usage history database (`Env::omp_db`).
pub struct Omp;

impl Provider for Omp {
    fn kind(&self) -> SourceKind {
        SourceKind::Omp
    }

    fn detect(&self, env: &Env) -> bool {
        env.omp_db.is_file()
    }

    fn fetch<'a>(&'a self, env: &'a Env) -> BoxFut<'a, Result<Vec<Reading>, FetchError>> {
        Box::pin(async move {
            let db = env.omp_db.clone();
            tokio::task::spawn_blocking(move || read_latest(&db))
                .await
                .map_err(|_| FetchError::Unavailable("The OMP database read failed"))?
        })
    }
}

/// One reading per OMP account, built from the newest row of each limit.
///
/// A missing file is `NotDetected`, a missing table or column is `Format`, a locked or unreadable database is
/// `Unavailable`. Providers other than `anthropic` and `google-antigravity` are skipped. Blocking: call it from
/// `spawn_blocking`.
pub fn read_latest(db: &Path) -> Result<Vec<Reading>, FetchError> {
    if !db.is_file() {
        return Err(FetchError::NotDetected);
    }
    let conn = open_read_only(db).map_err(map_sqlite)?;
    let mut stmt = conn.prepare(LATEST_SQL).map_err(map_sqlite)?;
    let mut rows = stmt.query([]).map_err(map_sqlite)?;

    let mut accounts: BTreeMap<(Service, Option<String>), Account> = BTreeMap::new();
    while let Some(row) = rows.next().map_err(map_sqlite)? {
        let provider: String = row.get(0).map_err(map_sqlite)?;
        let service = match provider.as_str() {
            "anthropic" => Service::Claude,
            "google-antigravity" => Service::Antigravity,
            _ => continue,
        };
        let account_key: Option<String> = row.get(1).map_err(map_sqlite)?;
        let email: Option<String> = row.get(2).map_err(map_sqlite)?;
        let label: Option<String> = row.get(3).map_err(map_sqlite)?;
        let window_label: Option<String> = row.get(4).map_err(map_sqlite)?;
        let used_fraction: Option<f64> = row.get(5).map_err(map_sqlite)?;
        let resets_at_ms: Option<i64> = row.get(6).map_err(map_sqlite)?;
        let recorded_at: i64 = row.get(7).map_err(map_sqlite)?;

        // One row with no name or a bad fraction is skipped alone; it must not hide every other OMP account.
        let Some(name) = label
            .filter(|l| !l.trim().is_empty())
            .or_else(|| window_label.clone().filter(|l| !l.trim().is_empty()))
        else {
            continue;
        };
        let window_secs = match window_label.as_deref() {
            Some("5 Hour") => Some(FIVE_HOUR_SECS),
            Some("7 Day") | Some("Weekly") => Some(WEEK_SECS),
            _ => None,
        };
        let Ok(used_percent) =
            checked_percent(used_fraction.map(|f| f * 100.0), "omp used_fraction")
        else {
            continue;
        };

        let account = accounts.entry((service, account_key)).or_default();
        if account.email.is_none() {
            account.email = email.filter(|e| !e.trim().is_empty());
        }
        account.observed_at_ms = account.observed_at_ms.max(recorded_at);
        account.buckets.push(Bucket {
            name,
            used_percent,
            window_secs,
            resets_at_ms,
        });
    }

    Ok(accounts
        .into_iter()
        .map(|((service, account_key), account)| {
            let local_id = account_key
                .clone()
                .unwrap_or_else(|| format!("omp-{}", service.as_str()));
            let label = account
                .email
                .clone()
                .or(account_key)
                .unwrap_or_else(|| local_id.clone());
            Reading {
                key: AccountKey::new(service, account.email.as_deref(), local_id),
                label,
                plan: None,
                buckets: account.buckets,
                source: SourceKind::Omp,
                observed_at_ms: account.observed_at_ms,
            }
        })
        .collect())
}

#[derive(Default)]
struct Account {
    email: Option<String>,
    observed_at_ms: i64,
    buckets: Vec<Bucket>,
}

/// Read-only URI open. Spaces and URI-special characters in the path are escaped; Windows backslashes become slashes.
fn open_read_only(path: &Path) -> rusqlite::Result<Connection> {
    let mut uri = String::from("file:");
    for c in path.to_string_lossy().chars() {
        match c {
            '\\' => uri.push('/'),
            ' ' => uri.push_str("%20"),
            '%' => uri.push_str("%25"),
            '?' => uri.push_str("%3f"),
            '#' => uri.push_str("%23"),
            c => uri.push(c),
        }
    }
    uri.push_str("?mode=ro");
    let conn = Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_URI,
    )?;
    conn.busy_timeout(Duration::from_secs(2))?;
    Ok(conn)
}

fn map_sqlite(err: rusqlite::Error) -> FetchError {
    match err {
        rusqlite::Error::SqliteFailure(failure, message) => {
            let message = message.unwrap_or_default();
            if message.contains("no such table") || message.contains("no such column") {
                FetchError::Format("omp table")
            } else if matches!(
                failure.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            ) {
                FetchError::Unavailable("The OMP database is busy")
            } else {
                FetchError::Unavailable("The OMP database could not be read")
            }
        }
        _ => FetchError::Format("omp row"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("qg-test-{}-{}", std::process::id(), name));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn db(&self) -> PathBuf {
            self.0.join("agent.db")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const SCHEMA: &str = "CREATE TABLE usage_history (
        id INTEGER PRIMARY KEY, recorded_at INTEGER NOT NULL, provider TEXT NOT NULL, account_key TEXT, email TEXT,
        account_id TEXT, limit_id TEXT, label TEXT, window_label TEXT, used_fraction REAL, status TEXT, resets_at INTEGER)";

    fn create(path: &Path) -> Connection {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn
    }

    #[allow(clippy::too_many_arguments)]
    fn insert(
        conn: &Connection,
        recorded_at: i64,
        provider: &str,
        account_key: &str,
        email: Option<&str>,
        limit_id: &str,
        window_label: &str,
        fraction: f64,
    ) {
        conn.execute(
            "INSERT INTO usage_history (recorded_at, provider, account_key, email, limit_id, label, window_label, \
             used_fraction, status, resets_at) VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, ?7, 'ok', ?8)",
            rusqlite::params![recorded_at, provider, account_key, email, limit_id, window_label, fraction, recorded_at + 1000],
        )
        .unwrap();
    }

    #[test]
    fn latest_row_wins() {
        let dir = TempDir::new("omp-latest");
        let conn = create(&dir.db());
        insert(
            &conn,
            100,
            "anthropic",
            "acc",
            Some("User@Example.com"),
            "five_hour",
            "5 Hour",
            0.10,
        );
        insert(
            &conn,
            200,
            "anthropic",
            "acc",
            Some("User@Example.com"),
            "five_hour",
            "5 Hour",
            0.30,
        );
        insert(
            &conn,
            150,
            "anthropic",
            "acc",
            Some("User@Example.com"),
            "seven_day",
            "7 Day",
            0.50,
        );
        drop(conn);

        let readings = read_latest(&dir.db()).unwrap();
        assert_eq!(readings.len(), 1);
        let r = &readings[0];
        assert_eq!(
            r.key,
            AccountKey::new(Service::Claude, Some("user@example.com"), "acc")
        );
        assert_eq!(r.label, "User@Example.com");
        assert_eq!(r.source, SourceKind::Omp);
        assert_eq!(r.plan, None);
        assert_eq!(r.observed_at_ms, 200);
        let five = r.buckets.iter().find(|b| b.name == "five_hour").unwrap();
        assert!((five.used_percent - 30.0).abs() < 1e-9);
        assert_eq!(five.window_secs, Some(18_000));
        assert_eq!(five.resets_at_ms, Some(1200));
        let week = r.buckets.iter().find(|b| b.name == "seven_day").unwrap();
        assert_eq!(week.window_secs, Some(604_800));
        assert_eq!(r.buckets.len(), 2);
    }

    #[test]
    fn fraction_to_percent() {
        let dir = TempDir::new("omp-fraction");
        let conn = create(&dir.db());
        insert(
            &conn,
            100,
            "google-antigravity",
            "g1",
            None,
            "gemini",
            "Weekly",
            0.21,
        );
        drop(conn);

        let readings = read_latest(&dir.db()).unwrap();
        assert_eq!(readings[0].key.service, Service::Antigravity);
        assert_eq!(readings[0].label, "g1");
        assert!((readings[0].buckets[0].used_percent - 21.0).abs() < 1e-9);
        assert_eq!(readings[0].buckets[0].window_secs, Some(604_800));
    }

    #[test]
    fn unknown_provider_skipped() {
        let dir = TempDir::new("omp-unknown");
        let conn = create(&dir.db());
        insert(
            &conn,
            100,
            "kilo",
            "k1",
            Some("k@example.com"),
            "x",
            "Weekly",
            0.5,
        );
        insert(
            &conn,
            100,
            "anthropic",
            "acc",
            None,
            "five_hour",
            "5 Hour",
            0.5,
        );
        drop(conn);

        let readings = read_latest(&dir.db()).unwrap();
        assert_eq!(readings.len(), 1);
        assert_eq!(readings[0].key.service, Service::Claude);
    }

    #[test]
    fn missing_table_is_format_error() {
        let dir = TempDir::new("omp-no-table");
        Connection::open(dir.db())
            .unwrap()
            .execute_batch("CREATE TABLE other (x INTEGER)")
            .unwrap();
        assert_eq!(read_latest(&dir.db()), Err(FetchError::Format("omp table")));
    }

    #[test]
    fn missing_file_is_not_detected() {
        let dir = TempDir::new("omp-no-file");
        assert_eq!(read_latest(&dir.db()), Err(FetchError::NotDetected));
    }

    #[test]
    fn open_does_not_block_writer() {
        let dir = TempDir::new("omp-writer");
        let writer = create(&dir.db());
        writer.pragma_update(None, "journal_mode", "WAL").unwrap();
        insert(
            &writer,
            100,
            "anthropic",
            "acc",
            None,
            "five_hour",
            "5 Hour",
            0.1,
        );

        let reader = open_read_only(&dir.db()).unwrap();
        reader
            .execute_batch("BEGIN; SELECT count(*) FROM usage_history")
            .unwrap();
        writer.busy_timeout(Duration::from_millis(200)).unwrap();
        insert(
            &writer,
            200,
            "anthropic",
            "acc",
            None,
            "five_hour",
            "5 Hour",
            0.4,
        );
        reader.execute_batch("COMMIT").unwrap();
        drop(reader);

        let readings = read_latest(&dir.db()).unwrap();
        assert!((readings[0].buckets[0].used_percent - 40.0).abs() < 1e-9);
    }

    #[test]
    fn path_with_spaces_opens() {
        let dir = TempDir::new("omp with space");
        let conn = create(&dir.db());
        insert(
            &conn,
            100,
            "anthropic",
            "acc",
            None,
            "five_hour",
            "5 Hour",
            0.2,
        );
        drop(conn);
        assert_eq!(read_latest(&dir.db()).unwrap().len(), 1);
    }
}
