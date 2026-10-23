//! What the proxy does with a decrypted YouTube response: classify it, extract
//! the videos / metadata / comments with the defensive parsers in `extract`, and
//! store them in the content cache. Never fails the page: every problem is
//! reported in the returned [`Capture`] and the caller just logs it.

use crate::content::{KIND_COMMENTS, KIND_META};
use crate::engine::Core;
use crate::extract::{self, Kind};
use crate::merge::merge_video_meta;
use crate::model::*;
use crate::util::valid_video_id;
use serde::Serialize;
use serde_json::Value;
use std::io::Read;

#[derive(Debug, Clone, Default, Serialize)]
pub struct Capture {
    pub ts: i64,
    pub url: String,
    /// "browse", "search", "next", "player", "html" or "ignored".
    pub what: String,
    pub kind: Option<String>,
    pub items: usize,
    pub comments: usize,
    pub video_id: Option<String>,
    pub snapshot_id: Option<i64>,
    pub note: Option<String>,
}

pub struct CaptureInput<'a> {
    pub url: &'a str,
    pub referer: Option<&'a str>,
    pub request_body: Option<&'a [u8]>,
    pub body: &'a [u8],
    pub content_type: Option<&'a str>,
    pub content_encoding: Option<&'a str>,
}

/// Decodes a captured body (identity or gzip; the proxy asks for gzip).
pub fn decode_body(body: &[u8], encoding: Option<&str>) -> Result<Vec<u8>, String> {
    match encoding.map(|e| e.trim().to_ascii_lowercase()).as_deref() {
        None | Some("") | Some("identity") => Ok(body.to_vec()),
        Some("gzip") | Some("x-gzip") => {
            let mut out = Vec::new();
            flate2::read::GzDecoder::new(body).take(64 * 1024 * 1024).read_to_end(&mut out).map_err(|e| format!("gzip: {e}"))?;
            Ok(out)
        }
        Some(other) => Err(format!("unsupported content-encoding {other}")),
    }
}

fn is_watch_or_feed_html(url: &str) -> bool {
    let Some(u) = extract::parse_url(url) else { return false };
    let host = u.host.as_str();
    (host == "www.youtube.com" || host == "youtube.com" || host == "m.youtube.com") && !u.path.starts_with("/s/") && !u.path.starts_with("/embed")
}

impl Core {
    /// Entry point for the proxy. Safe to call from any thread.
    pub fn capture(&self, input: CaptureInput<'_>) -> Capture {
        let now = self.now();
        let mut cap = Capture { ts: now, url: input.url.chars().take(200).collect(), what: "ignored".into(), ..Capture::default() };
        let result = self.capture_inner(&input, &mut cap);
        if let Err(note) = result {
            cap.note = Some(note);
        }
        self.record_capture(&cap);
        if cap.kind.is_some() || cap.note.is_some() {
            self.log_event(if cap.note.is_some() { "warn" } else { "info" }, "content.captured",
                serde_json::json!({"kind":cap.kind,"items":cap.items,"comments":cap.comments,"snapshot_id":cap.snapshot_id,"has_error":cap.note.is_some()}));
        }
        cap
    }

    fn capture_inner(&self, input: &CaptureInput<'_>, cap: &mut Capture) -> Result<(), String> {
        let body = decode_body(input.body, input.content_encoding)?;
        let is_html = input.content_type.is_some_and(|c| c.to_ascii_lowercase().contains("text/html"));
        if is_html {
            if !is_watch_or_feed_html(input.url) {
                return Ok(());
            }
            cap.what = "html".into();
            let html = String::from_utf8_lossy(&body);
            let data = extract::initial_data_from_html(&html);
            let ctx = extract::page_context(input.url);
            cap.kind = Some(ctx.kind.as_str().to_string());
            cap.video_id = ctx.video_id.clone();
            if let Some(json) = data.yt_initial_data.as_ref() {
                self.store_page_json(json, ctx.kind, ctx.query.as_deref(), ctx.video_id.as_deref(), input.url, false, cap)?;
            }
            if let Some(player) = data.yt_initial_player_response.as_ref() {
                self.store_video_meta(player, ctx.video_id.as_deref(), cap)?;
            }
            return Ok(());
        }
        let classified = extract::classify_request_bytes(input.url, input.request_body, input.referer);
        let Some(endpoint) = classified.endpoint.clone() else { return Ok(()) };
        cap.what = endpoint.clone();
        let json: Value = serde_json::from_slice(&body).map_err(|e| format!("response is not JSON: {e}"))?;
        let kind = classified.kind.unwrap_or(Kind::Other);
        cap.kind = Some(kind.as_str().to_string());
        cap.video_id = classified.video_id.clone();
        match endpoint.as_str() {
            "browse" | "search" => {
                self.store_page_json(&json, kind, classified.query.as_deref(), None, input.referer.unwrap_or(input.url), classified.continuation, cap)?;
            }
            "next" => {
                self.store_page_json(&json, Kind::Related, None, classified.video_id.as_deref(), input.referer.unwrap_or(input.url), classified.continuation, cap)?;
            }
            "player" => self.store_video_meta(&json, classified.video_id.as_deref(), cap)?,
            _ => {}
        }
        Ok(())
    }

