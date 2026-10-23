//! Fetching what Muninn's watch view shows when the content cache does not have
//! it yet: the watch page's metadata and related videos, and pages of comments
//! through YouTube's own continuation tokens. Plain HTTPS from Rust with a
//! consent cookie; nothing here needs the web views or a login.

use crate::capture::decode_body;
use crate::content::{KIND_COMMENTS, KIND_META};
use crate::engine::Core;
use crate::error::{Error, Result};
use crate::extract::{self, ContinuationKind};
use crate::util::valid_video_id;
use serde_json::{json, Value};

const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.4 Safari/605.1.15";
/// Continuation state per video: the token for the next page of comments.
pub const KIND_CONT: &str = "comments-cont";
const MAX_PAGE_BYTES: u64 = 16 * 1024 * 1024;

fn page_headers() -> Vec<(&'static str, String)> {
    vec![
        ("User-Agent", UA.to_string()),
        ("Accept-Language", "en-US,en;q=0.9".to_string()),
        ("Accept", "text/html,application/xhtml+xml".to_string()),
        ("Accept-Encoding", "gzip".to_string()),
        // Accepts the EU consent wall.
        ("Cookie", "SOCS=CAI; CONSENT=YES+cb".to_string()),
    ]
}

fn between<'a>(text: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let a = text.find(start)? + start.len();
    let b = text[a..].find(end)? + a;
    Some(&text[a..b])
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct WatchFetch {
    pub title: String,
    pub related: usize,
    pub has_comments_token: bool,
    pub comments: usize,
}

impl Core {
    /// Fetches the watch page, stores metadata + related videos, remembers the comment
    /// continuation and loads the first page of comments.
    pub fn fetch_watch(&self, video_id: &str) -> Result<WatchFetch> {
        if !valid_video_id(video_id) {
            return Err(Error::bad("invalid video_id"));
        }
        let url = format!("https://www.youtube.com/watch?v={video_id}&hl=en");
        let resp = self.fetcher.request("GET", &url, &page_headers(), None, MAX_PAGE_BYTES)?;
        if resp.status != 200 {
            return Err(Error::other(format!("YouTube answered {} for the watch page", resp.status)));
        }
        let body = decode_body(&resp.body, resp.content_encoding.as_deref()).map_err(Error::other)?;
        let html = String::from_utf8_lossy(&body);
        let data = extract::initial_data_from_html(&html);
        let client_version = between(&html, "\"INNERTUBE_CONTEXT_CLIENT_VERSION\":\"", "\"").unwrap_or("2.20260101.00.00").to_string();
        let visitor = between(&html, "\"VISITOR_DATA\":\"", "\"").unwrap_or("").to_string();
        let mut cap = crate::capture::Capture::default();
        if let Some(json) = data.yt_initial_data.as_ref() {
            self.store_video_meta_pub(json, Some(video_id), &mut cap).map_err(Error::other)?;
        }
        if let Some(player) = data.yt_initial_player_response.as_ref() {
            self.store_video_meta_pub(player, Some(video_id), &mut cap).map_err(Error::other)?;
        }
        let token = data.yt_initial_data.as_ref().and_then(extract::comments_section_token);
        let now = self.now();
        let mut out = WatchFetch { has_comments_token: token.is_some(), ..WatchFetch::default() };
        if let Some(meta) = self.content.get_json(KIND_META, video_id, now)? {
            out.title = meta["title"].as_str().unwrap_or("").to_string();
            out.related = meta["related"].as_array().map_or(0, |r| r.len());
        }
        if let Some(token) = token {
            self.put_cont(video_id, &token, &client_version, &visitor)?;
            out.comments = self.load_more_comments(video_id)?;
        }
        if out.title.is_empty() {
            return Err(Error::other("YouTube returned no data for this video (private, removed or blocked?)"));
        }
        Ok(out)
    }

    fn put_cont(&self, video_id: &str, token: &str, client_version: &str, visitor: &str) -> Result<()> {
        let value = json!({"token": token, "client_version": client_version, "visitor": visitor});
        self.content.put_json(KIND_CONT, video_id, &value, Some(2 * crate::util::HOUR_MS), self.now())
    }

