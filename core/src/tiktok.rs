//! TikTok is a first-class capture adapter: it produces the same Item,
//! VideoIngest, feed snapshots and cache IDs as the YouTube adapter. Browsing
//! responses are never used as media streams; yt-dlp downloads a separate copy.
use crate::{capture::{Capture, CaptureInput}, engine::Core, model::{FeedIngest, Item, VideoIngest}};
use serde_json::Value;
use std::collections::HashSet;

const MAX_ITEMS: usize = 500;
const MAX_DEPTH: usize = 35;

pub fn is_tiktok_url(url: &str) -> bool {
    let Some(u) = crate::extract::parse_url(url) else { return false };
    u.scheme == "https" && u.port.is_none()
        && matches!(u.host.as_str(), "tiktok.com" | "www.tiktok.com" | "m.tiktok.com")
}

pub fn video_id_of_url(url: &str) -> Option<String> {
    if !is_tiktok_url(url) { return None; }
    let u = crate::extract::parse_url(url)?;
    let p: Vec<&str> = u.path.split('/').filter(|s| !s.is_empty()).collect();
    let n = match p.as_slice() {
        [user, "video", id] if user.starts_with('@') => *id,
        ["embed", id] | ["share", "video", id] => *id,
        _ => return None,
    };
    id_of(n)
}

fn id_of(n: &str) -> Option<String> {
    let id = format!("tt_{n}");
    crate::util::valid_video_id(&id).then_some(id)
}

pub fn capturable(url: &str, html: bool) -> bool {
    if !is_tiktok_url(url) { return false; }
    let Some(u) = crate::extract::parse_url(url) else { return false };
    if html { return !["/embed/", "/login", "/passport"].iter().any(|p| u.path.starts_with(p)); }
    ["/api/recommend/item_list", "/api/post/item_list", "/api/following/item_list",
        "/api/search/general/full/", "/api/search/item/", "/api/related/item_list"]
        .iter().any(|p| u.path.starts_with(p))
}

fn text(v: &Value) -> Option<String> {
    v.as_str().filter(|s| !s.trim().is_empty()).map(|s| s.chars().take(500).collect())
}

fn number_id(v: &Value) -> Option<String> {
    v.as_str().map(str::to_owned).or_else(|| v.as_u64().map(|n| n.to_string())).and_then(|n| id_of(&n))
}

fn first_string(value: &Value, paths: &[&[&str]]) -> Option<String> {
    paths.iter().find_map(|path| {
        let at = path.iter().fold(value, |v, p| v.get(*p).unwrap_or(&Value::Null));
        text(at)
    })
}

fn cover(value: &Value) -> Option<String> {
    for key in ["cover", "originCover", "dynamicCover", "thumbnail"] {
        let v = &value["video"][key];
        let candidate = v["urlList"].as_array().and_then(|a| a.first()).and_then(text)
            .or_else(|| text(v));
        if let Some(url) = candidate.filter(|u| u.starts_with("https://") && u.len() < 2000) {
            return Some(url);
        }
    }
    None
}

