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

use crate::engine::{Core, RunningJob};
use crate::error::{Error, Result};
use crate::quality::Quality;
use crate::rules::retention_for_request;
use crate::util::valid_video_id;
use crate::videodb::{entry_id, EntryRow, STATUS_CANCELED, STATUS_DOWNLOADING, STATUS_FAILED};
use crate::ytdlp::search_path;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const CHUNK: u64 = 8 * 1024 * 1024;
/// Play-while-caching pipelines that may run at once (each is a yt-dlp + an ffmpeg process).
const MAX_STREAMS: usize = 4;

/// One upstream stream the feeder can serve ranges of.
#[derive(Clone)]
pub struct Source {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub len: u64,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct StreamState {
    /// "resolving", "starting", "caching", "done", "failed" or "canceled".
    pub phase: String,
    pub error: Option<String>,
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

#[derive(Default)]
pub struct Streams {
    /// Why starting a stream failed (no yt-dlp yet, too many running, ...), per video, for the watch view.
    pub errors: Mutex<HashMap<String, String>>,
    pub jobs: Mutex<HashMap<String, Arc<StreamJob>>>,
    pub sources: Mutex<HashMap<String, Source>>,
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
        return "ba[ext=m4a]/ba".into();
    }
    format!(
        "bv*[height<={height}][vcodec^=avc1]+ba[ext=m4a]/b[height<={height}][vcodec^=avc1][acodec^=mp4a]/bv*[height<={height}]+ba/b[height<={height}]"
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
    pub has_video: bool,
    pub has_audio: bool,
}

pub fn parse_resolved(json: &Value) -> Result<Resolved> {
    let one = |f: &Value| -> Option<ResolvedStream> {
        let url = f["url"].as_str()?.to_string();
        let headers = f["http_headers"].as_object().map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect()).unwrap_or_default();
        let len = f["filesize"].as_u64().or_else(|| clen_from_url(&url));
        let has_video = f["vcodec"].as_str().is_some_and(|v| v != "none");
        let has_audio = f["acodec"].as_str().is_some_and(|v| v != "none");
        Some(ResolvedStream { url, headers, len, has_video, has_audio })
    };
    let formats: Vec<&Value> = match json["requested_formats"].as_array() {
        Some(a) if !a.is_empty() => a.iter().collect(),
        _ => vec![json],
    };
    let streams: Vec<ResolvedStream> = formats.into_iter().filter_map(one).collect();
    if streams.is_empty() {
        return Err(Error::other("yt-dlp returned no stream URLs"));
    }
    Ok(Resolved { is_live: json["is_live"].as_bool().unwrap_or(false), duration: json["duration"].as_f64(), height: json["height"].as_u64().map(|h| h as u32), streams })
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

/// Learns the exact length with a one-byte range request.
pub fn probe_len(url: &str, headers: &[(String, String)]) -> Result<u64> {
    let resp = with_headers(upstream_agent().get(url), headers).set("Range", "bytes=0-0").call().map_err(|e| Error::other(format!("probing stream length: {e}")))?;
    resp.header("content-range")
        .and_then(|v| v.rsplit('/').next())
        .and_then(|t| t.trim().parse().ok())
        .ok_or_else(|| Error::other("the stream did not report its length"))
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

    /// Starts (or joins) the play-while-caching pipeline for a video.
    pub fn start_stream(self: &Arc<Self>, video_id: &str, audio_only: bool) -> Result<Arc<StreamJob>> {
        self.require_online()?;
        if !valid_video_id(video_id) {
            return Err(Error::bad("invalid video_id"));
        }
        {
            let jobs = self.streams.jobs.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(j) = jobs.get(video_id) {
                let phase = j.state.lock().unwrap_or_else(|e| e.into_inner()).phase.clone();
                if !matches!(phase.as_str(), "failed" | "canceled") {
                    return Ok(j.clone());
                }
            }
        }
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
        let hls_dir = hls_dir_for(&self.paths.cache_dir, video_id, &variant);
        let _ = std::fs::remove_dir_all(&hls_dir);
        std::fs::create_dir_all(&hls_dir)?;
        let job = Arc::new(StreamJob {
            video_id: video_id.to_string(),
            hls_dir: hls_dir.clone(),
            state: Mutex::new(StreamState {
                phase: "resolving".into(),
                started_ms: self.now(),
                variant: variant.clone(),
                entry_id: id.clone(),
                audio_only,
                ..StreamState::default()
            }),
        });
        self.streams.jobs.lock().unwrap_or_else(|e| e.into_inner()).insert(video_id.to_string(), job.clone());
        // Make the entry visible right away (status downloading) so the UI, the disk caps and the rules see it.
        let facts = self.facts_for(video_id);
        let decision = retention_for_request(&self.video.rules(), &facts);
        let mut row = EntryRow::new(video_id, &variant, &variant, "play", self.now());
        row.title = facts.title.clone();
        row.channel = facts.channel.clone();
        row.duration_seconds = facts.duration_seconds;
        row.retention_days = Some(decision.retention_days);
        row.rule = decision.rule.clone();
        row.expires_at = self.retention_expiry(self.now(), decision.retention_days);
        row.status = STATUS_DOWNLOADING.into();
        if let Some(old) = self.video.get_entry(&id)? {
            // A failed or canceled earlier try: start over on the same row.
            self.video.delete_entry(&old.id)?;
        }
        let estimate = crate::jobs::estimate_bytes(&variant, facts.duration_seconds, None);
        if let Ok(short) = self.enforce_caps(estimate) {
            if short > 0 {
                self.fail_stream(&job, &format!("not enough disk: {} MB short under the cache cap or minimum free space", short / 1_000_000 + 1), None);
                return Ok(job);
            }
        }
        self.video.insert_entry(&row)?;
        let running = Arc::new(RunningJob { cancel: false.into(), started_at: self.now() });
        self.running.lock().unwrap_or_else(|e| e.into_inner()).insert(id.clone(), running.clone());
        if self.is_offline() {
            self.offline_paused.lock().unwrap_or_else(|e|e.into_inner()).insert(id.clone());
            running.cancel.store(true, Ordering::SeqCst);
        }
        let core = self.clone();
        let job2 = job.clone();
        let spawned = std::thread::Builder::new().name(format!("muninn-stream-{video_id}")).spawn(move || {
            core.run_stream(&job2, &running, &ffmpeg, settings.watch_height, audio_only);
            core.running.lock().unwrap_or_else(|e| e.into_inner()).remove(&job2.state.lock().unwrap_or_else(|e| e.into_inner()).entry_id);
            core.wake_scheduler();
        });
        if spawned.is_err() {
            self.running.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
            self.fail_stream(&job, "could not start the stream thread", Some(&id));
        }
        Ok(job)
    }

    fn set_phase(&self, job: &StreamJob, phase: &str) {
        job.state.lock().unwrap_or_else(|e| e.into_inner()).phase = phase.into();
    }

    fn fail_stream(&self, job: &StreamJob, message: &str, entry: Option<&str>) {
        self.log_event("error", "stream.failed", serde_json::json!({"video_id":job.video_id,"entry_id":entry}));
        {
            let mut s = job.state.lock().unwrap_or_else(|e| e.into_inner());
            s.phase = "failed".into();
            s.error = Some(message.chars().take(400).collect());
        }
        let id = entry.map(str::to_string).unwrap_or_else(|| job.state.lock().unwrap_or_else(|e| e.into_inner()).entry_id.clone());
        let _ = self.video.set_status(&id, STATUS_FAILED, Some(&message.chars().take(400).collect::<String>()), self.now());
        let _ = self.video.set_progress(&id, 0, None, 0.0, None);
    }

    fn resolve_streams(&self, video_id: &str, selector: &str, cancel: &std::sync::atomic::AtomicBool) -> Result<Resolved> {
        self.require_online()?;
        let tools = self.tools();
        let bin = tools.ytdlp.path.clone().ok_or_else(|| Error::other("yt-dlp is not installed"))?;
        let mut args: Vec<String> =
            ["--ignore-config", "--no-playlist", "--no-warnings", "-f", selector, "-J"].iter().map(|s| s.to_string()).collect();
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
            .map_err(|e| Error::other(format!("could not start yt-dlp: {e}")))?;
        let mut out = child.stdout.take().ok_or_else(|| Error::other("no stdout"))?;
        let mut err = child.stderr.take().ok_or_else(|| Error::other("no stderr"))?;
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
            if let Some(st) = child.try_wait()? {
                break st;
            }
            if cancel.load(Ordering::SeqCst) || started.elapsed() > Duration::from_secs(120) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::other(if cancel.load(Ordering::SeqCst) { "canceled" } else { "yt-dlp took too long to resolve the video" }));
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        let stdout = t_out.join().unwrap_or_default();
        let stderr = t_err.join().unwrap_or_default();
        if !status.success() {
            let line = stderr.lines().rev().find(|l| l.contains("ERROR")).or_else(|| stderr.lines().last()).unwrap_or("yt-dlp failed").trim().to_string();
            return Err(Error::other(line));
        }
        let json: Value = serde_json::from_slice(&stdout)?;
        parse_resolved(&json)
    }

