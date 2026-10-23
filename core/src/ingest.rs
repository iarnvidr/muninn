//! What the extension (and the player) send in, and what the rules engine does
//! with it: new uploads, opened tabs and first plays become cache jobs.

use crate::content::*;
use crate::engine::{Core, OpenRequest};
use crate::error::{Error, Result};
use crate::model::*;
use crate::quality::Quality;
use crate::rules::{evaluate, retention_for_request, Decision, Encounter, VideoFacts};
use crate::util::valid_video_id;
use crate::videodb::{EntryRow, STATUS_CANCELED, STATUS_COMPLETE, STATUS_FAILED, STATUS_QUEUED};
use crate::ytdlp::plan_format;
use serde_json::{json, Value};

const FEED_KINDS: &[&str] = &["home", "subscriptions", "search", "related", "channel", "other"];
/// Safety valve: one feed snapshot may queue at most this many automatic jobs.
const MAX_AUTO_QUEUED_PER_INGEST: usize = 10;
const MAX_ITEMS_PER_SNAPSHOT: usize = 500;
pub(crate) const MAX_COMMENTS: usize = 600;

fn clean_items(items: &[Item]) -> Vec<Item> {
    let mut seen = std::collections::HashSet::new();
    items
        .iter()
        .filter(|i| valid_video_id(&i.video_id) && seen.insert(i.video_id.clone()))
        .take(MAX_ITEMS_PER_SNAPSHOT)
        .map(|i| {
            let mut i = i.clone();
            i.title.truncate(300);
            i
        })
        .collect()
}

impl Core {
    // --- ingest ---------------------------------------------------------------------

    pub fn ingest_feed(&self, req: FeedIngest) -> Result<Value> {
        let now = self.now();
        let settings = self.video.settings();
        let kind = if FEED_KINDS.contains(&req.kind.as_str()) { req.kind.clone() } else { "other".to_string() };
        let items = clean_items(&req.items);
        if items.is_empty() {
            return Ok(json!({"ok": true, "snapshot_id": null, "new_videos": [], "queued": []}));
        }
        let captured = req.captured_at.unwrap_or(now);
        let snapshot_id = self.content.add_snapshot(&kind, req.source_url.as_deref(), req.query.as_deref(), captured, &items)?;
        if kind == "search" {
            if let Some(q) = req.query.as_deref() {
                let value = serde_json::to_value(&items)?;
                self.content.put_json(KIND_SEARCH, &q.to_lowercase(), &value, ttl_ms(KIND_SEARCH, &settings), now)?;
            }
        }
        for item in &items {
            self.remember_item(item, now)?;
        }
        let ids: Vec<String> = items.iter().map(|i| i.video_id.clone()).collect();
        let new_videos = self.content.mark_seen(&ids, now)?;
        let mut queued = Vec::new();
        if kind == "subscriptions" && settings.auto_prefetch {
            for id in new_videos.iter().take(MAX_AUTO_QUEUED_PER_INGEST) {
                if let Some(entry) = self.apply_rules(id, Encounter::NewUpload)? {
                    queued.push(entry.id);
                }
            }
        }
        Ok(json!({"ok": true, "snapshot_id": snapshot_id, "new_videos": new_videos, "queued": queued}))
    }

    fn remember_item(&self, item: &Item, now: i64) -> Result<()> {
        let ttl = ttl_ms(KIND_ITEM, &self.video.settings());
        // A feed card knows less than a watch page; never overwrite richer data with blanks.
        let merged = match self.content.get_json(KIND_ITEM, &item.video_id, now)? {
            Some(old) => {
                let old: Item = serde_json::from_value(old).unwrap_or_default();
                Item {
                    video_id: item.video_id.clone(),
                    title: if item.title.is_empty() { old.title } else { item.title.clone() },
                    channel: item.channel.clone().or(old.channel),
                    channel_id: item.channel_id.clone().or(old.channel_id),
                    duration_seconds: item.duration_seconds.or(old.duration_seconds),
                    thumbnail: item.thumbnail.clone().or(old.thumbnail),
                    published_text: item.published_text.clone().or(old.published_text),
                    view_text: item.view_text.clone().or(old.view_text),
                    badge: item.badge.clone().or(old.badge),
                }
            }
            None => item.clone(),
        };
        self.content.put_json(KIND_ITEM, &item.video_id, &serde_json::to_value(&merged)?, ttl, now)?;
        self.queue_thumb(&item.video_id);
        Ok(())
    }

