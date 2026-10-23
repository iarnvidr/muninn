//! Local compatibility playback. Originals stay untouched; ffmpeg writes a
//! temporary HLS stream (fragmented MP4), with no YouTube requests or downloads.
//!
//! # Timeline contract
//!
//! The HLS output is **window relative**: media time 0 of the playlist is the
//! original (source) time `offset_seconds`. A window is prepared by seeking the
//! input (`-ss` before `-i`, frame accurate) to `offset_seconds` and encoding to
//! the end of the source, so a resume at 353 s encodes from about 348 s, never
//! from 0 s. Every status therefore carries:
//!
//! * `offset_seconds`: original time of playlist time 0 (the window start);
//! * `prepared_seconds`: window-relative length of the segments written so far;
//! * `prepared_until`: `offset_seconds + prepared_seconds`, original time;
//! * `duration_seconds`: duration of the whole *original* (entry metadata or
//!   `ffmpeg -i`), never the window length;
//! * `target_seconds`: the position the caller asked for; `ready` turns true
//!   once `prepared_until` is a little past it (or the window is complete);
//! * `job_id` / `generation`: identity of the ffmpeg job and of the latest
//!   viewing session that asked for it, so a stale stop cannot end a newer one.
//!
//! Original time = `offset_seconds + video.currentTime`. A seek outside
//! `[offset_seconds, prepared_until]` is answered by preparing a new window near
//! the new target (one ffmpeg job at a time, the previous one is cancelled).
//!
//! Sources whose streams are already H.264 (8-bit 4:2:0) with AAC or no audio are
//! remuxed with `-c copy` (cheap, I/O bound) from time 0; everything else is
//! encoded (VideoToolbox on macOS, x264 on development hosts).
use crate::{Core, Error, Result};
use serde_json::{json, Value};
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

/// Target HLS segment length, and the keyframe interval of encoded output.
pub const SEGMENT_SECONDS: f64 = 3.0;
/// How far before the requested position an encoded window starts.
const START_LEAD_SECONDS: f64 = 3.0;
/// `ready` once the prepared window reaches this far past the target.
const READY_MARGIN_SECONDS: f64 = 1.0;
/// A running encode is reused for a target this far beyond what it has written.
const REUSE_AHEAD_SECONDS: f64 = 12.0;
/// Finished windows kept for reopening (including the current one).
const MAX_RETAINED_OUTPUTS: usize = 2;
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Re-encode to H.264/AAC from a window start near the target.
    Encode,
    /// `-c copy` into fMP4 HLS (source is already H.264 + AAC).
    Remux,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Encode => "encode",
            Mode::Remux => "remux",
        }
    }
}

/// What `ffmpeg -i` reports about a source.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MediaProbe {
    pub duration: Option<f64>,
    pub video: Option<String>,
    /// First video stream is 8-bit 4:2:0.
    pub video_420_8bit: bool,
    pub audio: Option<String>,
}

impl MediaProbe {
    pub fn mode(&self) -> Mode {
        let video_ok = self.video.as_deref() == Some("h264") && self.video_420_8bit;
        let audio_ok = self.audio.as_deref().map_or(true, |a| a == "aac");
        if video_ok && audio_ok {
            Mode::Remux
        } else {
            Mode::Encode
        }
    }
}

pub struct LocalPlayback {
    id: u64,
    generation: AtomicU64,
    entry_id: String,
    dir_name: String,
    dir: PathBuf,
    offset: f64,
    mode: Mode,
    duration: Option<f64>,
    probe: MediaProbe,
    target: Mutex<f64>,
    cancel: AtomicBool,
    done: AtomicBool,
    error: Mutex<Option<String>>,
}

/// All prepared windows and running jobs, oldest first.
#[derive(Default)]
pub struct LocalPlaybacks {
    jobs: Vec<Arc<LocalPlayback>>,
    next_job: u64,
    next_generation: u64,
}

/// Window progress derived from an HLS playlist.
#[derive(Debug, PartialEq)]
pub struct WindowState {
    pub segments: usize,
    pub prepared_seconds: f64,
    pub finished: bool,
    pub ready: bool,
}

pub fn window_state(playlist: &str, offset: f64, target: f64) -> WindowState {
    let segments = playlist.lines().filter(|line| line.ends_with(".m4s")).count();
    let finished = playlist.contains("#EXT-X-ENDLIST");
    let prepared_seconds = playlist
        .lines()
        .filter_map(|line| line.strip_prefix("#EXTINF:")?.split(',').next()?.parse::<f64>().ok())
        .sum::<f64>();
    let ready = segments > 0 && (finished || offset + prepared_seconds >= target + READY_MARGIN_SECONDS);
    WindowState { segments, prepared_seconds, finished, ready }
}

