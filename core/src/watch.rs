//! The data behind Muninn's own watch view: make sure metadata, related videos,
//! comments and a playable source exist for a video, and report how far along
//! each is. Cache-only: a finished cache entry wins over growing disk-backed HLS.

use crate::content::KIND_META;
use crate::engine::Core;
use crate::error::{Error, Result};
use crate::stream::playlist_progress;
use crate::util::valid_video_id;
use crate::videodb::EntryRow;
use serde_json::{json, Value};
use std::sync::Arc;

impl Core {
    /// Persist an explicit background-tab acquisition without starting playback.
    /// The ordinary scheduler admits this queue entry into the local HLS/cache
    /// pipeline when capacity is available; opening many tabs never drops work
    /// at the foreground stream limit or preempts an existing download.
    pub fn prepare_background_watch(self: &Arc<Self>, video_id: &str, audio_only: bool) -> Result<Value> {
        if !valid_video_id(video_id) {
            return Err(Error::bad("invalid video_id"));
        }
        self.resume_cleanup(video_id)?;
        // Share admission with foreground opens and other background intents.
        let _guard = self.streams.start_lock.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = self.best_complete_entry(video_id) {
            self.video.touch(&entry.id, self.now())?;
            return Ok(self.watch_status(video_id));
        }
        if self.stream_job(video_id).is_some_and(|job| {
            matches!(job.state.lock().unwrap_or_else(|e| e.into_inner()).phase.as_str(),
                "resolving" | "starting" | "caching" | "done")
        }) {
            return Ok(self.watch_status(video_id));
        }
        if let Some(entry) = self.video.entries_for_video(video_id)?.into_iter().find(|entry| {
            matches!(entry.status.as_str(), "queued" | "downloading") || self.is_running(&entry.id)
        }) {
            self.video.touch(&entry.id, self.now())?;
            self.wake_scheduler();
            return Ok(self.watch_status(video_id));
        }
        let quality = if audio_only { crate::quality::Quality::Audio }
            else { crate::stream::variant_for_height(self.video.settings().watch_height) };
        let facts = self.facts_for(video_id);
        let decision = crate::rules::retention_for_request(&self.video.rules(), &facts);
        // A failed foreground/manual attempt may own this same row. Retag the
        // retry before create_entry wakes the scheduler; never retag a worker.
        if let Some(plan) = crate::ytdlp::plan_format(quality, self.tools().ffmpeg.found) {
            let id = crate::videodb::entry_id(video_id, plan.variant.as_str());
            if let Some(entry) = self.video.get_entry(&id)? {
                if matches!(entry.status.as_str(), "failed" | "canceled") && !self.is_running(&id) {
                    self.video.set_origin(&id, "background-tab")?;
                }
            }
        }
        self.create_entry(&facts, quality, &decision, "background-tab", false)?;
        Ok(self.watch_status(video_id))
    }

    /// Starts whatever is missing and returns the current status.
    pub fn watch_open(self: &Arc<Self>, video_id: &str, audio_only: bool) -> Result<Value> {
        if !valid_video_id(video_id) {
            return Err(Error::bad("invalid video_id"));
        }
        self.resume_cleanup(video_id)?;
        let cached=self.best_complete_entry(video_id);
        self.log_event("info", "watch.open", json!({"video_id":video_id,"cached":cached.is_some(),"entry_id":cached.as_ref().map(|e|&e.id),"container":cached.as_ref().and_then(|e|e.ext.as_ref()),"bytes":cached.as_ref().map(|e|e.size_bytes)}));
        self.ensure_meta(video_id);
        if let Some(entry)=self.best_complete_entry(video_id) {
            if matches!(entry.ext.as_deref(),Some("mkv"|"webm")) {self.prepare_local_playback_at(&entry.id,None)?;}
        }
        if self.best_complete_entry(video_id).is_none() {
            // Failures are reported through the status, not raised here.
            match self.start_stream(video_id, audio_only) {
                Ok(_) => {
                    self.streams.errors.lock().unwrap_or_else(|e| e.into_inner()).remove(video_id);
                }
                Err(e) => self.record_start_error(video_id, &e),
            }
        } else if let Some(e) = self.best_complete_entry(video_id) {
            let _ = self.video.touch(&e.id, self.now());
        }
        Ok(self.watch_status(video_id))
    }

