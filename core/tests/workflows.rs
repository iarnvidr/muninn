use muninn_core::automation::{Automation, CompletionAction, CompletionRule};
use muninn_core::imports::{MediaImport, TabsImport};
use muninn_core::model::PlaybackIngest;
use muninn_core::videodb::{EntryRow, STATUS_COMPLETE};
use muninn_core::{Core, CoreConfig};
use serde_json::json;

fn core() -> (tempfile::TempDir, std::sync::Arc<Core>) {
    let d = tempfile::tempdir().unwrap();
    let c = Core::open(CoreConfig {
        data_dir: d.path().into(),
        tool_dirs: Some(vec![]),
        ..Default::default()
    })
    .unwrap();
    c.set_now_for_tests(Some(1_000_000));
    (d, c)
}

#[test]
fn dragged_tabs_move_groups_reorder_and_can_be_ungrouped() {
    let (_d, c) = core();
    let change = |value| {
        c.change_organization(serde_json::from_value(value).unwrap())
            .unwrap()
    };
    change(json!({"kind":"tab_group","action":"create","name":"Research"}));
    change(json!({"kind":"tab_group","action":"create","name":"Watch"}));
    change(json!({"kind":"tab","action":"move","name":"Research","ids":["1","2"]}));
    let moved =
        change(json!({"kind":"tab","action":"move","name":"Research","ids":["2"],"before_id":"1"}));
    assert_eq!(moved.tab_groups["Research"].tab_ids, vec!["2", "1"]);
    let moved = change(json!({"kind":"tab","action":"move","name":"Watch","ids":["2"]}));
    assert_eq!(moved.tab_groups["Research"].tab_ids, vec!["1"]);
    assert_eq!(moved.tab_groups["Watch"].tab_ids, vec!["2"]);
    let moved = change(json!({"kind":"tab","action":"move","name":"","ids":["2"],"before_id":"1"}));
    assert!(moved.tab_groups["Watch"].tab_ids.is_empty());
    assert_eq!(moved.tab_order, vec!["2", "1"]);
}

#[test]
fn direct_channel_discovery_reports_terminal_states_and_rejects_bad_ids() {
    let (_d, c) = core();
    let channels = vec![muninn_core::subscriptions::DiscoveredChannel {
        channel_id: "UCabcdefghijklmnopqrstuv".into(),
        name: "Named channel".into(),
    }];
    c.ingest_channel_candidates(&channels, "ready", None).unwrap();
    assert_eq!(c.channel_discovery()["count"], 1);
    assert_eq!(c.known_channels()[0]["name"], "Named channel");
    assert!(c.subscriptions().is_empty());
    let bad = vec![muninn_core::subscriptions::DiscoveredChannel {
        channel_id: "bad".into(),
        name: "Bad".into(),
    }];
    assert!(c.ingest_channel_candidates(&bad, "ready", None).is_err());
    assert_eq!(c.known_channels().as_array().unwrap().len(), 1);
    c.ingest_channel_candidates(&[], "needs_sign_in", None).unwrap();
    assert_eq!(c.channel_discovery()["phase"], "needs_sign_in");
}
fn entry(c: &Core, id: &str, protected: bool) -> String {
    let mut e = EntryRow::new(id, "360", "360", "manual", c.now());
    let path = c.paths.video_dir.join(format!("{}.mp4", e.id));
    std::fs::write(&path, b"test media").unwrap();
    e.file_path = Some(path.to_string_lossy().into_owned());
    e.status = STATUS_COMPLETE.into();
    e.protected = protected;
    c.video.insert_entry(&e).unwrap();
    e.id
}
#[test]
fn completion_requires_ended_and_respects_grace_and_keep() {
    let (_d, c) = core();
    let id = entry(&c, "aaaaaaaaaaa", false);
    let kept = entry(&c, "bbbbbbbbbbb", true);
    c.ingest_playback(PlaybackIngest {
        video_id: "aaaaaaaaaaa".into(),
        position_seconds: 100.0,
        duration_seconds: Some(100.0),
        ..Default::default()
    })
    .unwrap();
    c.cleanup_completed().unwrap();
    assert!(
        c.video.get_entry(&id).unwrap().is_some(),
        "seeking to end must not delete"
    );
    for v in ["aaaaaaaaaaa", "bbbbbbbbbbb"] {
        c.ingest_playback(PlaybackIngest {
            video_id: v.into(),
            position_seconds: 100.0,
            duration_seconds: Some(100.0),
            ended: true,
            ..Default::default()
        })
        .unwrap();
    }
    c.cleanup_completed().unwrap();
    assert!(c.video.get_entry(&id).unwrap().is_some());
    c.set_now_for_tests(Some(1_000_000 + 86_400_001));
    c.cleanup_completed().unwrap();
    assert!(c.video.get_entry(&id).unwrap().is_none());
    assert!(c.video.get_entry(&kept).unwrap().is_some());
}
#[test]
fn resume_cancels_cleanup_and_keep_policy_promotes() {
    let (_d, c) = core();
    let id = entry(&c, "aaaaaaaaaaa", false);
    c.mark_completed("aaaaaaaaaaa").unwrap();
    c.resume_cleanup("aaaaaaaaaaa").unwrap();
    c.set_now_for_tests(Some(2_000_000));
    c.cleanup_completed().unwrap();
    assert!(c.video.get_entry(&id).unwrap().is_some());
    c.put_automation(&Automation {
        cleanup_delay_seconds: 0,
        completion_rules: vec![CompletionRule {
            name: "Keep everything".into(),
            matcher: Default::default(),
            action: CompletionAction::Keep,
        }],
        ..Default::default()
    })
    .unwrap();
    c.mark_completed("aaaaaaaaaaa").unwrap();
    c.cleanup_completed().unwrap();
    assert!(c.video.get_entry(&id).unwrap().unwrap().protected);
}