/// Window start for a target: a segment boundary a little before it. Remux
/// always starts at 0 because stream copy can only cut on source keyframes,
/// which would make the window start inexact.
pub fn window_start(target: f64, mode: Mode) -> f64 {
    if mode == Mode::Remux || !(target > START_LEAD_SECONDS) {
        return 0.0;
    }
    ((target - START_LEAD_SECONDS) / SEGMENT_SECONDS).floor() * SEGMENT_SECONDS
}

/// Same rule as the UI: resume only past 5 s and not within 10 s of the end.
pub fn resume_position(position: f64, duration: Option<f64>) -> f64 {
    if !(position > 5.0) {
        return 0.0;
    }
    if duration.is_some_and(|d| d > 0.0 && position >= d - 10.0) {
        return 0.0;
    }
    position
}

impl LocalPlayback {
    fn alive(&self) -> bool {
        !self.cancel.load(Ordering::SeqCst) && self.error.lock().unwrap().is_none()
    }
    /// Can this window serve `target` without starting a new job?
    fn covers(&self, target: f64) -> bool {
        if !self.alive() || target + 0.001 < self.offset {
            return false;
        }
        let Ok(playlist) = std::fs::read_to_string(self.dir.join("index.m3u8")) else {
            // Nothing written yet: a running job is reusable if the target is near its start.
            return !self.done.load(Ordering::SeqCst)
                && (self.mode == Mode::Remux || target <= self.offset + REUSE_AHEAD_SECONDS);
        };
        let state = window_state(&playlist, self.offset, target);
        if state.finished {
            return true;
        }
        if self.done.load(Ordering::SeqCst) {
            return false;
        }
        self.mode == Mode::Remux || target <= self.offset + state.prepared_seconds + REUSE_AHEAD_SECONDS
    }
}

impl Core {
    /// Prepares local playback from the saved resume position of the video.
    pub fn prepare_local_playback(self: &Arc<Self>, id: &str) -> Result<Value> {
        self.prepare_local_playback_at(id, None)
    }

