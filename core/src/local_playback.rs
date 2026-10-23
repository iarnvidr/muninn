//! Local compatibility playback. Originals stay untouched; ffmpeg writes a
//! temporary H.264/AAC HLS stream, with no YouTube requests or downloads.
use crate::{Core, Error, Result};
use serde_json::{json, Value};
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

pub struct LocalPlayback {
    dir: PathBuf,
    cancel: AtomicBool,
    done: AtomicBool,
    error: Mutex<Option<String>>,
}

impl Core {
    pub fn prepare_local_playback(self: &Arc<Self>, id: &str) -> Result<Value> {
        let entry = self.resolve_media_entry(id)?;
        if !entry.is_complete() {
            return Err(Error::bad("finish importing this copy first"));
        }
        let ffmpeg =
            self.tools().ffmpeg.path.ok_or_else(|| {
                Error::other("ffmpeg is not ready for local compatibility playback")
            })?;
        ensure_encoder(&ffmpeg, PLATFORM_ENCODER)?;
        let mut jobs = self
            .local_playbacks
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(job) = jobs.get(id) {
            if !job.cancel.load(Ordering::SeqCst) && job.error.lock().unwrap().is_none() {
                drop(jobs);
                return self
                    .local_playback_status(id)
                    .ok_or_else(|| Error::other("local playback was replaced; retry"));
            }
            if !job.done.load(Ordering::SeqCst) {
                let stopping = job.clone();
                drop(jobs);
                Self::wait_local_playback(&stopping)?;
                return self.prepare_local_playback(id);
            }
        }
        // Keep at most one compatibility stream. Cancel older work and remove
        // finished temporary streams, without touching any library originals.
        jobs.retain(|_, job| {
            job.cancel.store(true, Ordering::SeqCst);
            if job.done.load(Ordering::SeqCst) {
                let _ = std::fs::remove_dir_all(&job.dir);
                false
            } else {
                true
            }
        });
        let dir = self.paths.cache_dir.join("hls").join(format!("{id}.local"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        let job = Arc::new(LocalPlayback {
            dir,
            cancel: AtomicBool::new(false),
            done: AtomicBool::new(false),
            error: Mutex::new(None),
        });
        jobs.insert(id.into(), job.clone());
        drop(jobs);
        let core = self.clone();
        let id = id.to_string();
        let source = entry
            .file_path
            .clone()
            .ok_or_else(|| Error::bad("copy has no local file"))?;
        let job_for_thread = job.clone();
        if let Err(error)=std::thread::Builder::new().name("muninn-local-playback".into()).spawn(move || {
            let result = core.run_local_playback(&ffmpeg, &source, &job_for_thread);
            if let Err(error) = &result { *job_for_thread.error.lock().unwrap() = Some(error.to_string()); }
            job_for_thread.done.store(true, Ordering::SeqCst);
            core.log_event(if result.is_ok(){"info"}else{"error"}, "playback.local", json!({"entry_id":id,"success":result.is_ok(),"error":result.err().map(|e|e.to_string())}));
        }) {
            *job.error.lock().unwrap()=Some(error.to_string());job.done.store(true,Ordering::SeqCst);
            return Err(error.into());
        }
        self.local_playback_status(&entry.id)
            .ok_or_else(|| Error::other("local playback was replaced; retry"))
    }

    pub fn local_playback_status(&self, id: &str) -> Option<Value> {
        let jobs = self
            .local_playbacks
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let job = jobs.get(id)?;
        // Cancellation belongs to the previous viewing session. Never offer its
        // unfinished playlist (or its eventual "stopped" error) as a source for
        // a later session. A complete native file remains directly playable.
        if job.cancel.load(Ordering::SeqCst) { return None; }
        let error = job.error.lock().unwrap().clone();
        let playlist = std::fs::read_to_string(job.dir.join("index.m3u8")).unwrap_or_default();
        let segments = playlist
            .lines()
            .filter(|line| line.ends_with(".m4s"))
            .count();
        let finished = playlist.contains("#EXT-X-ENDLIST");
        let prepared_seconds = playlist
            .lines()
            .filter_map(|line| {
                line.strip_prefix("#EXTINF:")?
                    .split(',')
                    .next()?
                    .parse::<f64>()
                    .ok()
            })
            .sum::<f64>();
        Some(
            json!({"state":if error.is_some(){"error"}else if finished{"cached"}else if segments>0{"streaming"}else{"resolving"},
            "kind":"hls","local":true,"entry_id":id,"ready":error.is_none()&&segments>0,
            "progress":if finished{1.0}else{0.0},"prepared_seconds":prepared_seconds,"error":error,
            "url_path":format!("/h/{}/{}.local/index.m3u8",self.token(),id)}),
        )
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

    pub fn stop_local_playback(&self, id: &str) -> Result<()> {
        let job = self
            .local_playbacks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned();
        if let Some(job) = job {
            if !job.done.load(Ordering::SeqCst) {
                job.cancel.store(true, Ordering::SeqCst);
            }
            Self::wait_local_playback(&job)?;
        }
        Ok(())
    }

    fn run_local_playback(&self, ffmpeg: &str, source: &str, job: &LocalPlayback) -> Result<()> {
        let mut child = Command::new(ffmpeg)
            .args(local_playback_args(source, &job.dir, PLATFORM_ENCODER))
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
                } else if crate::util::free_space(&self.paths.data_dir)
                    .is_some_and(|n| n < settings.min_free_bytes)
                {
                    stopped = Some("not enough free disk space for compatible playback");
                } else {
                    let size = std::fs::read_dir(&job.dir)?
                        .filter_map(|e| e.ok()?.metadata().ok())
                        .map(|m| m.len())
                        .sum::<u64>();
                    if size > settings.max_disk_bytes / 4 {
                        stopped = Some("compatible playback reached its temporary disk limit");
                    }
                }
                if stopped.is_some() {
                    let _ = child.kill();
                    break Ok(child.wait()?);
                }
                std::thread::sleep(Duration::from_millis(500));
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

/// FFmpeg arguments that turn `source` into a fragmented-MP4 HLS event stream in `dir`.
fn local_playback_args(source: &str, dir: &std::path::Path, encoder: Encoder) -> Vec<String> {
    let mut args: Vec<String> = ["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-i", source,
        "-map", "0:v:0?", "-map", "0:a:0?"]
        .iter()
        .map(|arg| arg.to_string())
        .collect();
    args.extend(encoder_args(encoder).into_iter().map(String::from));
    args.extend(
        [
            "-pix_fmt", "yuv420p", "-vf", "scale=trunc(iw/2)*2:trunc(ih/2)*2", "-force_key_frames",
            "expr:gte(t,n_forced*3)", "-c:a", "aac", "-b:a", "160k", "-ac", "2", "-f", "hls",
            "-hls_time", "3", "-hls_playlist_type", "event", "-hls_segment_type", "fmp4",
            "-hls_fmp4_init_filename", "init.mp4", "-hls_flags", "independent_segments",
            "-hls_segment_filename",
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
        let args = local_playback_args("/in/video.mkv", std::path::Path::new("/hls"), Encoder::VideoToolbox);
        assert_eq!(pairs(&args, "-c:v"), ["h264_videotoolbox"]);
        assert_eq!(pairs(&args, "-q:v"), ["65"]);
        assert_eq!(pairs(&args, "-profile:v"), ["high"]);
        assert!(!args.iter().any(|a| a.contains("x264") || a == "-crf" || a == "-preset"));
        assert_eq!(PLATFORM_ENCODER == Encoder::VideoToolbox, cfg!(target_os = "macos"));
    }

    #[test]
    fn development_builds_keep_the_system_x264_arguments() {
        let args = local_playback_args("/in/video.mkv", std::path::Path::new("/hls"), Encoder::X264);
        assert_eq!(pairs(&args, "-c:v"), ["libx264"]);
        assert_eq!(pairs(&args, "-preset"), ["veryfast"]);
        assert_eq!(pairs(&args, "-crf"), ["22"]);
    }

    #[test]
    fn keeps_pixel_format_scale_keyframes_and_aac_for_every_encoder() {
        for encoder in [Encoder::VideoToolbox, Encoder::X264] {
            let args = local_playback_args("/in/video.mkv", std::path::Path::new("/hls"), encoder);
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
