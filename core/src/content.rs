//! (b) The content cache: SQLite with zlib-compressed JSON and per-kind TTLs.
//!
//! Holds video metadata (with related videos), comments, search results,
//! thumbnails, transcripts, feed snapshots and watch progress. Titles,
//! descriptions and thumbnails live for weeks; the first comments for a day;
//! search results for two hours; transcripts and progress forever; every feed
//! snapshot is kept (pruned only by age/count) and the feed is reported stale
//! after the refresh interval.

use crate::error::Result;
use crate::migrations::{open_db, open_memory, CONTENT_MIGRATIONS};
use crate::model::Item;
use crate::settings::Settings;
use crate::util::{DAY_MS, HOUR_MS, MINUTE_MS};
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Mutex;

pub const KIND_META: &str = "meta";
pub const KIND_ITEM: &str = "item";
pub const KIND_COMMENTS: &str = "comments";
pub const KIND_SEARCH: &str = "search";
pub const KIND_TRANSCRIPT: &str = "transcript";
pub const KIND_THUMB: &str = "thumb";

/// How long a kind of content stays fresh. `None` = persistent.
pub fn ttl_ms(kind: &str, settings: &Settings) -> Option<i64> {
    match kind {
        KIND_META | KIND_ITEM | KIND_THUMB => Some(settings.metadata_ttl_days as i64 * DAY_MS),
        KIND_COMMENTS => Some(settings.comments_ttl_hours as i64 * HOUR_MS),
        KIND_SEARCH => Some(settings.search_ttl_hours as i64 * HOUR_MS),
        KIND_TRANSCRIPT => None,
        _ => Some(DAY_MS),
    }
}

const SNAPSHOT_MAX_AGE_MS: i64 = 30 * DAY_MS;
const SNAPSHOT_MAX_PER_KIND: i64 = 500;

pub struct ContentCache {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SnapshotMeta {
    pub id: i64,
    pub kind: String,
    pub captured_at: i64,
    pub count: i64,
    pub source_url: Option<String>,
    pub query: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub meta: SnapshotMeta,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Progress {
    pub video_id: String,
    pub position_seconds: f64,
    pub duration_seconds: Option<f64>,
    pub watch_height: Option<u32>,
    pub first_played_at: i64,
    pub updated_at: i64,
}

impl Progress {
    pub fn fraction(&self) -> Option<f64> {
        let d = self.duration_seconds?;
        (d > 0.0).then(|| (self.position_seconds / d).clamp(0.0, 1.0))
    }
}

fn compress(data: &[u8]) -> Vec<u8> {
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::fast());
    let _ = enc.write_all(data);
    enc.finish().unwrap_or_default()
}

fn decompress(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    ZlibDecoder::new(data).read_to_end(&mut out)?;
    Ok(out)
}

impl ContentCache {
    pub fn open(path: &Path) -> Result<Self> {
        Ok(ContentCache { conn: Mutex::new(open_db(path, CONTENT_MIGRATIONS)?) })
    }

    pub fn in_memory() -> Result<Self> {
        Ok(ContentCache { conn: Mutex::new(open_memory(CONTENT_MIGRATIONS)?) })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    // --- key/value content with TTL -------------------------------------------------

    pub fn put_json(&self, kind: &str, key: &str, value: &Value, ttl: Option<i64>, now: i64) -> Result<()> {
        let raw = serde_json::to_vec(value)?;
        let data = compress(&raw);
        self.put_blob(kind, key, &data, true, ttl, now, raw.len() as i64)
    }

    pub fn put_raw(&self, kind: &str, key: &str, bytes: &[u8], ttl: Option<i64>, now: i64) -> Result<()> {
        self.put_blob(kind, key, bytes, false, ttl, now, bytes.len() as i64)
    }

    #[allow(clippy::too_many_arguments)]
    fn put_blob(&self, kind: &str, key: &str, data: &[u8], compressed: bool, ttl: Option<i64>, now: i64, _raw: i64) -> Result<()> {
        self.lock().execute(
            "INSERT INTO content (kind, key, data, compressed, fetched_at, expires_at, size)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(kind, key) DO UPDATE SET data = excluded.data, compressed = excluded.compressed,
               fetched_at = excluded.fetched_at, expires_at = excluded.expires_at, size = excluded.size",
            params![kind, key, data, compressed as i64, now, ttl.map(|t| now + t), data.len() as i64],
        )?;
        Ok(())
    }

    fn get_blob(&self, kind: &str, key: &str, now: i64) -> Result<Option<(Vec<u8>, bool, i64)>> {
        Ok(self
            .lock()
            .query_row(
                "SELECT data, compressed, fetched_at FROM content
                 WHERE kind = ?1 AND key = ?2 AND (expires_at IS NULL OR expires_at > ?3)",
                params![kind, key, now],
                |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, i64>(1)? != 0, r.get::<_, i64>(2)?)),
            )
            .optional()?)
    }