    /// Prepares (or reuses) a window near `position` (original time, seconds).
    /// `None` means the saved resume position. Returns the status.
    pub fn prepare_local_playback_at(self: &Arc<Self>, id: &str, position: Option<f64>) -> Result<Value> {
        let entry = self.resolve_media_entry(id)?;
        if !entry.is_complete() {
            return Err(Error::bad("finish importing this copy first"));
        }
        let source = entry.file_path.clone().ok_or_else(|| Error::bad("copy has no local file"))?;
        let ffmpeg = self
            .tools()
            .ffmpeg
            .path
            .ok_or_else(|| Error::other("ffmpeg is not ready for local compatibility playback"))?;
        // One preparation at a time: probing and spawning must not interleave.
        let _gate = self.local_playback_gate.lock().unwrap_or_else(|e| e.into_inner());
        let known_duration = entry.duration_seconds.map(f64::from).filter(|d| *d > 0.0);
        let requested = match position {
            Some(p) => p,
            None => resume_position(
                self.content.get_progress(&entry.video_id).ok().flatten().map_or(0.0, |p| p.position_seconds),
                known_duration,
            ),
        };
        let mut target = if requested.is_finite() && requested > 0.0 { requested } else { 0.0 };
        if let Some(d) = known_duration {
            target = target.min((d - 1.0).max(0.0));
        }

        let mut set = self.local_playbacks.lock().unwrap_or_else(|e| e.into_inner());
        set.jobs.retain(|job| !(job.done.load(Ordering::SeqCst) && !job.alive()));
        let reusable = set
            .jobs
            .iter()
            .filter(|job| job.entry_id == id && job.covers(target))
            .max_by_key(|job| job.generation.load(Ordering::SeqCst))
            .cloned();
        if let Some(job) = reusable {
            self.adopt(&mut set, &job, target);
            self.evict_outputs(&mut set, &job);
            return Ok(self.job_status(&job));
        }
        let probe = set.jobs.iter().find(|job| job.entry_id == id).map(|job| job.probe.clone());
        drop(set);
        let probe = probe.unwrap_or_else(|| probe_media(&ffmpeg, &source));
        let mode = probe.mode();
        if mode == Mode::Encode {
            ensure_encoder(&ffmpeg, PLATFORM_ENCODER)?;
        }
        let duration = probe.duration.or(known_duration);
        if let Some(d) = duration {
            target = target.min((d - 1.0).max(0.0));
        }
        let offset = window_start(target, mode);

        // Only one ffmpeg at a time: cancel whatever runs and wait until it is gone.
        let stopping: Vec<_> = {
            let set = self.local_playbacks.lock().unwrap_or_else(|e| e.into_inner());
            set.jobs.iter().filter(|job| !job.done.load(Ordering::SeqCst)).cloned().collect()
        };
        for job in &stopping {
            job.cancel.store(true, Ordering::SeqCst);
        }
        for job in &stopping {
            Self::wait_local_playback(job)?;
        }

        let mut set = self.local_playbacks.lock().unwrap_or_else(|e| e.into_inner());
        set.next_job += 1;
        set.next_generation += 1;
        let job_id = set.next_job;
        let dir_name = format!("{id}.local.{job_id}");
        let dir = self.paths.cache_dir.join("hls").join(&dir_name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        let job = Arc::new(LocalPlayback {
            id: job_id,
            generation: AtomicU64::new(set.next_generation),
            entry_id: id.to_string(),
            dir_name,
            dir,
            offset,
            mode,
            duration,
            probe,
            target: Mutex::new(target),
            cancel: AtomicBool::new(false),
            done: AtomicBool::new(false),
            error: Mutex::new(None),
        });
        set.jobs.push(job.clone());
        self.evict_outputs(&mut set, &job);
        drop(set);
        let core = self.clone();
        let entry_id = id.to_string();
        let thread_job = job.clone();
        let spawned = std::thread::Builder::new().name("muninn-local-playback".into()).spawn(move || {
            let result = core.run_local_playback(&ffmpeg, &source, &thread_job);
            if let Err(error) = &result {
                *thread_job.error.lock().unwrap() = Some(error.to_string());
            }
            // Partial output of a failed or cancelled job is useless; free it before
            // announcing that the worker is gone. A job that finished despite a cancel
            // request (it was already writing the end of the playlist) is a complete
            // window: keep it for reopening.
            if result.is_ok() {
                thread_job.cancel.store(false, Ordering::SeqCst);
            } else {
                let _ = std::fs::remove_dir_all(&thread_job.dir);
            }
            thread_job.done.store(true, Ordering::SeqCst);
            if result.is_ok() {
                let mut set = core.local_playbacks.lock().unwrap_or_else(|e| e.into_inner());
                core.evict_outputs(&mut set, &thread_job);
            }
            core.log_event(
                if result.is_ok() { "info" } else { "error" },
                "playback.local",
                json!({"entry_id":entry_id,"job_id":thread_job.id,"mode":thread_job.mode.name(),"offset_seconds":thread_job.offset,
                    "success":result.is_ok(),"error":result.err().map(|e|e.to_string())}),
            );
        });
        if let Err(error) = spawned {
            *job.error.lock().unwrap() = Some(error.to_string());
            job.done.store(true, Ordering::SeqCst);
            return Err(error.into());
        }
        self.log_event(
            "info",
            "playback.local.prepare",
            json!({"entry_id":id,"job_id":job.id,"mode":mode.name(),"offset_seconds":offset,"target_seconds":target}),
        );
        Ok(self.job_status(&job))
    }

    /// Makes `job` the current session for its entry: bumps its generation, records
    /// the new target and cancels every other running job (one ffmpeg at a time).
    fn adopt(&self, set: &mut LocalPlaybacks, job: &Arc<LocalPlayback>, target: f64) {
        set.next_generation += 1;
        job.generation.store(set.next_generation, Ordering::SeqCst);
        *job.target.lock().unwrap() = target;
        for other in &set.jobs {
            if !Arc::ptr_eq(other, job) && !other.done.load(Ordering::SeqCst) {
                other.cancel.store(true, Ordering::SeqCst);
            }
        }
    }

    /// Keeps finished windows bounded: at most `MAX_RETAINED_OUTPUTS` of them and
    /// no more than a quarter of the cache budget, newest first. Never `keep`.
    fn evict_outputs(&self, set: &mut LocalPlaybacks, keep: &Arc<LocalPlayback>) {
        let budget = self.video.settings().max_disk_bytes / 4;
        let mut finished: Vec<_> = set
            .jobs
            .iter()
            .filter(|job| job.done.load(Ordering::SeqCst) && job.alive())
            .cloned()
            .collect();
        finished.sort_by_key(|job| std::cmp::Reverse(job.generation.load(Ordering::SeqCst)));
        // A running `keep` will become a retained window, so it already holds a slot.
        let (mut count, mut bytes) = (usize::from(!keep.done.load(Ordering::SeqCst)), 0u64);
        let mut evicted = Vec::new();
        for job in finished {
            let size = dir_size(&job.dir);
            if Arc::ptr_eq(&job, keep) || (count < MAX_RETAINED_OUTPUTS && bytes + size <= budget) {
                count += 1;
                bytes += size;
            } else {
                let _ = std::fs::remove_dir_all(&job.dir);
                evicted.push(job.id);
            }
        }
        set.jobs.retain(|job| !evicted.contains(&job.id));
    }

    /// Status of the current window of an entry; `None` when nothing is prepared
    /// or the latest session was stopped.
    pub fn local_playback_status(&self, id: &str) -> Option<Value> {
        let job = {
            let set = self.local_playbacks.lock().unwrap_or_else(|e| e.into_inner());
            // Cancellation belongs to the previous viewing session. Never offer its
            // unfinished playlist (or its eventual "stopped" error) to a later one.
            set.jobs
                .iter()
                .filter(|job| job.entry_id == id && !job.cancel.load(Ordering::SeqCst))
                .max_by_key(|job| job.generation.load(Ordering::SeqCst))
                .cloned()?
        };
        Some(self.job_status(&job))
    }

    fn job_status(&self, job: &LocalPlayback) -> Value {
        let error = job.error.lock().unwrap().clone();
        let target = *job.target.lock().unwrap();
        let playlist = std::fs::read_to_string(job.dir.join("index.m3u8")).unwrap_or_default();
        let state = window_state(&playlist, job.offset, target);
        let prepared_until = job.offset + state.prepared_seconds;
        json!({"state":if error.is_some(){"error"}else if state.finished{"cached"}else if state.segments>0{"streaming"}else{"resolving"},
            "kind":"hls","local":true,"entry_id":job.entry_id,"ready":error.is_none()&&state.ready,
            "progress":if state.finished{1.0}else{0.0},"prepared_seconds":state.prepared_seconds,"error":error,
            "mode":job.mode.name(),"job_id":job.id,"generation":job.generation.load(Ordering::SeqCst),
            "offset_seconds":job.offset,"prepared_until":prepared_until,"window_complete":state.finished,
            "target_seconds":target,"duration_seconds":job.duration,
            "url_path":format!("/h/{}/{}/index.m3u8",self.token(),job.dir_name)})
    }

    fn wait_local_playback(job: &LocalPlayback) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !job.done.load(Ordering::SeqCst) {
            if Instant::now() >= deadline {
                return Err(Error::Conflict("local playback is still stopping".into()));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }

    /// Stops every running job of the entry. Finished windows stay for reopening.
    pub fn stop_local_playback(&self, id: &str) -> Result<()> {
        self.stop_local_playback_generation(id, None)
    }

    /// Stops the running job of the entry only if its latest session is
    /// `generation`. A stop from an older session (the job was reused or replaced
    /// since) is a no-op, so a late callback cannot end the current session.
    pub fn stop_local_playback_generation(&self, id: &str, generation: Option<u64>) -> Result<()> {
        let running: Vec<_> = self
            .local_playbacks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .jobs
            .iter()
            .filter(|job| job.entry_id == id && !job.done.load(Ordering::SeqCst))
            .filter(|job| generation.map_or(true, |g| job.generation.load(Ordering::SeqCst) == g))
            .cloned()
            .collect();
        for job in &running {
            job.cancel.store(true, Ordering::SeqCst);
        }
        for job in &running {
            Self::wait_local_playback(job)?;
        }
        Ok(())
    }

    fn run_local_playback(&self, ffmpeg: &str, source: &str, job: &LocalPlayback) -> Result<()> {
        let mut child = Command::new(ffmpeg)
            .args(local_playback_args(source, &job.dir, PLATFORM_ENCODER, job.offset, job.mode))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut stderr = child.stderr.take().unwrap();
        let reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stderr.by_ref().take(65536).read_to_end(&mut bytes);
            let _ = std::io::copy(&mut stderr, &mut std::io::sink());
            String::from_utf8_lossy(&bytes).into_owned()
        });
        let settings = self.video.settings();
        let mut stopped = None;
        let result = (|| -> Result<std::process::ExitStatus> {
            loop {
                if let Some(status) = child.try_wait()? {
                    break Ok(status);
                }
                if self.shutdown.load(Ordering::SeqCst) || job.cancel.load(Ordering::SeqCst) {
                    stopped = Some("local playback stopped");
                } else if crate::util::free_space(&self.paths.data_dir).is_some_and(|n| n < settings.min_free_bytes) {
                    stopped = Some("not enough free disk space for compatible playback");
                } else if dir_size(&job.dir) > settings.max_disk_bytes / 4 {
                    stopped = Some("compatible playback reached its temporary disk limit");
                }
                if stopped.is_some() {
                    let _ = child.kill();
                    break Ok(child.wait()?);
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        })();
        if result.is_err() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let stderr = reader.join().unwrap_or_default();
        let status = result?;
        if let Some(message) = stopped {
            return Err(Error::other(message));
        }
        if !status.success() {
            return Err(Error::other(explain_conversion_failure(&stderr)));
        }
        Ok(())
    }
}

fn dir_size(dir: &std::path::Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|entries| entries.filter_map(|e| e.ok()?.metadata().ok()).map(|m| m.len()).sum())
        .unwrap_or(0)
}

/// Runs `ffmpeg -i source` (no output, so it only prints stream information) and
/// parses the result. Failure or timeout yields an empty probe, which means
/// "encode, duration unknown".
fn probe_media(ffmpeg: &str, source: &str) -> MediaProbe {
    let Ok(mut child) = Command::new(ffmpeg)
        .args(["-nostdin", "-hide_banner", "-i", source])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    else {
        return MediaProbe::default();
    };
    let mut stderr = child.stderr.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.by_ref().take(65536).read_to_end(&mut bytes);
        let _ = std::io::copy(&mut stderr, &mut std::io::sink());
        String::from_utf8_lossy(&bytes).into_owned()
    });
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < PROBE_TIMEOUT => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
        }
    }
    parse_probe(&reader.join().unwrap_or_default())
}

