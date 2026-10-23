//! The download queue, the scheduler tick and cache maintenance (eviction,
//! promotion, thumbnails). The queue is the set of `queued` rows, so it survives
//! restarts; concurrency is bounded by the settings.

use crate::content::{KIND_THUMB, ttl_ms};
use crate::engine::{Core, RunningJob};
use crate::eviction::{self, Candidate, Caps};
use crate::error::{Error, Result};
use crate::quality::Quality;
use crate::range::{content_type_for, MediaFile};
use crate::util::free_space;
use crate::videodb::{EntryRow, STATUS_CANCELED, STATUS_DOWNLOADING, STATUS_FAILED, STATUS_QUEUED};
use crate::ytdlp::{plan_format, Event, JobSpec, Runner, YtDlpRunner};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize)]
pub struct DiskUsage {
    pub cache_bytes: u64,
    pub library_bytes: u64,
    pub free_bytes: Option<u64>,
    pub max_disk_bytes: u64,
    pub min_free_bytes: u64,
}

/// Rough size of a download before yt-dlp tells us, for admission control.
pub fn estimate_bytes(variant: &str, duration_seconds: Option<u32>, expected: Option<u64>) -> u64 {
    if let Some(e) = expected {
        return e;
    }
    let per_second: u64 = match variant {
        "audio" => 20_000,
        "360" => 100_000,
        "480" => 160_000,
        _ => 320_000,
    };
    per_second * duration_seconds.unwrap_or(600) as u64
}

impl Core {
    // --- scheduler ----------------------------------------------------------------------

    /// One scheduler pass: expire content, enforce disk caps, start queued jobs.
    pub fn tick(self: &Arc<Self>) {
        self.requeue_offline_jobs();
        if self.cleanup_completed().is_err() {
            self.log_event("error", "cleanup.failed", serde_json::json!({}));
        }
        self.poll_subscriptions();
        let now = self.now();
        if !self.is_offline() { let _ = self.content.purge_expired(now); }
        if let Err(e) = self.enforce_caps(0) {
            eprintln!("muninn: eviction pass failed: {e}");
        }
        self.start_queued();
    }

    fn runner(&self) -> Option<Arc<dyn Runner>> {
        if self.is_offline() { return None; }
        if let Some(r) = &self.runner {
            return Some(r.clone());
        }
        let bin = crate::ytdlp::find_ytdlp(&self.paths.bin_dir, &self.tool_dirs)?;
        Some(Arc::new(YtDlpRunner { bin }) as Arc<dyn Runner>)
    }

