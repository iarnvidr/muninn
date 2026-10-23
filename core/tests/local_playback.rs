use muninn_core::imports::MediaImport;
use muninn_core::{Core, CoreConfig};
use std::process::Command;
use std::time::{Duration, Instant};

#[test]
fn imported_mkv_plays_from_local_h264_hls_and_original_is_unchanged() {
    check_local_copy("mkv", "mpeg4");
}

#[test]
fn imported_av1_webm_is_prepared_automatically_without_a_player_switch() {
    check_local_copy("webm", "libaom-av1");
}

#[cfg(unix)]
#[test]
fn stop_acknowledges_worker_exit_before_same_copy_is_reopened() {
    check_stopped_copy("webm");
}

#[cfg(unix)]
#[test]
fn cancelling_mp4_recovery_does_not_poison_direct_playback_on_reopen() {
    check_stopped_copy("mp4");
}

#[cfg(unix)]
fn check_stopped_copy(ext: &str) {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    write_fake_ffmpeg(&bin, &dir.path().join("spawns.log"), PROBE_VP9_OPUS);
    let core = Core::open(CoreConfig {
        data_dir: dir.path().join("data"),
        tool_dirs: Some(vec![bin]),
        ..Default::default()
    })
    .unwrap();
    let source = dir.path().join(format!("copy.{ext}"));
    std::fs::write(&source, b"test worker input").unwrap();
    let imported = core
        .import_media(MediaImport {
            path: source,
            video_id: Some("dQw4w9WgXcQ".into()),
            metadata: None,
            move_file: false,
            enrich: false,
            dry_run: false,
        })
        .unwrap();
    let id = imported["entry"]["id"].as_str().unwrap();
    let mut settings = core.video.settings();
    settings.offline_mode = true;
    core.apply_settings(&settings).unwrap();
    for _ in 0..3 {
        core.prepare_local_playback(id).unwrap();
        core.stop_local_playback(id).unwrap();
        assert!(core.local_playback_status(id).is_none());
        if ext == "mp4" {
            let reopened = core.watch_open("dQw4w9WgXcQ", false).unwrap();
            assert_eq!(reopened["playback"]["kind"], "file", "{reopened}");
            assert_eq!(reopened["playback"]["ready"], true, "{reopened}");
            assert!(reopened["playback"]["error"].is_null(), "{reopened}");
        }
        assert_eq!(
            core.video.get_entry(id).unwrap().unwrap().status,
            "complete"
        );
    }
    core.prepare_local_playback(id).unwrap();
    assert!(core.local_playback_status(id).unwrap()["error"].is_null());
    core.stop_local_playback(id).unwrap();
    core.shutdown();
}

const PROBE_VP9_OPUS: &str = "  Duration: 00:10:00.00, start: 0.000000, bitrate: 100 kb/s\n  Stream #0:0: Video: vp9 (Profile 0), yuv420p(tv), 160x90, 10 fps\n  Stream #0:1: Audio: opus, 48000 Hz, mono, fltp\n";
const PROBE_H264_AAC: &str = "  Duration: 00:10:00.00, start: 0.000000, bitrate: 100 kb/s\n  Stream #0:0: Video: h264 (High), yuv420p(progressive), 160x90, 10 fps\n  Stream #0:1: Audio: aac (LC), 48000 Hz, stereo, fltp\n";
const VIDEO: &str = "dQw4w9WgXcQ";