/// Parses the stderr of `ffmpeg -i`: container duration and the first video and
/// audio stream (the ones local playback maps).
pub fn parse_probe(text: &str) -> MediaProbe {
    let mut probe = MediaProbe::default();
    for line in text.lines().map(str::trim_start) {
        if probe.duration.is_none() {
            if let Some(rest) = line.strip_prefix("Duration:") {
                probe.duration = rest.split(',').next().and_then(|t| parse_clock(t.trim()));
            }
        }
        if !line.starts_with("Stream #") {
            continue;
        }
        if probe.video.is_none() && !line.contains("(attached pic)") {
            if let Some(rest) = line.split(" Video: ").nth(1) {
                probe.video = rest.split(|c: char| c == ' ' || c == ',').next().map(str::to_string);
                probe.video_420_8bit = rest
                    .split(',')
                    .map(|part| part.trim().split(|c: char| c == '(' || c == ' ').next().unwrap_or(""))
                    .any(|pix| pix == "yuv420p" || pix == "yuvj420p");
                continue;
            }
        }
        if probe.audio.is_none() {
            if let Some(rest) = line.split(" Audio: ").nth(1) {
                probe.audio = rest.split(|c: char| c == ' ' || c == ',').next().map(str::to_string);
            }
        }
    }
    probe
}

