//! Bounded playlist metadata extraction. This never downloads media.
use serde::{Deserialize, Serialize};

pub const MAX_ITEMS: usize = 200;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlaylistItem { pub id: String, pub title: String, pub available: bool }
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Playlist { pub list_id: String, pub items: Vec<PlaylistItem>, pub active_index: Option<usize>, pub truncated: bool }

pub fn valid_list_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 256 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
}
pub fn parse_flat_json(raw: &str, list_id: &str, current_video: Option<&str>, requested_index: Option<usize>) -> Result<Playlist, String> {
    if !valid_list_id(list_id) { return Err("invalid playlist id".into()); }
    let root: serde_json::Value = serde_json::from_str(raw).map_err(|_| "invalid yt-dlp playlist response")?;
    let entries = root.get("entries").and_then(|v| v.as_array()).ok_or("playlist response has no entries")?;
    let truncated = entries.len() > MAX_ITEMS;
    let items: Vec<_> = entries.iter().take(MAX_ITEMS).filter_map(|v| {
        let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("");
        if id.len() != 11 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b)) { return None; }
        let unavailable = v.get("availability").and_then(|x| x.as_str()).is_some_and(|a| a != "public" && a != "unlisted") || v.get("is_live").and_then(|x| x.as_bool()) == Some(true);
        Some(PlaylistItem { id: id.into(), title: v.get("title").and_then(|x| x.as_str()).filter(|t| !t.trim().is_empty()).unwrap_or("Unavailable video").chars().take(500).collect(), available: !unavailable })
    }).collect();
    let active_index = current_video.and_then(|id| items.iter().position(|x| x.id == id)).or_else(|| requested_index.and_then(|i| i.checked_sub(1)).filter(|i| *i < items.len()));
    Ok(Playlist { list_id: list_id.into(), items, active_index, truncated })
}

#[cfg(test)] mod tests {
 use super::*;
 #[test] fn parses_order_sanitizes_and_marks_unavailable() {
  let raw = r#"{"entries":[{"id":"aaaaaaaaaaa","title":"First"},{"id":"bad id","title":"bad"},{"id":"bbbbbbbbbbb","title":"Members","availability":"subscriber_only"},{"id":"ccccccccccc"}]}"#;
  let p=parse_flat_json(raw,"PL_123",Some("bbbbbbbbbbb"),Some(1)).unwrap();
  assert_eq!(p.items.iter().map(|x|x.id.as_str()).collect::<Vec<_>>(),vec!["aaaaaaaaaaa","bbbbbbbbbbb","ccccccccccc"]);
  assert_eq!(p.active_index,Some(1)); assert!(!p.items[1].available); assert_eq!(p.items[2].title,"Unavailable video");
 }
 #[test] fn rejects_unsafe_list_id_and_caps_results() {
  assert!(parse_flat_json("{}","../x",None,None).is_err());
  let entries=(0..205).map(|i| format!(r#"{{"id":"{:011}","title":"x"}}"#,i)).collect::<Vec<_>>().join(",");
  let p=parse_flat_json(&format!(r#"{{"entries":[{entries}]}}"#),"PLx",None,None).unwrap();
  assert_eq!(p.items.len(),MAX_ITEMS); assert!(p.truncated);
 }
}
