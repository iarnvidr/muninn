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
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let tool = bin.join("ffmpeg");
    std::fs::write(
        &tool,
        "#!/bin/sh\nif [ \"$1\" = -version ]; then echo 'ffmpeg test'; exit 0; fi\nfor a in \"$@\"; do if [ \"$a\" = -encoders ]; then echo ' V..... h264_videotoolbox VideoToolbox H.264 Encoder'; exit 0; fi; done\nexec sleep 30\n",
    )
    .unwrap();
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
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
    let playlist = core
        .paths
        .cache_dir
        .join("hls")
        .join(format!("{id}.local/index.m3u8"));
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
