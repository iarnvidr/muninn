//! Playing a video that is not cached yet, while it is being cached.
//!
//! yt-dlp resolves the video and audio stream URLs. ffmpeg then reads them
//! (through the local range feeder below, which fetches 8 MB chunks the way
//! yt-dlp does, because YouTube throttles open-ended requests to ~130 KB/s) and
//! writes two outputs at once without re-encoding:
//!
//! * an HLS playlist of fragmented-MP4 segments, which the player starts on
//!   after the first two segments (a few seconds), and
//! * the final `.mp4` (faststart) that becomes the cache entry. When ffmpeg is
//!   done the same video plays from that file with no network.
//!
//! Nothing is held in RAM beyond the 64 KB copy buffers.
//!
//! Lifecycle rules: one video has at most one active job (starting is serialized
//! and a re-open joins it); an attempt that is being stopped is waited for, never
//! joined; transient failures before the first bytes are retried once with fresh
//! URLs; deliberate stops are logged as `stream.cancelled`, never as failures.

use crate::engine::{Core, RunningJob};
use crate::error::{Error, Result};
use crate::quality::Quality;
use crate::rules::retention_for_request;
use crate::stream_diag::{classify_text, reason_for_start_error, redact_error, FailureReason, StreamTuning};
use crate::util::valid_video_id;
use crate::videodb::{entry_id, EntryRow, STATUS_CANCELED, STATUS_DOWNLOADING, STATUS_FAILED};
use crate::ytdlp::search_path;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const CHUNK: u64 = 8 * 1024 * 1024;
/// Play-while-caching pipelines that may run at once (each is a yt-dlp + an ffmpeg process).
const MAX_STREAMS: usize = 4;
/// Longest error text kept in logs, entries and the watch status.
const ERROR_TEXT_MAX: usize = 200;

/// One upstream stream the feeder can serve ranges of.
#[derive(Clone)]
pub struct Source {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub len: u64,
    /// Set when the first media byte has been handed to ffmpeg.
    pub first_byte: Option<Arc<AtomicBool>>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct StreamState {
    /// "resolving", "starting", "caching", "done", "failed" or "canceled".
    pub phase: String,
    /// Redacted, bounded text; safe to show and to log.
    pub error: Option<String>,
    /// Category of `error` (see `FailureReason`), set when the phase is failed or canceled.
    pub reason: Option<String>,
    pub retryable: bool,
    pub attempt: u32,
    pub progress: f64,
    pub started_ms: i64,
    pub first_segment_ms: Option<i64>,
    pub variant: String,
    pub height: Option<u32>,
    pub entry_id: String,
    pub duration_seconds: Option<f64>,
    pub audio_only: bool,
}

pub struct StreamJob {
    pub video_id: String,
    pub state: Mutex<StreamState>,
    pub hls_dir: PathBuf,
}

/// Why starting failed before a job existed (no yt-dlp yet, too many running, ...).
#[derive(Debug, Clone)]
pub struct StartError {
    pub reason: FailureReason,
    pub message: String,
}

#[derive(Default)]
pub struct Streams {
    /// Why starting a stream failed, per video, for the watch view.
    pub errors: Mutex<HashMap<String, StartError>>,
    pub jobs: Mutex<HashMap<String, Arc<StreamJob>>>,
    pub sources: Mutex<HashMap<String, Source>>,
    /// Serializes admission so a double open cannot create two jobs for one video.
    pub start_lock: Mutex<()>,
    /// Why a running entry was asked to stop (`preempted`, `source_switch`, ...); absent means the user.
    pub cancel_causes: Mutex<HashMap<String, &'static str>>,
    pub tuning: Mutex<StreamTuning>,
}

/// What went wrong in one attempt.
#[derive(Debug, Clone)]
pub struct Failure {
    pub reason: FailureReason,
    pub message: String,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    /// Media bytes had already reached ffmpeg: the player may have started, so no silent retry.
    pub after_first_bytes: bool,
}

impl Failure {
    pub fn new(reason: FailureReason, message: impl Into<String>) -> Failure {
        Failure { reason, message: message.into(), exit_code: None, signal: None, after_first_bytes: false }
    }
    fn from_text(message: impl Into<String>, default: FailureReason) -> Failure {
        let message = message.into();
        let reason = match classify_text(&message) {
            FailureReason::Unknown => default,
            r => r,
        };
        Failure::new(reason, message)
    }
    fn with_status(mut self, status: &ExitStatus) -> Failure {
        self.exit_code = status.code();
        self.signal = signal_of(status);
        self
    }
    fn cancelled() -> Failure {
        Failure::new(FailureReason::Cancelled, "canceled")
    }
}

#[cfg(unix)]
fn signal_of(status: &ExitStatus) -> Option<i32> {
    std::os::unix::process::ExitStatusExt::signal(status)
}
#[cfg(not(unix))]
fn signal_of(_: &ExitStatus) -> Option<i32> {
    None
}

enum Outcome {
    Done,
    Canceled,
    Failed(Failure),
}

enum Admit {
    Join(Arc<StreamJob>),
    /// The previous attempt is being stopped; wait for it.
    Stopping,
    Fresh,
}

/// The variant a watch height is cached as.
pub fn variant_for_height(h: u32) -> Quality {
    match h {
        0..=360 => Quality::P360,
        361..=480 => Quality::P480,
        481..=720 => Quality::P720,
        _ => Quality::P1080,
    }
}

pub fn selector_for(height: u32, audio_only: bool) -> String {
    if audio_only {
        return "ba[ext=m4a][protocol=http]/ba[ext=m4a][protocol=https]/ba[protocol=http]/ba[protocol=https]".into();
    }
    format!(
        "bv*[height<={height}][vcodec^=avc1][protocol=http]+ba[ext=m4a][protocol=http]/bv*[height<={height}][vcodec^=avc1][protocol=https]+ba[ext=m4a][protocol=https]/b[height<={height}][vcodec^=avc1][acodec^=mp4a][protocol=http]/b[height<={height}][vcodec^=avc1][acodec^=mp4a][protocol=https]/bv*[height<={height}][protocol=http]+ba[protocol=http]/bv*[height<={height}][protocol=https]+ba[protocol=https]/b[height<={height}][protocol=http]/b[height<={height}][protocol=https]"
    )
}

pub fn hls_dir_for(cache_dir: &Path, video_id: &str, variant: &str) -> PathBuf {
    cache_dir.join("hls").join(format!("{video_id}.{variant}"))
}

/// These EVENT playlists are recordings being prepared, not live broadcasts.
/// Give native HLS the same explicit start used by hls.js. A saved resume point
/// remains a player seek; it must not change the shared playlist's default.
pub fn recorded_playlist_start(playlist: &str) -> String {
    let mut lines=playlist.lines();
    if lines.next()!=Some("#EXTM3U") {return playlist.to_owned();}
    let mut out=String::from("#EXTM3U\n#EXT-X-START:TIME-OFFSET=0,PRECISE=YES\n");
    for line in lines.filter(|line|!line.starts_with("#EXT-X-START:")){out.push_str(line);out.push('\n');}
    out
}

/// How many media segments of the playlist exist, and whether it is finished.
pub fn playlist_progress(dir: &Path) -> (usize, bool) {
    let text = std::fs::read_to_string(dir.join("index.m3u8")).unwrap_or_default();
    (text.lines().filter(|l| l.ends_with(".m4s")).count(), text.contains("#EXT-X-ENDLIST"))
}

pub fn ffmpeg_args(inputs: &[String], hls_dir: &Path, out_mp4: &Path, audio_only: bool) -> Vec<String> {
    let mut a: Vec<String> = ["-nostdin", "-hide_banner", "-loglevel", "warning", "-y", "-progress", "pipe:1", "-nostats"].iter().map(|s| s.to_string()).collect();
    for i in inputs {
        a.push("-i".into());
        a.push(i.clone());
    }
    let maps: Vec<String> = if audio_only {
        vec!["-map".into(), format!("{}:a:0", inputs.len() - 1)]
    } else if inputs.len() == 2 {
        vec!["-map".into(), "0:v:0".into(), "-map".into(), "1:a:0".into()]
    } else {
        vec!["-map".into(), "0:v:0".into(), "-map".into(), "0:a:0?".into()]
    };
    // Output 1: the HLS playlist the player follows while this runs.
    a.extend(maps.iter().cloned());
    for s in [
        "-c", "copy", "-f", "hls", "-hls_time", "3", "-hls_playlist_type", "event", "-hls_segment_type", "fmp4", "-hls_fmp4_init_filename", "init.mp4",
        "-hls_flags", "independent_segments",
    ] {
        a.push(s.into());
    }
    a.push("-hls_segment_filename".into());
    a.push(hls_dir.join("seg_%05d.m4s").display().to_string());
    a.push(hls_dir.join("index.m3u8").display().to_string());
    // Output 2: the file that stays in the cache.
    a.extend(maps);
    for s in ["-c", "copy", "-movflags", "+faststart", "-f", "mp4"] {
        a.push(s.into());
    }
    a.push(out_mp4.display().to_string());
    a
}

/// Resolved stream URLs from `yt-dlp -J`.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub is_live: bool,
    pub duration: Option<f64>,
    pub height: Option<u32>,
    pub streams: Vec<ResolvedStream>,
}

#[derive(Debug, Clone)]
pub struct ResolvedStream {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub len: Option<u64>,
    pub protocol: Option<String>,
    pub has_video: bool,
    pub has_audio: bool,
}

pub fn parse_resolved(json: &Value) -> Result<Resolved> {
    let one = |f: &Value| -> Option<ResolvedStream> {
        let url = f["url"].as_str()?.to_string();
        let headers = f["http_headers"].as_object().map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect()).unwrap_or_default();
        let len = f["filesize"].as_u64().or_else(|| clen_from_url(&url));
        let protocol = f["protocol"].as_str().map(str::to_owned);
        let has_video = f["vcodec"].as_str().is_some_and(|v| v != "none");
        let has_audio = f["acodec"].as_str().is_some_and(|v| v != "none");
        Some(ResolvedStream { url, headers, len, protocol, has_video, has_audio })
    };
    let formats: Vec<&Value> = match json["requested_formats"].as_array() {
        Some(a) if !a.is_empty() => a.iter().collect(),
        _ => vec![json],
    };
    let streams: Vec<ResolvedStream> = formats.into_iter().filter_map(one).collect();
    if streams.is_empty() {
        return Err(Error::other("yt-dlp returned no stream URLs"));
    }
    if streams.iter().any(|s| !matches!(s.protocol.as_deref(), Some("http" | "https"))) {
        return Err(Error::other("selected format is not a byte-range HTTP source; choose an HTTP(S) format instead of an HLS/DASH manifest"));
    }
    if streams.iter().any(|s| !matches!(url_scheme(&s.url), Some("http" | "https"))) {
        return Err(Error::other("selected format protocol does not match its HTTP(S) source URL; choose another format"));
    }
    if streams.iter().any(|s| manifest_path(&s.url)) {
        return Err(Error::other("selected format is a playlist/manifest, not a byte-range HTTP source; choose another format"));
    }
    Ok(Resolved { is_live: json["is_live"].as_bool().unwrap_or(false), duration: json["duration"].as_f64(), height: json["height"].as_u64().map(|h| h as u32), streams })
}