    pub fn ingest_video(&self, req: VideoIngest) -> Result<Value> {
        if !valid_video_id(&req.video_id) {
            return Err(Error::bad("invalid video_id"));
        }
        let now = self.now();
        let settings = self.video.settings();
        let related = clean_items(&req.related);
        let meta = json!({
            "video_id": req.video_id, "title": req.title, "channel": req.channel, "channel_id": req.channel_id,
            "duration_seconds": req.duration_seconds, "description": req.description.as_deref().map(|d| d.chars().take(5000).collect::<String>()),
            "thumbnail": req.thumbnail, "view_count": req.view_count, "published_text": req.published_text,
            "is_live": req.is_live, "related": related, "captured_at": req.captured_at.unwrap_or(now),
        });
        self.content.put_json(KIND_META, &req.video_id, &meta, ttl_ms(KIND_META, &settings), now)?;
        self.remember_item(
            &Item {
                video_id: req.video_id.clone(),
                title: req.title.clone(),
                channel: req.channel.clone(),
                channel_id: req.channel_id.clone(),
                duration_seconds: req.duration_seconds,
                thumbnail: req.thumbnail.clone(),
                published_text: req.published_text.clone(),
                view_text: None,
                badge: None,
            },
            now,
        )?;
        for item in &related {
            self.remember_item(item, now)?;
        }
        if !related.is_empty() {
            self.content.add_snapshot("related", None, None, now, &related)?;
        }
        Ok(json!({"ok": true}))
    }

    pub fn ingest_comments(&self, req: CommentsIngest) -> Result<Value> {
        if !valid_video_id(&req.video_id) {
            return Err(Error::bad("invalid video_id"));
        }
        let now = self.now();
        let mut comments = req.comments;
        comments.truncate(MAX_COMMENTS);
        for c in &mut comments {
            c.text.truncate(4000);
            c.author.truncate(200);
        }
        let value = json!({"video_id": req.video_id, "captured_at": req.captured_at.unwrap_or(now), "total": req.total, "comments": comments});
        self.content.put_json(KIND_COMMENTS, &req.video_id, &value, ttl_ms(KIND_COMMENTS, &self.video.settings()), now)?;
        Ok(json!({"ok": true, "stored": comments.len()}))
    }

    pub fn ingest_tab(&self, mut req: TabIngest) -> Result<Value> {
        let now = self.now();
        if req.tab_id.is_empty() {
            return Err(Error::bad("tab_id is required"));
        }
        if let Some(v) = &req.video_id {
            if !valid_video_id(v) {
                req.video_id = None;
            }
        }
        if !matches!(req.state.as_str(), "open" | "hidden" | "closed") {
            req.state = "open".into();
        }
        req.url.truncate(2000);
        let change = self.browsing.upsert_tab(&req, now)?;
        let mut queued = None;
        if let (Some(video), true) = (change.opened_video, req.state == "open") {
            if self.video.settings().auto_prefetch {
                if let Some(entry) = self.apply_rules(&video, Encounter::OpenTab)? {
                    queued = Some(entry.id);
                }
            }
        }
        Ok(json!({"ok": true, "queued": queued}))
    }

