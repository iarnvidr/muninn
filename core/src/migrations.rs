//! SQLite schema migrations for the three independent stores.
//!
//! Each database carries its own ordered list of migrations and records how many
//! have been applied in `PRAGMA user_version`. Migrations only ever get appended.

use crate::error::{Error, Result};
use rusqlite::Connection;
use std::path::Path;

/// Video cache index, rules and settings.
pub const VIDEO_MIGRATIONS: &[&str] = &[
    r#"
    CREATE TABLE entries (
        id TEXT PRIMARY KEY,
        video_id TEXT NOT NULL,
        variant TEXT NOT NULL,
        requested_quality TEXT NOT NULL,
        actual_height INTEGER,
        status TEXT NOT NULL,
        error TEXT,
        protected INTEGER NOT NULL DEFAULT 0,
        origin TEXT NOT NULL,
        size_bytes INTEGER NOT NULL DEFAULT 0,
        expected_bytes INTEGER,
        progress REAL NOT NULL DEFAULT 0,
        speed_bps INTEGER,
        file_path TEXT,
        ext TEXT,
        progressive INTEGER NOT NULL DEFAULT 0,
        created_at INTEGER NOT NULL,
        last_access_at INTEGER NOT NULL,
        started_at INTEGER,
        expires_at INTEGER,
        retention_days INTEGER,
        rule TEXT,
        title TEXT,
        channel TEXT,
        duration_seconds INTEGER
    );
    CREATE INDEX entries_video ON entries(video_id);
    CREATE INDEX entries_status ON entries(status);
    CREATE TABLE kv (key TEXT PRIMARY KEY, value TEXT NOT NULL);
    CREATE TABLE observations (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        ts INTEGER NOT NULL,
        video_id TEXT,
        itag INTEGER,
        mime TEXT,
        range_start INTEGER,
        range_end INTEGER,
        rn INTEGER,
        rbuf INTEGER,
        bytes INTEGER NOT NULL DEFAULT 0,
        duration_ms INTEGER,
        quality_label TEXT,
        clen INTEGER,
        host TEXT,
        copied INTEGER NOT NULL DEFAULT 0,
        note TEXT
    );
    CREATE INDEX observations_ts ON observations(ts);
    CREATE TABLE chunks (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        video_id TEXT NOT NULL,
        itag INTEGER NOT NULL,
        range_start INTEGER NOT NULL,
        range_end INTEGER NOT NULL,
        mime TEXT,
        path TEXT NOT NULL,
        bytes INTEGER NOT NULL,
        created_at INTEGER NOT NULL,
        UNIQUE(video_id, itag, range_start, range_end)
    );
    "#,
    // The passive-capture experiment is gone: YouTube's player never runs in Muninn.
    "DROP TABLE IF EXISTS observations; DROP TABLE IF EXISTS chunks;",
];

/// Content cache: metadata, comments, feed snapshots, thumbnails, transcripts,
/// search results and watch progress. Everything but progress and transcripts
/// carries a TTL.
pub const CONTENT_MIGRATIONS: &[&str] = &[
    r#"
    CREATE TABLE content (
        kind TEXT NOT NULL,
        key TEXT NOT NULL,
        data BLOB NOT NULL,
        compressed INTEGER NOT NULL DEFAULT 1,
        fetched_at INTEGER NOT NULL,
        expires_at INTEGER,
        size INTEGER NOT NULL,
        PRIMARY KEY (kind, key)
    );
    CREATE INDEX content_expiry ON content(expires_at);
    CREATE TABLE feed_snapshots (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        kind TEXT NOT NULL,
        source_url TEXT,
        query TEXT,
        captured_at INTEGER NOT NULL,
        count INTEGER NOT NULL,
        data BLOB NOT NULL
    );
    CREATE INDEX feed_kind_time ON feed_snapshots(kind, captured_at);
    CREATE TABLE progress (
        video_id TEXT PRIMARY KEY,
        position_seconds REAL NOT NULL,
        duration_seconds REAL,
        watch_height INTEGER,
        first_played_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL
    );
    CREATE TABLE seen_videos (
        video_id TEXT PRIMARY KEY,
        first_seen_at INTEGER NOT NULL
    );
    "#,
];