fn url_scheme(url: &str) -> Option<&str> { url.split_once("://").map(|(scheme, _)| scheme) }

/// Match the URL path only: signed query tokens may contain arbitrary suffixes.
fn manifest_path(url: &str) -> bool {
    let authority_and_path = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let path = authority_and_path.find('/').map(|i| &authority_and_path[i..]).unwrap_or("");
    let path = path.split(['?', '#']).next().unwrap_or("");
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".m3u8") || lower.ends_with(".mpd")
}

fn clen_from_url(url: &str) -> Option<u64> {
    let q = url.split_once('?')?.1;
    crate::extract::parse_query(q).into_iter().find(|(k, _)| k == "clen").and_then(|(_, v)| v.parse().ok())
}

fn upstream_agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(15)).timeout_read(Duration::from_secs(30)).build()
}

fn with_headers(mut req: ureq::Request, headers: &[(String, String)]) -> ureq::Request {
    for (k, v) in headers {
        let lk = k.to_ascii_lowercase();
        if lk == "range" || lk == "host" || lk == "accept-encoding" {
            continue;
        }
        req = req.set(k, v);
    }
    req
}

/// A length-probe failure and whether trying the same URL again can help.
#[derive(Debug, Clone)]
pub struct ProbeError {
    pub message: String,
    pub transient: bool,
}

/// Learns the exact length with a one-byte range request.
pub fn probe_len_detail(url: &str, headers: &[(String, String)]) -> std::result::Result<u64, ProbeError> {
    let resp = with_headers(upstream_agent().get(url), headers).set("Range", "bytes=0-0").call().map_err(|e| {
        let transient = match &e {
            ureq::Error::Status(code, _) => *code >= 500 || matches!(*code, 408 | 425 | 429),
            ureq::Error::Transport(t) => !matches!(t.kind(), ureq::ErrorKind::InvalidUrl | ureq::ErrorKind::UnknownScheme),
        };
        ProbeError { message: format!("probing stream length: {e}"), transient }
    })?;
    resp.header("content-range")
        .and_then(|v| v.rsplit('/').next())
        .and_then(|t| t.trim().parse().ok())
        .ok_or_else(|| ProbeError { message: "the stream did not report its length".into(), transient: false })
}

pub fn probe_len(url: &str, headers: &[(String, String)]) -> Result<u64> {
    probe_len_detail(url, headers).map_err(|e| Error::other(e.message))
}

/// Streams `[start, end]` of `source` into `send`, one chunk-sized request at a time.
/// Returns Err when the consumer went away or the upstream kept failing.
pub fn pump_range(source: &Source, start: u64, end: u64, send: &mut dyn FnMut(Vec<u8>) -> bool) -> std::result::Result<(), String> {
    let agent = upstream_agent();
    let mut pos = start;
    let mut failures = 0;
    while pos <= end {
        let chunk_end = (pos + CHUNK - 1).min(end);
        let resp = match with_headers(agent.get(&source.url), &source.headers).set("Range", &format!("bytes={pos}-{chunk_end}")).call() {
            Ok(r) => r,
            Err(e) => {
                failures += 1;
                if failures > 3 {
                    return Err(format!("upstream: {e}"));
                }
                std::thread::sleep(Duration::from_millis(400 * failures));
                continue;
            }
        };
        let mut reader = resp.into_reader();
        let mut buf = vec![0u8; 64 * 1024];
        let mut got_any = false;
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    got_any = true;
                    if let Some(flag) = &source.first_byte {
                        flag.store(true, Ordering::SeqCst);
                    }
                    pos += n as u64;
                    if !send(buf[..n].to_vec()) {
                        return Err("consumer closed".into());
                    }
                    if pos > chunk_end {
                        break;
                    }
                }
                Err(e) => {
                    failures += 1;
                    if failures > 3 {
                        return Err(format!("upstream read: {e}"));
                    }
                    break;
                }
            }
        }
        if !got_any && pos <= chunk_end {
            failures += 1;
            if failures > 3 {
                return Err("upstream returned no data".into());
            }
        }
    }
    Ok(())
}

impl Core {
    pub fn source(&self, key: &str) -> Option<Source> {
        self.streams.sources.lock().unwrap_or_else(|e| e.into_inner()).get(key).cloned()
    }

    pub fn stream_job(&self, video_id: &str) -> Option<Arc<StreamJob>> {
        self.streams.jobs.lock().unwrap_or_else(|e| e.into_inner()).get(video_id).cloned()
    }