fn item(value: &Value) -> Option<(Item, VideoIngest)> {
    if !value.get("video").is_some_and(Value::is_object) { return None; }
    let id = number_id(&value["id"]).or_else(|| number_id(&value["aweme_id"]))?;
    let handle = first_string(value, &[&["author", "uniqueId"], &["author", "unique_id"], &["author", "unique_id_str"]]);
    let handle = handle.filter(|h| (1..=30).contains(&h.len()) && h.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-')));
    let channel = first_string(value, &[&["author", "nickname"], &["author", "uniqueId"], &["author", "unique_id"]]);
    let title = first_string(value, &[&["desc"], &["description"], &["title"]])
        .unwrap_or_else(|| format!("TikTok by {}", handle.as_deref().or(channel.as_deref()).unwrap_or("unknown creator")));
    let duration = value["video"]["duration"].as_u64().and_then(|n| u32::try_from(n).ok());
    let thumbnail = cover(value);
    let author_id = first_string(value, &[&["author", "id"], &["author", "secUid"]]);
    let public = Item {
        video_id: id.clone(), title: title.clone(), channel: channel.clone(), channel_id: author_id.clone(),
        creator_handle: handle.clone(),
        duration_seconds: duration, thumbnail: thumbnail.clone(),
        view_text: value["stats"]["playCount"].as_u64().map(|n| format!("{n} views")),
        ..Item::default()
    };
    let detail = VideoIngest {
        video_id: id, title, channel, channel_id: author_id, duration_seconds: duration,
        thumbnail, description: first_string(value, &[&["desc"], &["description"]]),
        view_count: value["stats"]["playCount"].as_u64(), is_live: Some(false),
        ..VideoIngest::default()
    };
    Some((public, detail))
}

fn collect(v: &Value, depth: usize, seen: &mut HashSet<String>, items: &mut Vec<Item>, videos: &mut Vec<VideoIngest>) {
    if depth >= MAX_DEPTH || items.len() >= MAX_ITEMS { return; }
    if let Some((it, meta)) = item(v) {
        if seen.insert(it.video_id.clone()) { items.push(it); videos.push(meta); }
        return;
    }
    match v {
        Value::Array(values) => for child in values.iter().take(1000) {
            collect(child, depth + 1, seen, items, videos);
            if items.len() >= MAX_ITEMS { break; }
        },
        Value::Object(values) => for child in values.values().take(1000) {
            collect(child, depth + 1, seen, items, videos);
            if items.len() >= MAX_ITEMS { break; }
        },
        _ => {},
    }
}

fn html_json(body: &str) -> Option<Value> {
    for needle in ["__UNIVERSAL_DATA_FOR_REHYDRATION__", "SIGI_STATE", "sigi-persisted-data"] {
        let Some(start) = body.find(needle) else { continue };
        let Some(tag_end) = body[start..].find('>').map(|i| i + start + 1) else { continue };
        let Some(end) = body[tag_end..].find("</script>").map(|i| i + tag_end) else { continue };
        if end.saturating_sub(tag_end) <= 4 * 1024 * 1024 {
            if let Ok(json) = serde_json::from_str(&body[tag_end..end]) { return Some(json); }
        }
    }
    None
}

fn feed_kind(url: &str) -> &'static str {
    let path = crate::extract::parse_url(url).map(|u| u.path).unwrap_or_default();
    if path.starts_with("/api/post/item_list") || path.starts_with("/@") { "tiktok-profile" }
    else if path.starts_with("/api/following/item_list") || path.starts_with("/following") { "tiktok-following" }
    else if path.starts_with("/api/search/") || path.starts_with("/search") { "tiktok-search" }
    else if path.starts_with("/api/related/") { "tiktok-related" }
    else { "tiktok-for-you" }
}

