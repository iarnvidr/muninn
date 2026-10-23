//! JSON views for the UI and extension, built from the stores.

use crate::content::*;
use crate::engine::Core;
use crate::error::{Error, Result};
use crate::model::Item;
use crate::quality::{serves, Quality};
use crate::util::valid_video_id;
use crate::videodb::EntryRow;
use serde_json::{json, Value};
use std::collections::HashMap;

/// The state shown in the UI: protected, partial (incomplete), speculative
/// (prefetched, never played) or complete.
pub fn display_state(e: &EntryRow, played: bool) -> &'static str {
    if e.protected {
        "protected"
    } else if !e.is_complete() {
        "partial"
    } else if !played && matches!(e.origin.as_str(), "rule" | "prefetch" | "capture") {
        "speculative"
    } else {
        "complete"
    }
}

impl Core {
    pub fn entry_view(&self, e: &EntryRow) -> Value {
        let facts = self.facts_for(&e.video_id);
        let progress = self.content.get_progress(&e.video_id).ok().flatten();
        let completed_at = self.completions().get(&e.video_id).copied();
        let cleanup_due = if e.protected || self.automation().action(&facts) != Some(crate::automation::CompletionAction::Delete) { None }
            else { completed_at.map(|at| at + self.automation().cleanup_delay_seconds as i64*1000) };
        let watch_height = progress.as_ref().and_then(|p| p.watch_height);
        let cache_quality = Quality::parse(&e.variant).unwrap_or(Quality::None);
        let serve = if e.is_complete() || self.streamable(e) {
            serves(Some((cache_quality, e.actual_height)), watch_height)
        } else {
            serves(None, watch_height)
        };
        json!({
            "id": e.id, "video_id": e.video_id, "variant": e.variant, "requested_quality": e.requested_quality,
            "actual_height": e.actual_height,
            "state": display_state(e, progress.is_some()),
            "status": e.status, "error": e.error, "protected": e.protected, "origin": e.origin,
            "size_bytes": e.size_bytes, "expected_bytes": e.expected_bytes, "progress": e.progress, "speed_bps": e.speed_bps,
            "created_at": e.created_at, "last_access_at": e.last_access_at, "expires_at": e.expires_at,
            "completed_at":completed_at,"cleanup_due_at":cleanup_due,"retention_days":e.retention_days,
            "streamable": self.streamable(e), "media_url": format!("/media/{}", e.id),
            "title": facts.title.or(e.title.clone()), "channel": facts.channel.or(e.channel.clone()), "duration_seconds": facts.duration_seconds.or(e.duration_seconds),
            "thumb_url": format!("/thumb/{}", e.video_id),
            "position_seconds": progress.as_ref().map(|p| p.position_seconds), "watch_height": watch_height,
            "serves": serve, "rule": e.rule, "ext": e.ext,
        })
    }

    pub fn entries_view(&self, state: Option<&str>) -> Result<Vec<Value>> {
        Ok(self
            .video
            .list_entries()?
            .iter()
            .map(|e| self.entry_view(e))
            .filter(|v| state.map_or(true, |s| s.is_empty() || v["state"] == s))
            .collect())
    }

    fn item_view(&self, item: &Item, by_video: &HashMap<String, Vec<EntryRow>>) -> Value {
        let best = by_video.get(&item.video_id).and_then(|entries| {
            entries
                .iter()
                .filter(|e| e.status != "failed" && e.status != "canceled")
                .max_by_key(|e| (e.is_complete(), Quality::parse(&e.variant).map_or(0, |q| q.height())))
        });
        let progress = self.content.get_progress(&item.video_id).ok().flatten();
        let mut v = serde_json::to_value(item).unwrap_or(Value::Null);
        v["thumb_url"] = json!(format!("/thumb/{}", item.video_id));
        v["cached"] = match best {
            Some(e) => json!({"id": e.id, "variant": e.variant, "state": display_state(e, progress.is_some()), "progress": e.progress}),
            None => Value::Null,
        };
        v["position_seconds"] = json!(progress.map(|p| p.position_seconds));
        v
    }

    fn entries_by_video(&self) -> HashMap<String, Vec<EntryRow>> {
        let mut map: HashMap<String, Vec<EntryRow>> = HashMap::new();
        for e in self.video.list_entries().unwrap_or_default() {
            map.entry(e.video_id.clone()).or_default().push(e);
        }
        map
    }

    pub fn feed_view(&self, snapshot: Snapshot) -> Value {
        let by_video = self.entries_by_video();
        let settings = self.video.settings();
        let stale = self.content.feed_is_stale(&snapshot.meta.kind, self.now(), settings.feed_refresh_minutes);
        // "stale" describes the feed, not this particular (possibly old) snapshot.
        json!({
            "id": snapshot.meta.id, "kind": snapshot.meta.kind, "captured_at": snapshot.meta.captured_at,
            "source_url": snapshot.meta.source_url, "query": snapshot.meta.query, "stale": stale,
            "items": snapshot.items.iter().map(|i| self.item_view(i, &by_video)).collect::<Vec<_>>(),
        })
    }