    /// Fresh JSON only: an expired row reads as absent.
    pub fn get_json(&self, kind: &str, key: &str, now: i64) -> Result<Option<Value>> {
        match self.get_blob(kind, key, now)? {
            None => Ok(None),
            Some((data, compressed, _)) => {
                let raw = if compressed { decompress(&data)? } else { data };
                Ok(Some(serde_json::from_slice(&raw)?))
            }
        }
    }

    pub fn get_raw(&self, kind: &str, key: &str, now: i64) -> Result<Option<Vec<u8>>> {
        Ok(self.get_blob(kind, key, now)?.map(|(d, c, _)| if c { decompress(&d).unwrap_or_default() } else { d }))
    }

    pub fn has(&self, kind: &str, key: &str, now: i64) -> bool {
        self.get_blob(kind, key, now).ok().flatten().is_some()
    }

    /// Deletes expired rows and old feed snapshots. Returns the rows removed.
    pub fn purge_expired(&self, now: i64) -> Result<usize> {
        let conn = self.lock();
        let mut removed = conn.execute("DELETE FROM content WHERE expires_at IS NOT NULL AND expires_at <= ?1", [now])?;
        removed += conn.execute("DELETE FROM feed_snapshots WHERE captured_at < ?1", [now - SNAPSHOT_MAX_AGE_MS])?;
        Ok(removed)
    }

    pub fn stored_bytes(&self) -> i64 {
        self.lock()
            .query_row("SELECT COALESCE(SUM(size), 0) FROM content", [], |r| r.get(0))
            .unwrap_or(0)
    }

    // --- feed snapshots --------------------------------------------------------------

    /// Every snapshot is kept (up to the age/count limits); nothing overwrites an old one.
    pub fn add_snapshot(&self, kind: &str, source_url: Option<&str>, query: Option<&str>, captured_at: i64, items: &[Item]) -> Result<i64> {
        let data = compress(&serde_json::to_vec(items)?);
        let conn = self.lock();
        conn.execute(
            "INSERT INTO feed_snapshots (kind, source_url, query, captured_at, count, data) VALUES (?1,?2,?3,?4,?5,?6)",
            params![kind, source_url, query, captured_at, items.len() as i64, data],
        )?;
        let id = conn.last_insert_rowid();
        conn.execute(
            "DELETE FROM feed_snapshots WHERE kind = ?1 AND id NOT IN
             (SELECT id FROM feed_snapshots WHERE kind = ?1 ORDER BY captured_at DESC, id DESC LIMIT ?2)",
            params![kind, SNAPSHOT_MAX_PER_KIND],
        )?;
        Ok(id)
    }