fn parse_clock(text: &str) -> Option<f64> {
    let mut parts = text.split(':');
    let (h, m, s) = (parts.next()?, parts.next()?, parts.next()?);
    let seconds = h.parse::<f64>().ok()? * 3600.0 + m.parse::<f64>().ok()? * 60.0 + s.parse::<f64>().ok()?;
    (seconds.is_finite() && seconds > 0.0).then_some(seconds)
}

/// H.264 encoder for compatibility playback. The bundled FFmpeg is LGPL-only, so it has no
/// x264 (GPL); on macOS conversion uses Apple's VideoToolbox. Linux builds exist only for
/// development and tests, which run against the system ffmpeg and its libx264.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Encoder {
    VideoToolbox,
    X264,
}

const PLATFORM_ENCODER: Encoder = if cfg!(target_os = "macos") {
    Encoder::VideoToolbox
} else {
    Encoder::X264
};
const VIDEOTOOLBOX_ENCODER: &str = "h264_videotoolbox";
/// VideoToolbox quality, 1-100 (honoured on Apple Silicon). 65 is visually close to the
/// previous x264 `-crf 22` setting for ordinary screen and camera content.
const VIDEOTOOLBOX_QUALITY: &str = "65";

fn encoder_args(encoder: Encoder) -> Vec<&'static str> {
    match encoder {
        Encoder::VideoToolbox => vec![
            "-c:v", VIDEOTOOLBOX_ENCODER, "-q:v", VIDEOTOOLBOX_QUALITY, "-profile:v", "high",
            "-allow_sw", "1",
        ],
        Encoder::X264 => vec!["-c:v", "libx264", "-threads", "2", "-preset", "veryfast", "-crf", "22"],
    }
}