    fn run_stream(self: &Arc<Self>, job: &Arc<StreamJob>, running: &Arc<RunningJob>, ffmpeg: &str, watch_height: u32, audio_only: bool) {
        let (entry, variant) = {
            let s = job.state.lock().unwrap_or_else(|e| e.into_inner());
            (s.entry_id.clone(), s.variant.clone())
        };
        let selector = selector_for(watch_height, audio_only);
        // One retry: YouTube answers a transient error now and then (and the first call warms the JS runtime).
        let first = self.resolve_streams(&job.video_id, &selector, &running.cancel);
        let attempt = match first {
            Err(ref e) if !running.cancel.load(Ordering::SeqCst) && !e.to_string().contains("canceled") => {
                std::thread::sleep(Duration::from_millis(800));
                self.resolve_streams(&job.video_id, &selector, &running.cancel)
            }
            other => other,
        };
        let resolved = match attempt {
            Ok(r) => r,
            Err(e) => {
                if running.cancel.load(Ordering::SeqCst) {
                    self.finish_canceled(job, &entry);
                } else {
                    self.fail_stream(job, &e.to_string(), Some(&entry));
                }
                return;
            }
        };
        if resolved.is_live {
            self.fail_stream(job, "This is a live stream; Muninn's player plays recorded videos (open it on youtube.com instead)", Some(&entry));
            return;
        }
        {
            let mut s = job.state.lock().unwrap_or_else(|e| e.into_inner());
            s.phase = "starting".into();
            s.duration_seconds = resolved.duration;
            s.height = resolved.height;
        }
        // Register the feeder sources.
        let mut keys = Vec::new();
        let mut inputs = Vec::new();
        for (i, st) in resolved.streams.iter().enumerate() {
            let len = match st.len {
                Some(l) => l,
                None => match probe_len(&st.url, &st.headers) {
                    Ok(l) => l,
                    Err(e) => {
                        self.fail_stream(job, &e.to_string(), Some(&entry));
                        return;
                    }
                },
            };
            let key = format!("{}-{}/{}", job.video_id, variant, i);
            self.streams.sources.lock().unwrap_or_else(|e| e.into_inner()).insert(key.clone(), Source { url: st.url.clone(), headers: st.headers.clone(), len });
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
                self.fail_stream(job, &format!("could not start ffmpeg: {e}"), Some(&entry));
                self.drop_sources(&keys);
                return;
            }
        };
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let duration = resolved.duration;
        let video = self.clone();
        let job_p = job.clone();
        let entry_p = entry.clone();
        let progress_thread = std::thread::spawn(move || {
            let Some(stdout) = stdout else { return };
            let mut last = Instant::now() - Duration::from_secs(1);
            for line in BufReader::new(stdout).lines().map_while(|l| l.ok()) {
                if let Some(us) = line.strip_prefix("out_time_us=").and_then(|v| v.trim().parse::<i64>().ok()) {
                    if let (true, Some(d)) = (last.elapsed() > Duration::from_millis(500), duration.filter(|d| *d > 0.0)) {
                        last = Instant::now();
                        let frac = (us as f64 / 1e6 / d).clamp(0.0, 0.999);
                        job_p.state.lock().unwrap_or_else(|e| e.into_inner()).progress = frac;
                        let _ = video.video.set_progress(&entry_p, 0, None, frac, None);
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
        let mut canceled = false;
        let status = loop {
            match child.try_wait() {
                Ok(Some(st)) => break Some(st),
                Ok(None) => {}
                Err(_) => break None,
            }
            {
                let mut s = job.state.lock().unwrap_or_else(|e| e.into_inner());
                if s.first_segment_ms.is_none() && playlist_progress(&job.hls_dir).0 >= 2 {
                    s.first_segment_ms = Some(self.now());
                }
            }
            if running.cancel.load(Ordering::SeqCst) || self.shutdown.load(Ordering::SeqCst) {
                canceled = true;
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(150));
        };
        let _ = progress_thread.join();
        let tail = err_thread.join().unwrap_or_default();
        self.drop_sources(&keys);
        if canceled {
            let _ = std::fs::remove_file(&partial);
            self.finish_canceled(job, &entry);
            return;
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
                    self.fail_stream(job, "could not move the finished file into place", Some(&entry));
                    return;
                }
                let size = std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
                let height = resolved.height;
                let _ = self.video.set_file(&entry, &target.display().to_string(), "mp4", Some(size), height, true);
                let _ = self.video.complete_entry(&entry, size, height);
                self.log_event("info", "stream.cached", serde_json::json!({"video_id":job.video_id,"entry_id":entry,"bytes":size}));
                let mut s = job.state.lock().unwrap_or_else(|e| e.into_inner());
                s.phase = "done".into();
                s.progress = 1.0;
            }
            _ => {
                let _ = std::fs::remove_file(&partial);
                let message = tail.iter().rev().find(|l| !l.trim().is_empty()).cloned().unwrap_or_else(|| "ffmpeg failed".into());
                self.fail_stream(job, &message, Some(&entry));
            }
        }
    }

    fn finish_canceled(&self, job: &StreamJob, entry: &str) {
        self.set_phase(job, "canceled");
        let _ = self.video.set_status(entry, STATUS_CANCELED, None, self.now());
        let _ = self.video.set_progress(entry, 0, None, 0.0, None);
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
        assert!(s.starts_with("bv*[height<=480][vcodec^=avc1]+ba[ext=m4a]"), "{s}");
        assert_eq!(selector_for(0, true), "ba[ext=m4a]/ba");
    }

    #[test]
    fn resolved_formats_carry_urls_headers_and_exact_lengths() {
        let j = json!({"duration": 213.0, "height": 480, "requested_formats": [
            {"url": "https://rr1.googlevideo.com/videoplayback?clen=1000&itag=135", "http_headers": {"User-Agent": "UA", "Accept": "*/*"}, "vcodec": "avc1.4d401f", "acodec": "none"},
            {"url": "https://rr1.googlevideo.com/videoplayback?itag=140", "filesize": 55, "http_headers": {"User-Agent": "UA"}, "vcodec": "none", "acodec": "mp4a.40.2"}]});
        let r = parse_resolved(&j).unwrap();
        assert_eq!(r.streams.len(), 2);
        assert_eq!((r.streams[0].len, r.streams[0].has_video, r.streams[0].has_audio), (Some(1000), true, false));
        assert_eq!((r.streams[1].len, r.streams[1].has_audio), (Some(55), true));
        assert!(r.streams[0].headers.contains(&("User-Agent".to_string(), "UA".to_string())));
        assert_eq!(r.duration, Some(213.0));
        assert!(!r.is_live);
        assert!(parse_resolved(&json!({"is_live": true, "url": "https://x/y?clen=1"})).unwrap().is_live);
        assert!(parse_resolved(&json!({"title": "x"})).is_err());
        // A single combined format (no requested_formats) is one stream.
        let single = parse_resolved(&json!({"url": "https://x/y?clen=9", "vcodec": "avc1", "acodec": "mp4a"})).unwrap();
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
        let source = Source { url: format!("http://{addr}/v"), headers: vec![("X-Test".into(), "1".into())], len: data.len() as u64 };
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
}