#[test]
fn mostly_played_waits_one_day_and_does_not_keep_resetting_the_deadline() {
    let (_d, c) = core();
    let id = entry(&c, "aaaaaaaaaaa", false);
    let kept = entry(&c, "bbbbbbbbbbb", true);
    let report = |video: &str, pos: f64| {
        c.ingest_playback(PlaybackIngest {
            video_id: video.into(),
            position_seconds: pos,
            duration_seconds: Some(100.0),
            paused: Some(false),
            ..Default::default()
        })
        .unwrap()
    };
    report("aaaaaaaaaaa", 94.0);
    assert_eq!(c.cleanup_preview().unwrap().as_array().unwrap().len(), 0);
    report("aaaaaaaaaaa", 95.0);
    report("bbbbbbbbbbb", 95.0);
    let deadline = c.cleanup_preview().unwrap()[0]["due_at"].as_i64().unwrap();
    c.set_now_for_tests(Some(1_100_000));
    report("aaaaaaaaaaa", 97.0);
    assert_eq!(c.cleanup_preview().unwrap()[0]["due_at"], deadline);
    c.set_now_for_tests(Some(deadline - 1));
    c.cleanup_completed().unwrap();
    assert!(c.video.get_entry(&id).unwrap().is_some());
    c.set_now_for_tests(Some(deadline + 1));
    c.cleanup_completed().unwrap();
    assert!(c.video.get_entry(&id).unwrap().is_none());
    assert!(c.video.get_entry(&kept).unwrap().is_some());
}

