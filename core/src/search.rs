//! Explicit, bounded online search. Flat extraction never downloads media.
use crate::{Core, Error, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;

const LIMIT: usize = 12;
const MAX_OUTPUT: u64 = 2 * 1024 * 1024;
static SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(3);

#[derive(Debug, Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Category { Videos, Live, Channels }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchRequest { pub query: String, pub category: Category }

fn search_url(query: &str, category: Category) -> Result<String> {
    let query = query.trim();
    if query.is_empty() || query.chars().count() > 200 || query.chars().any(char::is_control) {
        return Err(Error::bad("Enter a search of 1–200 characters"));
    }
    let encoded: String = query.bytes().map(|b| {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() }
        else { format!("%{b:02X}") }
    }).collect();
    let filter = match category { Category::Videos => "EgIQAQ%3D%3D", Category::Live => "EgJAAQ%3D%3D", Category::Channels => "EgIQAg%3D%3D" };
    Ok(format!("https://www.youtube.com/results?search_query={encoded}&sp={filter}"))
}

fn parse_results(data: &Value, category: Category) -> Result<Vec<Value>> {
    let entries = data["entries"].as_array().ok_or_else(|| Error::bad("YouTube returned no search listing"))?;
    let mut seen = HashSet::new();
    Ok(entries.iter().filter_map(|entry| {
        let id = entry["id"].as_str()?;
        let is_channel = id.len() == 24 && id.starts_with("UC") && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b));
        let live = entry["is_live"] == true || entry["live_status"] == "is_live";
        if match category {
            Category::Channels => !is_channel,
            Category::Videos => !crate::util::valid_video_id(id) || live || entry["live_status"] == "is_upcoming",
            Category::Live => !crate::util::valid_video_id(id) || !live,
        } || !seen.insert(id.to_string()) { return None; }
        Some(json!({"id":id,"title":crate::extract::clip(entry["title"].as_str().unwrap_or(id),500),
            "channel":crate::extract::clip(entry["channel"].as_str().or_else(||entry["uploader"].as_str()).unwrap_or(""),200),
            "duration_seconds":entry["duration"].as_f64().filter(|n|n.is_finite() && *n > 0.0),
            "live":live,"kind":if is_channel {"channel"} else {"video"}}))
    }).take(LIMIT).collect())
}

