//! Contracts between the client and an acquisition backend.
//!
//! A backend may acquire media elsewhere in a future deployment, but playback
//! always requires a device-local cache. An upstream URL is never a player
//! source. The initial adapter is embedded and uses the existing local cache.

use muninn_domain::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const API_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchRequest {
    pub video_id: String,
    #[serde(default)]
    pub audio_only: bool,
}

impl WatchRequest {
    pub fn validate(&self) -> Result<()> {
        if !muninn_domain::util::valid_video_id(&self.video_id) {
            return Err(Error::bad("invalid video_id"));
        }
        Ok(())
    }
}

/// The watch boundary used by the embedded HTTP adapter and native client.
/// Calls may block (cookie access/helper startup), so native callers run them
/// off the main thread. Existing JSON status fields remain wire-compatible.
pub trait WatchBackend: Send + Sync {
    fn open_watch(&self, request: &WatchRequest) -> Result<Value>;
    /// Durable background acquisition, without consuming foreground playback
    /// slots or preempting other downloads. Existing queued work is reused.
    fn prepare_background(&self, request: &WatchRequest) -> Result<Value>;
    fn watch_status(&self, video_id: &str) -> Result<Value>;
    fn keep_entry(&self, entry_id: &str) -> Result<Value>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheKind {
    File,
    Hls,
}

/// A validated path to materialized local media. HLS may still be growing on
/// disk; it does not have to be a complete file before playback can start.
/// Fields are private so an arbitrary network URL cannot construct this type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheSource {
    kind: CacheKind,
    path: String,
}

fn component(value: &str) -> bool {
    !value.is_empty() && value != "." && value != ".."
        && value.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.'))
}

impl CacheSource {
    pub fn parse(kind: &str, path: &str) -> Result<Self> {
        let parts: Vec<_> = path.split('/').collect();
        let kind = match (kind, parts.as_slice()) {
            ("file", ["", "media", entry]) if component(entry) => CacheKind::File,
            ("hls", ["", "h", token, job, "index.m3u8"])
                if component(token) && component(job) => CacheKind::Hls,
            _ => return Err(Error::bad("playback requires a device-local cache source")),
        };
        Ok(Self { kind, path: path.into() })
    }

    pub fn kind(&self) -> CacheKind { self.kind }
    pub fn path(&self) -> &str { &self.path }

    /// Pending/error responses have no playable source. A response claiming
    /// readiness must identify one of the two disk-backed routes explicitly.
    pub fn from_watch_status(status: &Value) -> Result<Option<Self>> {
        let playback = &status["playback"];
        if playback["ready"].as_bool() != Some(true) { return Ok(None); }
        Self::parse(
            playback["kind"].as_str().unwrap_or_default(),
            playback["url_path"].as_str().unwrap_or_default(),
        ).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accepts_complete_and_still_growing_device_cache() {
        let file = CacheSource::parse("file", "/media/abcdefghijk.720").unwrap();
        assert_eq!(file.kind(), CacheKind::File);
        assert_eq!(file.path(), "/media/abcdefghijk.720");
        let s = json!({"playback":{"ready":true,"kind":"hls","progress":0.01,
            "url_path":"/h/token/abcdefghijk.720.local.42/index.m3u8"}});
        assert_eq!(CacheSource::from_watch_status(&s).unwrap().unwrap().kind(), CacheKind::Hls);
        assert_eq!(CacheSource::from_watch_status(&json!({"playback":{"ready":false}})).unwrap(), None);
    }

    #[test]
    fn remote_urls_internal_feeders_and_path_escape_are_not_playback() {
        for path in ["https://example.test/video", "//example.test/media/x", "/s/token/x",
            "/media/../x", "/media/..", "/media/.", "/media/", "/media/x?remote=1",
            "/media/%2e%2e", "/media/x%2fy", "/media/x\\y", "/media/x\n"] {
            assert!(CacheSource::parse("file", path).is_err(), "{path}");
        }
        for path in ["/h/t/../index.m3u8", "/h/t/j/video.ts", "/h/t/j/index.m3u8?x=1",
            "/h/t/j/../index.m3u8", "/media/video"] {
            assert!(CacheSource::parse("hls", path).is_err(), "{path}");
        }
        assert!(CacheSource::parse("remote", "/media/video").is_err());
        assert!(CacheSource::from_watch_status(&json!({"playback":{"ready":true}})).is_err());
    }

    #[test]
    fn deserialized_watch_requests_still_require_validation() {
        let req: WatchRequest = serde_json::from_value(json!({"video_id":"abcdefghijk"})).unwrap();
        assert!(!req.audio_only);
        assert!(req.validate().is_ok());
        assert!(WatchRequest { video_id: "../escape".into(), audio_only: false }.validate().is_err());
    }
}
