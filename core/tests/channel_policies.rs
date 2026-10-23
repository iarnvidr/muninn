use muninn_core::channel_groups::ChannelGroup;
use muninn_core::quality::Quality;
use muninn_core::subscriptions::Channel;
use muninn_core::{Core, CoreConfig};
use serde_json::json;

fn group(ids: Vec<String>) -> ChannelGroup {
    ChannelGroup {
        favourite: false,
        name: "Learning".into(),
        channel_ids: ids,
        quality: Quality::P1080,
        retention_days: 0,
        keep: false,
        enabled: true,
        poll_minutes: 60,
        max_per_poll: 3,
    }
}

#[test]
fn group_policy_is_inherited_channel_override_wins_and_membership_is_unambiguous() {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::open(CoreConfig {
        data_dir: dir.path().into(),
        tool_dirs: Some(vec![]),
        ..Default::default()
    })
    .unwrap();
    let first = "UCabcdefghijklmnopqrstuv".to_string();
    let second = "UCzyxwvutsrqponmlkjihgfe".to_string();
    let mut g = group(vec![first.clone(), second.clone()]);
    core.put_channel_groups(&[g.clone()]).unwrap();
    assert!(core
        .effective_subscriptions()
        .iter()
        .all(|c| c.quality == Quality::P1080 && c.enabled));
    core.put_subscriptions(&[Channel {
        channel_id: first.clone(),
        name: "First".into(),
        quality: Quality::P360,
        retention_days: 7,
        keep: false,
        enabled: false,
        poll_minutes: 5,
        max_per_poll: 1,
    }])
    .unwrap();
    g.quality = Quality::P720;
    core.put_channel_groups(&[g.clone()]).unwrap();
    let policies = core.effective_subscriptions();
    assert_eq!(
        policies
            .iter()
            .find(|c| c.channel_id == first)
            .unwrap()
            .quality,
        Quality::P360
    );
    assert!(
        !policies
            .iter()
            .find(|c| c.channel_id == first)
            .unwrap()
            .enabled
    );
    assert_eq!(
        policies
            .iter()
            .find(|c| c.channel_id == second)
            .unwrap()
            .quality,
        Quality::P720
    );
    let mut duplicate = g.clone();
    duplicate.name = "Music".into();
    assert!(core.put_channel_groups(&[g.clone(), duplicate]).is_err());
    drop(core);
    let core = Core::open(CoreConfig {
        data_dir: dir.path().into(),
        tool_dirs: Some(vec![]),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(core.channel_groups()[0].quality, Quality::P720);
    core.put_subscriptions(&[]).unwrap();
    assert!(core
        .effective_subscriptions()
        .iter()
        .all(|c| c.quality == Quality::P720));
}

#[test]
fn group_members_reach_the_scheduler_at_1080p() {
    struct Feed;
    impl muninn_core::ytdlp::Fetcher for Feed {
        fn get(&self, _: &str, _: u64) -> muninn_core::Result<Vec<u8>> {
            Ok(b"<feed><entry><yt:videoId>aaaaaaaaaaa</yt:videoId><title>New upload</title></entry></feed>".to_vec())
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("tools");
    std::fs::create_dir_all(&bin).unwrap();
    let ffmpeg = bin.join("ffmpeg");
    std::fs::write(&ffmpeg, "#!/bin/sh\necho ffmpeg-test\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&ffmpeg, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let core = Core::open(CoreConfig {
        data_dir: dir.path().join("data"),
        tool_dirs: Some(vec![bin]),
        fetcher: Some(std::sync::Arc::new(Feed)),
        ..Default::default()
    })
    .unwrap();
    core.set_now_for_tests(Some(10_000_000));
    core.put_channel_groups(&[group(vec!["UCabcdefghijklmnopqrstuv".into()])])
        .unwrap();
    core.poll_channels().unwrap();
    let entries = core.video.list_entries().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].variant, "1080");
    let plan = muninn_core::ytdlp::plan_format(Quality::P1080, true).unwrap();
    assert!(plan.selector.contains("height<=1080"));
}

#[test]
fn inline_catalogue_parses_uploads_and_matches_local_channel_identity() {
    let data = json!({"channel":"Creator","entries":[{"id":"aaaaaaaaaaa","title":"One","duration":125},{"id":"bad","title":"Skip"},{"id":"bbbbbbbbbbb","title":"Two"}]});
    let uploads = muninn_core::channel_catalog::upload_items(&data, "UCabcdefghijklmnopqrstuv");
    assert_eq!(uploads.len(), 2);
    assert_eq!(uploads[0]["duration_seconds"], 125.0);
    let dir = tempfile::tempdir().unwrap();
    let core = Core::open(CoreConfig {
        data_dir: dir.path().join("data"),
        tool_dirs: Some(vec![]),
        ..Default::default()
    })
    .unwrap();
    let source = dir.path().join("video.mp4");
    std::fs::write(&source, b"local original").unwrap();
    let meta = dir.path().join("video.info.json");
    std::fs::write(&meta,json!({"id":"aaaaaaaaaaa","title":"Local copy","channel_id":"UCabcdefghijklmnopqrstuv","channel":"Creator"}).to_string()).unwrap();
    core.import_media(muninn_core::imports::MediaImport {
        path: source,
        metadata: Some(meta),
        video_id: None,
        enrich: false,
        move_file: false,
        dry_run: false,
    })
    .unwrap();
    assert_eq!(
        core.channel_library("UCabcdefghijklmnopqrstuv").unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        core.channel_library("UCzyxwvutsrqponmlkjihgfe").unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(core.channel_library("bad").is_err());
}