pub async fn online(core: Arc<Core>, request: SearchRequest) -> Result<Value> {
    let url = search_url(&request.query, request.category)?;
    core.require_online()?;
    let run = async {
        let _permit = SLOTS.acquire().await.map_err(|_| Error::other("Search is unavailable"))?;
        core.require_online()?;
        let bin = core.tools().ytdlp.path.ok_or_else(|| Error::bad("yt-dlp is not ready. Check Settings and retry."))?;
        let cookies = core.youtube_cookie_file()?;
        let mut child = tokio::process::Command::new(bin)
            .args(cookies.as_ref().map(|jar| jar.args()).into_iter().flatten())
            .args(["--ignore-config", "--flat-playlist", "--skip-download", "--dump-single-json", "--playlist-end", "12", "--socket-timeout", "15", "--retries", "1", "--no-warnings", "--", &url])
            .env("PATH", crate::ytdlp::search_path()).stdin(Stdio::null()).stderr(Stdio::null()).stdout(Stdio::piped())
            .kill_on_drop(true).spawn().map_err(|_| Error::other("Could not start YouTube search"))?;
        let mut bytes = Vec::new();
        child.stdout.take().ok_or_else(|| Error::other("Search output unavailable"))?
            .take(MAX_OUTPUT + 1).read_to_end(&mut bytes).await?;
        if bytes.len() as u64 > MAX_OUTPUT { return Err(Error::bad("YouTube search response was too large")); }
        let status = child.wait().await?;
        if !status.success() { return Err(Error::bad("YouTube search failed. Retry or update yt-dlp in Settings.")); }
        core.require_online()?;
        let data: Value = serde_json::from_slice(&bytes).map_err(|_| Error::bad("Invalid YouTube search response"))?;
        Ok(json!({"items":parse_results(&data,request.category)?}))
    };
    tokio::select! {
        result = tokio::time::timeout(Duration::from_secs(35),run) => result.map_err(|_| Error::bad("YouTube search timed out. Press Enter to retry."))?,
        _ = async { while !core.is_offline() { tokio::time::sleep(Duration::from_millis(100)).await; } } => Err(Error::Conflict("Online search stopped: offline mode is on".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn query_is_bounded_and_cannot_inject_url_parameters() {
        assert!(search_url("  ",Category::Videos).is_err());
        assert!(search_url("a\nb",Category::Videos).is_err());
        assert!(search_url(&"a".repeat(201),Category::Videos).is_err());
        let url=search_url("café &sp=bad#x",Category::Channels).unwrap();
        assert!(url.contains("caf%C3%A9%20%26sp%3Dbad%23x"));
        assert!(url.ends_with("&sp=EgIQAg%3D%3D"));
    }
    #[test]
    fn results_are_typed_deduplicated_bounded_and_do_not_trust_external_urls() {
        let data=json!({"entries":[
            {"id":"aaaaaaaaaaa","title":"Video","duration":123,"url":"javascript:bad"},
            {"id":"aaaaaaaaaaa","title":"Duplicate"},
            {"id":"bbbbbbbbbbb","title":"Live","live_status":"is_live"},
            {"id":"UCabcdefghijklmnopqrstuv","title":"Channel"},
            {"id":"../bad","title":"Invalid"}
        ]});
        assert_eq!(parse_results(&data,Category::Videos).unwrap().len(),1);
        assert!(parse_results(&data,Category::Videos).unwrap()[0].get("url").is_none());
        assert_eq!(parse_results(&data,Category::Videos).unwrap()[0]["duration_seconds"],123.0);
        assert_eq!(parse_results(&data,Category::Live).unwrap()[0]["id"],"bbbbbbbbbbb");
        assert_eq!(parse_results(&data,Category::Channels).unwrap()[0]["kind"],"channel");
        let large=json!({"entries":(0..30).map(|i|json!({"id":format!("{i:011}")})).collect::<Vec<_>>()});
        assert_eq!(parse_results(&large,Category::Videos).unwrap().len(),LIMIT);
        assert!(parse_results(&json!({}),Category::Videos).is_err());
    }
    #[tokio::test]
    async fn offline_search_never_starts_a_helper() {
        let (_dir,core)=crate::engine::testutil::test_core(None,None);
        let mut settings=core.video.settings();settings.offline_mode=true;core.apply_settings(&settings).unwrap();
        let result=online(core,SearchRequest{query:"test".into(),category:Category::Videos}).await;
        assert!(matches!(result,Err(Error::Conflict(_))));
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn flat_helper_search_returns_only_metadata_with_safe_arguments() {
        use std::os::unix::fs::PermissionsExt;
        let (dir,core)=crate::engine::testutil::test_core(None,None);
        let tool=dir.path().join("search-fixture");let args=dir.path().join("args");
        let script=format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf '%s' '{{\"entries\":[{{\"id\":\"aaaaaaaaaaa\",\"title\":\"Synthetic\"}}]}}'\n",args.display());
        std::fs::write(&tool,script).unwrap();std::fs::set_permissions(&tool,std::fs::Permissions::from_mode(0o755)).unwrap();
        core.tools.write().unwrap().ytdlp.path=Some(tool.display().to_string());
        let result=online(core.clone(),SearchRequest{query:"cats &sp=bad".into(),category:Category::Videos}).await.unwrap();
        assert_eq!(result["items"][0]["title"],"Synthetic");
        let arguments=std::fs::read_to_string(args).unwrap();
        assert!(arguments.contains("--flat-playlist\n--skip-download\n"));
        assert!(arguments.contains("search_query=cats%20%26sp%3Dbad&sp=EgIQAQ%3D%3D"));
        assert!(core.video.list_entries().unwrap().is_empty());
        assert!(core.browsing.tabs().unwrap().is_empty());
    }
}