    pub fn list_snapshots(&self, kind: Option<&str>, limit: i64) -> Result<Vec<SnapshotMeta>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, kind, captured_at, count, source_url, query FROM feed_snapshots
             WHERE (?1 IS NULL OR kind = ?1) ORDER BY captured_at DESC, id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![kind, limit], snapshot_meta)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn get_snapshot(&self, id: i64) -> Result<Option<Snapshot>> {
        self.read_snapshot("WHERE id = ?1", params![id])
    }

    pub fn latest_snapshot(&self, kind: &str) -> Result<Option<Snapshot>> {
        self.read_snapshot("WHERE kind = ?1 ORDER BY captured_at DESC, id DESC LIMIT 1", params![kind])
    }

    fn read_snapshot(&self, clause: &str, args: impl rusqlite::Params) -> Result<Option<Snapshot>> {
        let conn = self.lock();
        let sql = format!("SELECT id, kind, captured_at, count, source_url, query, data FROM feed_snapshots {clause}");
        let row = conn
            .query_row(&sql, args, |r| Ok((snapshot_meta(r)?, r.get::<_, Vec<u8>>(6)?)))
            .optional()?;
        match row {
            None => Ok(None),
            Some((meta, data)) => {
                let items: Vec<Item> = serde_json::from_slice(&decompress(&data)?)?;
                Ok(Some(Snapshot { meta, items }))
            }
        }
    }

    /// The feed wants a refresh when its newest snapshot is older than the interval.
    pub fn feed_is_stale(&self, kind: &str, now: i64, refresh_minutes: u32) -> bool {
        match self.list_snapshots(Some(kind), 1).ok().and_then(|v| v.into_iter().next()) {
            None => true,
            Some(latest) => now - latest.captured_at >= refresh_minutes as i64 * MINUTE_MS,
        }
    }

    // --- watch progress --------------------------------------------------------------

    /// Records playback position. Returns true when this is the first time the
    /// video has been played (the "first play" encounter).
    pub fn record_progress(&self, video_id: &str, position: f64, duration: Option<f64>, height: Option<u32>, now: i64) -> Result<bool> {
        let conn = self.lock();
        let existing: Option<i64> = conn
            .query_row("SELECT first_played_at FROM progress WHERE video_id = ?1", [video_id], |r| r.get(0))
            .optional()?;
        conn.execute(
            "INSERT INTO progress (video_id, position_seconds, duration_seconds, watch_height, first_played_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)
             ON CONFLICT(video_id) DO UPDATE SET position_seconds = excluded.position_seconds,
               duration_seconds = COALESCE(excluded.duration_seconds, duration_seconds),
               watch_height = COALESCE(excluded.watch_height, watch_height),
               updated_at = excluded.updated_at",
            params![video_id, position.max(0.0), duration, height, now],
        )?;
        Ok(existing.is_none())
    }

    pub fn get_progress(&self, video_id: &str) -> Result<Option<Progress>> {
        Ok(self
            .lock()
            .query_row(
                "SELECT video_id, position_seconds, duration_seconds, watch_height, first_played_at, updated_at
                 FROM progress WHERE video_id = ?1",
                [video_id],
                |r| {
                    Ok(Progress {
                        video_id: r.get(0)?,
                        position_seconds: r.get(1)?,
                        duration_seconds: r.get(2)?,
                        watch_height: r.get::<_, Option<i64>>(3)?.map(|h| h as u32),
                        first_played_at: r.get(4)?,
                        updated_at: r.get(5)?,
                    })
                },
            )
            .optional()?)
    }

    // --- seen videos (new-upload detection) -------------------------------------------

    /// Marks ids as seen; returns those that were not seen before, in input order.
    pub fn mark_seen(&self, ids: &[String], now: i64) -> Result<Vec<String>> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let mut fresh = Vec::new();
        for id in ids {
            let n = tx.execute("INSERT OR IGNORE INTO seen_videos (video_id, first_seen_at) VALUES (?1, ?2)", params![id, now])?;
            if n == 1 {
                fresh.push(id.clone());
            }
        }
        tx.commit()?;
        Ok(fresh)
    }
}