    pub fn ingest_playback(&self, req: PlaybackIngest) -> Result<Value> {
        if !valid_video_id(&req.video_id) {
            return Err(Error::bad("invalid video_id"));
        }
        if !req.position_seconds.is_finite() || req.position_seconds < 0.0
            || req.duration_seconds.is_some_and(|d| !d.is_finite() || d <= 0.0) {
            return Err(Error::bad("invalid playback position or duration"));
        }
        if req.started { self.record_actual_playback(&req.video_id)?; }
        {
            let mut leases=self.playback_leases.lock().unwrap_or_else(|e|e.into_inner());
            leases.retain(|_,until|*until>self.now());
            if req.ended {leases.remove(&req.video_id);} else {leases.insert(req.video_id.clone(),self.now()+120_000);}
        }
        let reached = req.duration_seconds.is_some_and(|d| req.position_seconds / d >= self.automation().completion_percent as f64 / 100.0);
        if reached && (req.ended || req.paused.is_some()) {
            self.mark_completed(&req.video_id)?;
        } else if req.paused == Some(false) {
            self.resume_cleanup(&req.video_id)?;
        }
        let now = self.now();
        let first = self.content.record_progress(&req.video_id, req.position_seconds, req.duration_seconds, req.watch_height, now)?;
        self.browsing.set_position(&req.video_id, req.position_seconds, req.duration_seconds, req.watch_height, now)?;
        for e in self.video.entries_for_video(&req.video_id)? {
            self.video.touch(&e.id, now)?;
        }
        let mut queued = None;
        if first && self.video.settings().auto_prefetch {
            if let Some(entry) = self.apply_rules(&req.video_id, Encounter::FirstPlay)? {
                queued = Some(entry.id);
            }
        }
        Ok(json!({"ok": true, "first_play": first, "queued": queued, "completion_policy": self.completion_policy(&req.video_id)}))
    }

    // --- rules and cache requests -------------------------------------------------------

    pub fn facts_for(&self, video_id: &str) -> VideoFacts {
        let now = self.now();
        let item: Option<Item> = self
            .content
            .get_json(KIND_ITEM, video_id, now)
            .ok()
            .flatten()
            .and_then(|v| serde_json::from_value(v).ok());
        match item {
            Some(i) => VideoFacts {
                video_id: video_id.into(),
                title: Some(i.title).filter(|t| !t.is_empty()),
                channel: i.channel,
                channel_id: i.channel_id,
                duration_seconds: i.duration_seconds,
            },
            None => VideoFacts { video_id: video_id.into(), ..VideoFacts::default() },
        }
    }

    /// Runs the rules for an encounter and queues what they decide.
    pub fn apply_rules(&self, video_id: &str, encounter: Encounter) -> Result<Option<EntryRow>> {
        let facts = self.facts_for(video_id);
        let rules = self.video.rules();
        let Some(decision) = evaluate(&rules, &facts, encounter) else { return Ok(None) };
        if decision.quality == Quality::None {
            return Ok(None);
        }
        Ok(Some(self.create_entry(&facts, decision.quality, &decision, "rule", false)?))
    }

    /// A user request (button in the extension or the UI). An explicit quality
    /// overrides the rule's; retention still comes from the matching rule.
    pub fn request_cache(&self, req: CacheRequest) -> Result<(Option<EntryRow>, Decision)> {
        if !valid_video_id(&req.video_id) {
            return Err(Error::bad("invalid video_id"));
        }
        let now = self.now();
        let mut facts = self.facts_for(&req.video_id);
        if let Some(t) = req.title.clone().filter(|t| !t.is_empty()) {
            facts.title = Some(t);
        }
        facts.channel = req.channel.clone().or(facts.channel);
        facts.channel_id = req.channel_id.clone().or(facts.channel_id);
        facts.duration_seconds = req.duration_seconds.or(facts.duration_seconds);
        // Remember what the page told us so the entry has a title even without a feed.
        if let Some(title) = facts.title.clone() {
            self.remember_item(
                &Item {
                    video_id: req.video_id.clone(),
                    title,
                    channel: facts.channel.clone(),
                    channel_id: facts.channel_id.clone(),
                    duration_seconds: facts.duration_seconds,
                    ..Item::default()
                },
                now,
            )?;
        }
        let rules = self.video.rules();
        let explicit = match req.quality.as_deref() {
            Some(q) => Some(Quality::parse(q).ok_or_else(|| Error::bad(format!("unknown quality '{q}'")))?),
            None => None,
        };
        let mut decision = match explicit {
            Some(_) => retention_for_request(&rules, &facts),
            None => evaluate(&rules, &facts, Encounter::Manual).unwrap_or_else(|| retention_for_request(&rules, &facts)),
        };
        if let Some(q) = explicit {
            decision.quality = q;
        }
        if decision.quality == Quality::None && !req.protect {
            return Ok((None, decision));
        }
        let quality = if decision.quality == Quality::None { Quality::P360 } else { decision.quality };
        let entry = self.create_entry(&facts, quality, &decision, "manual", req.protect)?;
        Ok((Some(entry), decision))
    }