    pub fn best_complete_entry(&self, video_id: &str) -> Option<EntryRow> {
        self.video
            .entries_for_video(video_id)
            .ok()?
            .into_iter()
            .filter(|e| e.is_complete() && e.file_path.as_deref().is_some_and(|p| std::path::Path::new(p).exists()))
            .max_by_key(|e| e.actual_height.unwrap_or_else(|| crate::quality::Quality::parse(&e.variant).map_or(0, |q| q.height())))
    }

    /// Fetches the watch page in the background when metadata (or comments) are not cached.
    pub fn ensure_meta(self: &Arc<Self>, video_id: &str) {
        if self.is_offline() { return; }
        let now = self.now();
        let have_meta = self.content.has(KIND_META, video_id, now);
        let have_comments = self.content.has(crate::content::KIND_COMMENTS, video_id, now) || self.has_more_comments(video_id);
        if have_meta && have_comments {
            return;
        }
        {
            let mut f = self.meta_fetches.lock().unwrap_or_else(|e| e.into_inner());
            match f.get(video_id) {
                Some(state) if state.is_empty() => return,                  // running
                Some(state) if state == "ok" && have_meta => return,       // already tried for comments
                _ => {}
            }
            f.insert(video_id.to_string(), String::new());
        }
        let core = self.clone();
        let id = video_id.to_string();
        let spawned = std::thread::Builder::new().name(format!("muninn-meta-{id}")).spawn(move || {
            let result = core.fetch_watch(&id);
            let state = match result {
                Ok(_) => "ok".to_string(),
                Err(e) => e.to_string(),
            };
            core.meta_fetches.lock().unwrap_or_else(|e| e.into_inner()).insert(id, state);
        });
        if spawned.is_err() {
            self.meta_fetches.lock().unwrap_or_else(|e| e.into_inner()).insert(video_id.to_string(), "could not start the fetch thread".into());
        }
    }