    /// Loads the next page of comments for a video and merges it into the cache.
    /// Returns how many comments are stored afterwards; `Ok(0)` when there is no more.
    pub fn load_more_comments(&self, video_id: &str) -> Result<usize> {
        let now = self.now();
        let Some(state) = self.content.get_json(KIND_CONT, video_id, now)? else {
            return Ok(self.fetch_watch(video_id)?.comments);
        };
        let token = state["token"].as_str().unwrap_or("").to_string();
        if token.is_empty() {
            return Ok(0);
        }
        let client_version = state["client_version"].as_str().unwrap_or("2.20260101.00.00").to_string();
        let visitor = state["visitor"].as_str().unwrap_or("").to_string();
        let mut client = json!({"clientName": "WEB", "clientVersion": client_version, "hl": "en", "gl": "US"});
        if !visitor.is_empty() {
            client["visitorData"] = json!(visitor);
        }
        let body = json!({"context": {"client": client}, "continuation": token}).to_string();
        let headers = vec![
            ("User-Agent", UA.to_string()),
            ("Content-Type", "application/json".to_string()),
            ("Accept-Encoding", "gzip".to_string()),
            ("Accept-Language", "en-US,en;q=0.9".to_string()),
            ("Origin", "https://www.youtube.com".to_string()),
            ("X-Youtube-Client-Name", "1".to_string()),
            ("X-Youtube-Client-Version", client_version.clone()),
            ("Cookie", "SOCS=CAI; CONSENT=YES+cb".to_string()),
        ];
        let resp = self.fetcher.request("POST", "https://www.youtube.com/youtubei/v1/next?prettyPrint=false", &headers, Some(body.as_bytes()), MAX_PAGE_BYTES)?;
        if resp.status != 200 {
            return Err(Error::other(format!("YouTube answered {} for the comments", resp.status)));
        }
        let raw = decode_body(&resp.body, resp.content_encoding.as_deref()).map_err(Error::other)?;
        let json: Value = serde_json::from_slice(&raw)?;
        self.remember_reply_tokens(video_id, &json, &state)?;
        let found = extract::extract_comments(&json);
        let next = extract::extract_comment_continuations(&json).into_iter().find(|c| c.kind == ContinuationKind::Page && c.token != token).map(|c| c.token);
        let stored = if found.comments.is_empty() {
            self.content.get_json(KIND_COMMENTS, video_id, now)?.and_then(|v| v["comments"].as_array().map(|a| a.len())).unwrap_or(0)
        } else {
            self.merge_comments(video_id, found.comments, found.total, now)?
        };
        let next=next.filter(|_|stored<crate::ingest::MAX_COMMENTS);
        match next {
            Some(t) => self.put_cont(video_id, &t, &client_version, &visitor)?,
            None => self.put_cont(video_id, "", &client_version, &visitor)?,
        }
        Ok(stored)
    }

    fn reply_key(video: &str, comment: &str) -> String { format!("{video}:{comment}") }

    fn remember_reply_tokens(&self, video: &str, page: &Value, context: &Value) -> Result<()> {
        for (parent, token) in extract::comment_reply_tokens(page) {
            let key = Self::reply_key(video, &parent);
            if self.content.get_json("comment-replies", &key, self.now())?.is_some() { continue; }
            let mut state = context.clone();
            state["token"] = json!(token);
            state["comments"] = json!([]);
            self.content.put_json("comment-replies", &key, &state, Some(2 * crate::util::HOUR_MS), self.now())?;
        }
        Ok(())
    }

    pub fn load_replies(&self, video: &str, comment: &str, more: bool) -> Result<Value> {
        if !valid_video_id(video) || comment.is_empty() || comment.len()>200 { return Err(Error::bad("invalid video or comment id")); }
        let key = Self::reply_key(video, comment);
        if self.content.get_json("comment-replies", &key, self.now())?.is_none() { self.fetch_watch(video)?; }
        let mut state = self.content.get_json("comment-replies", &key, self.now())?
            .ok_or_else(||Error::not_found("reply link expired or unavailable; refresh comments and try again"))?;
        let token = state["token"].as_str().unwrap_or("").to_string();
        if (!state["loaded"].as_bool().unwrap_or(false) || more) && !token.is_empty() {
            let version = state["client_version"].as_str().unwrap_or("2.20260101.00.00");
            let body = json!({"context":{"client":{"clientName":"WEB","clientVersion":version,"hl":"en","gl":"US","visitorData":state["visitor"]}},"continuation":token}).to_string();
            let headers=vec![("User-Agent",UA.to_string()),("Content-Type","application/json".into()),("Cookie","SOCS=CAI; CONSENT=YES+cb".into()),
                ("X-Youtube-Client-Name","1".into()),("X-Youtube-Client-Version",version.into()),("Origin","https://www.youtube.com".into())];
            let resp=self.fetcher.request("POST","https://www.youtube.com/youtubei/v1/next?prettyPrint=false",&headers,Some(body.as_bytes()),MAX_PAGE_BYTES)?;
            if resp.status!=200 { return Err(Error::other(format!("YouTube answered {} for replies",resp.status))); }
            let raw=decode_body(&resp.body,resp.content_encoding.as_deref()).map_err(Error::other)?;
            let page:Value=serde_json::from_slice(&raw)?;
            let mut all:Vec<crate::model::Comment>=serde_json::from_value(state["comments"].clone()).unwrap_or_default();
            let mut seen:std::collections::HashSet<String>=all.iter().map(extract::comment_key).collect();
            for mut c in extract::extract_comments(&page).comments {
                c.is_reply=Some(true);
                if seen.insert(extract::comment_key(&c)) && all.len()<crate::ingest::MAX_COMMENTS {all.push(c);}
            }
            let next=extract::extract_comment_continuations(&page).into_iter().find(|c|c.kind==ContinuationKind::Page && c.token!=token).map(|c|c.token).unwrap_or_default();
            state["token"]=json!(next);state["comments"]=json!(all);state["loaded"]=json!(true);
            self.content.put_json("comment-replies",&key,&state,Some(2*crate::util::HOUR_MS),self.now())?;
        }
        Ok(json!({"comments":state["comments"],"has_more":state["token"].as_str().is_some_and(|s|!s.is_empty())}))
    }