/// FFmpeg arguments for one window of `source` as a fragmented-MP4 HLS event
/// stream in `dir`. `-ss` is an input option (before `-i`): ffmpeg seeks the
/// source, decodes from the preceding keyframe and drops frames before `offset`,
/// so playlist time 0 is exactly original time `offset` (see the module docs).
fn local_playback_args(source: &str, dir: &std::path::Path, encoder: Encoder, offset: f64, mode: Mode) -> Vec<String> {
    let mut args: Vec<String> = ["-nostdin", "-hide_banner", "-loglevel", "error", "-y"].iter().map(|a| a.to_string()).collect();
    if mode == Mode::Encode && offset > 0.0 {
        args.extend(["-ss".to_string(), format!("{offset:.3}")]);
    }
    args.extend(["-i", source, "-map", "0:v:0?", "-map", "0:a:0?"].iter().map(|a| a.to_string()));
    match mode {
        Mode::Remux => args.extend(["-c", "copy"].iter().map(|a| a.to_string())),
        Mode::Encode => {
            args.extend(encoder_args(encoder).into_iter().map(String::from));
            args.extend(
                ["-pix_fmt", "yuv420p", "-vf", "scale=trunc(iw/2)*2:trunc(ih/2)*2", "-force_key_frames",
                    "expr:gte(t,n_forced*3)", "-c:a", "aac", "-b:a", "160k", "-ac", "2"]
                    .iter()
                    .map(|arg| arg.to_string()),
            );
        }
    }
    args.extend(
        [
            "-f", "hls", "-hls_time", "3", "-hls_playlist_type", "event", "-hls_segment_type", "fmp4",
            "-hls_fmp4_init_filename", "init.mp4", "-hls_flags", "independent_segments", "-hls_segment_filename",
        ]
        .iter()
        .map(|arg| arg.to_string()),
    );
    args.push(dir.join("seg_%05d.m4s").display().to_string());
    args.push(dir.join("index.m3u8").display().to_string());
    args
}

fn encoder_list_has_videotoolbox(encoders: &str) -> bool {
    encoders
        .lines()
        .any(|line| line.split_whitespace().nth(1) == Some(VIDEOTOOLBOX_ENCODER))
}