#[test]
fn retention_upgrade_changes_old_defaults_but_preserves_custom_policies() {
    let (d, c) = core();
    let mut rules = c.video.rules();
    rules
        .iter_mut()
        .find(|r| r.id == "seed-everything-else")
        .unwrap()
        .retention_days = 3;
    rules
        .iter_mut()
        .find(|r| r.id == "seed-music")
        .unwrap()
        .retention_days = 11;
    c.video.put_rules(&rules).unwrap();
    let id = entry(&c, "aaaaaaaaaaa", false);
    c.video
        .set_expiry(
            &id,
            Some(c.now() + 3 * 86_400_000),
            Some(3),
            Some("Everything else"),
        )
        .unwrap();
    c.video
        .kv_put(
            "automation",
            r#"{"cleanup_completed":true,"cleanup_delay_seconds":60,"completion_rules":[]}"#,
        )
        .unwrap();
    let conn = rusqlite::Connection::open(d.path().join("db/video.sqlite")).unwrap();
    conn.execute("DELETE FROM kv WHERE key='watch-retention-v1'", [])
        .unwrap();
    drop(conn);
    drop(c);
    let c = Core::open(CoreConfig {
        data_dir: d.path().into(),
        tool_dirs: Some(vec![]),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(c.video.get_entry(&id).unwrap().unwrap().expires_at, None);
    assert_eq!(
        c.video
            .rules()
            .iter()
            .find(|r| r.id == "seed-everything-else")
            .unwrap()
            .retention_days,
        0
    );
    assert_eq!(
        c.video
            .rules()
            .iter()
            .find(|r| r.id == "seed-music")
            .unwrap()
            .retention_days,
        11
    );
    assert_eq!(c.automation().cleanup_delay_seconds, 86400);
    assert_eq!(c.automation().completion_percent, 95);
}
#[test]
fn imports_preserve_source_metadata_and_reject_duplicates_and_dry_run() {
    let (d, c) = core();
    let file = d.path().join("old video.mp4");
    std::fs::write(&file, b"video").unwrap();
    let sidecar = file.with_extension("info.json");
    std::fs::write(&sidecar,json!({"id":"aaaaaaaaaaa","title":"Real title","channel":"Real channel","height":360,"duration":123}).to_string()).unwrap();
    let req = |dry_run, move_file| MediaImport {
        path: file.clone(),
        video_id: None,
        metadata: None,
        dry_run,
        move_file,
        enrich: false,
    };
    assert_eq!(c.import_media(req(true, false)).unwrap()["dry_run"], true);
    assert!(c.video.list_entries().unwrap().is_empty());
    let result = c.import_media(req(false, false)).unwrap();
    assert_eq!(result["entry"]["title"], "Real title");
    assert_eq!(result["entry"]["protected"], true);
    assert!(file.exists());
    assert!(c.import_media(req(false, true)).is_err());
    assert!(file.exists());
    let row = c.video.get_entry("aaaaaaaaaaa.360").unwrap().unwrap();
    assert_eq!(std::fs::read(row.file_path.unwrap()).unwrap(), b"video");
}
#[test]
fn move_import_copies_before_removing_source() {
    let (d, c) = core();
    let file = d.path().join("old.mp4");
    std::fs::write(&file, b"video").unwrap();
    c.import_media(MediaImport {
        path: file.clone(),
        video_id: Some("aaaaaaaaaaa".into()),
        metadata: None,
        dry_run: false,
        move_file: true,
        enrich: false,
    })
    .unwrap();
    assert!(!file.exists());
    assert!(
        c.video
            .get_entry("aaaaaaaaaaa.720")
            .unwrap()
            .unwrap()
            .protected
    );
}
#[test]
fn tab_import_is_validated_deduplicated_and_asleep() {
    let (_d, c) = core();
    let url = "https://www.youtube.com/watch?v=aaaaaaaaaaa".to_string();
    let req = |urls| TabsImport {
        urls,
        dry_run: false,
    };
    assert!(c
        .import_tabs(req(vec![url.clone(), "https://evil.example/".into()]))
        .is_err());
    assert!(c.browsing.tabs().unwrap().is_empty());
    assert_eq!(
        c.import_tabs(req(vec![url.clone(), url.clone()])).unwrap()["count"],
        1
    );
    assert_eq!(c.import_tabs(req(vec![url])).unwrap()["count"], 0);
    assert_eq!(c.browsing.tabs().unwrap()[0].state, "hidden");
    assert!(c.video.list_entries().unwrap().is_empty());
}
#[test]
fn logs_are_persistent_and_bounded() {
    let (_d, c) = core();
    for n in 0..5 {
        c.log_event("info", "test", json!({"n":n}));
    }
    let log = c.logs(2);
    assert_eq!(log["events"].as_array().unwrap().len(), 2);
    assert_eq!(log["events"][0]["data"]["n"], 3);
    assert!(
        !std::fs::read_to_string(c.paths.data_dir.join("logs/muninn.jsonl"))
            .unwrap()
            .contains(&c.token())
    );
}

#[test]
fn channel_poller_deduplicates_and_does_not_redownload_after_eviction() {
    use muninn_core::quality::Quality;
    use muninn_core::subscriptions::Channel;
    struct Feed;
    impl muninn_core::ytdlp::Fetcher for Feed {
        fn get(&self, _: &str, _: u64) -> muninn_core::Result<Vec<u8>> {
            Ok(b"<feed><entry><yt:videoId>aaaaaaaaaaa</yt:videoId><title>Title &amp; more</title><author><name>Channel</name></author></entry></feed>".to_vec())
        }
    }
    let d = tempfile::tempdir().unwrap();
    let c = Core::open(CoreConfig {
        data_dir: d.path().into(),
        fetcher: Some(std::sync::Arc::new(Feed)),
        tool_dirs: Some(vec![]),
        ..Default::default()
    })
    .unwrap();
    c.set_now_for_tests(Some(1_000_000));
    c.put_subscriptions(&[Channel {
        name: "Test channel".into(),
        channel_id: "UCabcdefghijklmnopqrstuv".into(),
        quality: Quality::P480,
        retention_days: 7,
        keep: false,
        enabled: true,
        poll_minutes: 5,
        max_per_poll: 3,
    }])
    .unwrap();
    c.poll_channels().unwrap();
    let entries = c.video.list_entries().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].title.as_deref(), Some("Title & more"));
    c.evict(&entries[0].id, false).unwrap();
    c.set_now_for_tests(Some(2_000_000));
    c.poll_channels().unwrap();
    assert!(c.video.list_entries().unwrap().is_empty());
}