    /// True when a "load more" would fetch something.
    pub fn has_more_comments(&self, video_id: &str) -> bool {
        self.content
            .get_json(KIND_CONT, video_id, self.now())
            .ok()
            .flatten()
            .is_some_and(|s| s["token"].as_str().is_some_and(|t| !t.is_empty()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::testutil::test_core;
    use crate::ytdlp::{Fetcher, HttpResponse};
    use std::sync::{Arc, Mutex};

    struct Fake {
        calls: Mutex<Vec<(String, String)>>,
        page: String,
        comments: String,
    }

    impl Fetcher for Fake {
        fn get(&self, url: &str, _max: u64) -> Result<Vec<u8>> {
            Err(Error::other(format!("unexpected GET {url}")))
        }
        fn request(&self, method: &str, url: &str, headers: &[(&str, String)], body: Option<&[u8]>, _max: u64) -> Result<HttpResponse> {
            self.calls.lock().unwrap().push((method.to_string(), url.to_string()));
            assert!(headers.iter().any(|(k, v)| *k == "Cookie" && v.contains("SOCS")), "consent cookie is sent");
            if method == "POST" {
                let b: Value = serde_json::from_slice(body.unwrap()).unwrap();
                assert_eq!(b["continuation"], "TOKEN-1");
                assert_eq!(b["context"]["client"]["clientVersion"], "2.20261015.01.00");
                return Ok(HttpResponse { status: 200, content_encoding: None, body: self.comments.clone().into_bytes() });
            }
            Ok(HttpResponse { status: 200, content_encoding: None, body: self.page.clone().into_bytes() })
        }
    }

    fn comment(id: &str) -> Value {
        json!({"commentThreadRenderer": {"comment": {"commentRenderer": {"commentId": id, "authorText": {"simpleText": "Alice"},
            "contentText": {"runs": [{"text": format!("text {id}")}]}, "voteCount": {"simpleText": "3"}, "publishedTimeText": {"runs": [{"text": "1 day ago"}]}}}}})
    }

    #[test]
    fn watch_page_then_comments_land_in_the_cache() {
        let data = json!({
            "currentVideoEndpoint": {"watchEndpoint": {"videoId": "aaaaaaaaaaa"}},
            "contents": {"twoColumnWatchNextResults": {
                "results": {"results": {"contents": [
                    {"videoPrimaryInfoRenderer": {"title": {"runs": [{"text": "A fine video"}]}}},
                    {"itemSectionRenderer": {"sectionIdentifier": "comment-item-section", "contents": [{"continuationItemRenderer": {"continuationEndpoint": {"continuationCommand": {"token": "TOKEN-1"}}}}]}}]}},
                "secondaryResults": {"secondaryResults": {"results": [{"compactVideoRenderer": {"videoId": "ccccccccccc", "title": {"simpleText": "Next up"}, "lengthText": {"simpleText": "2:00"}}}]}}}}});
        let page = format!(r#"<html><script>var ytInitialData = {data};</script>"INNERTUBE_CONTEXT_CLIENT_VERSION":"2.20261015.01.00","VISITOR_DATA":"vis"</html>"#);
        let comments = json!({"onResponseReceivedEndpoints": [{"reloadContinuationItemsCommand": {"continuationItems": [comment("c1"), comment("c2"),
            {"continuationItemRenderer": {"continuationEndpoint": {"continuationCommand": {"token": "TOKEN-2"}}}}]}}]}).to_string();
        let fake = Arc::new(Fake { calls: Mutex::new(vec![]), page, comments });
        let (_d, core) = test_core(None, Some(fake.clone()));
        let out = core.fetch_watch("aaaaaaaaaaa").unwrap();
        assert_eq!(out.title, "A fine video");
        assert_eq!(out.related, 1);
        assert!(out.has_comments_token);
        assert_eq!(out.comments, 2, "{out:?}");
        assert!(core.has_more_comments("aaaaaaaaaaa"), "TOKEN-2 is kept for the next page");
        let stored = core.content.get_json(KIND_COMMENTS, "aaaaaaaaaaa", core.now()).unwrap().unwrap();
        assert_eq!(stored["comments"][0]["author"], "Alice");
        assert_eq!(fake.calls.lock().unwrap().len(), 2);
    }

    #[test]
    fn comment_pages_and_reply_threads_have_separate_continuations_and_caches() {
        struct ThreadFake { calls:Mutex<Vec<String>> }
        fn cir(token:&str)->Value {json!({"continuationItemRenderer":{"continuationEndpoint":{"continuationCommand":{"token":token}}}})}
        impl Fetcher for ThreadFake {
            fn get(&self,_:&str,_:u64)->Result<Vec<u8>> {unreachable!()}
            fn request(&self,method:&str,_:&str,_:&[(&str,String)],body:Option<&[u8]>,_:u64)->Result<HttpResponse> {
                if method=="GET" {
                    let data=json!({"currentVideoEndpoint":{"watchEndpoint":{"videoId":"aaaaaaaaaaa"}},"contents":{"videoPrimaryInfoRenderer":{"title":{"simpleText":"Example"}},"itemSectionRenderer":{"sectionIdentifier":"comment-item-section","contents":[cir("SECTION")]}}});
                    return Ok(HttpResponse{status:200,content_encoding:None,body:format!("<script>var ytInitialData = {data};</script>").into_bytes()});
                }
                let b:Value=serde_json::from_slice(body.unwrap()).unwrap();let token=b["continuation"].as_str().unwrap();self.calls.lock().unwrap().push(token.into());
                let items=match token {
                    "SECTION"=>{let mut c=comment("parent");c["commentThreadRenderer"]["replies"]=json!({"commentRepliesRenderer":{"contents":[cir("REPLY1")]}});vec![c,cir("PAGE2")]},
                    "PAGE2"=>vec![comment("second")],
                    "REPLY1"=>vec![json!({"commentRenderer":{"commentId":"reply1","authorText":{"simpleText":"Bob"},"contentText":{"simpleText":"A reply"}}}),cir("REPLY2")],
                    "REPLY2"=>vec![json!({"commentRenderer":{"commentId":"reply2","authorText":{"simpleText":"Carol"},"contentText":{"simpleText":"Another reply"}}})],
                    other=>panic!("unexpected token {other}"),
                };
                let value=json!({"onResponseReceivedEndpoints":[{"appendContinuationItemsAction":{"targetId":"comments-section","continuationItems":items}}]});
                Ok(HttpResponse{status:200,content_encoding:None,body:value.to_string().into_bytes()})
            }
        }
        let fake=Arc::new(ThreadFake{calls:Mutex::new(Vec::new())});let (_d,c)=test_core(None,Some(fake.clone()));
        assert_eq!(c.fetch_watch("aaaaaaaaaaa").unwrap().comments,1);
        let replies=c.load_replies("aaaaaaaaaaa","parent",false).unwrap();assert_eq!(replies["comments"].as_array().unwrap().len(),1);assert_eq!(replies["has_more"],true);
        c.load_replies("aaaaaaaaaaa","parent",false).unwrap();assert_eq!(fake.calls.lock().unwrap().len(),2,"opening cached replies doesn't refetch");
        assert_eq!(c.load_more_comments("aaaaaaaaaaa").unwrap(),2);
        assert!(!c.has_more_comments("aaaaaaaaaaa"));
        let replies=c.load_replies("aaaaaaaaaaa","parent",true).unwrap();assert_eq!(replies["comments"].as_array().unwrap().len(),2);assert_eq!(replies["has_more"],false);
        assert_eq!(*fake.calls.lock().unwrap(),vec!["SECTION","REPLY1","PAGE2","REPLY2"]);
        assert_eq!(c.video_detail("aaaaaaaaaaa").unwrap()["comments"]["comments"].as_array().unwrap().len(),2,"replies never consume top-level pagination");
    }

    #[test]
    fn a_missing_video_is_an_error_not_an_empty_page() {
        let fake = Arc::new(Fake { calls: Mutex::new(vec![]), page: "<html>nothing here</html>".into(), comments: String::new() });
        let (_d, core) = test_core(None, Some(fake));
        assert!(core.fetch_watch("aaaaaaaaaaa").is_err());
        assert!(core.fetch_watch("../evil").is_err());
    }
}