pub(crate) fn capture(core: &Core, input: &CaptureInput<'_>, cap: &mut Capture, body: &[u8]) -> Result<(), String> {
    let is_html = input.content_type.is_some_and(|s| s.to_ascii_lowercase().contains("text/html"));
    if !capturable(input.url, is_html) { return Ok(()); }
    let data = if is_html { html_json(&String::from_utf8_lossy(body)) } else { serde_json::from_slice(body).ok() };
    let Some(data) = data else { return Ok(()); };
    let mut items = Vec::new();
    let mut videos = Vec::new();
    collect(&data, 0, &mut HashSet::new(), &mut items, &mut videos);
    cap.what = if is_html { "html" } else { "tiktok-api" }.into();
    if items.is_empty() { return Ok(()); }
    let source = input.referer.filter(|u| is_tiktok_url(u)).unwrap_or(input.url);
    let path_id = video_id_of_url(source);
    if let Some(meta) = videos.into_iter().find(|v| Some(&v.video_id) == path_id.as_ref()) {
        cap.video_id = Some(meta.video_id.clone());
        core.ingest_video(meta).map_err(|e| e.to_string())?;
    }
    // A watch page is metadata for a single video, not an invented feed.
    if is_html && path_id.is_some() { cap.kind = Some("tiktok-video".into());cap.items = items.len();return Ok(()); }
    // The referer identifies the selected video, not the API's purpose.
    // In particular, /@creator/video/id is NOT a creator feed when the
    // response came from the related-video endpoint.
    let kind = if !is_html && feed_kind(input.url) == "tiktok-related" {
        let Some(parent) = path_id.as_ref() else {
            // Never publish recommendations without a known parent video.
            return Ok(());
        };
        items.retain(|item| &item.video_id != parent);
        "tiktok-related"
    } else {
        feed_kind(source)
    };
    if items.is_empty() { return Ok(()); }
    let item_count = items.len();
    let saved = core.ingest_feed(FeedIngest { kind: kind.into(), source_url: Some(source.chars().take(500).collect()),
        captured_at: Some(core.now()), items, ..FeedIngest::default() }).map_err(|e| e.to_string())?;
    cap.kind = Some(kind.into());
    cap.items = item_count;
    cap.snapshot_id = saved["snapshot_id"].as_i64();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{capture::CaptureInput, engine::testutil::test_core};
    use serde_json::json;

    #[test]
    fn api_and_hydration_use_native_cache_and_feed_snapshots() {
        let (_temp, core) = test_core(None, None);
        let video = json!({"id":"7668090902816017671","desc":"A caption", "author":{"uniqueId":"maker","nickname":"Maker"},
            "video":{"duration":15,"cover":{"urlList":["https://p16.tiktokcdn.com/cover.jpg"]}},"stats":{"playCount":123}});
        let body = json!({"itemList":[video.clone(),video.clone()]}).to_string();
        let result = core.capture(CaptureInput { url:"https://www.tiktok.com/api/recommend/item_list/?count=20",
            referer:Some("https://www.tiktok.com/foryou"),request_body:None,body:body.as_bytes(),
            content_type:Some("application/json"),content_encoding:None });
        assert_eq!(result.kind.as_deref(),Some("tiktok-for-you"));
        let snapshot = core.content.latest_snapshot("tiktok-for-you").unwrap().unwrap();
        assert_eq!(snapshot.items[0].video_id,"tt_7668090902816017671");
        assert_eq!(snapshot.items[0].creator_handle.as_deref(), Some("maker"));
        let html = format!("<script id=\"__UNIVERSAL_DATA_FOR_REHYDRATION__\" type=\"application/json\">{}</script>",json!({"__DEFAULT_SCOPE__":{"webapp.video-detail":{"itemInfo":{"itemStruct":video}}}}));
        let result = core.capture(CaptureInput {url:"https://www.tiktok.com/@maker/video/7668090902816017671",referer:None,
            request_body:None,body:html.as_bytes(),content_type:Some("text/html"),content_encoding:None});
        assert_eq!(result.video_id.as_deref(),Some("tt_7668090902816017671"));
        assert_eq!(core.facts_for("tt_7668090902816017671").title.as_deref(),Some("A caption"));
    }

    #[test]
    fn related_capture_preserves_watch_context_without_polluting_creator_feed() {
        let (_temp, core) = test_core(None, None);
        let watch = "https://www.tiktok.com/@maker/video/7668090902816017671";
        let endpoint = "https://www.tiktok.com/api/related/item_list";
        let body = json!({"itemList":[
            {"id":"7668090902816017671","desc":"Current","video":{"duration":10}},
            {"id":"7668090902816017672","desc":"Related","video":{"duration":11}}
        ]}).to_string();
        let result = core.capture(CaptureInput {url:endpoint,referer:Some(watch),request_body:None,
            body:body.as_bytes(),content_type:Some("application/json"),content_encoding:None});
        assert_eq!(result.kind.as_deref(),Some("tiktok-related"));
        let snapshot = core.content.latest_snapshot("tiktok-related").unwrap().unwrap();
        assert_eq!(snapshot.meta.source_url.as_deref(),Some(watch));
        assert_eq!(snapshot.items.len(),1);
        assert_eq!(snapshot.items[0].video_id,"tt_7668090902816017672");
        assert!(core.content.latest_snapshot("tiktok-profile").unwrap().is_none());

        let unattributed = core.capture(CaptureInput {url:endpoint,referer:None,request_body:None,
            body:body.as_bytes(),content_type:Some("application/json"),content_encoding:None});
        assert!(unattributed.snapshot_id.is_none());
        assert_eq!(core.content.latest_snapshot("tiktok-related").unwrap().unwrap().meta.id,snapshot.meta.id);
    }

    #[test]
    fn only_provider_data_and_safe_ids_are_accepted() {
        assert!(!is_tiktok_url("https://www.tiktok.com.evil.test/@a/video/7668090902816017671"));
        assert!(video_id_of_url("https://www.tiktok.com/@maker/video/7668090902816017671").is_some());
        assert!(video_id_of_url("https://www.tiktok.com/@maker/video/../../etc").is_none());
        assert!(!capturable("https://www.tiktok.com/api/login", false));
        assert!(!capturable("https://www.tiktok.com/@maker", false));
    }
}