    pub fn running_count(&self) -> usize {
        self.running.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_running(&self, id: &str) -> bool {
        self.running.lock().unwrap_or_else(|e| e.into_inner()).contains_key(id)
    }

    /// Starts as many queued jobs as the concurrency limit and disk caps allow.
    pub fn start_queued(self: &Arc<Self>) {
        let Some(runner) = self.runner() else { return };
        let settings = self.video.settings();
        let queued = match self.video.entries_with_status(STATUS_QUEUED) {
            Ok(q) => q,
            Err(_) => return,
        };
        for entry in queued {
            if self.is_offline() { break; }
            if matches!(entry.origin.as_str(),"rule"|"prefetch"|"subscription") && !settings.auto_prefetch { continue; }
            if entry.origin == "subscription" {
                if let Some(channel)=entry.rule.as_deref().and_then(|r|r.strip_prefix("channel:")) {
                    if !self.effective_subscriptions().iter().any(|c|c.enabled && c.channel_id==channel) { continue; }
                }
            }
            if self.running_count() >= settings.download_concurrency as usize {
                break;
            }
            if self.is_running(&entry.id) {
                continue;
            }
            // Admission: make room under both disk caps or fail the job with a clear reason.
            let estimate = estimate_bytes(&entry.variant, entry.duration_seconds, entry.expected_bytes);
            match self.enforce_caps(estimate) {
                Ok(0) => {}
                Ok(short) => {
                    let msg = format!("not enough disk: {} MB short under the cache cap or minimum free space", short / 1_000_000 + 1);
                    let _ = self.video.set_status(&entry.id, STATUS_FAILED, Some(&msg), self.now());
                    continue;
                }
                Err(_) => continue,
            }
            let job = Arc::new(RunningJob { cancel: false.into(), started_at: self.now() });
            self.running.lock().unwrap_or_else(|e| e.into_inner()).insert(entry.id.clone(), job.clone());
            if self.is_offline() {
                self.offline_paused.lock().unwrap_or_else(|e|e.into_inner()).insert(entry.id.clone());
                job.cancel.store(true, Ordering::SeqCst);
            }
            let _ = self.video.set_status(&entry.id, STATUS_DOWNLOADING, None, self.now());
            let core = self.clone();
            let runner = runner.clone();
            let id = entry.id.clone();
            if std::thread::Builder::new().name(format!("muninn-job-{id}")).spawn(move || {
                core.run_job(&id, runner.as_ref(), &job);
                core.running.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
                core.wake_scheduler();
            }).is_err()
            {
                self.running.lock().unwrap_or_else(|e| e.into_inner()).remove(&entry.id);
                let _ = self.video.set_status(&entry.id, STATUS_QUEUED, None, self.now());
            }
        }
    }

    fn run_job(&self, id: &str, runner: &dyn Runner, job: &RunningJob) {
        self.log_event("info", "download.started", serde_json::json!({"entry_id":id}));
        let Ok(Some(entry)) = self.video.get_entry(id) else { return };
        let settings = self.video.settings();
        let tools = self.tools();
        let Some(quality) = Quality::parse(&entry.variant) else {
            let _ = self.video.set_status(id, STATUS_FAILED, Some("unknown variant"), self.now());
            return;
        };
        let Some(plan) = plan_format(quality, tools.ffmpeg.found) else {
            let _ = self.video.set_status(id, STATUS_FAILED, Some("nothing to download"), self.now());
            return;
        };
        let out_dir = if entry.protected { self.paths.library_dir.clone() } else { self.paths.video_dir.clone() };
        let parallel = self.running_count().max(1) as u32;
        let limit = (entry.origin != "manual" && settings.prefetch_bandwidth_kbps > 0).then(|| (settings.prefetch_bandwidth_kbps / parallel).max(1));
        let spec = JobSpec {
            entry_id: entry.id.clone(),
            video_id: entry.video_id.clone(),
            selector: plan.selector,
            merge: plan.merge,
            out_dir: out_dir.clone(),
            limit_rate_kbps: limit,
            ffmpeg_dir: tools.ffmpeg_dir(),
            js_runtime: tools.js_runtime.name.clone().zip(tools.js_runtime.path.clone()),
        };
        let mut done: Option<(Option<u32>, String)> = None;
        let mut last_write = Instant::now() - Duration::from_secs(1);
        let mut expected_known: Option<u64> = None;
        let result = runner.run(
            &spec,
            &mut |event| match event {
                Event::Info { format_id, height, ext, expected_bytes } => {
                    let path = out_dir.join(format!("{}.{ext}", entry.id));
                    expected_known = expected_bytes.or(expected_known);
                    // A single format (no "a+b") is written front to back and can be played while it grows.
                    let progressive = !format_id.contains('+');
                    let _ = self.video.set_file(&entry.id, &path.display().to_string(), &ext, expected_bytes, height, progressive);
                }
                Event::Progress { downloaded, total, estimate, speed } => {
                    if last_write.elapsed() < Duration::from_millis(400) {
                        return;
                    }
                    last_write = Instant::now();
                    let denominator = total.or(estimate).or(expected_known);
                    let fraction = denominator.filter(|d| *d > 0).map_or(0.0, |d| (downloaded as f64 / d as f64).min(0.999));
                    let _ = self.video.set_progress(&entry.id, downloaded, total, fraction, speed);
                }
                Event::Done { height, ext: _, path } => done = Some((height, path)),
            },
            &job.cancel,
        );
        let now = self.now();
        match result {
            Ok(()) => {
                let path = done.as_ref().map(|(_, p)| PathBuf::from(p)).or_else(|| self.find_output(&out_dir, &entry.id));
                match path.filter(|p| p.exists()) {
                    Some(path) => {
                        let height = done.and_then(|(h, _)| h);
                        let current = self.video.get_entry(id).ok().flatten().unwrap_or(entry.clone());
                        // Promoted while downloading: finish in the library.
                        let final_path = if current.protected { self.move_to(&path, &self.paths.library_dir).unwrap_or(path) } else { path };
                        let size = std::fs::metadata(&final_path).map(|m| m.len()).unwrap_or(0);
                        let ext = final_path.extension().and_then(|e| e.to_str()).unwrap_or("mp4").to_string();
                        let _ = self.video.set_file(id, &final_path.display().to_string(), &ext, Some(size), height, true);
                        let _ = self.video.complete_entry(id, size, height);
                    }
                    None => {
                        let _ = self.video.set_status(id, STATUS_FAILED, Some("yt-dlp finished but no output file was found"), now);
                    }
                }
            }
            Err(message) => {
                self.remove_partial(&out_dir, &entry.id);
                let _ = self.video.set_progress(id, 0, None, 0.0, None);
                if message == "canceled" {
                    let _ = self.video.set_status(id, STATUS_CANCELED, None, now);
                } else {
                    let mut message = message.chars().take(400).collect::<String>();
                    if message.contains("Requested format is not available") && !tools.ffmpeg.found {
                        message.push_str(" - YouTube offers no single-file stream at this quality for this video and ffmpeg is not available to merge streams (it ships inside Muninn; this build is missing it)");
                    }
                    let _ = self.video.set_status(id, STATUS_FAILED, Some(&message), now);
                }
            }
        }
        if let Ok(Some(row)) = self.video.get_entry(id) {
            self.log_event(if row.status == STATUS_FAILED { "error" } else { "info" }, "download.finished", serde_json::json!({"entry_id":id,"status":row.status,"bytes":row.size_bytes}));
        }
    }

    fn find_output(&self, dir: &Path, entry_id: &str) -> Option<PathBuf> {
        let prefix = format!("{entry_id}.");
        std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(&prefix) && !n.ends_with(".part") && !n.ends_with(".ytdl") && !n.contains(".f"))
        })
    }

    /// Removes everything a download of this entry may have left behind.
    fn remove_partial(&self, dir: &Path, entry_id: &str) {
        let prefix = format!("{entry_id}.");
        if let Ok(read) = std::fs::read_dir(dir) {
            for e in read.flatten() {
                if e.file_name().to_str().is_some_and(|n| n.starts_with(&prefix)) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }

    fn move_to(&self, file: &Path, dir: &Path) -> Result<PathBuf> {
        std::fs::create_dir_all(dir)?;
        let target = dir.join(file.file_name().ok_or_else(|| Error::other("no file name"))?);
        if target == file {
            return Ok(target);
        }
        if std::fs::rename(file, &target).is_err() {
            std::fs::copy(file, &target)?;
            std::fs::remove_file(file)?;
        }
        Ok(target)
    }

    // --- entry operations ----------------------------------------------------------------

    pub fn promote(&self, id: &str) -> Result<EntryRow> {
        let _guard = self.entry_lock.lock().unwrap_or_else(|e|e.into_inner());
        self.log_event("info", "entry.keep", serde_json::json!({"entry_id":id}));
        let entry = self.video.get_entry(id)?.ok_or_else(|| Error::not_found("no such entry"))?;
        if entry.protected && entry.file_path.as_deref().is_some_and(|p| Path::new(p).starts_with(&self.paths.library_dir)) {
            return Ok(entry);
        }
        if entry.is_complete() {
            let path = entry.file_path.clone().ok_or_else(|| Error::other("entry has no file"))?;
            let moved = self.move_to(Path::new(&path), &self.paths.library_dir)?;
            self.video.set_protected(id, Some(&moved.display().to_string()))?;
        } else {
            // Still downloading: the finished file is moved into the library when the job completes.
            self.video.set_protected(id, None)?;
        }
        self.video.get_entry(id)?.ok_or_else(|| Error::not_found("no such entry"))
    }

    pub fn cancel(&self, id: &str) -> Result<()> {
        self.offline_paused.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
        let entry = self.video.get_entry(id)?.ok_or_else(|| Error::not_found("no such entry"))?;
        if let Some(job) = self.running.lock().unwrap_or_else(|e| e.into_inner()).get(id) {
            job.cancel.store(true, Ordering::SeqCst);
        } else if entry.status == STATUS_QUEUED {
            self.video.set_status(id, STATUS_CANCELED, None, self.now())?;
        }
        Ok(())
    }

    pub fn release(&self, id: &str, retention_days: u32) -> Result<EntryRow> {
        if retention_days > 3650 { return Err(Error::bad("retention must be <=3650 days")); }
        let _guard = self.entry_lock.lock().unwrap_or_else(|e|e.into_inner());
        let row = self.video.get_entry(id)?.ok_or_else(||Error::not_found("no such entry"))?;
        if self.is_running(id) { return Err(Error::Conflict("wait for this download to finish before releasing it".into())); }
        let path = if row.is_complete() {
            row.file_path.as_deref().map(|p|self.move_to(Path::new(p), &self.paths.video_dir)).transpose()?
        } else {None};
        self.video.set_unprotected(id, path.as_ref().map(|p|p.to_str().unwrap_or_default()), self.retention_expiry(self.now(),retention_days))?;
        self.video.set_expiry(id, self.retention_expiry(self.now(),retention_days), Some(retention_days), None)?;
        self.log_event("info","entry.released",serde_json::json!({"entry_id":id,"retention_days":retention_days}));
        self.video.get_entry(id)?.ok_or_else(||Error::not_found("no such entry"))
    }

    pub fn retry(&self, id: &str) -> Result<()> {
        let entry = self.video.get_entry(id)?.ok_or_else(|| Error::not_found("no such entry"))?;
        if matches!(entry.status.as_str(), STATUS_FAILED | STATUS_CANCELED) {
            self.video.set_status(id, STATUS_QUEUED, None, self.now())?;
            self.wake_scheduler();
            Ok(())
        } else {
            Err(Error::Conflict(format!("entry is {}, not failed or canceled", entry.status)))
        }
    }

    /// Deletes an entry's files and row. Library entries need `force`.
    pub fn evict(&self, id: &str, force: bool) -> Result<()> {
        let _guard = self.entry_lock.lock().unwrap_or_else(|e|e.into_inner());
        let entry = self.video.get_entry(id)?.ok_or_else(|| Error::not_found("no such entry"))?;
        if entry.protected && !force {
            return Err(Error::Conflict("entry is in the library; evict with force to delete it".into()));
        }
        if let Some(job) = self.running.lock().unwrap_or_else(|e| e.into_inner()).get(id) {
            job.cancel.store(true, Ordering::SeqCst);
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.is_running(id) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
        if self.is_running(id) { return Err(Error::Conflict("download is still stopping; retry deletion in a moment".into())); }
        for dir in [&self.paths.video_dir, &self.paths.library_dir] {
            self.remove_partial(dir, &entry.id);
        }
        if let Some(path) = &entry.file_path {
            let _ = std::fs::remove_file(path);
        }
        let hls=crate::stream::hls_dir_for(&self.paths.cache_dir,&entry.video_id,&entry.variant);
        match std::fs::remove_dir_all(&hls) { Ok(())=>{},Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},Err(e)=>return Err(e.into()) }
        let removed = {
            let mut jobs=self.streams.jobs.lock().unwrap_or_else(|e|e.into_inner());
            if jobs.get(&entry.video_id).is_some_and(|job|job.state.lock().unwrap_or_else(|e|e.into_inner()).entry_id==entry.id) {
                jobs.remove(&entry.video_id);true
            } else {false}
        };
        if removed {self.streams.sources.lock().unwrap_or_else(|e|e.into_inner()).retain(|key,_|!key.starts_with(&format!("{}/",entry.video_id)));}
        self.video.delete_entry(id)?;
        self.log_event("info", "entry.deleted", serde_json::json!({"entry_id":id}));
        Ok(())
    }

    // --- disk caps -------------------------------------------------------------------------

    pub fn disk_usage(&self) -> DiskUsage {
        let settings = self.video.settings();
        DiskUsage {
            cache_bytes: self.video.cache_bytes(),
            library_bytes: self.video.library_bytes(),
            free_bytes: free_space(&self.paths.data_dir),
            max_disk_bytes: settings.max_disk_bytes,
            min_free_bytes: settings.min_free_bytes,
        }
    }

    pub fn eviction_candidates(&self) -> Vec<Candidate> {
        self.video
            .list_entries()
            .unwrap_or_default()
            .into_iter()
            .map(|e| {
                let watched = self.content.get_progress(&e.video_id).ok().flatten().map(|p| p.fraction().unwrap_or(0.5));
                Candidate {
                    active: self.is_running(&e.id) || self.playback_active(&e.video_id),
                    complete: e.is_complete(),
                    id: e.id,
                    protected: e.protected,
                    origin: e.origin,
                    size_bytes: e.size_bytes,
                    last_access_at: e.last_access_at,
                    expires_at: e.expires_at,
                    watched,
                }
            })
            .collect()
    }

    /// Evicts per policy so that `incoming` more bytes fit under the caps.
    /// Returns the bytes still missing afterwards (0 = it fits).
    pub fn enforce_caps(&self, incoming: u64) -> Result<u64> {
        let settings = self.video.settings();
        let caps = Caps { max_disk_bytes: settings.max_disk_bytes, min_free_bytes: settings.min_free_bytes };
        let plan = eviction::plan(&self.eviction_candidates(), self.now(), self.video.cache_bytes(), free_space(&self.paths.data_dir), incoming, caps);
        for id in &plan.evict {
            let _ = self.evict(id, false);
        }
        Ok(plan.shortfall)
    }

    // --- serving -----------------------------------------------------------------------------

    /// Picks the entry for `/media/<id>`: an exact entry id, or the best streamable
    /// variant of a video id.
    pub fn resolve_media_entry(&self, id: &str) -> Result<EntryRow> {
        let (video, variant) = crate::videodb::split_entry_id(id).ok_or_else(|| Error::bad("invalid media id"))?;
        let candidates: Vec<EntryRow> = self
            .video
            .entries_for_video(video)?
            .into_iter()
            .filter(|e| variant.map_or(true, |v| e.variant == v) && self.streamable(e))
            .collect();
        let choices: Vec<(usize, Quality, Option<u32>)> =
            candidates.iter().enumerate().map(|(i, e)| (i, Quality::parse(&e.variant).unwrap_or(Quality::None), e.actual_height)).collect();
        // Prefer a finished copy over one still downloading, then the sharpest.
        let complete: Vec<_> = choices.iter().filter(|(i, _, _)| candidates[*i].is_complete()).cloned().collect();
        let pick = crate::quality::best_variant(if complete.is_empty() { &choices } else { &complete });
        pick.and_then(|i| candidates.get(i).cloned()).ok_or_else(|| Error::not_found("nothing streamable cached for that id"))
    }

    pub fn streamable(&self, e: &EntryRow) -> bool {
        let has_file = e.file_path.as_deref().is_some_and(|p| Path::new(p).exists());
        has_file && (e.is_complete() || (e.status == STATUS_DOWNLOADING && e.progressive && self.is_running(&e.id)))
    }

    pub fn media_file(self: &Arc<Self>, id: &str) -> Result<MediaFile> {
        let entry = self.resolve_media_entry(id)?;
        self.video.touch(&entry.id, self.now())?;
        let path = PathBuf::from(entry.file_path.clone().unwrap_or_default());
        let ct = content_type_for(entry.ext.as_deref().unwrap_or("mp4"));
        let mut media = MediaFile::complete(path, ct);
        if !entry.is_complete() {
            media.final_size = entry.expected_bytes;
            let core = self.clone();
            let entry_id = entry.id.clone();
            media.growing = Arc::new(move || core.is_running(&entry_id));
        }
        Ok(media)
    }

    // --- thumbnails --------------------------------------------------------------------------

    pub fn queue_thumb(&self, video_id: &str) {
        if self.is_offline() {return;}
        if self.content.has(KIND_THUMB, video_id, self.now()) {
            return;
        }
        let (lock, cv) = &self.thumbs;
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        if q.1.insert(video_id.to_string()) && q.0.len() < 2000 {
            q.0.push_back(video_id.to_string());
            cv.notify_one();
        }
    }

    pub fn thumb_bytes(&self, video_id: &str) -> Option<Vec<u8>> {
        let hit = self.content.get_raw(KIND_THUMB, video_id, if self.is_offline(){0}else{self.now()}).ok().flatten();
        if hit.is_none() {
            self.queue_thumb(video_id);
        }
        hit
    }

    /// Fetches one queued thumbnail; returns false when the queue is empty.
    pub fn fetch_one_thumb(&self) -> bool {
        let next = {
            let mut q = self.thumbs.0.lock().unwrap_or_else(|e| e.into_inner());
            let id = q.0.pop_front();
            if let Some(id) = &id {
                q.1.remove(id);
            }
            id
        };
        let Some(id) = next else { return false };
        let url = format!("https://i.ytimg.com/vi/{id}/mqdefault.jpg");
        match self.fetcher.get(&url, 2 * 1024 * 1024) {
            Ok(bytes) if !bytes.is_empty() => {
                let ttl = ttl_ms(KIND_THUMB, &self.video.settings());
                let _ = self.content.put_raw(KIND_THUMB, &id, &bytes, ttl, self.now());
            }
            Ok(_) | Err(_) => {}
        }
        true
    }

    pub(crate) fn thumb_loop(self: Arc<Self>) {
        while !self.shutdown.load(Ordering::SeqCst) {
            if !self.fetch_one_thumb() {
                let (lock, cv) = &self.thumbs;
                let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
                let _ = cv.wait_timeout_while(guard, Duration::from_secs(10), |q| q.0.is_empty() && !self.shutdown.load(Ordering::SeqCst));
            } else {
                std::thread::sleep(Duration::from_millis(120));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::testutil::test_core;
    use crate::model::{CacheRequest, PlaybackIngest};
    use crate::videodb::STATUS_COMPLETE;
    use crate::ytdlp::Fetcher;
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    /// A yt-dlp stand-in: reports progress, writes a file of `size` bytes slowly.
    struct FakeRunner {
        size: usize,
        delay_ms: u64,
        concurrent: AtomicUsize,
        max_concurrent: AtomicUsize,
        fail_for: Option<&'static str>,
    }

    impl FakeRunner {
        fn new(size: usize, delay_ms: u64) -> Arc<FakeRunner> {
            Arc::new(FakeRunner { size, delay_ms, concurrent: AtomicUsize::new(0), max_concurrent: AtomicUsize::new(0), fail_for: None })
        }
    }

    impl Runner for FakeRunner {
        fn run(&self, spec: &JobSpec, events: &mut dyn FnMut(Event), cancel: &AtomicBool) -> std::result::Result<(), String> {
            let now = self.concurrent.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_concurrent.fetch_max(now, Ordering::SeqCst);
            let result = (|| {
                if self.fail_for == Some(spec.video_id.as_str()) {
                    return Err("ERROR: Video unavailable".to_string());
                }
                let path = spec.out_dir.join(format!("{}.mp4", spec.entry_id));
                events(Event::Info { format_id: "18".into(), height: Some(360), ext: "mp4".into(), expected_bytes: Some(self.size as u64) });
                let mut f = std::fs::File::create(&path).map_err(|e| e.to_string())?;
                let steps = 4;
                for i in 1..=steps {
                    if cancel.load(Ordering::SeqCst) {
                        return Err("canceled".into());
                    }
                    f.write_all(&vec![7u8; self.size / steps]).map_err(|e| e.to_string())?;
                    events(Event::Progress { downloaded: (self.size / steps * i) as u64, total: Some(self.size as u64), estimate: None, speed: Some(1000) });
                    std::thread::sleep(Duration::from_millis(self.delay_ms));
                }
                events(Event::Done { height: Some(360), ext: "mp4".into(), path: path.display().to_string() });
                Ok(())
            })();
            self.concurrent.fetch_sub(1, Ordering::SeqCst);
            result
        }
    }

    fn wait_until(core: &Arc<Core>, what: &str, f: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !f() {
            core.start_queued();
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn queue(core: &Arc<Core>, id: &str, q: &str) {
        core.request_cache(CacheRequest { video_id: id.into(), quality: Some(q.into()), ..CacheRequest::default() }).unwrap();
    }

    #[test]
    fn worker_respects_the_concurrency_limit_and_completes_jobs() {
        let runner = FakeRunner::new(4000, 60);
        let (_d, core) = test_core(Some(runner.clone()), None);
        let mut s = core.video.settings();
        s.download_concurrency = 2;
        core.video.put_settings(&s).unwrap();
        let ids = ["aaaaaaaaaaa", "bbbbbbbbbbb", "ccccccccccc", "ddddddddddd", "eeeeeeeeeee"];
        for id in ids {
            queue(&core, id, "360");
        }
        core.start_queued();
        assert_eq!(core.running_count(), 2, "only two jobs may start at once");
        wait_until(&core, "all downloads", || core.video.entries_with_status(STATUS_COMPLETE).unwrap().len() == 5);
        assert_eq!(runner.max_concurrent.load(Ordering::SeqCst), 2);
        let e = core.video.get_entry("aaaaaaaaaaa.360").unwrap().unwrap();
        assert_eq!((e.size_bytes, e.actual_height, e.progress), (4000, Some(360), 1.0));
        assert!(Path::new(e.file_path.as_deref().unwrap()).starts_with(&core.paths.video_dir));
    }

    #[test]
    fn failed_jobs_are_reported_and_can_be_retried() {
        let runner = Arc::new(FakeRunner { size: 400, delay_ms: 1, concurrent: AtomicUsize::new(0), max_concurrent: AtomicUsize::new(0), fail_for: Some("aaaaaaaaaaa") });
        let (_d, core) = test_core(Some(runner), None);
        queue(&core, "aaaaaaaaaaa", "audio");
        wait_until(&core, "failure", || core.video.get_entry("aaaaaaaaaaa.audio").unwrap().unwrap().status == STATUS_FAILED);
        let e = core.video.get_entry("aaaaaaaaaaa.audio").unwrap().unwrap();
        assert!(e.error.unwrap().contains("Video unavailable"));
        core.retry("aaaaaaaaaaa.audio").unwrap();
        assert!(core.retry("nope").is_err());
    }

    #[test]
    fn promote_moves_the_file_to_the_library_and_protects_it() {
        let (_d, core) = test_core(Some(FakeRunner::new(400, 1)), None);
        queue(&core, "aaaaaaaaaaa", "360");
        wait_until(&core, "download", || core.video.get_entry("aaaaaaaaaaa.360").unwrap().unwrap().is_complete());
        let e = core.promote("aaaaaaaaaaa.360").unwrap();
        assert!(e.protected && e.expires_at.is_none());
        let p = PathBuf::from(e.file_path.unwrap());
        assert!(p.starts_with(&core.paths.library_dir) && p.exists());
        assert!(!core.paths.video_dir.join("aaaaaaaaaaa.360.mp4").exists());
        // Library entries survive eviction passes and refuse plain eviction.
        core.set_now_for_tests(Some(core.now() + 3650 * crate::util::DAY_MS));
        core.enforce_caps(u64::MAX / 4).unwrap();
        assert!(core.video.get_entry("aaaaaaaaaaa.360").unwrap().is_some());
        assert!(matches!(core.evict("aaaaaaaaaaa.360", false), Err(Error::Conflict(_))));
        core.evict("aaaaaaaaaaa.360", true).unwrap();
        assert!(!p.exists());
    }

    #[test]
    fn promoting_while_downloading_finishes_in_the_library() {
        let (_d, core) = test_core(Some(FakeRunner::new(4000, 80)), None);
        queue(&core, "aaaaaaaaaaa", "360");
        core.start_queued();
        core.promote("aaaaaaaaaaa.360").unwrap();
        wait_until(&core, "download", || core.video.get_entry("aaaaaaaaaaa.360").unwrap().unwrap().is_complete());
        let e = core.video.get_entry("aaaaaaaaaaa.360").unwrap().unwrap();
        assert!(e.protected);
        assert!(Path::new(e.file_path.as_deref().unwrap()).starts_with(&core.paths.library_dir));
    }

    #[test]
    fn expired_entries_are_evicted_by_the_tick() {
        let (_d, core) = test_core(Some(FakeRunner::new(400, 1)), None);
        queue(&core, "aaaaaaaaaaa", "audio");
        wait_until(&core, "download", || core.video.get_entry("aaaaaaaaaaa.audio").unwrap().unwrap().is_complete());
        let path = core.video.get_entry("aaaaaaaaaaa.audio").unwrap().unwrap().file_path.unwrap();
        core.video.set_expiry("aaaaaaaaaaa.audio",Some(core.now()+3*crate::util::DAY_MS),Some(3),None).unwrap();
        core.tick();
        assert!(core.video.get_entry("aaaaaaaaaaa.audio").unwrap().is_some(), "not expired yet");
        core.set_now_for_tests(Some(core.now() + 4 * crate::util::DAY_MS));
        core.tick();
        assert!(core.video.get_entry("aaaaaaaaaaa.audio").unwrap().is_none());
        assert!(!Path::new(&path).exists());
    }

    #[test]
    fn disk_cap_evicts_speculative_before_partially_watched() {
        let (_d, core) = test_core(Some(FakeRunner::new(1000, 1)), None);
        let mut s = core.video.settings();
        s.max_disk_bytes = 3000; // sanitized floor is 256 MiB, so write the row directly below
        core.video.put_settings(&s).unwrap();
        for (id, origin) in [("aaaaaaaaaaa", "rule"), ("bbbbbbbbbbb", "rule"), ("ccccccccccc", "rule")] {
            let mut e = EntryRow::new(id, "audio", "audio", origin, 10);
            let path = core.paths.video_dir.join(format!("{id}.audio.m4a"));
            std::fs::write(&path, vec![0u8; 1000]).unwrap();
            e.file_path = Some(path.display().to_string());
            e.status = STATUS_COMPLETE.into();
            e.size_bytes = 1000;
            e.last_access_at = 100 + id.as_bytes()[0] as i64; // a oldest
            e.expires_at = Some(core.now() + crate::util::DAY_MS);
            core.video.insert_entry(&e).unwrap();
        }
        // The oldest entry has been partially watched.
        core.ingest_playback(PlaybackIngest { video_id: "aaaaaaaaaaa".into(), position_seconds: 50.0, duration_seconds: Some(100.0), ..PlaybackIngest::default() }).unwrap();
        // touch() in ingest_playback refreshes last_access; make "a" the oldest again.
        core.video.touch("aaaaaaaaaaa.audio", 1).unwrap();
        // Force a tiny cap by planning directly (settings floor prevents < 256MiB through the API).
        let caps = Caps { max_disk_bytes: 2000, min_free_bytes: 0 };
        let plan = eviction::plan(&core.eviction_candidates(), core.now(), core.video.cache_bytes(), None, 0, caps);
        assert_eq!(plan.evict, vec!["bbbbbbbbbbb.audio"], "speculative unplayed goes first, though the partially watched one is older");
    }

    #[test]
    fn jobs_wait_for_ytdlp_when_it_is_not_installed() {
        let (_d, core) = test_core(None, None);
        queue(&core, "aaaaaaaaaaa", "360");
        core.start_queued();
        assert_eq!(core.running_count(), 0);
        assert_eq!(core.video.get_entry("aaaaaaaaaaa.360").unwrap().unwrap().status, STATUS_QUEUED);
    }

    #[test]
    fn media_resolution_prefers_complete_then_sharpest() {
        let (_d, core) = test_core(None, None);
        let mk = |variant: &str, status: &str, height: Option<u32>| {
            let mut e = EntryRow::new("aaaaaaaaaaa", variant, variant, "manual", 1);
            let path = core.paths.video_dir.join(format!("aaaaaaaaaaa.{variant}.mp4"));
            std::fs::write(&path, b"data").unwrap();
            e.file_path = Some(path.display().to_string());
            e.status = status.into();
            e.actual_height = height;
            e.progressive = true;
            core.video.insert_entry(&e).unwrap();
        };
        mk("audio", STATUS_COMPLETE, None);
        mk("360", STATUS_COMPLETE, Some(360));
        mk("720", STATUS_QUEUED, None); // not streamable: nothing running
        assert_eq!(core.resolve_media_entry("aaaaaaaaaaa").unwrap().variant, "360");
        assert_eq!(core.resolve_media_entry("aaaaaaaaaaa.audio").unwrap().variant, "audio");
        assert!(core.resolve_media_entry("aaaaaaaaaaa.720").is_err());
        assert!(core.resolve_media_entry("zzzzzzzzzzz").is_err());
        assert!(core.resolve_media_entry("../../etc/passwd").is_err());
    }

    struct FakeThumbs;
    impl Fetcher for FakeThumbs {
        fn get(&self, url: &str, _max: u64) -> Result<Vec<u8>> {
            assert!(url.starts_with("https://i.ytimg.com/vi/"), "{url}");
            Ok(vec![0xff, 0xd8, 0xff, 0xe0])
        }
    }

    #[test]
    fn thumbnails_are_fetched_once_and_cached() {
        let (_d, core) = test_core(None, Some(Arc::new(FakeThumbs)));
        assert!(core.thumb_bytes("aaaaaaaaaaa").is_none(), "first request only queues the fetch");
        assert!(core.fetch_one_thumb());
        assert!(!core.fetch_one_thumb());
        assert_eq!(core.thumb_bytes("aaaaaaaaaaa").unwrap(), vec![0xff, 0xd8, 0xff, 0xe0]);
    }

    #[test]
    fn size_estimates() {
        assert_eq!(estimate_bytes("360", Some(100), Some(5)), 5);
        assert_eq!(estimate_bytes("audio", Some(100), None), 2_000_000);
        assert!(estimate_bytes("720", None, None) > estimate_bytes("360", None, None));
    }
}