#[test]
fn cli_uses_running_api_for_config_imports_and_nonzero_errors() {
    use std::process::Command;
    let (d, c) = core();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    c.set_port(listener.local_addr().unwrap().port());
    let app = muninn_core::server::router(c.clone());
    runtime.spawn(async move {
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        axum::serve(listener, app).await.unwrap();
    });
    let cli = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_muninn-cli"))
            .arg("--data-dir")
            .arg(d.path())
            .args(args)
            .output()
            .unwrap()
    };
    let status = cli(&["status"]);
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let config = d.path().join("policy.json");
    std::fs::write(&config, r#"{"cleanup_completed":false}"#).unwrap();
    let saved = cli(&["automation", "set", config.to_str().unwrap()]);
    assert!(saved.status.success());
    assert!(!c.automation().cleanup_completed);
    let imported = cli(&["open", "https://www.youtube.com/watch?v=aaaaaaaaaaa"]);
    assert!(imported.status.success());
    assert_eq!(c.browsing.tabs().unwrap().len(), 1);
    let error = cli(&["keep", "aaaaaaaaaaa.360"]);
    assert!(!error.status.success());
    assert!(String::from_utf8_lossy(&error.stderr).contains("404"));
    assert_eq!(c.logs(1)["events"][0]["event"], "api.failed");
    for args in [
        vec!["group", "create", "Research"],
        vec!["group", "assign", "Research", "1", "2"],
        vec!["group", "collapse", "Research"],
        vec!["collection", "create", "Watch later"],
        vec!["collection", "add", "Watch later", "aaaaaaaaaaa"],
        vec!["tag", "create", "Music"],
        vec!["tag", "add", "Music", "aaaaaaaaaaa"],
        vec!["tag", "rename", "Music", "Audio"],
    ] {
        let result = cli(&args);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    assert_eq!(c.organization().tags["Audio"], vec!["aaaaaaaaaaa"]);
    assert!(c.organization().tab_groups["Research"].collapsed);
    let invalid = cli(&["typo"]);
    assert!(!invalid.status.success());
}

#[test]
fn theatre_player_state_and_hls_requests_are_in_the_diagnostic_logs() {
    let (_d,c)=core();
    let runtime=tokio::runtime::Runtime::new().unwrap();
    let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base=format!("http://{}",listener.local_addr().unwrap());
    let app=muninn_core::server::router(c.clone());
    runtime.spawn(async move{axum::serve(tokio::net::TcpListener::from_std(listener).unwrap(),app).await.unwrap();});
    for event in ["ui.player.theatre","ui.playback.state"] {
        let response=ureq::post(&format!("{base}/api/logs")).set("Authorization",&format!("Bearer {}",c.token())).set("Content-Type","application/json").send_string(&json!({"event":event,"message":"regression"}).to_string()).unwrap();
        assert_eq!(response.status(),200);
    }
    let dir=c.paths.cache_dir.join("hls/dQw4w9WgXcQ.720.local");std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("index.m3u8"),"#EXTM3U\n#EXT-X-ENDLIST\n").unwrap();
    let response=ureq::get(&format!("{base}/h/{}/dQw4w9WgXcQ.720.local/index.m3u8",c.token())).call().unwrap();
    assert_eq!(response.status(),200);
    let logs=c.logs(20);let events=logs["events"].as_array().unwrap();
    for event in ["ui.player.theatre","ui.playback.state","hls.served"]{assert!(events.iter().any(|e|e["event"]==event),"missing {event}");}
    assert!(!logs.to_string().contains(&c.token()));
}