    pub fn watch_status(&self, video_id: &str) -> Value {
        let now = if self.is_offline() { 0 } else { self.now() };
        let have_meta = self.content.has(KIND_META, video_id, now);
        let fetch = self.meta_fetches.lock().unwrap_or_else(|e| e.into_inner()).get(video_id).cloned();
        let (meta_state, meta_error) = match (have_meta, fetch.as_deref()) {
            (true, _) => ("ready", None),
            (false, Some("")) | (false, None) => ("loading", None),
            (false, Some("ok")) => ("loading", None),
            (false, Some(err)) => ("error", Some(err.to_string())),
        };
        let playback = if let Some(e) = self.best_complete_entry(video_id) {
            if let Some(local)=self.local_playback_status(&e.id) {local} else {
            json!({
                "state": "cached", "kind": "file", "entry_id": e.id, "variant": e.variant, "height": e.actual_height, "ready": true,
                "url_path": format!("/media/{}", e.id), "progress": 1.0,
            })
            }
        } else if let Some(job) = self.stream_job(video_id) {
            let s = job.state.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let (segments, finished) = playlist_progress(&job.hls_dir);
            let ready = (segments >= 2 || finished) && matches!(s.phase.as_str(), "caching" | "done");
            let dir = format!("{}.{}", video_id, s.variant);
            json!({
                "state": match s.phase.as_str() { "failed" | "canceled" => "error", "resolving" | "starting" => "resolving", _ => "streaming" },
                "phase": s.phase, "kind": "hls", "entry_id": s.entry_id, "variant": s.variant, "height": s.height, "ready": ready,
                "progress": s.progress, "error": s.error, "reason": s.reason, "retryable": s.retryable, "attempt": s.attempt, "segments": segments, "duration_seconds": s.duration_seconds, "audio_only": s.audio_only,
                "url_path": format!("/h/{}/{}/index.m3u8", self.token(), dir),
                "started_ms": s.started_ms, "start_budget_ms": self.streams.tuning.lock().unwrap_or_else(|e| e.into_inner()).worst_case_start().as_millis() as u64, "first_segment_ms": s.first_segment_ms,
                "time_to_first_segment_ms": s.first_segment_ms.map(|f| f - s.started_ms),
            })
        } else if let Some(err) = self.streams.errors.lock().unwrap_or_else(|e| e.into_inner()).get(video_id).cloned() {
            json!({"state": "error", "ready": false, "error": err.message, "reason": err.reason.as_str(), "retryable": err.reason.retryable()})
        } else {
            json!({"state": "none", "ready": false})
        };
        let comments_count=self.content.get_json(crate::content::KIND_COMMENTS,video_id,now).ok().flatten().and_then(|v|v["comments"].as_array().map(|a|a.len())).unwrap_or(0);
        json!({"video_id": video_id, "meta_state": meta_state, "meta_error": meta_error, "comments_loading":fetch.as_deref()==Some(""),"comments_count":comments_count,"has_more_comments": self.has_more_comments(video_id), "playback": playback, "completion_policy": self.completion_policy(video_id)})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::testutil::test_core;
    use crate::videodb::STATUS_COMPLETE;

    #[test]
    fn background_intents_are_durable_beyond_four_tabs_and_never_report_playback() {
        let (dir, core) = test_core(None, None);
        let mut settings = core.video.settings();
        settings.auto_prefetch = false;
        settings.offline_mode = true;
        core.apply_settings(&settings).unwrap();
        core.tools.write().unwrap().ffmpeg.found = true;
        let ids: Vec<String> = (0..8).map(|index| format!("video{index:06}")).collect();
        for id in &ids {
            assert_eq!(core.prepare_background_watch(id, false).unwrap()["playback"]["ready"], false);
        }
        let repeats: Vec<_> = (0..8).map(|_| {
            let core = core.clone();
            let id = ids[0].clone();
            std::thread::spawn(move || core.prepare_background_watch(&id, false).unwrap())
        }).collect();
        for thread in repeats { thread.join().unwrap(); }
        let entries = core.video.list_entries().unwrap();
        assert_eq!(entries.len(), 8);
        assert!(entries.iter().all(|entry| entry.status == "queued" && entry.origin == "background-tab" && entry.variant == "720"));
        core.start_queued();
        assert_eq!(core.running_count(), 0, "offline intent stays pending without network");
        assert!(core.streams.jobs.lock().unwrap().is_empty());
        assert!(core.meta_fetches.lock().unwrap().is_empty());
        assert!(core.pending_opens(0).1.is_empty(), "background intent must not request focus");
        for id in &ids { assert!(core.content.get_progress(id).unwrap().is_none()); }
        drop(core);
        let reopened = Core::open(crate::CoreConfig {
            data_dir: dir.path().to_path_buf(), tool_dirs: Some(vec![]), ..Default::default()
        }).unwrap();
        assert_eq!(reopened.video.entries_with_status("queued").unwrap().len(), 8);
        assert!(reopened.pending_opens(0).1.is_empty());
        assert_eq!(reopened.running_count(), 0);
    }

    #[test]
    fn background_intent_reuses_kept_cache_without_new_work_or_progress() {
        let (_dir, core) = test_core(None, None);
        let mut settings = core.video.settings();
        settings.offline_mode = true;
        core.apply_settings(&settings).unwrap();
        let path = core.paths.library_dir.join("aaaaaaaaaaa.360.mp4");
        std::fs::write(&path, b"synthetic cache").unwrap();
        let mut entry = EntryRow::new("aaaaaaaaaaa", "360", "360", "manual", 1);
        entry.status = STATUS_COMPLETE.into();
        entry.protected = true;
        entry.file_path = Some(path.display().to_string());
        entry.ext = Some("mp4".into());
        core.video.insert_entry(&entry).unwrap();
        let status = core.prepare_background_watch("aaaaaaaaaaa", false).unwrap();
        assert_eq!(status["playback"]["url_path"], "/media/aaaaaaaaaaa.360");
        let entries = core.video.entries_for_video("aaaaaaaaaaa").unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].protected && path.exists());
        assert!(core.stream_job("aaaaaaaaaaa").is_none());
        assert!(core.content.get_progress("aaaaaaaaaaa").unwrap().is_none());
        assert!(core.pending_opens(0).1.is_empty());
    }

    #[test]
    fn background_intent_reuses_existing_queue_and_retags_only_a_stopped_retry() {
        let (_dir, core) = test_core(None, None);
        core.tools.write().unwrap().ffmpeg.found = true;
        let mut entry = EntryRow::new("aaaaaaaaaaa", "720", "720", "manual", 1);
        entry.protected = true;
        core.video.insert_entry(&entry).unwrap();
        core.prepare_background_watch("aaaaaaaaaaa", false).unwrap();
        assert_eq!(core.video.get_entry(&entry.id).unwrap().unwrap().origin, "manual");
        core.video.set_status(&entry.id, "canceled", None, core.now()).unwrap();
        core.prepare_background_watch("aaaaaaaaaaa", false).unwrap();
        let revived = core.video.get_entry(&entry.id).unwrap().unwrap();
        assert_eq!((revived.origin.as_str(), revived.status.as_str()), ("background-tab", "queued"));
        assert!(revived.protected);
        assert_eq!(core.video.entries_for_video("aaaaaaaaaaa").unwrap().len(), 1);
        assert!(core.content.get_progress("aaaaaaaaaaa").unwrap().is_none());
        assert!(core.prepare_background_watch("../invalid", false).is_err());
    }

    #[test]
    fn player_completion_policy_uses_current_settings_and_matching_rules() {
        use crate::automation::{Automation, CompletionAction, CompletionRule};
        let (_d, core) = test_core(None, None);
        let mut config = Automation { completion_percent: 80, cleanup_delay_seconds: 7200, ..Default::default() };
        core.put_automation(&config).unwrap();
        let policy = core.watch_status("aaaaaaaaaaa")["completion_policy"].clone();
        assert_eq!(policy, json!({"completion_percent":80,"cleanup_delay_seconds":7200,"action":"delete"}));
        config.cleanup_completed = false;
        core.put_automation(&config).unwrap();
        assert!(core.watch_status("aaaaaaaaaaa")["completion_policy"]["action"].is_null());
        config.completion_rules.push(CompletionRule { name: "Keep these videos".into(), matcher: Default::default(), action: CompletionAction::Keep });
        core.put_automation(&config).unwrap();
        assert_eq!(core.watch_status("aaaaaaaaaaa")["completion_policy"]["action"], "keep");
    }

    #[test]
    fn a_complete_cache_entry_wins_and_is_served_from_disk() {
        let (_d, core) = test_core(None, None);
        let path = core.paths.video_dir.join("aaaaaaaaaaa.480.mp4");
        std::fs::write(&path, vec![0u8; 100]).unwrap();
        let mut e = EntryRow::new("aaaaaaaaaaa", "480", "480", "play", 1);
        e.status = STATUS_COMPLETE.into();
        e.file_path = Some(path.display().to_string());
        e.ext = Some("mp4".into());
        e.size_bytes = 100;
        e.actual_height = Some(480);
        core.video.insert_entry(&e).unwrap();
        let s = core.watch_status("aaaaaaaaaaa");
        assert_eq!(s["playback"]["state"], "cached");
        assert_eq!(s["playback"]["url_path"], "/media/aaaaaaaaaaa.480");
        assert_eq!(s["meta_state"], "loading");
        // Opening it never starts a stream.
        let (_d2, core2) = test_core(None, None);
        assert!(core2.stream_job("aaaaaaaaaaa").is_none());
    }

    #[test]
    fn nothing_cached_and_no_job_reports_none_and_a_bad_id_is_refused() {
        let (_d, core) = test_core(None, None);
        assert_eq!(core.watch_status("aaaaaaaaaaa")["playback"]["state"], "none");
        let core = std::sync::Arc::new(core);
        assert!(core.watch_open("nope", false).is_err());
    }
}
