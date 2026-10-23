//! Wire types shared by the ingest API, the stores and the UI.

use serde::{Deserialize, Serialize};

/// A video as it appears in a feed, search result or related list.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Item {
    pub video_id: String,
    pub title: String,
    pub channel: Option<String>,
    pub channel_id: Option<String>,
    pub duration_seconds: Option<u32>,
    pub thumbnail: Option<String>,
    pub published_text: Option<String>,
    pub view_text: Option<String>,
    pub badge: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FeedIngest {
    pub kind: String,
    pub source_url: Option<String>,
    pub query: Option<String>,
    pub video_id: Option<String>,
    pub captured_at: Option<i64>,
    pub continuation: bool,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoIngest {
    pub video_id: String,
    pub title: String,
    pub channel: Option<String>,
    pub channel_id: Option<String>,
    pub duration_seconds: Option<u32>,
    pub description: Option<String>,
    pub thumbnail: Option<String>,
    pub view_count: Option<u64>,
    pub published_text: Option<String>,
    pub is_live: Option<bool>,
    pub related: Vec<Item>,
    pub captured_at: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Comment {
    pub id: Option<String>,
    pub author: String,
    pub text: String,
    pub likes: Option<u64>,
    pub published_text: Option<String>,
    pub reply_count: Option<u64>,
    pub is_reply: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CommentsIngest {
    pub video_id: String,
    pub captured_at: Option<i64>,
    pub total: Option<u64>,
    pub comments: Vec<Comment>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TabIngest {
    #[serde(deserialize_with = "string_or_number")]
    pub tab_id: String,
    pub url: String,
    pub title: Option<String>,
    pub video_id: Option<String>,
    pub scroll_y: Option<f64>,
    pub position_seconds: Option<f64>,
    pub duration_seconds: Option<f64>,
    pub watch_height: Option<u32>,
    pub state: String,
    pub ts: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaybackIngest {
    /// Set only after the media element has emitted `playing`.
    pub started: bool,
    pub ended: bool,
    pub video_id: String,
    pub position_seconds: f64,
    pub duration_seconds: Option<f64>,
    pub watch_height: Option<u32>,
    pub paused: Option<bool>,
    #[serde(deserialize_with = "opt_string_or_number")]
    pub tab_id: Option<String>,
    pub ts: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CacheRequest {
    pub video_id: String,
    pub quality: Option<String>,
    pub protect: bool,
    pub title: Option<String>,
    pub channel: Option<String>,
    pub channel_id: Option<String>,
    pub duration_seconds: Option<u32>,
}

fn string_or_number<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v {
        serde_json::Value::String(s) => s,
        serde_json::Value::Number(n) => n.to_string(),
        _ => String::new(),
    })
}

fn opt_string_or_number<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v {
        serde_json::Value::String(s) => Some(s),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tolerates_missing_and_numeric_fields() {
        let t: TabIngest = serde_json::from_str(r#"{"tab_id": 12, "url": "https://www.youtube.com/", "state":"open"}"#).unwrap();
        assert_eq!(t.tab_id, "12");
        let p: PlaybackIngest = serde_json::from_str(r#"{"video_id":"dQw4w9WgXcQ","position_seconds":3.5,"tab_id":7}"#).unwrap();
        assert_eq!(p.tab_id.as_deref(), Some("7"));
        let i: Item = serde_json::from_str(r#"{"video_id":"dQw4w9WgXcQ","title":"x","channel":null}"#).unwrap();
        assert!(i.channel.is_none());
    }
}