fn snapshot_meta(r: &rusqlite::Row<'_>) -> rusqlite::Result<SnapshotMeta> {
    Ok(SnapshotMeta { id: r.get(0)?, kind: r.get(1)?, captured_at: r.get(2)?, count: r.get(3)?, source_url: r.get(4)?, query: r.get(5)? })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn item(id: &str) -> Item {
        Item { video_id: id.into(), title: format!("title {id}"), ..Item::default() }
    }

    #[test]
    fn ttl_table_matches_the_design() {
        let s = Settings::default();
        assert_eq!(ttl_ms(KIND_COMMENTS, &s), Some(24 * HOUR_MS));
        assert_eq!(ttl_ms(KIND_SEARCH, &s), Some(2 * HOUR_MS));
        assert_eq!(ttl_ms(KIND_META, &s), Some(21 * DAY_MS));
        assert_eq!(ttl_ms(KIND_THUMB, &s), Some(21 * DAY_MS));
        assert_eq!(ttl_ms(KIND_TRANSCRIPT, &s), None);
    }

    #[test]
    fn entries_expire_at_their_ttl() {
        let c = ContentCache::in_memory().unwrap();
        let t0 = 1_000_000;
        c.put_json(KIND_COMMENTS, "vid", &json!({"a": [1, 2, 3]}), Some(24 * HOUR_MS), t0).unwrap();
        c.put_json(KIND_SEARCH, "q", &json!({"r": 1}), Some(2 * HOUR_MS), t0).unwrap();
        c.put_json(KIND_TRANSCRIPT, "vid", &json!({"t": "hello"}), None, t0).unwrap();
        // Fresh just before expiry.
        assert!(c.get_json(KIND_SEARCH, "q", t0 + 2 * HOUR_MS - 1).unwrap().is_some());
        // Gone at expiry, while comments still live.
        assert!(c.get_json(KIND_SEARCH, "q", t0 + 2 * HOUR_MS).unwrap().is_none());
        assert!(c.get_json(KIND_COMMENTS, "vid", t0 + 2 * HOUR_MS).unwrap().is_some());
        assert!(c.get_json(KIND_COMMENTS, "vid", t0 + 24 * HOUR_MS).unwrap().is_none());
        // Transcripts never expire.
        assert!(c.get_json(KIND_TRANSCRIPT, "vid", t0 + 3650 * DAY_MS).unwrap().is_some());
        // Purge removes exactly the expired rows.
        let removed = c.purge_expired(t0 + 25 * HOUR_MS).unwrap();
        assert_eq!(removed, 2);
        assert!(c.get_json(KIND_TRANSCRIPT, "vid", t0 + 25 * HOUR_MS).unwrap().is_some());
    }

    #[test]
    fn json_is_compressed_and_roundtrips() {
        let c = ContentCache::in_memory().unwrap();
        let big = json!({"text": "abc".repeat(20_000)});
        c.put_json(KIND_META, "v", &big, Some(DAY_MS), 0).unwrap();
        assert!(c.stored_bytes() < 5_000, "60 kB of repetitive JSON should compress to a few kB");
        assert_eq!(c.get_json(KIND_META, "v", 1).unwrap().unwrap(), big);
    }

    #[test]
    fn raw_blobs_such_as_thumbnails() {
        let c = ContentCache::in_memory().unwrap();
        c.put_raw(KIND_THUMB, "v", &[0xff, 0xd8, 0xff], Some(DAY_MS), 0).unwrap();
        assert_eq!(c.get_raw(KIND_THUMB, "v", 1).unwrap().unwrap(), vec![0xff, 0xd8, 0xff]);
        assert!(c.has(KIND_THUMB, "v", 1));
        assert!(!c.has(KIND_THUMB, "v", DAY_MS + 1));
    }

    #[test]
    fn every_feed_snapshot_is_kept_and_staleness_follows_the_interval() {
        let c = ContentCache::in_memory().unwrap();
        assert!(c.feed_is_stale("home", 0, 45));
        let t0 = 10 * DAY_MS;
        let a = c.add_snapshot("home", Some("https://www.youtube.com/"), None, t0, &[item("aaaaaaaaaaa")]).unwrap();
        let b = c.add_snapshot("home", None, None, t0 + 10 * MINUTE_MS, &[item("bbbbbbbbbbb"), item("ccccccccccc")]).unwrap();
        assert_ne!(a, b);
        let list = c.list_snapshots(Some("home"), 10).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, b, "newest first");
        assert_eq!(c.get_snapshot(a).unwrap().unwrap().items[0].video_id, "aaaaaaaaaaa");
        assert_eq!(c.latest_snapshot("home").unwrap().unwrap().items.len(), 2);
        assert!(!c.feed_is_stale("home", t0 + 10 * MINUTE_MS + 44 * MINUTE_MS, 45));
        assert!(c.feed_is_stale("home", t0 + 10 * MINUTE_MS + 45 * MINUTE_MS, 45));
        // Other kinds are independent; old snapshots age out.
        assert!(c.feed_is_stale("subscriptions", t0, 45));
        c.purge_expired(t0 + 40 * DAY_MS).unwrap();
        assert!(c.list_snapshots(None, 10).unwrap().is_empty());
    }

    #[test]
    fn progress_first_play_and_resume() {
        let c = ContentCache::in_memory().unwrap();
        assert!(c.record_progress("dQw4w9WgXcQ", 3.0, Some(200.0), Some(360), 1).unwrap());
        assert!(!c.record_progress("dQw4w9WgXcQ", 50.0, None, None, 2).unwrap());
        let p = c.get_progress("dQw4w9WgXcQ").unwrap().unwrap();
        assert_eq!(p.position_seconds, 50.0);
        assert_eq!(p.duration_seconds, Some(200.0), "duration kept when a later update omits it");
        assert_eq!(p.watch_height, Some(360));
        assert!((p.fraction().unwrap() - 0.25).abs() < 1e-9);
        assert!(c.get_progress("zzzzzzzzzzz").unwrap().is_none());
    }

    #[test]
    fn seen_videos_report_only_new_ones() {
        let c = ContentCache::in_memory().unwrap();
        let ids: Vec<String> = vec!["a".into(), "b".into(), "a".into()];
        assert_eq!(c.mark_seen(&ids, 1).unwrap(), vec!["a", "b"]);
        assert_eq!(c.mark_seen(&["b".to_string(), "c".to_string()], 2).unwrap(), vec!["c"]);
    }
}
