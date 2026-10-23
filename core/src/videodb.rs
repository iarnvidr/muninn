//! (a) The video cache index: which entries exist, where their bytes are, and
//! their state. Also holds rules and settings.
//! The bytes themselves live as files under `cache/` and `library/`.

use crate::error::{Error, Result};
use crate::migrations::{open_db, open_memory, VIDEO_MIGRATIONS};
use crate::rules::{seed_rules, Rule};
use crate::settings::Settings;
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::path::Path;
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq)]
pub struct EntryRow {
    pub id: String,
    pub video_id: String,
    pub variant: String,
    pub requested_quality: String,
    pub actual_height: Option<u32>,
    pub status: String,
    pub error: Option<String>,
    pub protected: bool,
    pub origin: String,
    pub size_bytes: u64,
    pub expected_bytes: Option<u64>,
    pub progress: f64,
    pub speed_bps: Option<u64>,
    pub file_path: Option<String>,
    pub ext: Option<String>,
    pub progressive: bool,
    pub created_at: i64,
    pub last_access_at: i64,
    pub started_at: Option<i64>,
    pub expires_at: Option<i64>,
    pub retention_days: Option<u32>,
    pub rule: Option<String>,
    pub title: Option<String>,
    pub channel: Option<String>,
    pub duration_seconds: Option<u32>,
}

pub const STATUS_QUEUED: &str = "queued";
pub const STATUS_DOWNLOADING: &str = "downloading";
pub const STATUS_COMPLETE: &str = "complete";
pub const STATUS_FAILED: &str = "failed";
pub const STATUS_CANCELED: &str = "canceled";

impl EntryRow {
    pub fn new(video_id: &str, variant: &str, requested: &str, origin: &str, now: i64) -> EntryRow {
        EntryRow {
            id: entry_id(video_id, variant),
            video_id: video_id.into(),
            variant: variant.into(),
            requested_quality: requested.into(),
            actual_height: None,
            status: STATUS_QUEUED.into(),
            error: None,
            protected: false,
            origin: origin.into(),
            size_bytes: 0,
            expected_bytes: None,
            progress: 0.0,
            speed_bps: None,
            file_path: None,
            ext: None,
            progressive: false,
            created_at: now,
            last_access_at: now,
            started_at: None,
            expires_at: None,
            retention_days: None,
            rule: None,
            title: None,
            channel: None,
            duration_seconds: None,
        }
    }

    pub fn is_complete(&self) -> bool {
        self.status == STATUS_COMPLETE
    }
}

/// `<videoId>.<variant>`; neither part can contain a dot or slash.
pub fn entry_id(video_id: &str, variant: &str) -> String {
    format!("{video_id}.{variant}")
}

/// Splits an entry id. A bare video id yields `(id, None)`.
pub fn split_entry_id(id: &str) -> Option<(&str, Option<&str>)> {
    match id.split_once('.') {
        None => crate::util::valid_video_id(id).then_some((id, None)),
        Some((video, variant)) => {
            (crate::util::valid_video_id(video) && matches!(variant, "audio" | "360" | "480" | "720" | "1080")).then_some((video, Some(variant)))
        }
    }
}

const ENTRY_COLS: &str = "id, video_id, variant, requested_quality, actual_height, status, error, protected, origin, size_bytes,
    expected_bytes, progress, speed_bps, file_path, ext, progressive, created_at, last_access_at, started_at, expires_at,
    retention_days, rule, title, channel, duration_seconds";

fn entry_from(r: &Row<'_>) -> rusqlite::Result<EntryRow> {
    Ok(EntryRow {
        id: r.get(0)?,
        video_id: r.get(1)?,
        variant: r.get(2)?,
        requested_quality: r.get(3)?,
        actual_height: r.get::<_, Option<i64>>(4)?.map(|v| v as u32),
        status: r.get(5)?,
        error: r.get(6)?,
        protected: r.get::<_, i64>(7)? != 0,
        origin: r.get(8)?,
        size_bytes: r.get::<_, i64>(9)?.max(0) as u64,
        expected_bytes: r.get::<_, Option<i64>>(10)?.map(|v| v.max(0) as u64),
        progress: r.get(11)?,
        speed_bps: r.get::<_, Option<i64>>(12)?.map(|v| v.max(0) as u64),
        file_path: r.get(13)?,
        ext: r.get(14)?,
        progressive: r.get::<_, i64>(15)? != 0,
        created_at: r.get(16)?,
        last_access_at: r.get(17)?,
        started_at: r.get(18)?,
        expires_at: r.get(19)?,
        retention_days: r.get::<_, Option<i64>>(20)?.map(|v| v as u32),
        rule: r.get(21)?,
        title: r.get(22)?,
        channel: r.get(23)?,
        duration_seconds: r.get::<_, Option<i64>>(24)?.map(|v| v as u32),
    })
}

