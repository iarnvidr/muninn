//! (c) Browsing state: tiny SQLite. Tab URLs, history, scroll position, the
//! selected video and its playback timestamp.

use crate::error::Result;
use crate::migrations::{open_db, open_memory, BROWSING_MIGRATIONS};
use crate::model::TabIngest;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::Path;
use std::sync::Mutex;

const HISTORY_MAX: i64 = 5000;

pub struct BrowsingState {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TabRow {
    pub tab_id: String,
    pub url: String,
    pub title: Option<String>,
    pub video_id: Option<String>,
    pub scroll_y: Option<f64>,
    pub position_seconds: Option<f64>,
    pub duration_seconds: Option<f64>,
    pub watch_height: Option<u32>,
    pub state: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HistoryRow {
    pub url: String,
    pub title: Option<String>,
    pub video_id: Option<String>,
    pub ts: i64,
}

/// What changed when a tab report was applied.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TabChange {
    /// The tab now shows a video it did not show before ("opened a tab").
    pub opened_video: Option<String>,
}

impl BrowsingState {
    pub fn open(path: &Path) -> Result<Self> {
        Ok(BrowsingState { conn: Mutex::new(open_db(path, BROWSING_MIGRATIONS)?) })
    }

    pub fn in_memory() -> Result<Self> {
        Ok(BrowsingState { conn: Mutex::new(open_memory(BROWSING_MIGRATIONS)?) })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn upsert_tab(&self, t: &TabIngest, now: i64) -> Result<TabChange> {
        let conn = self.lock();
        let previous: Option<(String, Option<String>)> = conn
            .query_row("SELECT url, video_id FROM tabs WHERE tab_id = ?1", [&t.tab_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        if t.state == "closed" {
            conn.execute("DELETE FROM tabs WHERE tab_id = ?1", [&t.tab_id])?;
            return Ok(TabChange::default());
        }
        let ts = t.ts.unwrap_or(now);
        // Keep known values when a report omits them (a hidden tab reports no scroll).
        conn.execute(
            "INSERT INTO tabs (tab_id, url, title, video_id, scroll_y, position_seconds, duration_seconds, watch_height, state, updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
             ON CONFLICT(tab_id) DO UPDATE SET
               url = excluded.url,
               title = COALESCE(excluded.title, title),
               video_id = excluded.video_id,
               scroll_y = COALESCE(excluded.scroll_y, scroll_y),
               position_seconds = CASE WHEN excluded.video_id IS tabs.video_id
                   THEN COALESCE(excluded.position_seconds, position_seconds) ELSE excluded.position_seconds END,
               duration_seconds = CASE WHEN excluded.video_id IS tabs.video_id
                   THEN COALESCE(excluded.duration_seconds, duration_seconds) ELSE excluded.duration_seconds END,
               watch_height = COALESCE(excluded.watch_height, watch_height),
               state = excluded.state, updated_at = excluded.updated_at",
            params![t.tab_id, t.url, t.title, t.video_id, t.scroll_y, t.position_seconds, t.duration_seconds, t.watch_height, t.state, ts],
        )?;
        let url_changed = previous.as_ref().map_or(true, |(u, _)| *u != t.url);
        if url_changed && !t.url.is_empty() {
            conn.execute(
                "INSERT INTO history (url, title, video_id, ts) VALUES (?1, ?2, ?3, ?4)",
                params![t.url, t.title, t.video_id, ts],
            )?;
            conn.execute(
                "DELETE FROM history WHERE id NOT IN (SELECT id FROM history ORDER BY id DESC LIMIT ?1)",
                [HISTORY_MAX],
            )?;
        }
        let prev_video = previous.and_then(|(_, v)| v);
        let opened_video = match (&t.video_id, &prev_video) {
            (Some(now_v), prev) if prev.as_ref() != Some(now_v) => Some(now_v.clone()),
            _ => None,
        };
        Ok(TabChange { opened_video })
    }

    pub fn tabs(&self) -> Result<Vec<TabRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT tab_id, url, title, video_id, scroll_y, position_seconds, duration_seconds, watch_height, state, updated_at
             FROM tabs ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(TabRow {
                tab_id: r.get(0)?,
                url: r.get(1)?,
                title: r.get(2)?,
                video_id: r.get(3)?,
                scroll_y: r.get(4)?,
                position_seconds: r.get(5)?,
                duration_seconds: r.get(6)?,
                watch_height: r.get::<_, Option<i64>>(7)?.map(|h| h as u32),
                state: r.get(8)?,
                updated_at: r.get(9)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn history(&self, limit: i64) -> Result<Vec<HistoryRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare("SELECT url, title, video_id, ts FROM history ORDER BY id DESC LIMIT ?1")?;
        let rows = stmt.query_map([limit], |r| Ok(HistoryRow { url: r.get(0)?, title: r.get(1)?, video_id: r.get(2)?, ts: r.get(3)? }))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Updates the playback position of every tab showing this video.
    pub fn set_position(&self, video_id: &str, position: f64, duration: Option<f64>, height: Option<u32>, now: i64) -> Result<()> {
        self.lock().execute(
            "UPDATE tabs SET position_seconds = ?2, duration_seconds = COALESCE(?3, duration_seconds),
               watch_height = COALESCE(?4, watch_height), updated_at = ?5 WHERE video_id = ?1",
            params![video_id, position, duration, height, now],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(id: &str, url: &str, video: Option<&str>, state: &str) -> TabIngest {
        TabIngest { tab_id: id.into(), url: url.into(), video_id: video.map(String::from), state: state.into(), ..TabIngest::default() }
    }

    #[test]
    fn tracks_tabs_history_and_open_video_changes() {
        let b = BrowsingState::in_memory().unwrap();
        let c = b.upsert_tab(&tab("1", "https://www.youtube.com/", None, "open"), 10).unwrap();
        assert_eq!(c.opened_video, None);
        let c = b.upsert_tab(&tab("1", "https://www.youtube.com/watch?v=dQw4w9WgXcQ", Some("dQw4w9WgXcQ"), "open"), 11).unwrap();
        assert_eq!(c.opened_video.as_deref(), Some("dQw4w9WgXcQ"));
        // A scroll/position report on the same video does not count as a new open.
        let mut again = tab("1", "https://www.youtube.com/watch?v=dQw4w9WgXcQ", Some("dQw4w9WgXcQ"), "open");
        again.position_seconds = Some(42.0);
        again.scroll_y = Some(300.0);
        assert_eq!(b.upsert_tab(&again, 12).unwrap().opened_video, None);
        // Hiding the tab keeps the last scroll and position.
        b.upsert_tab(&tab("1", "https://www.youtube.com/watch?v=dQw4w9WgXcQ", Some("dQw4w9WgXcQ"), "hidden"), 13).unwrap();
        let rows = b.tabs().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].state, "hidden");
        assert_eq!(rows[0].position_seconds, Some(42.0));
        assert_eq!(rows[0].scroll_y, Some(300.0));
        // History has one row per URL change.
        assert_eq!(b.history(10).unwrap().len(), 2);
        // Closing removes the tab but keeps history.
        b.upsert_tab(&tab("1", "", None, "closed"), 14).unwrap();
        assert!(b.tabs().unwrap().is_empty());
        assert_eq!(b.history(10).unwrap().len(), 2);
    }

    #[test]
    fn position_updates_reach_tabs_on_that_video() {
        let b = BrowsingState::in_memory().unwrap();
        b.upsert_tab(&tab("1", "https://www.youtube.com/watch?v=aaaaaaaaaaa", Some("aaaaaaaaaaa"), "open"), 1).unwrap();
        b.upsert_tab(&tab("2", "https://www.youtube.com/watch?v=bbbbbbbbbbb", Some("bbbbbbbbbbb"), "open"), 1).unwrap();
        b.set_position("aaaaaaaaaaa", 12.5, Some(100.0), Some(720), 5).unwrap();
        let rows = b.tabs().unwrap();
        let a = rows.iter().find(|t| t.tab_id == "1").unwrap();
        let other = rows.iter().find(|t| t.tab_id == "2").unwrap();
        assert_eq!((a.position_seconds, a.watch_height), (Some(12.5), Some(720)));
        assert_eq!(other.position_seconds, None);
    }
}