/// Fail early, with a clear message, if this FFmpeg cannot encode with VideoToolbox.
fn ensure_encoder(ffmpeg: &str, encoder: Encoder) -> Result<()> {
    if encoder != Encoder::VideoToolbox {
        return Ok(());
    }
    let output = Command::new(ffmpeg)
        .args(["-nostdin", "-hide_banner", "-encoders"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| Error::other(format!("could not run ffmpeg: {e}")))?;
    if encoder_list_has_videotoolbox(&String::from_utf8_lossy(&output.stdout)) {
        Ok(())
    } else {
        Err(Error::other(
            "compatible playback needs the VideoToolbox H.264 encoder, which this ffmpeg build does not provide",
        ))
    }
}

fn explain_conversion_failure(stderr: &str) -> String {
    let detail = stderr.chars().take(2000).collect::<String>();
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("videotoolbox") || lower.contains("compression session") {
        format!("VideoToolbox H.264 encoding is not available on this Mac, so compatible playback cannot start: {detail}")
    } else {
        format!("local compatibility conversion failed: {detail}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(args: &[String], flag: &str) -> Vec<String> {
        args.windows(2).filter(|w| w[0] == flag).map(|w| w[1].clone()).collect()
    }

    #[test]
    fn macos_encodes_with_videotoolbox_and_never_uses_x264() {
        let args = local_playback_args("/in/video.mkv", std::path::Path::new("/hls"), Encoder::VideoToolbox, 0.0, Mode::Encode);
        assert_eq!(pairs(&args, "-c:v"), ["h264_videotoolbox"]);
        assert_eq!(pairs(&args, "-q:v"), ["65"]);
        assert_eq!(pairs(&args, "-profile:v"), ["high"]);
        assert!(!args.iter().any(|a| a.contains("x264") || a == "-crf" || a == "-preset"));
        assert_eq!(PLATFORM_ENCODER == Encoder::VideoToolbox, cfg!(target_os = "macos"));
    }

    #[test]
    fn development_builds_keep_the_system_x264_arguments() {
        let args = local_playback_args("/in/video.mkv", std::path::Path::new("/hls"), Encoder::X264, 0.0, Mode::Encode);
        assert_eq!(pairs(&args, "-c:v"), ["libx264"]);
        assert_eq!(pairs(&args, "-preset"), ["veryfast"]);
        assert_eq!(pairs(&args, "-crf"), ["22"]);
    }

    #[test]
    fn keeps_pixel_format_scale_keyframes_and_aac_for_every_encoder() {
        for encoder in [Encoder::VideoToolbox, Encoder::X264] {
            let args = local_playback_args("/in/video.mkv", std::path::Path::new("/hls"), encoder, 0.0, Mode::Encode);
            assert_eq!(pairs(&args, "-pix_fmt"), ["yuv420p"]);
            assert_eq!(pairs(&args, "-vf"), ["scale=trunc(iw/2)*2:trunc(ih/2)*2"]);
            assert_eq!(pairs(&args, "-force_key_frames"), ["expr:gte(t,n_forced*3)"]);
            assert_eq!(pairs(&args, "-c:a"), ["aac"]);
            assert_eq!(pairs(&args, "-b:a"), ["160k"]);
            assert_eq!(pairs(&args, "-hls_segment_type"), ["fmp4"]);
            assert_eq!(pairs(&args, "-i"), ["/in/video.mkv"]);
            assert_eq!(args[args.len() - 2], "/hls/seg_%05d.m4s");
            assert_eq!(args[args.len() - 1], "/hls/index.m3u8");
        }
    }

    #[test]
    fn input_seek_precedes_the_input_and_remux_copies_from_the_start() {
        let args = local_playback_args("/in/video.webm", std::path::Path::new("/hls"), Encoder::X264, 45.0, Mode::Encode);
        let ss = args.iter().position(|a| a == "-ss").unwrap();
        let input = args.iter().position(|a| a == "-i").unwrap();
        assert!(ss < input, "-ss must be an input option: {args:?}");
        assert_eq!(args[ss + 1], "45.000");
        assert!(!args.iter().any(|a| a == "-output_ts_offset" || a == "-copyts"), "window-relative timeline");
        // The start of a window is not a seek.
        let from_zero = local_playback_args("/in/video.webm", std::path::Path::new("/hls"), Encoder::X264, 0.0, Mode::Encode);
        assert!(!from_zero.iter().any(|a| a == "-ss"));
        let remux = local_playback_args("/in/video.mkv", std::path::Path::new("/hls"), Encoder::VideoToolbox, 0.0, Mode::Remux);
        assert_eq!(pairs(&remux, "-c"), ["copy"]);
        assert!(!remux.iter().any(|a| a == "-c:v" || a == "-c:a" || a == "-ss" || a.contains("x264") || a.contains("videotoolbox")), "{remux:?}");
        assert_eq!(pairs(&remux, "-hls_segment_type"), ["fmp4"]);
    }

    #[test]
    fn window_starts_a_segment_boundary_before_the_target_and_never_before_zero() {
        assert_eq!(window_start(0.0, Mode::Encode), 0.0);
        assert_eq!(window_start(3.0, Mode::Encode), 0.0);
        assert_eq!(window_start(50.0, Mode::Encode), 45.0);
        assert_eq!(window_start(353.0, Mode::Encode), 348.0);
        assert_eq!(window_start(f64::NAN, Mode::Encode), 0.0);
        assert_eq!(window_start(353.0, Mode::Remux), 0.0);
        for t in [4.0, 7.9, 50.0, 353.4, 7200.0] {
            let start = window_start(t, Mode::Encode);
            assert!(start <= t - 3.0 + 1e-9 && t - start < 6.0 + 1e-9, "{t} -> {start}");
            assert_eq!(start % SEGMENT_SECONDS, 0.0);
        }
    }

    #[test]
    fn readiness_is_measured_on_the_original_timeline() {
        let playlist = "#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:3.0,\nseg_00000.m4s\n#EXTINF:3.0,\nseg_00001.m4s\n";
        // Window starts at 350 s, so 6 s prepared reaches 356 s.
        let at_353 = window_state(playlist, 350.0, 353.0);
        assert_eq!((at_353.segments, at_353.prepared_seconds, at_353.finished, at_353.ready), (2, 6.0, false, true));
        assert!(!window_state(playlist, 350.0, 356.0).ready, "needs a margin past the target");
        // The same 6 s of media would be far too little for a target the window has not reached.
        assert!(!window_state(playlist, 0.0, 353.0).ready, "a prefix from 0 does not cover 353 s");
        assert!(!window_state("#EXTM3U\n", 0.0, 0.0).ready);
        let finished = format!("{playlist}#EXT-X-ENDLIST\n");
        assert!(window_state(&finished, 0.0, 500.0).ready, "a finished window is playable");
    }

    #[test]
    fn resume_rule_matches_the_player() {
        assert_eq!(resume_position(5.0, Some(100.0)), 0.0);
        assert_eq!(resume_position(353.0, Some(900.0)), 353.0);
        assert_eq!(resume_position(895.0, Some(900.0)), 0.0);
        assert_eq!(resume_position(353.0, None), 353.0);
    }

    #[test]
    fn probe_output_picks_remux_only_for_h264_aac_by_stream_info() {
        let h264 = "Input #0, matroska,webm, from 'x.webm':\n  Duration: 00:01:30.04, start: 0.000000, bitrate: 900 kb/s\n  Stream #0:0: Video: h264 (High), yuv420p(tv, bt709, progressive), 320x180, 30 fps, 30 tbr, 1k tbn\n  Stream #0:1(eng): Audio: aac (LC), 48000 Hz, stereo, fltp\n";
        let probe = parse_probe(h264);
        assert!((probe.duration.unwrap() - 90.04).abs() < 1e-6);
        assert_eq!((probe.video.as_deref(), probe.audio.as_deref(), probe.video_420_8bit), (Some("h264"), Some("aac"), true));
        assert_eq!(probe.mode(), Mode::Remux);
        let av1 = "  Duration: 00:00:04.00, start: 0.0\n  Stream #0:0: Video: av1 (Main), yuv420p(tv), 160x90\n  Stream #0:1: Audio: opus, 48000 Hz, stereo, fltp\n";
        assert_eq!(parse_probe(av1).mode(), Mode::Encode);
        let vp9 = "  Stream #0:0: Video: vp9 (Profile 0), yuv420p(tv), 160x90\n";
        assert_eq!(parse_probe(vp9).mode(), Mode::Encode);
        let opus = "  Stream #0:0: Video: h264 (High), yuv420p, 160x90\n  Stream #0:1: Audio: opus, 48000 Hz\n";
        assert_eq!(parse_probe(opus).mode(), Mode::Encode, "h264 with opus still needs an audio encode");
        let ten_bit = "  Stream #0:0: Video: h264 (High 10), yuv420p10le(tv), 160x90\n  Stream #0:1: Audio: aac (LC), 48000 Hz\n";
        assert_eq!(parse_probe(ten_bit).mode(), Mode::Encode, "10-bit H.264 is not hardware/WebKit friendly");
        let silent = parse_probe("  Stream #0:0: Video: h264 (Main), yuv420p(progressive), 160x90\n");
        assert_eq!(silent.mode(), Mode::Remux);
        assert_eq!(parse_probe("garbage").mode(), Mode::Encode);
        assert_eq!(parse_probe("  Duration: N/A, bitrate: N/A\n").duration, None);
    }

    #[test]
    fn detects_the_videotoolbox_encoder_in_the_encoder_list() {
        let list = "Encoders:\n V..... libopenh264 OpenH264\n V..... h264_videotoolbox VideoToolbox H.264 Encoder\n A....D aac AAC\n";
        assert!(encoder_list_has_videotoolbox(list));
        assert!(!encoder_list_has_videotoolbox("Encoders:\n V....D mpeg4 MPEG-4 part 2\n"));
        // The name must be the encoder column, not a word inside a description.
        assert!(!encoder_list_has_videotoolbox("Encoders:\n V..... other uses h264_videotoolbox internally\n"));
    }

    #[test]
    fn videotoolbox_failures_get_a_clear_message() {
        let message = explain_conversion_failure("[h264_videotoolbox @ 0x1] cannot create compression session: -12908");
        assert!(message.starts_with("VideoToolbox H.264 encoding is not available on this Mac"));
        assert!(message.contains("-12908"));
        assert!(explain_conversion_failure("Invalid data found").starts_with("local compatibility conversion failed"));
    }
}