/// Browsing state: tiny. Tab URLs, history, scroll and playback position.
pub const BROWSING_MIGRATIONS: &[&str] = &[
    r#"
    CREATE TABLE tabs (
        tab_id TEXT PRIMARY KEY,
        url TEXT NOT NULL,
        title TEXT,
        video_id TEXT,
        scroll_y REAL,
        position_seconds REAL,
        duration_seconds REAL,
        watch_height INTEGER,
        state TEXT NOT NULL,
        updated_at INTEGER NOT NULL
    );
    CREATE TABLE history (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        url TEXT NOT NULL,
        title TEXT,
        video_id TEXT,
        ts INTEGER NOT NULL
    );
    CREATE INDEX history_ts ON history(ts);
    "#,
];

/// Brings `conn` up to the latest schema. Refuses a database written by a newer app.
pub fn migrate(conn: &mut Connection, migrations: &[&str]) -> Result<()> {
    let current: usize = conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))? as usize;
    if current > migrations.len() {
        return Err(Error::other(format!(
            "database schema version {current} is newer than this app understands ({})",
            migrations.len()
        )));
    }
    for (index, sql) in migrations.iter().enumerate().skip(current) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.execute_batch(&format!("PRAGMA user_version = {}", index + 1))?;
        tx.commit()?;
    }
    Ok(())
}

/// Opens (creating if needed) a database file with WAL and a busy timeout, then migrates it.
pub fn open_db(path: &Path, migrations: &[&str]) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut conn = Connection::open(path)?;
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;")?;
    migrate(&mut conn, migrations)?;
    Ok(conn)
}

pub fn open_memory(migrations: &[&str]) -> Result<Connection> {
    let mut conn = Connection::open_in_memory()?;
    migrate(&mut conn, migrations)?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(conn: &Connection) -> i64 {
        conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn fresh_databases_reach_latest_version() {
        for list in [VIDEO_MIGRATIONS, CONTENT_MIGRATIONS, BROWSING_MIGRATIONS] {
            let conn = open_memory(list).unwrap();
            assert_eq!(version(&conn), list.len() as i64);
        }
    }

    #[test]
    fn migrations_are_idempotent_and_incremental() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.sqlite");
        {
            let mut conn = Connection::open(&path).unwrap();
            // Pretend an older app applied only the first migration of a longer list.
            let longer: Vec<&str> = vec![CONTENT_MIGRATIONS[0], "CREATE TABLE later (x INTEGER);"];
            migrate(&mut conn, &longer[..1]).unwrap();
            assert_eq!(version(&conn), 1);
            conn.execute("INSERT INTO seen_videos VALUES ('abc', 1)", []).unwrap();
            migrate(&mut conn, &longer).unwrap();
            assert_eq!(version(&conn), 2);
            migrate(&mut conn, &longer).unwrap();
            assert_eq!(version(&conn), 2);
            let kept: i64 = conn.query_row("SELECT COUNT(*) FROM seen_videos", [], |r| r.get(0)).unwrap();
            assert_eq!(kept, 1, "existing data survives later migrations");
        }
        // And the real opener is idempotent across restarts.
        let a = open_db(&path.with_extension("b"), CONTENT_MIGRATIONS).unwrap();
        drop(a);
        let b = open_db(&path.with_extension("b"), CONTENT_MIGRATIONS).unwrap();
        assert_eq!(version(&b), CONTENT_MIGRATIONS.len() as i64);
    }

    #[test]
    fn newer_database_is_refused() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA user_version = 99").unwrap();
        let err = migrate(&mut conn, BROWSING_MIGRATIONS).unwrap_err();
        assert!(err.to_string().contains("newer"), "{err}");
    }

    #[test]
    fn a_failing_migration_rolls_back() {
        let mut conn = Connection::open_in_memory().unwrap();
        let bad = ["CREATE TABLE ok (x INTEGER);", "CREATE TABLE half (x INTEGER); THIS IS NOT SQL;"];
        assert!(migrate(&mut conn, &bad).is_err());
        assert_eq!(version(&conn), 1);
        let half: i64 = conn
            .query_row("SELECT COUNT(*) FROM sqlite_master WHERE name = 'half'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(half, 0);
    }
}