    /// Creates (or revives) the entry for a video at a quality and queues it.
    pub fn create_entry(&self, facts: &VideoFacts, quality: Quality, decision: &Decision, origin: &str, protect: bool) -> Result<EntryRow> {
        let now = self.now();
        let has_ffmpeg = self.tools().ffmpeg.found;
        let plan = plan_format(quality, has_ffmpeg).ok_or_else(|| Error::bad("nothing to cache for quality 'none'"))?;
        let existing = self.video.entries_for_video(&facts.video_id)?;
        let id = crate::videodb::entry_id(&facts.video_id, plan.variant.as_str());
        let expires = if protect { None } else { self.retention_expiry(now, decision.retention_days) };

        if let Some(mut e) = existing.iter().find(|e| e.id == id).cloned() {
            if matches!(e.status.as_str(), STATUS_FAILED | STATUS_CANCELED) {
                self.video.set_status(&e.id, STATUS_QUEUED, None, now)?;
                e.status = STATUS_QUEUED.into();
            }
            // A new request extends retention, never shortens it.
            if !e.protected {
                let later = match (e.expires_at, expires) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    _ => None,
                };
                if later != e.expires_at {
                    self.video.set_expiry(&e.id, later, Some(decision.retention_days), decision.rule.as_deref())?;
                }
            }
            self.video.touch(&e.id, now)?;
            if protect {
                self.promote(&e.id)?;
            }
            self.wake_scheduler();
            return Ok(self.video.get_entry(&e.id)?.unwrap_or(e));
        }
        // A sharper copy that is already cached or on its way covers a lower request.
        if origin != "manual" && plan.variant != Quality::Audio {
            if let Some(better) = existing.iter().find(|e| {
                e.variant != "audio"
                    && Quality::parse(&e.variant).map_or(0, |q| q.height()) >= plan.variant.height()
                    && matches!(e.status.as_str(), "queued" | "downloading" | STATUS_COMPLETE)
            }) {
                self.video.touch(&better.id, now)?;
                if protect {
                    self.promote(&better.id)?;
                }
                return Ok(self.video.get_entry(&better.id)?.unwrap_or_else(|| better.clone()));
            }
        }
        let mut row = EntryRow::new(&facts.video_id, plan.variant.as_str(), quality.as_str(), origin, now);
        row.title = facts.title.clone();
        row.channel = facts.channel.clone();
        row.duration_seconds = facts.duration_seconds;
        row.retention_days = Some(decision.retention_days);
        row.rule = decision.rule.clone();
        row.expires_at = expires;
        row.protected = protect;
        if plan.downgraded {
            row.error = Some(format!("ffmpeg not found: caching {}p as 360p", quality.height()));
        }
        self.video.insert_entry(&row)?;
        self.queue_thumb(&facts.video_id);
        self.wake_scheduler();
        Ok(row)
    }

    /// `POST /api/open`: make sure something is cached and tell the app to show it.
    pub fn open_video(&self, video_id: &str, quality: Option<&str>) -> Result<(Option<EntryRow>, Decision)> {
        if !valid_video_id(video_id) {
            return Err(Error::bad("invalid video_id"));
        }
        let now = self.now();
        let existing = self.video.entries_for_video(video_id)?;
        let usable = existing.iter().find(|e| matches!(e.status.as_str(), "queued" | "downloading" | STATUS_COMPLETE)).cloned();
        let result = match (usable, quality) {
            (Some(e), None) => {
                self.video.touch(&e.id, now)?;
                (Some(e), Decision { rule_id: None, rule: None, quality: Quality::None, retention_days: 0 })
            }
            (_, q) => self.request_cache(CacheRequest {
                video_id: video_id.into(),
                quality: Some(q.unwrap_or("360").to_string()),
                ..CacheRequest::default()
            })?,
        };
        let req = {
            let mut opens = self.opens.lock().unwrap_or_else(|e| e.into_inner());
            opens.0 += 1;
            let req = OpenRequest { seq: opens.0, video_id: video_id.into(), ts: now };
            opens.1.push_back(req.clone());
            while opens.1.len() > 50 {
                opens.1.pop_front();
            }
            req
        };
        if let Some(hook) = self.open_hook.read().unwrap_or_else(|e| e.into_inner()).clone() {
            hook(&req);
        }
        Ok(result)
    }

    pub fn pending_opens(&self, after: u64) -> (u64, Vec<OpenRequest>) {
        let opens = self.opens.lock().unwrap_or_else(|e| e.into_inner());
        (opens.0, opens.1.iter().filter(|r| r.seq > after).cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::testutil::test_core;
    use crate::rules::Trigger;

    fn item(id: &str, title: &str, channel: &str, dur: u32) -> Item {
        Item { video_id: id.into(), title: title.into(), channel: Some(channel.into()), duration_seconds: Some(dur), ..Item::default() }
    }

    fn feed(kind: &str, items: Vec<Item>) -> FeedIngest {
        FeedIngest { kind: kind.into(), items, ..FeedIngest::default() }
    }

    #[test]
    fn feed_snapshots_are_saved_and_dedup_filtered() {
        let (_d, core) = test_core(None, None);
        let r = core
            .ingest_feed(feed("home", vec![item("aaaaaaaaaaa", "A", "C", 100), item("aaaaaaaaaaa", "dup", "C", 100), item("bad", "x", "C", 1), item("bbbbbbbbbbb", "B", "C", 5)]))
            .unwrap();
        assert!(r["snapshot_id"].is_i64());
        let snap = core.content.latest_snapshot("home").unwrap().unwrap();
        assert_eq!(snap.items.iter().map(|i| i.video_id.as_str()).collect::<Vec<_>>(), vec!["aaaaaaaaaaa", "bbbbbbbbbbb"]);
        // A second snapshot does not replace the first.
        core.ingest_feed(feed("home", vec![item("ccccccccccc", "C", "C", 5)])).unwrap();
        assert_eq!(core.content.list_snapshots(Some("home"), 10).unwrap().len(), 2);
        // Item metadata is remembered for the rules engine.
        assert_eq!(core.facts_for("aaaaaaaaaaa").title.as_deref(), Some("A"));
        // Unknown kinds are stored as "other"; empty feeds store nothing.
        core.ingest_feed(feed("weird", vec![item("ddddddddddd", "D", "C", 5)])).unwrap();
        assert_eq!(core.content.list_snapshots(Some("other"), 10).unwrap().len(), 1);
        let r = core.ingest_feed(feed("home", vec![])).unwrap();
        assert!(r["snapshot_id"].is_null());
    }

    #[test]
    fn new_uploads_from_favorite_channels_are_queued_at_480_or_360_without_ffmpeg() {
        let (_d, core) = test_core(None, None);
        let mut rules = core.video.rules();
        rules.iter_mut().find(|r| r.trigger == Trigger::NewUpload).unwrap().matcher.channels = vec!["Fav".into()];
        core.video.put_rules(&rules).unwrap();
        let r = core
            .ingest_feed(feed("subscriptions", vec![item("aaaaaaaaaaa", "New one", "Fav", 600), item("bbbbbbbbbbb", "Other", "Nope", 600)]))
            .unwrap();
        assert_eq!(r["new_videos"].as_array().unwrap().len(), 2);
        let queued = r["queued"].as_array().unwrap();
        assert_eq!(queued.len(), 1);
        // No ffmpeg in the test environment: 480p honestly becomes 360p (format 18).
        assert_eq!(queued[0], "aaaaaaaaaaa.360");
        let e = core.video.get_entry("aaaaaaaaaaa.360").unwrap().unwrap();
        assert_eq!((e.origin.as_str(), e.requested_quality.as_str(), e.retention_days), ("rule", "480", Some(0)));
        assert!(e.error.as_deref().unwrap().contains("ffmpeg"));
        // Seeing the same videos again is not a new upload.
        let again = core.ingest_feed(feed("subscriptions", vec![item("aaaaaaaaaaa", "New one", "Fav", 600)])).unwrap();
        assert!(again["new_videos"].as_array().unwrap().is_empty());
        assert!(again["queued"].as_array().unwrap().is_empty());
    }

    #[test]
    fn auto_prefetch_switch_disables_rule_triggers() {
        let (_d, core) = test_core(None, None);
        let mut s = core.video.settings();
        s.auto_prefetch = false;
        core.video.put_settings(&s).unwrap();
        core.ingest_video(VideoIngest { video_id: "aaaaaaaaaaa".into(), title: "Podcast #1".into(), duration_seconds: Some(7200), ..VideoIngest::default() }).unwrap();
        let r = core
            .ingest_tab(TabIngest { tab_id: "1".into(), url: "https://www.youtube.com/watch?v=aaaaaaaaaaa".into(), video_id: Some("aaaaaaaaaaa".into()), state: "open".into(), ..TabIngest::default() })
            .unwrap();
        assert!(r["queued"].is_null());
        assert!(core.video.list_entries().unwrap().is_empty());
    }

    #[test]
    fn opening_a_podcast_tab_queues_audio_only() {
        let (_d, core) = test_core(None, None);
        core.ingest_video(VideoIngest { video_id: "aaaaaaaaaaa".into(), title: "Great Podcast #12 with a guest".into(), channel: Some("Pod".into()), duration_seconds: Some(5400), ..VideoIngest::default() }).unwrap();
        let tab = |state: &str| TabIngest {
            tab_id: "7".into(),
            url: "https://www.youtube.com/watch?v=aaaaaaaaaaa".into(),
            video_id: Some("aaaaaaaaaaa".into()),
            state: state.into(),
            ..TabIngest::default()
        };
        let r = core.ingest_tab(tab("open")).unwrap();
        assert_eq!(r["queued"], "aaaaaaaaaaa.audio");
        let e = core.video.get_entry("aaaaaaaaaaa.audio").unwrap().unwrap();
        assert_eq!((e.retention_days, e.origin.as_str(), e.rule.as_deref()), (Some(0), "rule", Some("Podcasts / interviews")));
        // The same tab reporting again does not queue twice.
        assert!(core.ingest_tab(tab("open")).unwrap()["queued"].is_null());
        assert_eq!(core.video.list_entries().unwrap().len(), 1);
    }

    #[test]
    fn first_play_triggers_talking_head_360() {
        let (_d, core) = test_core(None, None);
        core.ingest_video(VideoIngest { video_id: "aaaaaaaaaaa".into(), title: "My honest review".into(), duration_seconds: Some(600), ..VideoIngest::default() }).unwrap();
        let play = |pos: f64| PlaybackIngest { video_id: "aaaaaaaaaaa".into(), position_seconds: pos, duration_seconds: Some(600.0), watch_height: Some(720), ..PlaybackIngest::default() };
        let r = core.ingest_playback(play(1.0)).unwrap();
        assert_eq!(r["first_play"], true);
        assert_eq!(r["queued"], "aaaaaaaaaaa.360");
        let r = core.ingest_playback(play(6.0)).unwrap();
        assert_eq!(r["first_play"], false);
        assert!(r["queued"].is_null());
        // Cached quality vs watch quality is tracked separately.
        assert_eq!(core.content.get_progress("aaaaaaaaaaa").unwrap().unwrap().watch_height, Some(720));
    }

    #[test]
    fn manual_requests_override_quality_but_keep_rule_retention() {
        let (_d, core) = test_core(None, None);
        let mut rules=core.video.rules();
        rules.iter_mut().find(|r|r.id=="seed-music").unwrap().retention_days=30;
        core.video.put_rules(&rules).unwrap();
        core.ingest_video(VideoIngest { video_id: "aaaaaaaaaaa".into(), title: "Band - Song (Official Video)".into(), duration_seconds: Some(240), ..VideoIngest::default() }).unwrap();
        // Music rule is manual-only 720p/30d; no ffmpeg here so it lands as 360.
        let (e, d) = core.request_cache(CacheRequest { video_id: "aaaaaaaaaaa".into(), ..CacheRequest::default() }).unwrap();
        assert_eq!(d.quality, Quality::P720);
        assert_eq!(d.retention_days, 30);
        assert_eq!(e.unwrap().id, "aaaaaaaaaaa.360");
        // "Everything else" decides none: nothing is queued without an explicit quality.
        let (e, d) = core.request_cache(CacheRequest { video_id: "bbbbbbbbbbb".into(), title: Some("Random".into()), ..CacheRequest::default() }).unwrap();
        assert!(e.is_none());
        assert_eq!((d.quality, d.retention_days), (Quality::None, 0));
        // An explicit quality wins and defaults to retaining until watched.
        let (e, d) = core.request_cache(CacheRequest { video_id: "bbbbbbbbbbb".into(), quality: Some("audio".into()), ..CacheRequest::default() }).unwrap();
        let e = e.unwrap();
        assert_eq!((e.id.as_str(), d.retention_days), ("bbbbbbbbbbb.audio", 0));
        assert!(e.expires_at.is_none());
        assert!(core.request_cache(CacheRequest { video_id: "nope".into(), ..CacheRequest::default() }).is_err());
        assert!(core.request_cache(CacheRequest { video_id: "bbbbbbbbbbb".into(), quality: Some("4k".into()), ..CacheRequest::default() }).is_err());
    }

    #[test]
    fn automatic_requests_reuse_sharper_copies_but_manual_quality_changes_create_a_copy() {
        let (_d, core) = test_core(None, None);
        let mut e = EntryRow::new("aaaaaaaaaaa", "720", "720", "manual", 1);
        e.status = STATUS_COMPLETE.into();
        core.video.insert_entry(&e).unwrap();
        let facts=VideoFacts{video_id:"aaaaaaaaaaa".into(),..Default::default()};
        let decision=retention_for_request(&[],&facts);
        assert_eq!(core.create_entry(&facts,Quality::P360,&decision,"rule",false).unwrap().id,"aaaaaaaaaaa.720");
        let (got, _) = core.request_cache(CacheRequest { video_id: "aaaaaaaaaaa".into(), quality: Some("360".into()), ..CacheRequest::default() }).unwrap();
        assert_eq!(got.unwrap().id, "aaaaaaaaaaa.360");
        assert_eq!(core.video.list_entries().unwrap().len(), 2);
    }

    #[test]
    fn comments_are_capped_and_expire_after_a_day() {
        let (_d, core) = test_core(None, None);
        core.set_now_for_tests(Some(1_000_000));
        let comments: Vec<Comment> = (0..900).map(|i| Comment { author: format!("a{i}"), text: "hi".into(), ..Comment::default() }).collect();
        core.ingest_comments(CommentsIngest { video_id: "aaaaaaaaaaa".into(), comments, ..CommentsIngest::default() }).unwrap();
        let got = core.content.get_json(KIND_COMMENTS, "aaaaaaaaaaa", 1_000_001).unwrap().unwrap();
        assert_eq!(got["comments"].as_array().unwrap().len(), 600);
        assert!(core.content.get_json(KIND_COMMENTS, "aaaaaaaaaaa", 1_000_000 + 24 * 3_600_000).unwrap().is_none());
    }

    #[test]
    fn open_requests_are_sequenced() {
        let (_d, core) = test_core(None, None);
        let (e, _) = core.open_video("aaaaaaaaaaa", None).unwrap();
        assert_eq!(e.unwrap().id, "aaaaaaaaaaa.360");
        core.open_video("aaaaaaaaaaa", None).unwrap();
        let (seq, items) = core.pending_opens(0);
        assert_eq!((seq, items.len()), (2, 2));
        assert_eq!(core.pending_opens(1).1.len(), 1);
        assert!(core.pending_opens(2).1.is_empty());
        assert_eq!(core.video.list_entries().unwrap().len(), 1);
    }
}