#[test]
fn recorded_hls_http_start_hint_is_present_for_both_conversion_and_download_playlists() {
    let (_d,c)=core();let runtime=tokio::runtime::Runtime::new().unwrap();let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap();listener.set_nonblocking(true).unwrap();
    let base=format!("http://{}",listener.local_addr().unwrap());let app=muninn_core::server::router(c.clone());
    runtime.spawn(async move{axum::serve(tokio::net::TcpListener::from_std(listener).unwrap(),app).await.unwrap();});
    let raw="#EXTM3U\n#EXT-X-PLAYLIST-TYPE:EVENT\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:3.0,\nseg_00000.m4s\n";
    for name in ["dQw4w9WgXcQ.720","dQw4w9WgXcQ.720.local"] {
        let dir=c.paths.cache_dir.join("hls").join(name);std::fs::create_dir_all(&dir).unwrap();std::fs::write(dir.join("index.m3u8"),raw).unwrap();
        let url=format!("{base}/h/{}/{name}/index.m3u8",c.token());
        let response=ureq::get(&url).call().unwrap();assert_eq!(response.header("Cache-Control"),Some("no-store"));assert_eq!(response.header("Content-Type"),Some("application/vnd.apple.mpegurl"));
        let length=response.header("Content-Length").unwrap().parse::<usize>().unwrap();let body=response.into_string().unwrap();assert!(body.contains("#EXT-X-START:TIME-OFFSET=0,PRECISE=YES"));assert!(body.contains("seg_00000.m4s"));assert_eq!(body.len(),length);
        let head=ureq::head(&url).call().unwrap();assert_eq!(head.header("Content-Length").unwrap().parse::<usize>().unwrap(),length);assert_eq!(head.into_string().unwrap(),"");
        assert_eq!(std::fs::read_to_string(dir.join("index.m3u8")).unwrap(),raw,"serving must not race ffmpeg by editing its playlist");
    }
}

#[test]
fn cleanup_removes_hls_duplicate() {
    let (_d, c) = core();
    let id = entry(&c, "aaaaaaaaaaa", false);
    c.put_automation(&Automation {
        cleanup_delay_seconds: 0,
        ..Default::default()
    })
    .unwrap();
    c.mark_completed("aaaaaaaaaaa").unwrap();
    let hls = muninn_core::stream::hls_dir_for(&c.paths.cache_dir, "aaaaaaaaaaa", "360");
    std::fs::create_dir_all(&hls).unwrap();
    std::fs::write(hls.join("seg_00001.m4s"), b"segment").unwrap();
    c.streams.jobs.lock().unwrap().insert(
        "aaaaaaaaaaa".into(),
        std::sync::Arc::new(muninn_core::stream::StreamJob {
            video_id: "aaaaaaaaaaa".into(),
            state: std::sync::Mutex::new(Default::default()),
            hls_dir: hls.clone(),
        }),
    );
    c.cleanup_completed().unwrap();
    assert!(c.video.get_entry(&id).unwrap().is_none());
    assert!(!hls.exists());
    assert!(c.streams.jobs.lock().unwrap().is_empty());
}