pub struct VideoDb {
    conn: Mutex<Connection>,
}

impl VideoDb {
    pub fn open(path: &Path) -> Result<Self> {
        let db = VideoDb { conn: Mutex::new(open_db(path, VIDEO_MIGRATIONS)?) };
        db.seed()?;
        Ok(db)
    }

    pub fn in_memory() -> Result<Self> {
        let db = VideoDb { conn: Mutex::new(open_memory(VIDEO_MIGRATIONS)?) };
        db.seed()?;
        Ok(db)
    }

    fn seed(&self) -> Result<()> {
        if self.kv_get("rules")?.is_none() {
            self.put_rules(&seed_rules())?;
        }
        if self.kv_get("settings")?.is_none() {
            self.put_settings(&Settings::default())?;
        }
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    // --- kv: settings, rules ---------------------------------------------------------

    pub fn kv_get(&self, key: &str) -> Result<Option<String>> {
        Ok(self.lock().query_row("SELECT value FROM kv WHERE key = ?1", [key], |r| r.get(0)).optional()?)
    }

    pub fn kv_put(&self, key: &str, value: &str) -> Result<()> {
        self.lock().execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub(crate) fn kv_put_many(&self, values: &[(&str, String)]) -> Result<()> {
        let mut db = self.lock();
        let tx = db.transaction()?;
        for (key, value) in values {
            tx.execute("INSERT INTO kv (key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![key,value])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn settings(&self) -> Settings {
        self.kv_get("settings")
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
            .unwrap_or_default()
            .sanitized()
    }

    pub fn put_settings(&self, settings: &Settings) -> Result<()> {
        self.kv_put("settings", &serde_json::to_string(&settings.clone().sanitized())?)
    }

    pub fn rules(&self) -> Vec<Rule> {
        self.kv_get("rules").ok().flatten().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(seed_rules)
    }

    pub fn put_rules(&self, rules: &[Rule]) -> Result<()> {
        self.kv_put("rules", &serde_json::to_string(rules)?)
    }

    // --- entries ---------------------------------------------------------------------

    pub fn insert_entry(&self, e: &EntryRow) -> Result<()> {
        self.lock().execute(
            &format!("INSERT INTO entries ({ENTRY_COLS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25)"),
            params![
                e.id, e.video_id, e.variant, e.requested_quality, e.actual_height, e.status, e.error, e.protected as i64, e.origin,
                e.size_bytes as i64, e.expected_bytes.map(|v| v as i64), e.progress, e.speed_bps.map(|v| v as i64), e.file_path, e.ext,
                e.progressive as i64, e.created_at, e.last_access_at, e.started_at, e.expires_at, e.retention_days, e.rule, e.title,
                e.channel, e.duration_seconds
            ],
        )?;
        Ok(())
    }

    pub fn get_entry(&self, id: &str) -> Result<Option<EntryRow>> {
        Ok(self
            .lock()
            .query_row(&format!("SELECT {ENTRY_COLS} FROM entries WHERE id = ?1"), [id], entry_from)
            .optional()?)
    }

    pub fn list_entries(&self) -> Result<Vec<EntryRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!("SELECT {ENTRY_COLS} FROM entries ORDER BY last_access_at DESC, id"))?;
        let rows = stmt.query_map([], entry_from)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn entries_for_video(&self, video_id: &str) -> Result<Vec<EntryRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!("SELECT {ENTRY_COLS} FROM entries WHERE video_id = ?1 ORDER BY id"))?;
        let rows = stmt.query_map([video_id], entry_from)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn entries_with_status(&self, status: &str) -> Result<Vec<EntryRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!("SELECT {ENTRY_COLS} FROM entries WHERE status = ?1 ORDER BY created_at, id"))?;
        let rows = stmt.query_map([status], entry_from)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn delete_entry(&self, id: &str) -> Result<()> {
        self.lock().execute("DELETE FROM entries WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn set_status(&self, id: &str, status: &str, error: Option<&str>, now: i64) -> Result<()> {
        let started = if status == STATUS_DOWNLOADING { Some(now) } else { None };
        self.lock().execute(
            "UPDATE entries SET status = ?2, error = ?3, started_at = COALESCE(?4, started_at),
               speed_bps = CASE WHEN ?2 = 'downloading' THEN speed_bps ELSE NULL END WHERE id = ?1",
            params![id, status, error, started],
        )?;
        Ok(())
    }

    pub fn set_progress(&self, id: &str, size: u64, expected: Option<u64>, progress: f64, speed: Option<u64>) -> Result<()> {
        self.lock().execute(
            "UPDATE entries SET size_bytes = ?2, expected_bytes = COALESCE(?3, expected_bytes), progress = ?4, speed_bps = ?5 WHERE id = ?1",
            params![id, size as i64, expected.map(|v| v as i64), progress, speed.map(|v| v as i64)],
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn set_file(&self, id: &str, path: &str, ext: &str, expected: Option<u64>, actual_height: Option<u32>, progressive: bool) -> Result<()> {
        self.lock().execute(
            "UPDATE entries SET file_path = ?2, ext = ?3, expected_bytes = COALESCE(?4, expected_bytes),
               actual_height = COALESCE(?5, actual_height), progressive = ?6 WHERE id = ?1",
            params![id, path, ext, expected.map(|v| v as i64), actual_height, progressive as i64],
        )?;
        Ok(())
    }

    pub fn complete_entry(&self, id: &str, size: u64, actual_height: Option<u32>) -> Result<()> {
        self.lock().execute(
            "UPDATE entries SET status = 'complete', error = NULL, size_bytes = ?2, expected_bytes = ?2, progress = 1.0,
               speed_bps = NULL, actual_height = COALESCE(?3, actual_height) WHERE id = ?1",
            params![id, size as i64, actual_height],
        )?;
        Ok(())
    }

    pub fn touch(&self, id: &str, now: i64) -> Result<()> {
        self.lock().execute("UPDATE entries SET last_access_at = ?2 WHERE id = ?1", params![id, now])?;
        Ok(())
    }

    pub fn set_expiry(&self, id: &str, expires_at: Option<i64>, retention_days: Option<u32>, rule: Option<&str>) -> Result<()> {
        self.lock().execute(
            "UPDATE entries SET expires_at = ?2, retention_days = ?3, rule = COALESCE(?4, rule) WHERE id = ?1",
            params![id, expires_at, retention_days, rule],
        )?;
        Ok(())
    }

    pub fn set_protected(&self, id: &str, file_path: Option<&str>) -> Result<()> {
        self.lock().execute(
            "UPDATE entries SET protected = 1, expires_at = NULL, file_path = COALESCE(?2, file_path) WHERE id = ?1",
            params![id, file_path],
        )?;
        Ok(())
    }

    pub fn set_unprotected(&self, id: &str, file_path: Option<&str>, expires_at: Option<i64>) -> Result<()> {
        self.lock().execute(
            "UPDATE entries SET protected = 0, expires_at = ?3, file_path = COALESCE(?2, file_path) WHERE id = ?1",
            params![id, file_path, expires_at],
        )?;
        Ok(())
    }

    /// After a restart nothing is running: downloads that were mid-flight go back to the queue.
    pub fn requeue_interrupted(&self) -> Result<usize> {
        Ok(self.lock().execute("UPDATE entries SET status = 'queued', speed_bps = NULL WHERE status = 'downloading'", [])?)
    }

    pub fn cache_bytes(&self) -> u64 {
        self.sum_size(false)
    }

    pub fn library_bytes(&self) -> u64 {
        self.sum_size(true)
    }

    fn sum_size(&self, protected: bool) -> u64 {
        self.lock()
            .query_row("SELECT COALESCE(SUM(size_bytes), 0) FROM entries WHERE protected = ?1", [protected as i64], |r| r.get::<_, i64>(0))
            .unwrap_or(0)
            .max(0) as u64
    }

    pub fn ensure_open(&self) -> Result<()> {
        self.lock().query_row("SELECT 1", [], |r| r.get::<_, i64>(0)).map(|_| ()).map_err(Error::from)
    }
}

/// Merges overlapping or adjacent inclusive byte ranges.
pub fn merge_ranges(mut ranges: Vec<(i64, i64)>) -> Vec<(i64, i64)> {
    ranges.sort();
    let mut out: Vec<(i64, i64)> = Vec::new();
    for (s, e) in ranges {
        match out.last_mut() {
            Some(last) if s <= last.1 + 1 => last.1 = last.1.max(e),
            _ => out.push((s, e)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_ids_parse() {
        assert_eq!(split_entry_id("dQw4w9WgXcQ.360"), Some(("dQw4w9WgXcQ", Some("360"))));
        assert_eq!(split_entry_id("dQw4w9WgXcQ"), Some(("dQw4w9WgXcQ", None)));
        assert_eq!(split_entry_id("dQw4w9WgXcQ.1080"), Some(("dQw4w9WgXcQ",Some("1080"))));
        assert_eq!(split_entry_id("dQw4w9WgXcQ.2160"), None);
        assert_eq!(split_entry_id("../x.360"), None);
        assert_eq!(split_entry_id("dQw4w9WgXcQ.360/../../x"), None);
    }

    #[test]
    fn seeds_rules_and_settings_once() {
        let db = VideoDb::in_memory().unwrap();
        assert_eq!(db.rules().len(), 5);
        assert_eq!(db.settings(), Settings::default());
        let mut s = db.settings();
        s.download_concurrency = 3;
        db.put_settings(&s).unwrap();
        db.seed().unwrap();
        assert_eq!(db.settings().download_concurrency, 3, "re-seeding must not reset user edits");
    }

    #[test]
    fn entry_lifecycle() {
        let db = VideoDb::in_memory().unwrap();
        let mut e = EntryRow::new("dQw4w9WgXcQ", "360", "360", "manual", 100);
        e.title = Some("Title".into());
        db.insert_entry(&e).unwrap();
        db.set_status(&e.id, STATUS_DOWNLOADING, None, 110).unwrap();
        db.set_file(&e.id, "/tmp/x.mp4", "mp4", Some(1000), Some(360), true).unwrap();
        db.set_progress(&e.id, 500, None, 0.5, Some(100)).unwrap();
        let got = db.get_entry(&e.id).unwrap().unwrap();
        assert_eq!((got.status.as_str(), got.size_bytes, got.expected_bytes, got.progressive), ("downloading", 500, Some(1000), true));
        assert_eq!(db.cache_bytes(), 500);
        db.complete_entry(&e.id, 1000, None).unwrap();
        assert!(db.get_entry(&e.id).unwrap().unwrap().is_complete());
        db.set_protected(&e.id, Some("/lib/x.mp4")).unwrap();
        assert_eq!((db.cache_bytes(), db.library_bytes()), (0, 1000));
        assert_eq!(db.requeue_interrupted().unwrap(), 0);
        db.delete_entry(&e.id).unwrap();
        assert!(db.get_entry(&e.id).unwrap().is_none());
    }

    #[test]
    fn interrupted_downloads_return_to_the_queue() {
        let db = VideoDb::in_memory().unwrap();
        let e = EntryRow::new("dQw4w9WgXcQ", "audio", "audio", "rule", 1);
        db.insert_entry(&e).unwrap();
        db.set_status(&e.id, STATUS_DOWNLOADING, None, 2).unwrap();
        assert_eq!(db.requeue_interrupted().unwrap(), 1);
        assert_eq!(db.get_entry(&e.id).unwrap().unwrap().status, "queued");
    }

    #[test]
    fn range_merging() {
        assert_eq!(merge_ranges(vec![(5, 9), (0, 4), (20, 30), (25, 26)]), vec![(0, 9), (20, 30)]);
    }
}