    fn tuning(&self) -> StreamTuning {
        *self.streams.tuning.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Helper output made safe for logs, entries and the player: no URLs, hosts, IDs, paths or tokens.
    pub(crate) fn redact_stream_text(&self, text: &str, video_id: &str) -> String {
        redact_error(text, video_id, &[self.token()], &std::env::var("HOME").unwrap_or_default(), ERROR_TEXT_MAX)
    }

    /// A lifecycle event carrying the job's identity, attempt and elapsed time.
    fn stream_event(&self, level: &str, event: &str, job: &StreamJob, extra: Value) {
        let (entry, attempt, started, phase) = {
            let s = job.state.lock().unwrap_or_else(|e| e.into_inner());
            (s.entry_id.clone(), s.attempt, s.started_ms, s.phase.clone())
        };
        let mut data = json!({"video_id":job.video_id,"entry_id":entry,"attempt":attempt,"elapsed_ms":(self.now() - started).max(0),"phase":phase});
        if let (Some(map), Value::Object(more)) = (data.as_object_mut(), extra) {
            map.extend(more);
        }
        self.log_event(level, event, data);
    }

    fn is_current_job(&self, job: &StreamJob) -> bool {
        self.streams.jobs.lock().unwrap_or_else(|e| e.into_inner()).get(&job.video_id).is_some_and(|j| std::ptr::eq(&**j, job))
    }

    /// Asks a running entry to stop and records why, so the log can tell a deliberate stop from a failure.
    pub(crate) fn cancel_with_cause(&self, entry: &str, cause: &'static str) {
        // Only a running entry can be stopped; a cause for anything else would never be cleared.
        if let Some(job) = self.running.lock().unwrap_or_else(|e| e.into_inner()).get(entry) {
            self.streams.cancel_causes.lock().unwrap_or_else(|e| e.into_inner()).insert(entry.to_string(), cause);
            job.cancel.store(true, Ordering::SeqCst);
        }
    }

    /// Decides what a (re-)open of a video does with the job it already has.
    fn admit_existing(&self, video_id: &str, audio_only: bool) -> Admit {
        let Some(job) = self.stream_job(video_id) else { return Admit::Fresh };
        let (phase, entry, job_audio) = {
            let s = job.state.lock().unwrap_or_else(|e| e.into_inner());
            (s.phase.clone(), s.entry_id.clone(), s.audio_only)
        };
        let running = self.running.lock().unwrap_or_else(|e| e.into_inner()).get(&entry).cloned();
        match phase.as_str() {
            // A finished job's thread may still be unwinding: its entry and HLS directory are not free yet.
            "failed" | "canceled" => if running.is_some() { Admit::Stopping } else { Admit::Fresh },
            "done" => Admit::Join(job),
            _ => {
                if running.as_ref().is_some_and(|r| r.cancel.load(Ordering::SeqCst)) {
                    Admit::Stopping
                } else if job_audio != audio_only {
                    self.cancel_with_cause(&entry, "source_switch");
                    Admit::Stopping
                } else {
                    Admit::Join(job)
                }
            }
        }
    }

    /// Starts (or joins) the play-while-caching pipeline for a video. At most one job per video exists:
    /// a re-open joins the active one and waits for one that is being stopped.
    pub fn start_stream(self: &Arc<Self>, video_id: &str, audio_only: bool) -> Result<Arc<StreamJob>> {
        self.require_online()?;
        if !valid_video_id(video_id) {
            return Err(Error::bad("invalid video_id"));
        }
        let deadline = Instant::now() + self.tuning().stop_wait;
        loop {
            let guard = self.streams.start_lock.lock().unwrap_or_else(|e| e.into_inner());
            match self.admit_existing(video_id, audio_only) {
                Admit::Join(job) => {
                    self.stream_event("info", "stream.joined", &job, json!({"explicit":true}));
                    return Ok(job);
                }
                Admit::Fresh => return self.start_fresh(video_id, audio_only),
                Admit::Stopping => {
                    drop(guard);
                    if Instant::now() >= deadline {
                        return Err(Error::other("the previous attempt is still stopping; try again in a moment"));
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
            }
        }
    }

    /// Explicit playback outranks speculative background downloads: when the concurrency budget is
    /// spent, stop the newest speculative ones (they are queued again and resume later).
    fn preempt_speculative(&self) -> usize {
        let concurrency = self.video.settings().download_concurrency as usize;
        let running: Vec<(String, Arc<RunningJob>)> = self.running.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        let over = (running.len() + 1).saturating_sub(concurrency);
        if over == 0 {
            return 0;
        }
        let stream_entries: Vec<String> = self.streams.jobs.lock().unwrap_or_else(|e| e.into_inner()).values().map(|j| j.state.lock().unwrap_or_else(|e| e.into_inner()).entry_id.clone()).collect();
        let mut candidates: Vec<(i64, String)> = running
            .into_iter()
            .filter(|(id, r)| {
                !stream_entries.contains(id)
                    && !r.cancel.load(Ordering::SeqCst)
                    && self.video.get_entry(id).ok().flatten().is_some_and(|e| matches!(e.origin.as_str(), "rule" | "prefetch" | "subscription" | "capture"))
            })
            .map(|(id, r)| (r.started_at, id))
            .collect();
        candidates.sort_by_key(|(started, _)| std::cmp::Reverse(*started));
        let mut stopped = 0;
        for (_, id) in candidates.into_iter().take(over) {
            self.cancel_with_cause(&id, "preempted");
            self.log_event("info", "download.preempted", json!({"entry_id":id,"reason":"explicit_playback"}));
            stopped += 1;
        }
        stopped
    }

    fn start_fresh(self: &Arc<Self>, video_id: &str, audio_only: bool) -> Result<Arc<StreamJob>> {
        let active = self
            .streams
            .jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .filter(|j| matches!(j.state.lock().unwrap_or_else(|e| e.into_inner()).phase.as_str(), "resolving" | "starting" | "caching"))
            .count();
        if active >= MAX_STREAMS {
            return Err(Error::other(format!("Muninn is already caching {active} videos for playback; wait for one to finish (or cancel one in the Library)")));
        }
        let tools = self.tools();
        if !tools.ytdlp.installed {
            return Err(Error::other("yt-dlp is not ready yet; Muninn is setting it up, try again in a moment"));
        }
        let Some(ffmpeg) = tools.ffmpeg.path.clone() else {
            return Err(Error::other("ffmpeg is missing from this build, so Muninn cannot play and cache at the same time"));
        };
        let settings = self.video.settings();
        let quality = if audio_only { Quality::Audio } else { variant_for_height(settings.watch_height) };
        let variant = quality.as_str().to_string();
        let id = entry_id(video_id, &variant);
        let old = self.video.get_entry(&id)?;
        if old.as_ref().is_some_and(|o| o.is_complete() && o.file_path.as_deref().is_some_and(|p| Path::new(p).exists())) {
            // A finished copy exists: never replace it with a new download.
            return Err(Error::Conflict("this video is already cached".into()));
        }
        let facts = self.facts_for(video_id);
        let hls_dir = hls_dir_for(&self.paths.cache_dir, video_id, &variant);
        let job = Arc::new(StreamJob {
            video_id: video_id.to_string(),
            hls_dir: hls_dir.clone(),
            state: Mutex::new(StreamState {
                phase: "resolving".into(),
                started_ms: self.now(),
                variant: variant.clone(),
                entry_id: id.clone(),
                audio_only,
                attempt: 1,
                ..StreamState::default()
            }),
        });
        // Admission first: a start that cannot get disk space must not preempt background downloads
        // or touch the previous entry (its row, its protected flag, its partial files).
        let estimate = crate::jobs::estimate_bytes(&variant, facts.duration_seconds, None);
        if let Ok(short) = self.enforce_caps(estimate) {
            if short > 0 {
                self.streams.jobs.lock().unwrap_or_else(|e| e.into_inner()).insert(video_id.to_string(), job.clone());
                self.stream_event("info", "stream.start", &job, json!({"origin":"explicit","variant":variant,"audio_only":audio_only,"active_streams":active,"queue_position":active,"running_jobs":self.running_count(),"concurrency":settings.download_concurrency,"preempted_background":0}));
                self.fail_stream(&job, &Failure::new(FailureReason::DiskSpace, format!("not enough disk: {} MB short under the cache cap or minimum free space", short / 1_000_000 + 1)));
                return Ok(job);
            }
        }
        let preempted = self.preempt_speculative();
        let _ = std::fs::remove_dir_all(&hls_dir);
        std::fs::create_dir_all(&hls_dir)?;
        let running_jobs = self.running_count();
        self.streams.jobs.lock().unwrap_or_else(|e| e.into_inner()).insert(video_id.to_string(), job.clone());
        self.stream_event("info", "stream.start", &job, json!({"origin":"explicit","variant":variant,"audio_only":audio_only,"active_streams":active,"queue_position":active,"running_jobs":running_jobs,"concurrency":settings.download_concurrency,"preempted_background":preempted}));
        // Make the entry visible right away (status downloading) so the UI, the disk caps and the rules see it.
        let decision = retention_for_request(&self.video.rules(), &facts);
        let mut row = EntryRow::new(video_id, &variant, &variant, "play", self.now());
        row.title = facts.title.clone();
        row.channel = facts.channel.clone();
        row.duration_seconds = facts.duration_seconds;
        row.retention_days = Some(decision.retention_days);
        row.rule = decision.rule.clone();
        row.expires_at = self.retention_expiry(self.now(), decision.retention_days);
        row.status = STATUS_DOWNLOADING.into();
        if let Some(old) = old {
            // A failed or canceled earlier try (or an index row whose file is gone): start over on the
            // same row, keeping what the user decided about it.
            row.protected = old.protected;
            self.video.delete_entry(&old.id)?;
        }
        self.video.insert_entry(&row)?;
        let running = Arc::new(RunningJob { cancel: false.into(), started_at: self.now() });
        self.running.lock().unwrap_or_else(|e| e.into_inner()).insert(id.clone(), running.clone());
        self.stream_event("info", "stream.admitted", &job, json!({"origin":"explicit","queue_position":0}));
        if self.is_offline() {
            self.offline_paused.lock().unwrap_or_else(|e| e.into_inner()).insert(id.clone());
            running.cancel.store(true, Ordering::SeqCst);
        }
        let core = self.clone();
        let job2 = job.clone();
        let running2 = running.clone();
        let spawned = std::thread::Builder::new().name(format!("muninn-stream-{video_id}")).spawn(move || {
            core.run_stream(&job2, &running2, &ffmpeg, settings.watch_height, audio_only);
            let entry = job2.state.lock().unwrap_or_else(|e| e.into_inner()).entry_id.clone();
            {
                // A newer job may already own this entry id; only remove our own registration.
                let mut map = core.running.lock().unwrap_or_else(|e| e.into_inner());
                if map.get(&entry).is_some_and(|r| Arc::ptr_eq(r, &running2)) {
                    map.remove(&entry);
                    // Cleared after the registration is gone, so a late cancel cannot leave a stale cause.
                    core.streams.cancel_causes.lock().unwrap_or_else(|e| e.into_inner()).remove(&entry);
                }
            }
            core.wake_scheduler();
        });
        if spawned.is_err() {
            self.running.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
            self.fail_stream(&job, &Failure::new(FailureReason::HelperExit, "could not start the stream thread"));
        }
        Ok(job)
    }

    fn set_phase(&self, job: &StreamJob, phase: &str) {
        job.state.lock().unwrap_or_else(|e| e.into_inner()).phase = phase.into();
    }

    /// Records a failed start: one `stream.failed` event with a reason category, the entry marked
    /// failed (only if this job still owns it) and the state the player shows. Only this entry's
    /// partial output is touched, never another copy of the video.
    fn fail_stream(&self, job: &StreamJob, failure: &Failure) {
        let message = self.redact_stream_text(&failure.message, &job.video_id);
        let reason = failure.reason;
        let entry = job.state.lock().unwrap_or_else(|e| e.into_inner()).entry_id.clone();
        self.stream_event(
            if reason.expected() { "warn" } else { "error" },
            "stream.failed",
            job,
            json!({"origin":"explicit","reason":reason.as_str(),"retryable":reason.retryable(),"exit_code":failure.exit_code,"exit_signal":failure.signal,"after_first_bytes":failure.after_first_bytes,"error":message}),
        );
        if self.is_current_job(job) {
            let _ = self.video.set_status(&entry, STATUS_FAILED, Some(&message), self.now());
            let _ = self.video.set_progress(&entry, 0, None, 0.0, None);
        }
        let mut s = job.state.lock().unwrap_or_else(|e| e.into_inner());
        s.error = Some(message);
        s.reason = Some(reason.as_str().into());
        s.retryable = reason.retryable();
        s.phase = "failed".into();
    }

    /// A deliberate stop (another source, preemption, shutdown, the user): not a failure.
    fn finish_canceled(&self, job: &StreamJob) {
        let entry = job.state.lock().unwrap_or_else(|e| e.into_inner()).entry_id.clone();
        let cause = if self.shutdown.load(Ordering::SeqCst) {
            "shutdown"
        } else if let Some(c) = self.streams.cancel_causes.lock().unwrap_or_else(|e| e.into_inner()).get(&entry).copied() {
            c
        } else if self.offline_paused.lock().unwrap_or_else(|e| e.into_inner()).contains(&entry) {
            "offline"
        } else {
            "user"
        };
        self.stream_event("info", "stream.cancelled", job, json!({"origin":"explicit","cause":cause}));
        if self.is_current_job(job) {
            let _ = self.video.set_status(&entry, STATUS_CANCELED, None, self.now());
            let _ = self.video.set_progress(&entry, 0, None, 0.0, None);
        }
        let mut s = job.state.lock().unwrap_or_else(|e| e.into_inner());
        s.reason = Some(FailureReason::Cancelled.as_str().into());
        s.retryable = true;
        s.phase = "canceled".into();
    }

    fn resolve_streams(&self, job: &StreamJob, selector: &str, cancel: &AtomicBool, tuning: &StreamTuning) -> std::result::Result<Resolved, Failure> {
        let video_id = job.video_id.as_str();
        if self.is_offline() {
            return Err(Failure::new(FailureReason::Offline, "Offline mode is on"));
        }
        let tools = self.tools();
        let bin = tools.ytdlp.path.clone().ok_or_else(|| Failure::new(FailureReason::ToolsPending, "yt-dlp is not installed"))?;
        let mut args: Vec<String> =
            ["--ignore-config", "--no-playlist", "--no-warnings", "-f", selector, "-J"].iter().map(|s| s.to_string()).collect();
        let cookies = self.youtube_cookie_file()
            .map_err(|_| Failure::new(FailureReason::HelperExit, "Could not prepare the YouTube session"))?;
        if let Some(jar) = &cookies { args.extend(jar.args()); }
        if let Some(dir) = tools.ffmpeg_dir() {
            args.push("--ffmpeg-location".into());
            args.push(dir.display().to_string());
        }
        if let (Some(name), Some(path)) = (tools.js_runtime.name.clone(), tools.js_runtime.path.clone()) {
            args.push("--js-runtimes".into());
            args.push(format!("{name}:{path}"));
        }
        args.push("--".into());
        args.push(format!("https://www.youtube.com/watch?v={video_id}"));
        let mut child = Command::new(&bin)
            .args(&args)
            .env("PATH", search_path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| Failure::new(FailureReason::HelperExit, format!("could not start yt-dlp: {e}")))?;
        self.stream_event("info", "helper.launched", job, json!({"helper":"yt-dlp"}));
        let mut out = child.stdout.take().ok_or_else(|| Failure::new(FailureReason::HelperExit, "no stdout"))?;
        let mut err = child.stderr.take().ok_or_else(|| Failure::new(FailureReason::HelperExit, "no stderr"))?;
        let t_out = std::thread::spawn(move || {
            let mut s = Vec::new();
            let _ = out.by_ref().take(32 * 1024 * 1024).read_to_end(&mut s);
            s
        });
        let t_err = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = err.by_ref().take(1024 * 1024).read_to_string(&mut s);
            s
        });
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(st)) => break st,
                Ok(None) => {}
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(Failure::new(FailureReason::HelperExit, e.to_string()));
                }
            }
            if cancel.load(Ordering::SeqCst) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Failure::cancelled());
            }
            if started.elapsed() > tuning.resolve_timeout {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Failure::new(FailureReason::ResolveTimeout, "yt-dlp took too long to resolve the video"));
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let stdout = t_out.join().unwrap_or_default();
        let stderr = t_err.join().unwrap_or_default();
        if !status.success() {
            let line = stderr.lines().rev().find(|l| l.contains("ERROR")).or_else(|| stderr.lines().last()).unwrap_or("yt-dlp failed").trim().to_string();
            return Err(Failure::from_text(line, FailureReason::HelperExit).with_status(&status));
        }
        let json: Value = serde_json::from_slice(&stdout).map_err(|_| Failure::new(FailureReason::HelperExit, "yt-dlp returned unreadable output"))?;
        parse_resolved(&json).map_err(|e| { let message=e.to_string(); if message.contains("selected format") { Failure::new(FailureReason::FormatUnavailable,message) } else { Failure::from_text(message, FailureReason::Unknown) } })
    }

    /// Resolves with one re-resolve for failures that can be transient (network trouble or an
    /// unrecognised error; the first call also warms the JS runtime). Unavailable, restricted,
    /// live and format errors are final and are not repeated.
    fn resolve_with_retry(&self, job: &StreamJob, selector: &str, cancel: &AtomicBool, tuning: &StreamTuning) -> std::result::Result<Resolved, Failure> {
        let first = self.resolve_streams(job, selector, cancel, tuning);
        match first {
            Err(f) if matches!(f.reason, FailureReason::TransportRetryable | FailureReason::HelperExit | FailureReason::Unknown) && !cancel.load(Ordering::SeqCst) => {
                self.stream_event("warn", "source.resolve_retry", job, json!({"reason":f.reason.as_str(),"backoff_ms":tuning.resolve_backoff.as_millis() as u64}));
                if sleep_unless(cancel, tuning.resolve_backoff) {
                    return Err(Failure::cancelled());
                }
                self.resolve_streams(job, selector, cancel, tuning)
            }
            other => other,
        }
    }

    /// Learns a stream's length, retrying only errors that can pass (network trouble, 5xx, 429).
    fn probe_with_retry(&self, job: &StreamJob, st: &ResolvedStream, cancel: &AtomicBool, tuning: &StreamTuning) -> std::result::Result<u64, Failure> {
        let mut last = None;
        for n in 1..=tuning.probe_attempts.max(1) {
            match probe_len_detail(&st.url, &st.headers) {
                Ok(len) => return Ok(len),
                Err(e) => {
                    let again = e.transient && n < tuning.probe_attempts;
                    if again {
                        let backoff = tuning.probe_backoff * n;
                        self.stream_event("warn", "source.probe_retry", job, json!({"probe_attempt":n,"backoff_ms":backoff.as_millis() as u64,"error":self.redact_stream_text(&e.message, &job.video_id)}));
                        if sleep_unless(cancel, backoff) {
                            return Err(Failure::cancelled());
                        }
                    }
                    last = Some(e);
                    if !again {
                        break;
                    }
                }
            }
        }
        let e = last.unwrap_or(ProbeError { message: "the stream did not report its length".into(), transient: false });
        Err(Failure::new(FailureReason::LengthProbeFailed, e.message))
    }

    /// Runs the pipeline; transient failures before any media arrived get one more attempt with
    /// fresh URLs, everything else is reported once.
    fn run_stream(self: &Arc<Self>, job: &Arc<StreamJob>, running: &Arc<RunningJob>, ffmpeg: &str, watch_height: u32, audio_only: bool) {
        let tuning = self.tuning();
        let mut attempt = 0;
        loop {
            attempt += 1;
            {
                let mut s = job.state.lock().unwrap_or_else(|e| e.into_inner());
                s.attempt = attempt;
                s.phase = "resolving".into();
            }
            if attempt > 1 {
                let _ = std::fs::remove_dir_all(&job.hls_dir);
                let _ = std::fs::create_dir_all(&job.hls_dir);
            }
            match self.stream_attempt(job, running, ffmpeg, watch_height, audio_only, &tuning) {
                Outcome::Done => return,
                Outcome::Canceled => {
                    self.finish_canceled(job);
                    return;
                }
                Outcome::Failed(f) => {
                    let stopping = running.cancel.load(Ordering::SeqCst) || self.shutdown.load(Ordering::SeqCst);
                    if stopping {
                        self.finish_canceled(job);
                        return;
                    }
                    if !(f.reason.auto_retry() && !f.after_first_bytes && attempt < tuning.max_attempts) {
                        self.fail_stream(job, &f);
                        return;
                    }
                    self.stream_event("warn", "stream.retrying", job, json!({"reason":f.reason.as_str(),"retryable":true,"backoff_ms":tuning.retry_backoff.as_millis() as u64,"error":self.redact_stream_text(&f.message, &job.video_id)}));
                    if sleep_unless(&running.cancel, tuning.retry_backoff) {
                        self.finish_canceled(job);
                        return;
                    }
                }
            }
        }
    }

    fn stream_attempt(self: &Arc<Self>, job: &Arc<StreamJob>, running: &Arc<RunningJob>, ffmpeg: &str, watch_height: u32, audio_only: bool, tuning: &StreamTuning) -> Outcome {
        let (entry, variant) = {
            let s = job.state.lock().unwrap_or_else(|e| e.into_inner());
            (s.entry_id.clone(), s.variant.clone())
        };
        if running.cancel.load(Ordering::SeqCst) {
            return Outcome::Canceled;
        }
        let selector = selector_for(watch_height, audio_only);
        let resolve_started = Instant::now();
        let resolved = match self.resolve_with_retry(job, &selector, &running.cancel, tuning) {
            Ok(r) => r,
            Err(f) if f.reason == FailureReason::Cancelled => return Outcome::Canceled,
            Err(f) => return Outcome::Failed(f),
        };
        self.stream_event(
            "info",
            "source.resolved",
            job,
            json!({"streams":resolved.streams.len(),"length_known":resolved.streams.iter().filter(|s|s.len.is_some()).count(),"is_live":resolved.is_live,"height":resolved.height,"resolve_ms":resolve_started.elapsed().as_millis() as u64}),
        );
        if resolved.is_live {
            return Outcome::Failed(Failure::new(FailureReason::LiveStream, "This is a live stream; Muninn's player plays recorded videos (open it on youtube.com instead)"));
        }
        {
            let mut s = job.state.lock().unwrap_or_else(|e| e.into_inner());
            s.phase = "starting".into();
            s.duration_seconds = resolved.duration;
            s.height = resolved.height;
        }
        // Register the feeder sources.
        let seen_bytes = Arc::new(AtomicBool::new(false));
        let mut keys = Vec::new();
        let mut inputs = Vec::new();
        for (i, st) in resolved.streams.iter().enumerate() {
            let len = match st.len {
                Some(l) => l,
                None => match self.probe_with_retry(job, st, &running.cancel, tuning) {
                    Ok(l) => l,
                    Err(f) => {
                        self.drop_sources(&keys);
                        return if f.reason == FailureReason::Cancelled { Outcome::Canceled } else { Outcome::Failed(f) };
                    }
                },
            };
            let key = format!("{}-{}/{}", job.video_id, variant, i);
            self.streams.sources.lock().unwrap_or_else(|e| e.into_inner()).insert(key.clone(), Source { url: st.url.clone(), headers: st.headers.clone(), len, first_byte: Some(seen_bytes.clone()) });
            inputs.push(format!("http://127.0.0.1:{}/s/{}/{}", self.port(), self.token(), key));
            keys.push(key);
        }
        let audio_only = audio_only || resolved.streams.iter().all(|s| !s.has_video);
        let out_dir = self.paths.video_dir.clone();
        let _ = std::fs::create_dir_all(&out_dir);
        let partial = out_dir.join(format!("{entry}.partial.mp4"));
        let final_path = out_dir.join(format!("{entry}.mp4"));
        let args = ffmpeg_args(&inputs, &job.hls_dir, &partial, audio_only);
        let mut child = match Command::new(ffmpeg).args(&args).env("PATH", search_path()).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
            Ok(c) => c,
            Err(e) => {
                self.drop_sources(&keys);
                return Outcome::Failed(Failure::new(FailureReason::HelperExit, format!("could not start ffmpeg: {e}")));
            }
        };
        let launched = Instant::now();
        self.stream_event("info", "helper.launched", job, json!({"helper":"ffmpeg"}));
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let duration = resolved.duration;
        let video = self.clone();
        let job_p = job.clone();
        let entry_p = entry.clone();
        let seen_p = seen_bytes.clone();
        let progress_thread = std::thread::spawn(move || {
            let Some(stdout) = stdout else { return };
            let mut last = Instant::now() - Duration::from_secs(1);
            for line in BufReader::new(stdout).lines().map_while(|l| l.ok()) {
                if let Some(us) = line.strip_prefix("out_time_us=").and_then(|v| v.trim().parse::<i64>().ok()) {
                    if us > 0 {
                        seen_p.store(true, Ordering::SeqCst);
                    }
                    if let (true, Some(d)) = (last.elapsed() > Duration::from_millis(500), duration.filter(|d| *d > 0.0)) {
                        last = Instant::now();
                        let frac = (us as f64 / 1e6 / d).clamp(0.0, 0.999);
                        job_p.state.lock().unwrap_or_else(|e| e.into_inner()).progress = frac;
                        if video.is_current_job(&job_p) {
                            let _ = video.video.set_progress(&entry_p, 0, None, frac, None);
                        }
                    }
                }
            }
        });
        let err_thread = std::thread::spawn(move || {
            let mut tail: Vec<String> = Vec::new();
            if let Some(e) = stderr {
                for line in BufReader::new(e).lines().map_while(|l| l.ok()) {
                    tail.push(line);
                    if tail.len() > 8 {
                        tail.remove(0);
                    }
                }
            }
            tail
        });
        self.set_phase(job, "caching");
        let (mut canceled, mut timed_out, mut logged_first_bytes, mut logged_segment) = (false, false, false, false);
        let status = loop {
            match child.try_wait() {
                Ok(Some(st)) => break Some(st),
                Ok(None) => {}
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
            }
            let segments = playlist_progress(&job.hls_dir).0;
            if !logged_first_bytes && (seen_bytes.load(Ordering::SeqCst) || segments >= 1) {
                logged_first_bytes = true;
                self.stream_event("info", "first_bytes", job, json!({"since_helper_ms":launched.elapsed().as_millis() as u64}));
            }
            if segments >= 2 && !logged_segment {
                logged_segment = true;
                self.stream_event("info", "first_segment", job, json!({"since_helper_ms":launched.elapsed().as_millis() as u64}));
            }
            {
                let mut s = job.state.lock().unwrap_or_else(|e| e.into_inner());
                if s.first_segment_ms.is_none() && segments >= 2 {
                    s.first_segment_ms = Some(self.now());
                }
            }
            if running.cancel.load(Ordering::SeqCst) || self.shutdown.load(Ordering::SeqCst) {
                canceled = true;
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            if !logged_first_bytes && launched.elapsed() > tuning.no_first_bytes_timeout {
                timed_out = true;
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let _ = progress_thread.join();
        let tail = err_thread.join().unwrap_or_default();
        self.drop_sources(&keys);
        let segments = playlist_progress(&job.hls_dir).0;
        let got_bytes = seen_bytes.load(Ordering::SeqCst) || segments >= 1;
        // A fast helper can finish between two polls; the timeline still gets its milestones.
        if got_bytes && !logged_first_bytes && !canceled {
            self.stream_event("info", "first_bytes", job, json!({"since_helper_ms":launched.elapsed().as_millis() as u64}));
        }
        if segments >= 2 && !logged_segment && !canceled {
            self.stream_event("info", "first_segment", job, json!({"since_helper_ms":launched.elapsed().as_millis() as u64}));
        }
        if canceled {
            let _ = std::fs::remove_file(&partial);
            return Outcome::Canceled;
        }
        if timed_out {
            let _ = std::fs::remove_file(&partial);
            return Outcome::Failed(Failure::new(FailureReason::NoFirstBytesTimeout, format!("no media data arrived within {} s", tuning.no_first_bytes_timeout.as_secs().max(1))));
        }
        match status {
            Some(st) if st.success() && partial.exists() => {
                let current = self.video.get_entry(&entry).ok().flatten();
                let protect = current.as_ref().is_some_and(|e| e.protected);
                let target = if protect { self.paths.library_dir.join(final_path.file_name().unwrap_or_default()) } else { final_path };
                if let Some(parent) = target.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if std::fs::rename(&partial, &target).is_err() {
                    let mut f = Failure::new(FailureReason::HelperExit, "could not move the finished file into place");
                    f.after_first_bytes = true;
                    return Outcome::Failed(f);
                }
                let size = std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
                let height = resolved.height;
                let _ = self.video.set_file(&entry, &target.display().to_string(), "mp4", Some(size), height, true);
                let _ = self.video.complete_entry(&entry, size, height);
                self.log_event("info", "stream.cached", json!({"video_id":job.video_id,"entry_id":entry,"bytes":size}));
                let mut s = job.state.lock().unwrap_or_else(|e| e.into_inner());
                s.phase = "done".into();
                s.progress = 1.0;
                Outcome::Done
            }
            other => {
                let _ = std::fs::remove_file(&partial);
                let mut f = failure_from_ffmpeg_tail(&tail);
                if let Some(st) = other.as_ref() {
                    f = f.with_status(st);
                }
                f.after_first_bytes = got_bytes;
                Outcome::Failed(f)
            }
        }
    }

    fn drop_sources(&self, keys: &[String]) {
        let mut s = self.streams.sources.lock().unwrap_or_else(|e| e.into_inner());
        for k in keys {
            s.remove(k);
        }
    }

    /// Stale HLS directories from a previous run mean nothing now.
    pub fn clear_hls(&self) {
        let _ = std::fs::remove_dir_all(self.paths.cache_dir.join("hls"));
    }

    /// Why a start failed before any job existed, as the watch status reports it.
    pub(crate) fn record_start_error(&self, video_id: &str, error: &Error) {
        let message = self.redact_stream_text(&error.to_string(), video_id);
        let reason = reason_for_start_error(&error.to_string());
        self.log_event(
            if reason.expected() { "warn" } else { "error" },
            "stream.failed",
            json!({"video_id":video_id,"origin":"explicit","phase":"admission","reason":reason.as_str(),"retryable":reason.retryable(),"attempt":0,"elapsed_ms":0,"error":message}),
        );
        self.streams.errors.lock().unwrap_or_else(|e| e.into_inner()).insert(video_id.to_string(), StartError { reason, message });
    }
}

/// Classifies ffmpeg's failure from its whole stderr tail (the cause is often not the last line)
/// and reports the last non-empty line as the message.
fn failure_from_ffmpeg_tail(tail: &[String]) -> Failure {
    let lines: Vec<&str> = tail.iter().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    let message = lines.last().map_or("ffmpeg failed".to_string(), |l| l.to_string());
    let reason = match classify_text(&lines.join(" ")) {
        FailureReason::Unknown => FailureReason::HelperExit,
        r => r,
    };
    Failure::new(reason, message)
}

/// Sleeps up to `total`, returning true early if `cancel` is raised.
fn sleep_unless(cancel: &AtomicBool, total: Duration) -> bool {
    let end = Instant::now() + total;
    while Instant::now() < end {
        if cancel.load(Ordering::SeqCst) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20).min(total));
    }
    cancel.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn watch_heights_map_to_cache_variants() {
        assert_eq!(variant_for_height(240), Quality::P360);
        assert_eq!(variant_for_height(480), Quality::P480);
        assert_eq!(variant_for_height(720), Quality::P720);
        assert_eq!(variant_for_height(1080), Quality::P1080);
    }

    #[test]
    fn selectors_prefer_h264_and_aac_that_copy_into_mp4() {
        let s = selector_for(480, false);
        assert!(s.starts_with("bv*[height<=480][vcodec^=avc1][protocol=http]+ba[ext=m4a][protocol=http]"), "{s}");
        assert!(s.contains("/bv*[height<=480][vcodec^=avc1][protocol=https]+ba[ext=m4a][protocol=https]"), "{s}");
        assert_eq!(selector_for(0, true), "ba[ext=m4a][protocol=http]/ba[ext=m4a][protocol=https]/ba[protocol=http]/ba[protocol=https]");
    }

    #[test]
    fn resolved_formats_carry_urls_headers_and_exact_lengths() {
        let j = json!({"duration": 213.0, "height": 480, "requested_formats": [
            {"url": "https://rr1.googlevideo.com/videoplayback?clen=1000&itag=135", "http_headers": {"User-Agent": "UA", "Accept": "*/*"}, "protocol":"https", "vcodec": "avc1.4d401f", "acodec": "none"},
            {"url": "https://rr1.googlevideo.com/videoplayback?itag=140", "filesize": 55, "protocol":"https", "http_headers": {"User-Agent": "UA"}, "vcodec": "none", "acodec": "mp4a.40.2"}]});
        let r = parse_resolved(&j).unwrap();
        assert_eq!(r.streams.len(), 2);
        assert_eq!((r.streams[0].len, r.streams[0].has_video, r.streams[0].has_audio), (Some(1000), true, false));
        assert_eq!((r.streams[1].len, r.streams[1].has_audio), (Some(55), true));
        assert!(r.streams[0].headers.contains(&("User-Agent".to_string(), "UA".to_string())));
        assert_eq!(r.duration, Some(213.0));
        assert!(!r.is_live);
        assert!(parse_resolved(&json!({"is_live": true, "url": "https://x/y?clen=1", "protocol":"https"})).unwrap().is_live);
        assert!(parse_resolved(&json!({"title": "x"})).is_err());
        // yt-dlp's http_dash_segments/http_dash_manifest are not byte-addressable HTTP URLs.
        // Even if a resolver labels one as http(s), never hand a playlist/manifest to range feeder.
        for (protocol, url) in [
            ("http_dash_segments", "https://host/video.m4s"),
            ("http_dash_manifest", "https://host/manifest.m3u8"),
            ("https", "https://host/index.m3u8"),
            ("http", "http://host/master.m3u8?token=redacted"),
            ("https", "https://host/manifest.mpd"),
            ("https", "https://host/media.M3U8?token=opaque"),
        ] {
            let selected = json!({"url":url,"protocol":protocol,"vcodec":"avc1","acodec":"mp4a","http_headers":{"User-Agent":"fixture-agent"}});
            assert!(parse_resolved(&selected).is_err(), "must reject {protocol} {url}");
        }
        for protocol in ["http", "https"] {
            let valid = json!({"url":format!("{protocol}://host/media.mp4?clen=9"),"protocol":protocol,"vcodec":"avc1","acodec":"mp4a"});
            assert_eq!(parse_resolved(&valid).unwrap().streams[0].len, Some(9));
        }
        // A single combined format (no requested_formats) is one stream.
        let single = parse_resolved(&json!({"url": "https://x/y?clen=9", "protocol":"https", "vcodec": "avc1", "acodec": "mp4a"})).unwrap();
        assert_eq!(single.streams.len(), 1);
    }

    #[test]
    fn ffmpeg_writes_the_hls_playlist_and_the_final_file_in_one_pass() {
        let args = ffmpeg_args(&["http://a/v".into(), "http://a/a".into()], Path::new("/h"), Path::new("/o/x.partial.mp4"), false);
        let joined = args.join(" ");
        assert!(joined.contains("-i http://a/v -i http://a/a"));
        assert!(joined.contains("-map 0:v:0 -map 1:a:0 -c copy -f hls"), "{joined}");
        assert!(joined.contains("-hls_segment_type fmp4"));
        assert!(joined.ends_with("-movflags +faststart -f mp4 /o/x.partial.mp4"), "{joined}");
        assert_eq!(args.iter().filter(|a| *a == "-map").count(), 4);
        let audio = ffmpeg_args(&["http://a/a".into()], Path::new("/h"), Path::new("/o/x.mp4"), true).join(" ");
        assert!(audio.contains("-map 0:a:0"), "{audio}");
    }

    #[test]
    fn playlist_progress_counts_finished_segments() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(playlist_progress(dir.path()), (0, false));
        std::fs::write(dir.path().join("index.m3u8"), "#EXTM3U\n#EXTINF:3.0,\nseg_00000.m4s\n#EXTINF:3.0,\nseg_00001.m4s\n").unwrap();
        assert_eq!(playlist_progress(dir.path()), (2, false));
        std::fs::write(dir.path().join("index.m3u8"), "#EXTM3U\n#EXTINF:3.0,\nseg_00000.m4s\n#EXT-X-ENDLIST\n").unwrap();
        assert_eq!(playlist_progress(dir.path()), (1, true));
    }

    #[test]
    fn recorded_event_and_finished_playlists_start_at_zero_without_moving_segments() {
        let event="#EXTM3U\n#EXT-X-PLAYLIST-TYPE:EVENT\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:3.0,\nseg_00000.m4s\n#EXTINF:3.0,\nseg_00001.m4s\n";
        for playlist in [event.to_owned(),format!("{event}#EXT-X-ENDLIST\n")] {
            let output=recorded_playlist_start(&playlist);
            assert!(output.starts_with("#EXTM3U\n#EXT-X-START:TIME-OFFSET=0,PRECISE=YES\n"));
            assert_eq!(output.lines().filter(|line|!line.starts_with("#EXT-X-START:")).collect::<Vec<_>>(),playlist.lines().collect::<Vec<_>>());
            assert_eq!(recorded_playlist_start(&output),output);
        }
        assert_eq!(recorded_playlist_start("not a playlist"),"not a playlist");
    }

    #[test]
    fn pumping_a_range_chunks_upstream_requests_and_stops_when_the_consumer_leaves() {
        use std::io::{Read as _, Write as _};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let data: Vec<u8> = (0..(CHUNK as usize * 2 + 5000)).map(|i| (i % 251) as u8).collect();
        let served = Arc::new(Mutex::new(Vec::<String>::new()));
        let (d2, s2) = (data.clone(), served.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (d, s) = (d2.clone(), s2.clone());
                std::thread::spawn(move || {
                    let mut stream = stream;
                    let mut buf = [0u8; 2048];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let range = req.lines().find(|l| l.to_ascii_lowercase().starts_with("range:")).unwrap_or("").to_string();
                    assert!(req.lines().any(|l| l.eq_ignore_ascii_case("x-test: 1")), "custom source header must be preserved: {req}");
                    s.lock().unwrap().push(range.clone());
                    let spec = range.split('=').nth(1).unwrap_or("0-0").trim().to_string();
                    let (a, b) = spec.split_once('-').unwrap();
                    let (a, b): (usize, usize) = (a.parse().unwrap(), b.parse().unwrap());
                    let body = &d[a..=b.min(d.len() - 1)];
                    let head = format!("HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(body);
                });
            }
        });
        let source = Source { url: format!("http://{addr}/v"), headers: vec![("X-Test".into(), "1".into())], len: data.len() as u64, first_byte: None };
        let mut got = Vec::new();
        pump_range(&source, 10, data.len() as u64 - 1, &mut |b| {
            got.extend_from_slice(&b);
            true
        })
        .unwrap();
        assert_eq!(got, data[10..]);
        let ranges = served.lock().unwrap().clone();
        assert_eq!(ranges.len(), 3, "8 MB chunks: {ranges:?}");
        assert!(ranges[0].contains("bytes=10-"), "{ranges:?}");
        // The consumer hanging up ends the pump.
        let mut n = 0;
        let r = pump_range(&source, 0, data.len() as u64 - 1, &mut |_| {
            n += 1;
            n < 3
        });
        assert!(r.is_err());
    }

    // --- lifecycle tests: fake yt-dlp / ffmpeg scripts, no network ----------------------------

    use crate::engine::{Core, CoreConfig};
    use crate::videodb::{STATUS_COMPLETE, STATUS_QUEUED};
    use crate::ytdlp::{Event, JobSpec, Runner};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::AtomicUsize;

    const ID: &str = "aaaaaaaaaaa";

    struct Rig {
        _data: tempfile::TempDir,
        bin: tempfile::TempDir,
        core: Arc<Core>,
    }

    fn script(dir: &Path, name: &str, body: &str) {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn fast_tuning(core: &Core) {
        let mut t = core.streams.tuning.lock().unwrap();
        t.no_first_bytes_timeout = Duration::from_millis(400);
        t.resolve_backoff = Duration::from_millis(10);
        t.probe_backoff = Duration::from_millis(10);
        t.retry_backoff = Duration::from_millis(10);
        t.stop_wait = Duration::from_secs(5);
    }

    /// `ytdlp` is the body after the `--version` handshake; `ffmpeg` the body after `-version`.
    fn rig(ytdlp: &str, ffmpeg: &str, runner: Option<Arc<dyn Runner>>) -> Rig {
        // Writing an executable while another thread forks can make exec fail with ETXTBSY:
        // create the scripts and run their first (`--version`) probe one rig at a time.
        static SCRIPTS: Mutex<()> = Mutex::new(());
        let _serial = SCRIPTS.lock().unwrap_or_else(|e| e.into_inner());
        let data = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        script(bin.path(), "yt-dlp", &format!("if [ \"$1\" = --version ]; then echo 2025.01.01; exit 0; fi\necho x >> \"$0.calls\"\n{ytdlp}\n"));
        script(bin.path(), "ffmpeg", &format!("if [ \"$1\" = -version ]; then echo 'ffmpeg test'; exit 0; fi\n{ffmpeg}\n"));
        let core = Core::open(CoreConfig { data_dir: data.path().to_path_buf(), tool_dirs: Some(vec![bin.path().to_path_buf()]), runner, ..CoreConfig::default() }).unwrap();
        fast_tuning(&core);
        Rig { _data: data, bin, core }
    }

    impl Rig {
        fn ytdlp_calls(&self) -> usize {
            std::fs::read_to_string(self.bin.path().join("yt-dlp.calls")).map(|s| s.lines().count()).unwrap_or(0)
        }
        fn events(&self, name: &str) -> Vec<Value> {
            self.core.logs(1000)["events"].as_array().unwrap().iter().filter(|e| e["event"] == name).map(|e| e["data"].clone()).collect()
        }
        fn log_text(&self) -> String {
            self.core.logs(1000).to_string()
        }
    }

    fn wait_for(what: &str, f: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !f() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn phase(job: &StreamJob) -> String {
        job.state.lock().unwrap().phase.clone()
    }

    fn wait_terminal(job: &Arc<StreamJob>) {
        wait_for("a terminal phase", || matches!(phase(job).as_str(), "failed" | "canceled" | "done"));
    }

    /// ffmpeg stand-in that behaves: progress, a two-segment playlist, the output file.
    const FFMPEG_OK: &str = "for a; do last=\"$a\"; case \"$a\" in *index.m3u8) idx=\"$a\";; esac; done\necho out_time_us=1000000\nprintf '#EXTM3U\\n#EXTINF:3.0,\\nseg_00000.m4s\\n#EXTINF:3.0,\\nseg_00001.m4s\\n#EXT-X-ENDLIST\\n' > \"$idx\"\necho data > \"$last\"\nexit 0";
    const FFMPEG_HANG: &str = "exec sleep 30";

    fn ytdlp_json(url: &str, with_len: bool) -> String {
        let len = if with_len { ",\"filesize\":1000" } else { "" };
        format!("cat <<'EOF'\n{{\"duration\":10.0,\"height\":360,\"requested_formats\":[{{\"url\":\"{url}\",\"protocol\":\"http\"{len},\"vcodec\":\"avc1\",\"acodec\":\"none\"}},{{\"url\":\"{url}\",\"protocol\":\"http\"{len},\"vcodec\":\"none\",\"acodec\":\"mp4a\"}}]}}\nEOF")
    }

    #[test]
    fn playback_resolver_receives_native_cookies_and_removes_jar_after_resolution() {
        let body = format!("jar=''\nwhile [ $# -gt 0 ]; do if [ \"$1\" = --cookies ]; then shift; jar=\"$1\"; fi; shift; done\n[ -n \"$jar\" ] && grep -q 'fixture-session' \"$jar\" || {{ echo 'ERROR: Sign in' >&2; exit 1; }}\nprintf '%s' \"$jar\" > \"$0.jar\"\n{}", ytdlp_json("http://127.0.0.1:9/v", true));
        let r = rig(&body, FFMPEG_OK, None);
        let key = "a".repeat(64);
        let now = r.core.now();
        r.core.observe_youtube_account(true, Some(&key), now).unwrap();
        r.core.observe_youtube_cookies(&key, now, vec![crate::youtube_session::YoutubeCookie {
            domain: ".youtube.com".into(), path: "/".into(), name: "SID".into(), value: "fixture-session".into(),
            secure: true, http_only: true, expires: None,
        }]);
        let job = r.core.start_stream(ID, false).unwrap();
        wait_terminal(&job);
        assert_eq!(phase(&job), "done", "{}", r.log_text());
        let path = std::fs::read_to_string(r.bin.path().join("yt-dlp.jar")).unwrap();
        assert!(!Path::new(&path).exists());
        assert!(!r.log_text().contains("fixture-session"));
    }

    #[test]
    fn failed_resolution_is_categorised_not_retried_and_redacted() {
        let r = rig(&format!("echo 'ERROR: [youtube] {ID}: Video unavailable. This video has been removed' >&2\nexit 1"), FFMPEG_OK, None);
        let job = r.core.start_stream(ID, false).unwrap();
        wait_terminal(&job);
        let s = job.state.lock().unwrap().clone();
        assert_eq!((s.phase.as_str(), s.reason.as_deref(), s.retryable), ("failed", Some("unavailable"), false));
        assert!(!s.error.unwrap().contains(ID), "the video id must not survive in the stored error");
        assert_eq!(r.ytdlp_calls(), 1, "a final error is not retried");
        let failed = r.events("stream.failed");
        assert_eq!(failed.len(), 1, "{failed:?}");
        let f = &failed[0];
        assert_eq!((f["reason"].as_str(), f["retryable"].as_bool(), f["exit_code"].as_i64(), f["attempt"].as_u64()), (Some("unavailable"), Some(false), Some(1), Some(1)));
        assert!(f["elapsed_ms"].is_number() && f["video_id"] == ID && f["origin"] == "explicit", "{f}");
        assert!(f["error"].as_str().unwrap().len() <= ERROR_TEXT_MAX && !f["error"].as_str().unwrap().contains(ID));
        assert_eq!(r.core.video.get_entry(&format!("{ID}.{}", s.variant)).unwrap().unwrap().status, STATUS_FAILED);
        let status = r.core.watch_status(ID);
        assert_eq!((status["playback"]["state"].as_str(), status["playback"]["reason"].as_str(), status["playback"]["retryable"].as_bool()), (Some("error"), Some("unavailable"), Some(false)));
    }

    #[test]
    fn selected_manifest_is_format_unavailable_and_not_retried() {
        let r = rig("cat <<'EOF'\n{\"url\":\"https://example.invalid/master.m3u8\",\"protocol\":\"https\",\"vcodec\":\"avc1\",\"acodec\":\"mp4a\",\"http_headers\":{\"User-Agent\":\"fixture-agent\"}}\nEOF", FFMPEG_OK, None);
        let job = r.core.start_stream(ID, false).unwrap();
        wait_terminal(&job);
        assert_eq!(job.state.lock().unwrap().reason.as_deref(), Some("format_unavailable"));
        assert_eq!(r.ytdlp_calls(), 1, "format rejection must not trigger resolution retry");
        assert_eq!(r.events("source.resolve_retry").len(), 0);
        assert_eq!(r.events("helper.launched").len(), 1, "only yt-dlp version probe; ffmpeg must not launch");
    }

    #[test]
    fn unrecognised_resolution_errors_are_retried_once_and_failure_text_is_redacted() {
        let r = rig(
            &format!("echo 'ERROR: unable to download {ID} from https://rr1---sn-abc.googlevideo.com/videoplayback?{p}=SECRETSIG&expire=1 (rr1.googlevideo.com) at /opt/scratch/cache/x.mp4' >&2\nexit 1", p = "si".to_string() + "g"),
            FFMPEG_OK,
            None,
        );
        let job = r.core.start_stream(ID, false).unwrap();
        wait_terminal(&job);
        assert_eq!(r.ytdlp_calls(), 2, "one re-resolve for a possibly transient unknown error");
        assert_eq!(r.events("source.resolve_retry").len(), 1);
        let log = r.log_text();
        for leaked in ["googlevideo", "SECRETSIG", "/opt/scratch", "videoplayback"] {
            assert!(!log.contains(leaked), "{leaked} leaked into the log");
        }
        let status = r.core.watch_status(ID).to_string();
        assert!(!status.contains("googlevideo") && !status.contains("SECRETSIG"), "{status}");
        assert_eq!(r.events("stream.failed")[0]["reason"], "helper_exit");
        assert_eq!(r.events("stream.failed")[0]["retryable"], true);
    }

    #[test]
    fn no_first_bytes_times_out_with_a_retryable_reason_and_removes_the_partial_file() {
        let r = rig(&ytdlp_json("http://127.0.0.1:9/v", true), FFMPEG_HANG, None);
        r.core.streams.tuning.lock().unwrap().max_attempts = 1;
        let job = r.core.start_stream(ID, false).unwrap();
        wait_terminal(&job);
        let s = job.state.lock().unwrap().clone();
        assert_eq!((s.phase.as_str(), s.reason.as_deref(), s.retryable), ("failed", Some("no_first_bytes_timeout"), true));
        assert_eq!(r.events("helper.launched").iter().map(|e| e["helper"].as_str().unwrap().to_string()).collect::<Vec<_>>(), ["yt-dlp", "ffmpeg"]);
        assert_eq!(r.events("source.resolved").len(), 1);
        assert!(r.events("first_bytes").is_empty());
        assert!(!r.core.paths.video_dir.join(format!("{}.partial.mp4", s.entry_id)).exists());
    }

    #[test]
    fn a_helper_exiting_nonzero_reports_status_and_a_redacted_stderr_tail() {
        let r = rig(&ytdlp_json("http://127.0.0.1:9/v", true), "echo 'Error opening input file /opt/scratch/secret/clip.mp4: Permission denied' >&2\nexit 3", None);
        let job = r.core.start_stream(ID, false).unwrap();
        wait_terminal(&job);
        let f = r.events("stream.failed").remove(0);
        assert_eq!((f["reason"].as_str(), f["exit_code"].as_i64(), f["after_first_bytes"].as_bool()), (Some("helper_exit"), Some(3), Some(false)), "{f}");
        assert!(f["error"].as_str().unwrap().contains("Permission denied") && !f["error"].as_str().unwrap().contains("scratch"), "{f}");
        assert!(!job.state.lock().unwrap().error.clone().unwrap().contains("scratch"));
    }

    #[test]
    fn a_helper_killed_by_a_signal_records_the_signal() {
        let r = rig(&ytdlp_json("http://127.0.0.1:9/v", true), "kill -9 $$", None);
        let job = r.core.start_stream(ID, false).unwrap();
        wait_terminal(&job);
        let f = r.events("stream.failed").remove(0);
        assert_eq!((f["reason"].as_str(), f["exit_signal"].as_i64()), (Some("helper_exit"), Some(9)), "{f}");
        assert!(f["exit_code"].is_null());
    }

    /// Serves probe requests: the first `fail` get `status`, the rest report a 1000-byte length.
    fn probe_server(fail: usize, status: u16) -> (String, Arc<AtomicUsize>) {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v", listener.local_addr().unwrap());
        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut stream = stream;
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let n = h.fetch_add(1, Ordering::SeqCst);
                let reply = if n < fail {
                    format!("HTTP/1.1 {status} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                } else {
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-0/1000\r\nContent-Length: 1\r\nConnection: close\r\n\r\nx".to_string()
                };
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        (url, hits)
    }

    #[test]
    fn a_transient_length_probe_failure_is_retried_with_backoff_then_the_stream_starts() {
        let (url, hits) = probe_server(2, 503);
        let r = rig(&ytdlp_json(&url, false), FFMPEG_OK, None);
        let job = r.core.start_stream(ID, false).unwrap();
        wait_terminal(&job);
        assert_eq!(phase(&job), "done", "{:?}", job.state.lock().unwrap().error);
        assert_eq!(r.events("source.probe_retry").len(), 2);
        assert_eq!(hits.load(Ordering::SeqCst), 4, "two failed probes, then one per stream");
        assert_eq!(r.ytdlp_calls(), 1, "no re-resolve was needed");
        assert!(r.events("stream.failed").is_empty());
        for e in ["stream.start", "stream.admitted", "source.resolved", "first_bytes", "first_segment", "stream.cached"] {
            assert_eq!(r.events(e).len(), 1, "{e}");
        }
        assert_eq!(r.core.video.get_entry(&job.state.lock().unwrap().entry_id).unwrap().unwrap().status, STATUS_COMPLETE);
    }

    #[test]
    fn a_persistent_probe_failure_re_resolves_once_then_fails_as_length_probe_failed() {
        let (url, hits) = probe_server(usize::MAX, 503);
        let r = rig(&ytdlp_json(&url, false), FFMPEG_OK, None);
        let job = r.core.start_stream(ID, false).unwrap();
        wait_terminal(&job);
        let s = job.state.lock().unwrap().clone();
        assert_eq!((s.phase.as_str(), s.reason.as_deref(), s.retryable, s.attempt), ("failed", Some("length_probe_failed"), true, 2));
        assert_eq!(r.ytdlp_calls(), 2, "fresh URLs for the second attempt");
        assert_eq!(hits.load(Ordering::SeqCst), 6, "3 probes per attempt");
        assert_eq!(r.events("stream.retrying").len(), 1);
        let log = r.log_text();
        assert!(!log.contains("127.0.0.1"), "the host must not be logged");
        assert_eq!(r.events("stream.failed").len(), 1, "retries are not failures");
    }

    #[test]
    fn a_permanent_probe_error_is_not_retried_on_the_same_url() {
        let (url, hits) = probe_server(usize::MAX, 404);
        let r = rig(&ytdlp_json(&url, false), FFMPEG_OK, None);
        r.core.streams.tuning.lock().unwrap().max_attempts = 1;
        let job = r.core.start_stream(ID, false).unwrap();
        wait_terminal(&job);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert_eq!(job.state.lock().unwrap().reason.as_deref(), Some("length_probe_failed"));
    }

    #[test]
    fn deliberate_cancellation_is_logged_as_cancelled_never_as_failed() {
        let r = rig(&ytdlp_json("http://127.0.0.1:9/v", true), FFMPEG_HANG, None);
        r.core.streams.tuning.lock().unwrap().no_first_bytes_timeout = Duration::from_secs(60);
        let job = r.core.start_stream(ID, false).unwrap();
        wait_for("caching", || phase(&job) == "caching");
        let entry = job.state.lock().unwrap().entry_id.clone();
        r.core.cancel(&entry).unwrap();
        wait_terminal(&job);
        assert_eq!(phase(&job), "canceled");
        assert!(r.events("stream.failed").is_empty());
        let c = r.events("stream.cancelled");
        assert_eq!((c.len(), c[0]["cause"].as_str()), (1, Some("user")), "{c:?}");
        assert_eq!(r.core.video.get_entry(&entry).unwrap().unwrap().status, STATUS_CANCELED);
        assert_eq!(r.core.watch_status(ID)["playback"]["reason"], "cancelled");
        // Shutdown is a cancellation too.
        let r2 = rig(&ytdlp_json("http://127.0.0.1:9/v", true), FFMPEG_HANG, None);
        r2.core.streams.tuning.lock().unwrap().no_first_bytes_timeout = Duration::from_secs(60);
        let job2 = r2.core.start_stream(ID, false).unwrap();
        wait_for("caching", || phase(&job2) == "caching");
        r2.core.shutdown();
        wait_terminal(&job2);
        assert_eq!(r2.events("stream.cancelled")[0]["cause"], "shutdown");
        assert!(r2.events("stream.failed").is_empty());
    }

    #[test]
    fn switching_the_source_stops_the_old_job_as_a_cancellation_and_starts_one_new_job() {
        let r = rig(&ytdlp_json("http://127.0.0.1:9/v", true), FFMPEG_HANG, None);
        r.core.streams.tuning.lock().unwrap().no_first_bytes_timeout = Duration::from_secs(60);
        let video = r.core.start_stream(ID, false).unwrap();
        wait_for("caching", || phase(&video) == "caching");
        let audio = r.core.start_stream(ID, true).unwrap();
        assert!(!Arc::ptr_eq(&video, &audio));
        assert_eq!(phase(&video), "canceled", "the old attempt was fully stopped before the new one started");
        assert!(matches!(phase(&audio).as_str(), "resolving" | "starting" | "caching"));
        assert_eq!(r.events("stream.cancelled")[0]["cause"], "source_switch");
        assert!(r.events("stream.failed").is_empty());
        assert_eq!(r.core.running_count(), 1);
        r.core.shutdown();
    }

    #[test]
    fn reopening_while_an_attempt_is_stopping_waits_and_gets_a_fresh_live_job() {
        let r = rig(&ytdlp_json("http://127.0.0.1:9/v", true), FFMPEG_HANG, None);
        r.core.streams.tuning.lock().unwrap().no_first_bytes_timeout = Duration::from_secs(60);
        let first = r.core.start_stream(ID, false).unwrap();
        wait_for("caching", || phase(&first) == "caching");
        let entry = first.state.lock().unwrap().entry_id.clone();
        r.core.cancel(&entry).unwrap();
        let second = r.core.start_stream(ID, false).unwrap();
        assert!(!Arc::ptr_eq(&first, &second), "a job that is being stopped must not be handed out");
        assert!(matches!(phase(&second).as_str(), "resolving" | "starting" | "caching"), "{}", phase(&second));
        assert_eq!(r.core.running_count(), 1);
        wait_for("second caching", || phase(&second) == "caching");
        assert_eq!(r.core.running_count(), 1, "the old thread must not have removed the new registration");
        assert!(r.core.is_running(&entry));
        r.core.shutdown();
    }

    #[test]
    fn concurrent_opens_and_retries_yield_exactly_one_active_job() {
        let r = rig(&ytdlp_json("http://127.0.0.1:9/v", true), FFMPEG_HANG, None);
        r.core.streams.tuning.lock().unwrap().no_first_bytes_timeout = Duration::from_secs(60);
        let jobs: Vec<Arc<StreamJob>> = (0..8)
            .map(|_| {
                let core = r.core.clone();
                std::thread::spawn(move || core.start_stream(ID, false).unwrap())
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|t| t.join().unwrap())
            .collect();
        assert!(jobs.iter().all(|j| Arc::ptr_eq(j, &jobs[0])), "every open joins the same job");
        wait_for("caching", || phase(&jobs[0]) == "caching");
        assert_eq!(r.ytdlp_calls(), 1);
        assert_eq!((r.core.running_count(), r.events("stream.start").len()), (1, 1));
        r.core.shutdown();
    }

    #[test]
    fn retrying_a_failed_stream_from_many_threads_starts_one_new_job() {
        let r = rig(&format!("[ \"$(wc -l < \"$0.calls\")\" -le 1 ] && {{ echo 'ERROR: Video unavailable' >&2; exit 1; }}\n{}", ytdlp_json("http://127.0.0.1:9/v", true)), FFMPEG_HANG, None);
        r.core.streams.tuning.lock().unwrap().no_first_bytes_timeout = Duration::from_secs(60);
        let first = r.core.start_stream(ID, false).unwrap();
        wait_terminal(&first);
        assert_eq!(phase(&first), "failed");
        let jobs: Vec<Arc<StreamJob>> = (0..6)
            .map(|_| {
                let core = r.core.clone();
                std::thread::spawn(move || core.start_stream(ID, false).unwrap())
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|t| t.join().unwrap())
            .collect();
        assert!(!Arc::ptr_eq(&jobs[0], &first));
        assert!(jobs.iter().all(|j| Arc::ptr_eq(j, &jobs[0])));
        wait_for("caching", || phase(&jobs[0]) == "caching");
        assert_eq!((r.ytdlp_calls(), r.core.running_count()), (2, 1));
        r.core.shutdown();
    }

    #[test]
    fn a_failed_attempt_keeps_other_copies_and_the_users_protection() {
        let r = rig(&format!("echo 'ERROR: Video unavailable' >&2\nexit 1"), FFMPEG_OK, None);
        // A finished copy of the same video in another variant, and a protected failed row for this one.
        let other = r.core.paths.video_dir.join(format!("{ID}.audio.mp4"));
        std::fs::write(&other, vec![1u8; 64]).unwrap();
        let mut done = EntryRow::new(ID, "audio", "audio", "manual", 1);
        done.status = STATUS_COMPLETE.into();
        done.file_path = Some(other.display().to_string());
        done.size_bytes = 64;
        r.core.video.insert_entry(&done).unwrap();
        let variant = variant_for_height(r.core.video.settings().watch_height).as_str().to_string();
        assert_ne!(variant, "audio");
        let mut failed = EntryRow::new(ID, &variant, &variant, "manual", 1);
        failed.status = STATUS_FAILED.into();
        failed.protected = true;
        r.core.video.insert_entry(&failed).unwrap();
        let job = r.core.start_stream(ID, false).unwrap();
        wait_terminal(&job);
        assert!(other.exists() && r.core.video.get_entry(&format!("{ID}.audio")).unwrap().unwrap().is_complete());
        let row = r.core.video.get_entry(&format!("{ID}.{variant}")).unwrap().unwrap();
        assert!(row.protected, "restarting must not drop the library flag");
        assert_eq!(row.status, STATUS_FAILED);
    }

    #[test]
    fn a_finished_copy_is_never_replaced_by_a_new_stream() {
        let r = rig("exit 1", FFMPEG_OK, None);
        let variant = variant_for_height(r.core.video.settings().watch_height).as_str().to_string();
        let path = r.core.paths.video_dir.join(format!("{ID}.{variant}.mp4"));
        std::fs::write(&path, b"whole").unwrap();
        let mut done = EntryRow::new(ID, &variant, &variant, "manual", 1);
        done.status = STATUS_COMPLETE.into();
        done.file_path = Some(path.display().to_string());
        r.core.video.insert_entry(&done).unwrap();
        assert!(r.core.start_stream(ID, false).is_err());
        assert!(path.exists() && r.core.video.get_entry(&done.id).unwrap().unwrap().is_complete());
    }

    /// A background download that runs until it is canceled.
    struct Endless(AtomicUsize);
    impl Runner for Endless {
        fn run(&self, _: &JobSpec, _: &mut dyn FnMut(Event), cancel: &AtomicBool) -> std::result::Result<(), String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            while !cancel.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err("canceled".into())
        }
    }

    #[test]
    fn explicit_playback_preempts_speculative_prefetch_when_slots_are_saturated() {
        let runner = Arc::new(Endless(AtomicUsize::new(0)));
        let r = rig(&ytdlp_json("http://127.0.0.1:9/v", true), FFMPEG_HANG, Some(runner.clone()));
        r.core.streams.tuning.lock().unwrap().no_first_bytes_timeout = Duration::from_secs(60);
        let mut s = r.core.video.settings();
        s.download_concurrency = 2;
        s.auto_prefetch = true;
        r.core.video.put_settings(&s).unwrap();
        for (i, (video, origin)) in [("bbbbbbbbbbb", "prefetch"), ("ccccccccccc", "manual")].into_iter().enumerate() {
            let mut e = EntryRow::new(video, "360", "360", origin, 10 + i as i64);
            e.status = STATUS_QUEUED.into();
            r.core.video.insert_entry(&e).unwrap();
        }
        r.core.start_queued();
        wait_for("both downloads running", || runner.0.load(Ordering::SeqCst) == 2);
        assert_eq!(r.core.running_count(), 2, "saturated");
        let job = r.core.start_stream(ID, false).unwrap();
        wait_for("caching", || phase(&job) == "caching");
        // The speculative download went back to the queue; the user's manual one kept running.
        wait_for("prefetch requeued", || r.core.video.get_entry("bbbbbbbbbbb.360").unwrap().unwrap().status == STATUS_QUEUED);
        assert_eq!(r.core.video.get_entry("ccccccccccc.360").unwrap().unwrap().status, STATUS_DOWNLOADING);
        assert_eq!(r.events("stream.start")[0]["preempted_background"], 1);
        assert_eq!(r.events("download.preempted").len(), 1);
        assert!(r.core.is_running(&format!("{ID}.{}", job.state.lock().unwrap().variant)));
        // The preempted download must not sneak back in while the slots are still full.
        r.core.start_queued();
        assert_eq!(r.core.video.get_entry("bbbbbbbbbbb.360").unwrap().unwrap().status, STATUS_QUEUED);
        r.core.shutdown();
    }

    #[test]
    fn failed_disk_admission_keeps_the_previous_entry_and_does_not_preempt_downloads() {
        let runner = Arc::new(Endless(AtomicUsize::new(0)));
        let r = rig(&ytdlp_json("http://127.0.0.1:9/v", true), FFMPEG_HANG, Some(runner.clone()));
        let mut s = r.core.video.settings();
        s.download_concurrency = 1;
        s.auto_prefetch = true;
        r.core.video.put_settings(&s).unwrap();
        let mut e = EntryRow::new("bbbbbbbbbbb", "360", "360", "prefetch", 10);
        e.status = STATUS_QUEUED.into();
        r.core.video.insert_entry(&e).unwrap();
        r.core.start_queued();
        wait_for("download running", || runner.0.load(Ordering::SeqCst) == 1);
        let variant = variant_for_height(s.watch_height).as_str().to_string();
        let mut failed = EntryRow::new(ID, &variant, &variant, "manual", 1);
        failed.status = STATUS_FAILED.into();
        failed.protected = true;
        r.core.video.insert_entry(&failed).unwrap();
        s.min_free_bytes = 1 << 60;
        r.core.video.put_settings(&s).unwrap();
        let job = r.core.start_stream(ID, false).unwrap();
        wait_terminal(&job);
        assert_eq!(job.state.lock().unwrap().reason.as_deref(), Some("disk_space"));
        assert!(r.events("download.preempted").is_empty(), "a refused start must not stop other downloads");
        assert_eq!(r.core.video.get_entry("bbbbbbbbbbb.360").unwrap().unwrap().status, STATUS_DOWNLOADING);
        let row = r.core.video.get_entry(&format!("{ID}.{variant}")).unwrap().expect("the previous entry survives a refused start");
        assert!(row.protected, "and keeps the library flag");
        r.core.shutdown();
    }

    #[test]
    fn a_cancel_cause_is_only_recorded_for_a_running_entry() {
        let r = rig("exit 1", FFMPEG_OK, None);
        r.core.cancel_with_cause("nothing-running.360", "preempted");
        assert!(r.core.streams.cancel_causes.lock().unwrap().is_empty());
    }

    #[test]
    fn ffmpeg_failures_are_classified_from_the_whole_tail() {
        let tail: Vec<String> = ["HTTP error 503 Service Unavailable", "Error in the pull function.", "", "Conversion failed!"].iter().map(|s| s.to_string()).collect();
        let f = failure_from_ffmpeg_tail(&tail);
        assert_eq!(f.reason, FailureReason::TransportRetryable);
        assert_eq!(f.message, "Conversion failed!");
        assert_eq!(failure_from_ffmpeg_tail(&[]).reason, FailureReason::HelperExit);
    }

    #[test]
    fn opening_without_ready_tools_reports_a_retryable_reason_and_logs_it() {
        let (_d, core) = crate::engine::testutil::test_core(None, None);
        let status = core.watch_open(ID, false).unwrap();
        assert_eq!((status["playback"]["state"].as_str(), status["playback"]["reason"].as_str(), status["playback"]["retryable"].as_bool()), (Some("error"), Some("tools_pending"), Some(true)));
        let events = core.logs(100)["events"].as_array().unwrap().clone();
        let failed = events.iter().find(|e| e["event"] == "stream.failed").unwrap();
        assert_eq!((failed["data"]["reason"].as_str(), failed["data"]["phase"].as_str()), (Some("tools_pending"), Some("admission")));
    }
}