#[test]
fn automatic_eviction_preserves_the_playing_video_even_after_expiry() {
    let (_d, c) = core();
    let id = entry(&c, "aaaaaaaaaaa", false);
    c.video
        .set_expiry(&id, Some(c.now() - 1), None, None)
        .unwrap();
    c.ingest_playback(PlaybackIngest {
        video_id: "aaaaaaaaaaa".into(),
        position_seconds: 50.0,
        duration_seconds: Some(100.0),
        paused: Some(false),
        ..Default::default()
    })
    .unwrap();
    c.enforce_caps(0).unwrap();
    assert!(c.video.get_entry(&id).unwrap().is_some());
    c.set_now_for_tests(Some(2_000_000));
    c.enforce_caps(0).unwrap();
    assert!(c.video.get_entry(&id).unwrap().is_none());
}

#[test]
fn releasing_a_kept_import_moves_the_managed_copy_and_sets_expiry() {
    let (_d, c) = core();
    let id = entry(&c, "aaaaaaaaaaa", false);
    c.promote(&id).unwrap();
    let row = c.release(&id, 7).unwrap();
    assert!(!row.protected);
    assert_eq!(row.expires_at, Some(c.now() + 7 * 86400 * 1000));
    assert!(std::path::Path::new(row.file_path.as_ref().unwrap()).starts_with(&c.paths.video_dir));
    assert!(c.release(&id, 3651).is_err());
}

#[test]
fn organization_moves_tabs_and_renames_collections_without_touching_media() {
    let (_d, c) = core();
    let id = entry(&c, "aaaaaaaaaaa", true);
    let change = |value| {
        c.change_organization(serde_json::from_value(value).unwrap())
            .unwrap()
    };
    for name in ["Work", "Music"] {
        change(json!({"kind":"tab_group","action":"create","name":name}));
    }
    change(json!({"kind":"tab_group","action":"assign","name":"Work","ids":["1","2"]}));
    change(json!({"kind":"tab_group","action":"assign","name":"Music","ids":["2"]}));
    assert_eq!(c.organization().tab_groups["Work"].tab_ids, vec!["1"]);
    change(json!({"kind":"tab_group","action":"collapse","name":"Music","collapsed":true}));
    assert!(c.organization().tab_groups["Music"].collapsed);
    for name in ["Watch later", "Favorites"] {
        change(json!({"kind":"collection","action":"create","name":name}));
        change(json!({"kind":"collection","action":"add","name":name,"ids":["aaaaaaaaaaa"]}));
    }
    change(json!({"kind":"collection","action":"rename","name":"Favorites","new_name":"Keepers"}));
    assert_eq!(c.organization().collections["Keepers"], vec!["aaaaaaaaaaa"]);
    assert!(c
        .change_organization(
            serde_json::from_value(
                json!({"kind":"collection","action":"add","name":"Keepers","ids":["invalid"]})
            )
            .unwrap()
        )
        .is_err());
    assert_eq!(
        c.organization().collections["Keepers"].len(),
        1,
        "failed changes must be atomic"
    );
    change(json!({"kind":"collection","action":"delete","name":"Keepers"}));
    assert!(c.video.get_entry(&id).unwrap().is_some());
}

#[test]
fn reply_links_resolve_entity_comments_without_using_the_entity_key_as_comment_id() {
    let page = json!({"contents":[{"commentThreadRenderer":{"commentViewModel":{"commentViewModel":{"commentKey":"entity-parent"}},
        "replies":{"commentRepliesRenderer":{"contents":[{"continuationItemRenderer":{"continuationEndpoint":{"continuationCommand":{"token":"REPLIES"}}}}]}}}}],
        "frameworkUpdates":{"entityBatchUpdate":{"mutations":[{"entityKey":"entity-parent","payload":{"commentEntityPayload":{"properties":{"commentId":"real-comment-id"}}}}]}}});
    assert_eq!(
        muninn_core::extract::comment_reply_tokens(&page),
        vec![("real-comment-id".into(), "REPLIES".into())]
    );
}