    pub fn video_detail(&self, video_id: &str) -> Result<Value> {
        if !valid_video_id(video_id) {
            return Err(Error::bad("invalid video_id"));
        }
        let now = if self.is_offline() { 0 } else { self.now() };
        let meta = self.content.get_json(KIND_META, video_id, now)?;
        let comments = self.content.get_json(KIND_COMMENTS, video_id, now)?;
        let related: Vec<Item> = meta
            .as_ref()
            .and_then(|m| m.get("related").cloned())
            .and_then(|r| serde_json::from_value(r).ok())
            .unwrap_or_default();
        let by_video = self.entries_by_video();
        let entries = self.video.entries_for_video(video_id)?;
        let progress = self.content.get_progress(video_id)?;
        let watch_height = progress.as_ref().and_then(|p| p.watch_height);
        let best = entries
            .iter()
            .filter(|e| self.streamable(e))
            .max_by_key(|e| (e.is_complete(), e.actual_height.unwrap_or_else(|| Quality::parse(&e.variant).map_or(0, |q| q.height()))));
        let serve = serves(best.map(|e| (Quality::parse(&e.variant).unwrap_or(Quality::None), e.actual_height)), watch_height);
        Ok(json!({
            "video_id": video_id, "meta": meta, "comments": comments,
            "has_more_comments":self.has_more_comments(video_id),
            "related": related.iter().map(|i| self.item_view(i, &by_video)).collect::<Vec<_>>(),
            "entries": entries.iter().map(|e| self.entry_view(e)).collect::<Vec<_>>(),
            "position_seconds": progress.as_ref().map(|p| p.position_seconds), "watch_height": watch_height, "serves": serve,
        }))
    }

    pub fn status_view(&self) -> Value {
        let tools = self.tools();
        json!({
            "ok": true, "app": "muninn", "version": crate::engine::VERSION, "paired": true,
            "data_dir": self.paths.data_dir, "port": self.port(),
            "tools": tools,
        })
    }

    pub fn jobs_view(&self) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        for status in ["downloading", "queued", "failed"] {
            for e in self.video.entries_with_status(status)? {
                out.push(json!({"entry_id": e.id, "status": e.status, "progress": e.progress, "speed_bps": e.speed_bps, "error": e.error, "started_at": e.started_at}));
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::testutil::test_core;
    use crate::model::{FeedIngest, VideoIngest};
    use crate::videodb::STATUS_COMPLETE;

    #[test]
    fn state_derivation() {
        let mut e = EntryRow::new("aaaaaaaaaaa", "360", "360", "rule", 1);
        assert_eq!(display_state(&e, false), "partial");
        e.status = STATUS_COMPLETE.into();
        assert_eq!(display_state(&e, false), "speculative");
        assert_eq!(display_state(&e, true), "complete");
        e.origin = "manual".into();
        assert_eq!(display_state(&e, false), "complete");
        e.protected = true;
        assert_eq!(display_state(&e, false), "protected");
    }

    #[test]
    fn feed_view_shows_cache_state_and_progress() {
        let (_d, core) = test_core(None, None);
        let item = |id: &str| Item { video_id: id.into(), title: id.into(), ..Item::default() };
        core.ingest_feed(FeedIngest { kind: "home".into(), items: vec![item("aaaaaaaaaaa"), item("bbbbbbbbbbb")], ..FeedIngest::default() }).unwrap();
        let mut e = EntryRow::new("aaaaaaaaaaa", "360", "360", "rule", 1);
        e.status = STATUS_COMPLETE.into();
        core.video.insert_entry(&e).unwrap();
        let view = core.feed_view(core.content.latest_snapshot("home").unwrap().unwrap());
        assert_eq!(view["items"][0]["cached"]["state"], "speculative");
        assert!(view["items"][1]["cached"].is_null());
        assert_eq!(view["items"][0]["thumb_url"], "/thumb/aaaaaaaaaaa");
        assert_eq!(view["stale"], false);
        core.set_now_for_tests(Some(core.now() + 2 * 3_600_000));
        assert_eq!(core.feed_view(core.content.latest_snapshot("home").unwrap().unwrap())["stale"], true);
    }

    #[test]
    fn video_detail_reports_cache_quality_against_watch_quality() {
        let (_d, core) = test_core(None, None);
        core.ingest_video(VideoIngest { video_id: "aaaaaaaaaaa".into(), title: "T".into(), related: vec![Item { video_id: "bbbbbbbbbbb".into(), title: "R".into(), ..Item::default() }], ..VideoIngest::default() }).unwrap();
        let path = core.paths.video_dir.join("aaaaaaaaaaa.360.mp4");
        std::fs::write(&path, b"x").unwrap();
        let mut e = EntryRow::new("aaaaaaaaaaa", "360", "360", "manual", 1);
        e.status = STATUS_COMPLETE.into();
        e.file_path = Some(path.display().to_string());
        e.actual_height = Some(360);
        core.video.insert_entry(&e).unwrap();
        core.content.record_progress("aaaaaaaaaaa", 5.0, Some(100.0), Some(720), 5).unwrap();
        let d = core.video_detail("aaaaaaaaaaa").unwrap();
        assert_eq!(d["serves"], "downgrade");
        assert_eq!(d["watch_height"], 720);
        assert_eq!(d["related"][0]["video_id"], "bbbbbbbbbbb");
        assert_eq!(d["entries"][0]["serves"], "downgrade");
        assert!(core.video_detail("bad").is_err());
    }
}