/// An ffmpeg stand-in that answers `-version`, `-encoders` and `ffmpeg -i`
/// probes immediately, records every preparation (`-hls_time`) in `log`, and
/// then behaves like a long-running encoder.
#[cfg(unix)]
fn write_fake_ffmpeg(bin: &std::path::Path, log: &std::path::Path, probe: &str) {
    use std::os::unix::fs::PermissionsExt;
    let tool = bin.join("ffmpeg");
    std::fs::write(
        &tool,
        format!(
            "#!/bin/sh\nif [ \"$1\" = -version ]; then echo 'ffmpeg test'; exit 0; fi\nfor a in \"$@\"; do if [ \"$a\" = -encoders ]; then echo ' V..... h264_videotoolbox VideoToolbox H.264 Encoder'; exit 0; fi; done\nfor a in \"$@\"; do if [ \"$a\" = -hls_time ]; then echo \"$*\" >> '{}'; exec sleep 30; fi; done\ncat >&2 <<'EOF'\n{}EOF\nexit 1\n",
            log.display(),
            probe
        ),
    )
    .unwrap();
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A wrapper that records every preparation and then runs the real ffmpeg.
#[cfg(unix)]
fn write_counting_ffmpeg(bin: &std::path::Path, log: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let real = String::from_utf8(Command::new("sh").args(["-c", "command -v ffmpeg"]).output().unwrap().stdout).unwrap();
    let tool = bin.join("ffmpeg");
    std::fs::write(
        &tool,
        format!(
            "#!/bin/sh\nfor a in \"$@\"; do if [ \"$a\" = -hls_time ]; then echo \"$*\" >> '{}'; fi; done\nexec '{}' \"$@\"\n",
            log.display(),
            real.trim()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Spawns recorded so far, after giving a just-started worker time to launch ffmpeg.
fn wait_spawns(log: &std::path::Path, count: usize) -> Vec<String> {
    let started = Instant::now();
    while spawns(log).len() < count && started.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    spawns(log)
}

fn spawns(log: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(log).unwrap_or_default().lines().map(str::to_string).collect()
}

fn hls_playlist(core: &Core, playback: &serde_json::Value) -> std::path::PathBuf {
    let url = playback["url_path"].as_str().unwrap();
    let dir = url.rsplit('/').nth(1).unwrap();
    core.paths.cache_dir.join("hls").join(dir).join("index.m3u8")
}

fn synthetic(dir: &std::path::Path, name: &str, seconds: u32, codecs: &[&str]) -> std::path::PathBuf {
    let path = dir.join(name);
    let mut args: Vec<String> = ["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=160x90:rate=10",
        "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=24000", "-t"]
        .iter().map(|a| a.to_string()).collect();
    args.push(seconds.to_string());
    args.extend(codecs.iter().map(|a| a.to_string()));
    assert!(Command::new("ffmpeg").args(args).arg(&path).status().unwrap().success());
    path
}

const VP9_OPUS: &[&str] = &["-c:v", "libvpx-vp9", "-deadline", "realtime", "-cpu-used", "8", "-b:v", "80k", "-g", "30", "-c:a", "libopus", "-b:a", "16k", "-ac", "1"];
const H264_AAC: &[&str] = &["-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "24k", "-ac", "1"];

struct Fixture {
    _dir: tempfile::TempDir,
    core: std::sync::Arc<Core>,
    log: std::path::PathBuf,
    root: std::path::PathBuf,
}

/// An isolated core. `real` runs the system ffmpeg through a spawn counter;
/// otherwise a fake long-running ffmpeg is used. Offline, so nothing can
/// reach the network.
#[cfg(unix)]
fn fixture(real: bool, probe: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let log = dir.path().join("spawns.log");
    if real {
        write_counting_ffmpeg(&bin, &log);
    } else {
        write_fake_ffmpeg(&bin, &log, probe);
    }
    let core = Core::open(CoreConfig { data_dir: dir.path().join("data"), tool_dirs: Some(vec![bin]), ..Default::default() }).unwrap();
    let mut settings = core.video.settings();
    settings.offline_mode = true;
    settings.min_free_bytes = 0;
    core.apply_settings(&settings).unwrap();
    let root = dir.path().to_path_buf();
    Fixture { _dir: dir, core, log, root }
}

fn import(core: &std::sync::Arc<Core>, path: &std::path::Path, video: &str) -> String {
    let imported = core
        .import_media(MediaImport { path: path.to_path_buf(), video_id: Some(video.into()), metadata: None, move_file: false, enrich: false, dry_run: false })
        .unwrap();
    imported["entry"]["id"].as_str().unwrap().to_string()
}

fn wait_for(core: &Core, id: &str, what: &str, done: impl Fn(&serde_json::Value) -> bool) -> serde_json::Value {
    let started = Instant::now();
    loop {
        let status = core.local_playback_status(id).unwrap_or(serde_json::Value::Null);
        assert_ne!(status["state"], "error", "{status}");
        if done(&status) {
            return status;
        }
        assert!(started.elapsed() < Duration::from_secs(30), "timed out waiting for {what}: {status}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(unix)]
#[test]
fn resume_prepares_a_window_near_the_position_instead_of_encoding_the_prefix() {
    if Command::new("ffmpeg").arg("-version").output().is_err() {
        return;
    }
    let f = fixture(true, "");
    let source = synthetic(&f.root, "clip.webm", 90, VP9_OPUS);
    let id = import(&f.core, &source, VIDEO);
    f.core.content.record_progress(VIDEO, 50.0, Some(90.0), None, f.core.now()).unwrap();
    let started = Instant::now();
    let opened = f.core.watch_open(VIDEO, false).unwrap();
    assert_eq!(opened["playback"]["local"], true, "{opened}");
    assert_eq!(opened["playback"]["mode"], "encode");
    let ready = wait_for(&f.core, &id, "first ready status", |s| s["ready"] == true);
    let first_ready = started.elapsed();
    // Window starts a segment boundary before the 50 s resume point, on the original timeline.
    assert_eq!(ready["offset_seconds"], 45.0, "{ready}");
    assert_eq!(ready["target_seconds"], 50.0);
    assert!(ready["prepared_until"].as_f64().unwrap() >= 51.0, "segments must cover the target: {ready}");
    assert!(ready["prepared_seconds"].as_f64().unwrap() < 30.0, "the first ready window must not be a prefix from zero: {ready}");
    // The duration is the original's (probed), not the window's.
    assert!((ready["duration_seconds"].as_f64().unwrap() - 90.0).abs() < 1.0, "{ready}");
    assert!(first_ready < Duration::from_secs(20), "{first_ready:?}");
    let done = wait_for(&f.core, &id, "complete window", |s| s["window_complete"] == true);
    // ffmpeg encoded only 45 s..end: not 90 s from zero.
    let prepared = done["prepared_seconds"].as_f64().unwrap();
    assert!((43.0..=48.0).contains(&prepared), "{done}");
    assert!((done["prepared_until"].as_f64().unwrap() - 90.0).abs() < 3.0, "{done}");
    let log = spawns(&f.log);
    assert_eq!(log.len(), 1, "{log:?}");
    let words: Vec<&str> = log[0].split(' ').collect();
    let ss = words.iter().position(|w| *w == "-ss").expect("input seek");
    assert!(ss < words.iter().position(|w| *w == "-i").unwrap(), "-ss must precede -i: {log:?}");
    assert_eq!(words[ss + 1], "45.000");
    // The prepared window decodes, and nothing was downloaded.
    assert!(Command::new("ffmpeg").args(["-nostdin", "-hide_banner", "-loglevel", "error", "-i"]).arg(hls_playlist(&f.core, &done)).args(["-f", "null", "-"]).status().unwrap().success());
    assert!(f.core.stream_job(VIDEO).is_none(), "local open must not start a download");
    assert_eq!(std::fs::read(&source).unwrap(), std::fs::read(f.core.video.get_entry(&id).unwrap().unwrap().file_path.unwrap()).unwrap());
    f.core.shutdown();
}

#[cfg(unix)]
#[test]
fn reopening_and_seeking_inside_the_window_reuse_it_and_seeking_outside_restarts_near_the_target() {
    if Command::new("ffmpeg").arg("-version").output().is_err() {
        return;
    }
    let f = fixture(true, "");
    let source = synthetic(&f.root, "clip.webm", 90, VP9_OPUS);
    let id = import(&f.core, &source, VIDEO);
    f.core.content.record_progress(VIDEO, 50.0, Some(90.0), None, f.core.now()).unwrap();
    let first = f.core.watch_open(VIDEO, false).unwrap()["playback"].clone();
    wait_for(&f.core, &id, "complete window", |s| s["window_complete"] == true);
    assert_eq!(spawns(&f.log).len(), 1);
    // Reopen, as the UI does repeatedly: same job, same output, no new ffmpeg.
    let reopened = f.core.watch_open(VIDEO, false).unwrap()["playback"].clone();
    assert_eq!(reopened["job_id"], first["job_id"], "{reopened}");
    assert_eq!(reopened["url_path"], first["url_path"]);
    assert_eq!(reopened["window_complete"], true, "finished output must not be removed and redone");
    // A seek inside the finished window only moves the target.
    let inside = f.core.prepare_local_playback_at(&id, Some(70.0)).unwrap();
    assert_eq!((inside["job_id"].clone(), inside["target_seconds"].clone()), (first["job_id"].clone(), serde_json::json!(70.0)));
    assert!(inside["generation"].as_u64() > first["generation"].as_u64());
    assert_eq!(spawns(&f.log).len(), 1);
    // A seek before the window start needs a new window near the target.
    let back = f.core.prepare_local_playback_at(&id, Some(10.0)).unwrap();
    assert_ne!(back["job_id"], first["job_id"]);
    assert_eq!(back["offset_seconds"], 6.0, "{back}");
    let back_ready = wait_for(&f.core, &id, "window near 10 s", |s| s["ready"] == true);
    assert_eq!(back_ready["offset_seconds"], 6.0);
    assert_eq!(spawns(&f.log).len(), 2);
    wait_for(&f.core, &id, "complete window", |s| s["window_complete"] == true);
    // The first window was kept, so returning to 60 s starts nothing, whichever window serves it.
    let again = f.core.prepare_local_playback_at(&id, Some(60.0)).unwrap();
    assert_eq!(again["ready"], true, "{again}");
    assert_eq!(spawns(&f.log).len(), 2);
    f.core.shutdown();
}

#[cfg(unix)]
#[test]
fn a_stale_stop_cannot_end_the_current_session() {
    let f = fixture(false, PROBE_VP9_OPUS);
    let source = f.root.join("clip.webm");
    std::fs::write(&source, b"not real media").unwrap();
    let id = import(&f.core, &source, VIDEO);
    let first = f.core.prepare_local_playback_at(&id, Some(100.0)).unwrap();
    // Reusing the running job starts a newer session on it.
    let second = f.core.prepare_local_playback_at(&id, Some(105.0)).unwrap();
    assert_eq!(second["job_id"], first["job_id"]);
    assert!(second["generation"].as_u64() > first["generation"].as_u64());
    f.core.stop_local_playback_generation(&id, first["generation"].as_u64()).unwrap();
    assert_eq!(f.core.local_playback_status(&id).unwrap()["job_id"], second["job_id"], "stale stop killed the new session");
    // A seek far outside the window replaces the job (and cancels the old one).
    // Let the first job actually launch ffmpeg before it is replaced (else it can be cancelled unspawned).
    assert_eq!(wait_spawns(&f.log, 1).len(), 1);
    let third = f.core.prepare_local_playback_at(&id, Some(400.0)).unwrap();
    assert_ne!(third["job_id"], second["job_id"]);
    assert_eq!(third["offset_seconds"], 396.0);
    assert_eq!(wait_spawns(&f.log, 2).len(), 2);
    // The late stop of the replaced session is a no-op for the new one...
    f.core.stop_local_playback_generation(&id, second["generation"].as_u64()).unwrap();
    let alive = f.core.local_playback_status(&id).unwrap();
    assert_eq!((alive["job_id"].clone(), alive["error"].clone()), (third["job_id"].clone(), serde_json::Value::Null), "{alive}");
    // ...while the stop for the current session ends it.
    f.core.stop_local_playback_generation(&id, third["generation"].as_u64()).unwrap();
    assert!(f.core.local_playback_status(&id).is_none());
    f.core.shutdown();
}

#[cfg(unix)]
#[test]
fn a_late_handled_older_prepare_cannot_replace_the_newer_session() {
    let f = fixture(false, PROBE_VP9_OPUS);
    let source = f.root.join("clip.webm");
    std::fs::write(&source, b"not real media").unwrap();
    let id = import(&f.core, &source, VIDEO);
    let newer = f.core.prepare_local_playback_for(&id, Some(100.0), Some(("page-a", 2))).unwrap();
    assert_eq!(wait_spawns(&f.log, 1).len(), 1);
    // The older request (seq 1, a far seek) is handled after seq 2: refused, nothing changes.
    let late = f.core.prepare_local_playback_for(&id, Some(400.0), Some(("page-a", 1)));
    assert!(late.is_err(), "stale prepare was served: {late:?}");
    let now = f.core.local_playback_status(&id).unwrap();
    assert_eq!((now["job_id"].clone(), now["generation"].clone()), (newer["job_id"].clone(), newer["generation"].clone()));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(spawns(&f.log).len(), 1, "stale prepare started an ffmpeg");
    // The same sequence number again (a retry) and later ones are served.
    let again = f.core.prepare_local_playback_for(&id, Some(105.0), Some(("page-a", 2))).unwrap();
    assert!(again["generation"].as_u64() > newer["generation"].as_u64());
    // A reloaded page restarts its counter under a new token.
    assert!(f.core.prepare_local_playback_for(&id, Some(106.0), Some(("page-b", 1))).is_ok());
    // Requests without a token keep working.
    assert!(f.core.prepare_local_playback_at(&id, Some(107.0)).is_ok());
    f.core.shutdown();
}

#[cfg(unix)]
#[test]
fn h264_aac_in_mkv_is_remuxed_without_encoding() {
    if Command::new("ffmpeg").arg("-version").output().is_err() {
        return;
    }
    let f = fixture(true, "");
    let source = synthetic(&f.root, "clip.mkv", 90, H264_AAC);
    let id = import(&f.core, &source, VIDEO);
    let status = f.core.prepare_local_playback_at(&id, Some(50.0)).unwrap();
    assert_eq!(status["mode"], "remux", "{status}");
    assert_eq!(status["offset_seconds"], 0.0, "stream copy starts at zero so the timeline stays exact");
    wait_for(&f.core, &id, "remuxed window", |s| s["ready"] == true && s["prepared_until"].as_f64().unwrap() >= 51.0);
    let done = wait_for(&f.core, &id, "complete remux", |s| s["window_complete"] == true);
    assert!((done["prepared_seconds"].as_f64().unwrap() - 90.0).abs() < 4.0, "{done}");
    let log = spawns(&f.log);
    assert_eq!(log.len(), 1);
    assert!(log[0].contains("-c copy"), "{log:?}");
    assert!(!log[0].contains("libx264") && !log[0].contains("videotoolbox") && !log[0].contains("-ss "), "{log:?}");
    assert!(Command::new("ffmpeg").args(["-nostdin", "-hide_banner", "-loglevel", "error", "-i"]).arg(hls_playlist(&f.core, &done)).args(["-f", "null", "-"]).status().unwrap().success());
    f.core.shutdown();
}

#[cfg(unix)]
#[test]
fn the_codecs_decide_the_path_not_the_file_name() {
    for (probe, mode) in [(PROBE_H264_AAC, "remux"), (PROBE_VP9_OPUS, "encode")] {
        let f = fixture(false, probe);
        let source = f.root.join("clip.webm");
        std::fs::write(&source, b"not real media").unwrap();
        let id = import(&f.core, &source, VIDEO);
        let status = f.core.prepare_local_playback_at(&id, Some(0.0)).unwrap();
        assert_eq!(status["mode"], mode, "{status}");
        assert_eq!(status["duration_seconds"], 600.0, "duration comes from ffmpeg -i when the entry has none");
        assert!(wait_spawns(&f.log, 1)[0].contains(if mode == "remux" { "-c copy" } else { "-pix_fmt" }));
        f.core.shutdown();
    }
}

#[cfg(unix)]
#[test]
fn local_open_never_starts_a_download_and_a_direct_mp4_needs_no_preparation() {
    for ext in ["mp4", "webm"] {
        let f = fixture(false, PROBE_VP9_OPUS);
        let source = f.root.join(format!("clip.{ext}"));
        std::fs::write(&source, b"not real media").unwrap();
        let id = import(&f.core, &source, VIDEO);
        let opened = f.core.watch_open(VIDEO, false).unwrap();
        assert!(f.core.stream_job(VIDEO).is_none(), "{opened}");
        assert!(opened["playback"]["error"].is_null(), "{opened}");
        if ext == "mp4" {
            assert_eq!(opened["playback"]["kind"], "file", "{opened}");
            assert_eq!(opened["playback"]["url_path"], format!("/media/{id}"));
            assert!(f.core.local_playback_status(&id).is_none());
            assert!(spawns(&f.log).is_empty(), "direct playback must not run ffmpeg");
        } else {
            assert_eq!(opened["playback"]["local"], true, "{opened}");
        }
        f.core.shutdown();
    }
}

#[cfg(unix)]
#[test]
fn finished_windows_are_kept_for_reopening_but_bounded() {
    if Command::new("ffmpeg").arg("-version").output().is_err() {
        return;
    }
    let f = fixture(true, "");
    let source = synthetic(&f.root, "clip.webm", 12, VP9_OPUS);
    let videos = ["aaaaaaaaaaa", "bbbbbbbbbbb", "ccccccccccc"];
    let ids: Vec<String> = videos.iter().map(|v| import(&f.core, &source, v)).collect();
    let dirs = |f: &Fixture| -> usize {
        std::fs::read_dir(f.core.paths.cache_dir.join("hls")).map(|d| d.filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().contains(".local.")).count()).unwrap_or(0)
    };
    f.core.prepare_local_playback_at(&ids[0], Some(0.0)).unwrap();
    wait_for(&f.core, &ids[0], "first complete", |s| s["window_complete"] == true);
    f.core.prepare_local_playback_at(&ids[1], Some(0.0)).unwrap();
    wait_for(&f.core, &ids[1], "second complete", |s| s["window_complete"] == true);
    assert_eq!(dirs(&f), 2, "opening another copy must not delete the finished one");
    let reopened = f.core.prepare_local_playback_at(&ids[0], Some(0.0)).unwrap();
    assert_eq!(reopened["window_complete"], true);
    assert_eq!(spawns(&f.log).len(), 2, "reopening a retained copy runs nothing");
    f.core.prepare_local_playback_at(&ids[2], Some(0.0)).unwrap();
    wait_for(&f.core, &ids[2], "third complete", |s| s["window_complete"] == true);
    assert_eq!(dirs(&f), 2, "finished windows are evicted beyond the retention bound");
    assert!(f.core.local_playback_status(&ids[1]).is_none(), "the least recently used window is the one evicted");
    assert!(f.core.local_playback_status(&ids[0]).is_some());
    f.core.shutdown();
}

fn check_local_copy(ext: &str, codec: &str) {
    // The macOS builder has ffmpeg for real media checks; a developer without
    // ffmpeg can still run the pure core suite.
    if Command::new("ffmpeg").arg("-version").output().is_err() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join(format!("clip.{ext}"));
    let result = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=160x90:rate=15",
            "-t",
            "4",
            "-c:v",
            codec,
        ])
        .arg(&source)
        .status()
        .unwrap();
    assert!(result.success());
    let original = std::fs::read(&source).unwrap();
    let ffmpeg = Command::new("sh")
        .args(["-c", "command -v ffmpeg"])
        .output()
        .unwrap();
    let tool_dir = std::path::PathBuf::from(String::from_utf8(ffmpeg.stdout).unwrap().trim())
        .parent()
        .unwrap()
        .to_path_buf();
    let core = Core::open(CoreConfig {
        data_dir: dir.path().join("data"),
        tool_dirs: Some(vec![tool_dir]),
        ..Default::default()
    })
    .unwrap();
    let mut settings = core.video.settings();
    settings.min_free_bytes = 0;
    core.video.put_settings(&settings).unwrap();
    let imported = core
        .import_media(MediaImport {
            path: source.clone(),
            video_id: Some("dQw4w9WgXcQ".into()),
            metadata: None,
            move_file: false,
            dry_run: false,
            enrich: false,
        })
        .unwrap();
    let id = imported["entry"]["id"].as_str().unwrap();
    for kind in [
        muninn_core::content::KIND_META,
        muninn_core::content::KIND_COMMENTS,
    ] {
        core.content
            .put_json(
                kind,
                "dQw4w9WgXcQ",
                &serde_json::json!({}),
                None,
                core.now(),
            )
            .unwrap();
    }
    let initial = core.watch_open("dQw4w9WgXcQ", false).unwrap();
    assert_eq!(initial["playback"]["local"], true);
    let started = Instant::now();
    loop {
        let status = core.local_playback_status(id).unwrap();
        assert_ne!(status["state"], "error", "{status}");
        if status["state"] == "cached" {
            assert_eq!(status["ready"], true);
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(30), "{status}");
        std::thread::sleep(Duration::from_millis(100));
    }
    let entry = core.video.get_entry(id).unwrap().unwrap();
    let playback = core.watch_status("dQw4w9WgXcQ");
    assert_eq!(playback["playback"]["local"], true);
    assert_eq!(playback["playback"]["kind"], "hls");
    assert_eq!(std::fs::read(entry.file_path.unwrap()).unwrap(), original);
    assert_eq!(std::fs::read(source).unwrap(), original);
    let playlist = hls_playlist(&core, &playback["playback"]);
    assert!(Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(playlist)
        .args(["-f", "null", "-"])
        .status()
        .unwrap()
        .success());
    assert!(core.prepare_local_playback("missing").is_err());
    let report_path = dir.path().join("support.zip");
    muninn_core::diagnostics::export(&core.paths.data_dir, &report_path).unwrap();
    let mut zip = zip::ZipArchive::new(std::fs::File::open(report_path).unwrap()).unwrap();
    let report: serde_json::Value =
        serde_json::from_reader(zip.by_name("report.json").unwrap()).unwrap();
    assert_eq!(report["schema_version"], 2);
    assert_eq!(report["database"]["recent_media"][0]["container"], ext);
    assert_eq!(report["database"]["recent_media"][0]["file_exists"], true);
    assert_eq!(
        report["database"]["recent_media"][0]["actual_bytes"],
        original.len() as u64
    );
    assert!(
        report["database"]["media_probes"][0]["streams"]
            .to_string()
            .contains(if ext == "webm" { "av1" } else { "mpeg4" }),
        "{report}"
    );
    assert!(!report
        .to_string()
        .contains(&dir.path().to_string_lossy().to_string()));
    core.shutdown();
}