    /// Feed items, video metadata, related videos and comments from one document.
    #[allow(clippy::too_many_arguments)]
    fn store_page_json(&self, json: &Value, kind: Kind, query: Option<&str>, video_id: Option<&str>, source_url: &str, continuation: bool, cap: &mut Capture) -> Result<(), String> {
        self.discover_channels(json, kind == Kind::Subscriptions || source_url.contains("/feed/channels")).map_err(|e|e.to_string())?;
        let now = self.now();
        if kind == Kind::Related || kind == Kind::Video {
            if let Some(id) = video_id.filter(|v| valid_video_id(v)) {
                cap.video_id = Some(id.to_string());
                self.store_video_meta(json, Some(id), cap)?;
                let found = extract::extract_comments(json);
                if !found.comments.is_empty() {
                    cap.comments = self.merge_comments(id, found.comments, found.total, now).map_err(|e| e.to_string())?;
                }
            }
            return Ok(());
        }
        let items = extract::extract_feed(json, false);
        cap.items = items.len();
        if items.is_empty() {
            return Ok(());
        }
        let feed_kind = match kind {
            Kind::Home | Kind::Subscriptions | Kind::Search | Kind::Channel => kind.as_str(),
            _ => "other",
        };
        let out = self
            .ingest_feed(FeedIngest {
                kind: feed_kind.into(),
                source_url: Some(source_url.chars().take(500).collect()),
                query: query.map(str::to_string),
                video_id: None,
                captured_at: Some(now),
                continuation,
                items,
            })
            .map_err(|e| e.to_string())?;
        cap.snapshot_id = out["snapshot_id"].as_i64();
        Ok(())
    }

    pub(crate) fn store_video_meta_pub(&self, json: &Value, video_id: Option<&str>, cap: &mut Capture) -> Result<(), String> {
        self.store_video_meta(json, video_id, cap)
    }

    fn store_video_meta(&self, json: &Value, video_id: Option<&str>, cap: &mut Capture) -> Result<(), String> {
        let Some(mut meta) = extract::extract_video_meta(json) else { return Ok(()) };
        if meta.video_id.is_empty() {
            if let Some(id) = video_id {
                meta.video_id = id.to_string();
            }
        }
        if !valid_video_id(&meta.video_id) || meta.title.is_empty() && meta.related.is_empty() {
            return Ok(());
        }
        let now = self.now();
        let old: Option<VideoIngest> = self
            .content
            .get_json(KIND_META, &meta.video_id, now)
            .ok()
            .flatten()
            .and_then(|v| serde_json::from_value(v).ok());
        let merged = merge_video_meta(&meta, old.as_ref());
        cap.video_id = Some(merged.video_id.clone());
        self.ingest_video(merged).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Adds comments to what is cached for the video (first page replaces nothing,
    /// later pages append) and returns how many are stored.
    pub fn merge_comments(&self, video_id: &str, fresh: Vec<Comment>, total: Option<u64>, now: i64) -> crate::error::Result<usize> {
        let old: Option<CommentsIngest> = self.content.get_json(KIND_COMMENTS, video_id, now)?.and_then(|v| serde_json::from_value(v).ok());
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut all: Vec<Comment> = Vec::new();
        let old_total = old.as_ref().and_then(|o| o.total);
        for c in old.into_iter().flat_map(|o| o.comments).chain(fresh) {
            if seen.insert(extract::comment_key(&c)) {
                all.push(c);
            }
        }
        let stored = all.len().min(crate::ingest::MAX_COMMENTS);
        self.ingest_comments(CommentsIngest { video_id: video_id.into(), captured_at: Some(now), total: total.or(old_total), comments: all })?;
        Ok(stored)
    }

    fn record_capture(&self, cap: &Capture) {
        let mut log = self.capture_log.lock().unwrap_or_else(|e| e.into_inner());
        log.push_back(cap.clone());
        while log.len() > 200 {
            log.pop_front();
        }
    }

    pub fn recent_captures(&self, limit: usize) -> Vec<Capture> {
        let log = self.capture_log.lock().unwrap_or_else(|e| e.into_inner());
        log.iter().rev().take(limit).cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::testutil::test_core;
    use serde_json::json;
    use std::io::Write;

    fn gz(bytes: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        e.write_all(bytes).unwrap();
        e.finish().unwrap()
    }

    fn video(id: &str, title: &str) -> Value {
        json!({"videoRenderer": {"videoId": id, "title": {"runs": [{"text": title}]},
            "ownerText": {"runs": [{"text": "Chan", "navigationEndpoint": {"browseEndpoint": {"browseId": "UCabcdefghijklmnopqrstuv"}}}]},
            "lengthText": {"simpleText": "3:05"}, "thumbnail": {"thumbnails": [{"url": "https://i.ytimg.com/vi/x/hq.jpg", "width": 480, "height": 270}]}}})
    }

    #[test]
    fn a_gzipped_browse_response_lands_as_a_feed_snapshot() {
        let (_d, core) = test_core(None, None);
        let body = json!({"contents": {"items": [video("aaaaaaaaaaa", "First"), video("bbbbbbbbbbb", "Second")]}}).to_string();
        let req = json!({"browseId": "FEwhat_to_watch", "context": {}}).to_string();
        let cap = core.capture(CaptureInput {
            url: "https://www.youtube.com/youtubei/v1/browse?prettyPrint=false",
            referer: Some("https://www.youtube.com/"),
            request_body: Some(req.as_bytes()),
            body: &gz(body.as_bytes()),
            content_type: Some("application/json"),
            content_encoding: Some("gzip"),
        });
        assert_eq!((cap.what.as_str(), cap.kind.as_deref(), cap.items), ("browse", Some("home"), 2), "{cap:?}");
        let snap = core.content.latest_snapshot("home").unwrap().unwrap();
        assert_eq!(snap.items.len(), 2);
        assert_eq!(core.recent_captures(5).len(), 1);
    }

    #[test]
    fn watch_page_html_stores_metadata_without_failing_on_junk() {
        let (_d, core) = test_core(None, None);
        let data = json!({"currentVideoEndpoint": {"watchEndpoint": {"videoId": "aaaaaaaaaaa"}}, "contents": {"twoColumnWatchNextResults": {"results": {"results": {"contents": [{"videoPrimaryInfoRenderer": {"title": {"runs": [{"text": "Hello"}]}}}]}},
            "secondaryResults": {"secondaryResults": {"results": [json!({"compactVideoRenderer": video("ccccccccccc", "Related one")["videoRenderer"].clone()})]}}}}});
        let html = format!("<html><script>var ytInitialData = {};</script></html>", data);
        let cap = core.capture(CaptureInput {
            url: "https://www.youtube.com/watch?v=aaaaaaaaaaa",
            referer: None,
            request_body: None,
            body: html.as_bytes(),
            content_type: Some("text/html; charset=utf-8"),
            content_encoding: None,
        });
        assert_eq!(cap.what, "html");
        assert_eq!(cap.video_id.as_deref(), Some("aaaaaaaaaaa"));
        assert!(core.content.get_json(KIND_META, "aaaaaaaaaaa", core.now()).unwrap().is_some(), "{cap:?}");
        let junk = core.capture(CaptureInput {
            url: "https://www.youtube.com/youtubei/v1/next",
            referer: None,
            request_body: Some(b"not json"),
            body: b"<<<",
            content_type: Some("application/json"),
            content_encoding: None,
        });
        assert!(junk.note.is_some(), "junk is reported, not raised");
    }

    #[test]
    fn unrelated_urls_and_encodings_are_ignored_or_reported() {
        let (_d, core) = test_core(None, None);
        let c = core.capture(CaptureInput { url: "https://www.youtube.com/s/player/base.js", referer: None, request_body: None, body: b"x", content_type: Some("text/javascript"), content_encoding: None });
        assert_eq!(c.what, "ignored");
        let c = core.capture(CaptureInput { url: "https://www.youtube.com/youtubei/v1/browse", referer: None, request_body: None, body: b"x", content_type: Some("application/json"), content_encoding: Some("br") });
        assert!(c.note.unwrap().contains("unsupported"));
    }

    #[test]
    fn comment_pages_accumulate() {
        let (_d, core) = test_core(None, None);
        let mk = |n: usize, off: usize| (0..n).map(|i| Comment { id: Some(format!("c{}", off + i)), author: "a".into(), text: format!("t{}", off + i), ..Comment::default() }).collect::<Vec<_>>();
        assert_eq!(core.merge_comments("aaaaaaaaaaa", mk(20, 0), Some(500), 1000).unwrap(), 20);
        assert_eq!(core.merge_comments("aaaaaaaaaaa", mk(20, 10), None, 1000).unwrap(), 30);
        let got = core.content.get_json(KIND_COMMENTS, "aaaaaaaaaaa", 1001).unwrap().unwrap();
        assert_eq!((got["comments"].as_array().unwrap().len(), got["total"].clone()), (30, json!(500)));
    }
}
