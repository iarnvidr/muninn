//! Merging of captured YouTube data across responses.
//!
//! Port of the pure parts of `extension/lib.js`:
//!
//! * [`CommentMerger`]: accumulates pages of comments per video (capped per
//!   video, de-duplicated with [`crate::extract::comment_key`], LRU over videos)
//!   and reports whether a page changed anything.
//! * [`merge_video_meta`]: fills the empty fields of one [`VideoIngest`] from
//!   another (e.g. a `player` response merged with a `next` response).
//! * [`library_quality_for_height`]: the yt-dlp quality to request for "Save to
//!   library" given the currently playing height.

use std::collections::HashSet;

use crate::extract::comment_key;
use crate::model::{Comment, VideoIngest};

/// Result of [`CommentMerger::add`].
#[derive(Debug, Clone, Default)]
pub struct MergeResult {
    /// Everything collected so far for the video, in arrival order.
    pub comments: Vec<Comment>,
    pub total: Option<u64>,
    /// True when the call added a comment or changed the total.
    pub changed: bool,
}

struct VideoState {
    id: String,
    seen: HashSet<String>,
    list: Vec<Comment>,
    total: Option<u64>,
}

/// Merges comment pages per video, up to `max_per_video` each, remembering at
/// most `max_videos` videos (least recently used are dropped).
pub struct CommentMerger {
    /// Per-video cap; may be changed between calls (0 collects nothing).
    pub max_per_video: usize,
    max_videos: usize,
    /// Least recently used first.
    videos: Vec<VideoState>,
}

impl CommentMerger {
    /// `max_videos == 0` means the default of 30.
    pub fn new(max_per_video: usize, max_videos: usize) -> Self {
        Self { max_per_video, max_videos: if max_videos == 0 { 30 } else { max_videos }, videos: Vec::new() }
    }

    /// Move (or create) the state of `video_id` to the most recently used slot.
    fn touch(&mut self, video_id: &str) -> usize {
        if let Some(pos) = self.videos.iter().position(|v| v.id == video_id) {
            let st = self.videos.remove(pos);
            self.videos.push(st);
        } else {
            self.videos.push(VideoState { id: video_id.to_string(), seen: HashSet::new(), list: Vec::new(), total: None });
            while self.videos.len() > self.max_videos {
                self.videos.remove(0);
            }
        }
        self.videos.len() - 1
    }

    /// Add a page of comments (and the total, when known) for a video.
    pub fn add(&mut self, video_id: &str, comments: &[Comment], total: Option<u64>) -> MergeResult {
        let max = self.max_per_video;
        let idx = self.touch(video_id);
        let st = &mut self.videos[idx];
        let mut changed = false;
        for c in comments {
            if st.list.len() >= max {
                break;
            }
            if st.seen.insert(comment_key(c)) {
                st.list.push(c.clone());
                changed = true;
            }
        }
        if let Some(t) = total {
            if st.total != Some(t) {
                st.total = Some(t);
                changed = true;
            }
        }
        MergeResult { comments: st.list.clone(), total: st.total, changed }
    }

    /// Number of comments collected for a video (0 when unknown).
    pub fn count(&mut self, video_id: &str) -> usize {
        if self.videos.iter().any(|v| v.id == video_id) {
            let idx = self.touch(video_id);
            self.videos[idx].list.len()
        } else {
            0
        }
    }
}

fn fill_string(a: &str, b: &str) -> String {
    if a.is_empty() {
        b.to_string()
    } else {
        a.to_string()
    }
}

/// `a` when it is `Some` and not an empty string, else `b`.
fn fill_opt_string(a: &Option<String>, b: &Option<String>) -> Option<String> {
    match a.as_deref() {
        Some(s) if !s.is_empty() => a.clone(),
        _ => b.clone().filter(|s| !s.is_empty()),
    }
}

/// Fill the empty fields of `a` from `b`: every non-empty field of `a` wins,
/// `b` supplies the rest (`related` included).
pub fn merge_video_meta(a: &VideoIngest, b: Option<&VideoIngest>) -> VideoIngest {
    let Some(b) = b else { return a.clone() };
    VideoIngest {
        video_id: fill_string(&a.video_id, &b.video_id),
        title: fill_string(&a.title, &b.title),
        channel: fill_opt_string(&a.channel, &b.channel),
        channel_id: fill_opt_string(&a.channel_id, &b.channel_id),
        duration_seconds: a.duration_seconds.or(b.duration_seconds),
        description: fill_opt_string(&a.description, &b.description),
        thumbnail: fill_opt_string(&a.thumbnail, &b.thumbnail),
        view_count: a.view_count.or(b.view_count),
        published_text: fill_opt_string(&a.published_text, &b.published_text),
        is_live: a.is_live.or(b.is_live),
        related: if a.related.is_empty() { b.related.clone() } else { a.related.clone() },
        captured_at: a.captured_at.or(b.captured_at),
    }
}

/// Quality to request for "Save to library" given the current playing height.
pub fn library_quality_for_height(height: Option<u32>) -> &'static str {
    match height {
        Some(h) if h >= 720 => "720",
        Some(h) if h >= 480 => "480",
        _ => "360",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Item;

    fn c(id: &str) -> Comment {
        Comment { id: Some(id.into()), author: "a".into(), text: id.into(), ..Comment::default() }
    }

    fn ids(list: &[Comment]) -> Vec<&str> {
        list.iter().map(|c| c.id.as_deref().unwrap_or("")).collect()
    }

    #[test]
    fn comment_merger_caps_per_video_dedupes_and_tracks_total() {
        let mut m = CommentMerger::new(3, 2);
        let r = m.add("v1", &[c("1"), c("2")], Some(10));
        assert_eq!(ids(&r.comments), ["1", "2"]);
        assert_eq!(r.total, Some(10));
        assert!(r.changed);
        let r = m.add("v1", &[c("2"), c("3"), c("4")], None);
        assert_eq!(ids(&r.comments), ["1", "2", "3"]); // capped at 3
        assert_eq!(r.total, Some(10)); // total is kept
        let r = m.add("v1", &[c("5")], None);
        assert!(!r.changed);
        assert_eq!(m.count("v1"), 3);
        assert_eq!(m.count("never-seen"), 0);
        // a changed total alone counts as a change
        assert!(m.add("v1", &[], Some(11)).changed);
        assert!(!m.add("v1", &[], Some(11)).changed);

        let mut dynamic = CommentMerger::new(1, 0);
        assert_eq!(dynamic.add("x", &[c("1"), c("2")], None).comments.len(), 1);
        dynamic.max_per_video = 2; // the cap can change between calls
        assert_eq!(dynamic.add("x", &[c("1"), c("2")], None).comments.len(), 2);
        let mut none = CommentMerger::new(0, 0);
        assert_eq!(none.add("x", &[c("1")], None).comments.len(), 0);
    }

    #[test]
    fn comment_merger_dedupes_comments_without_ids_by_author_and_text() {
        let mut m = CommentMerger::new(10, 5);
        let anon = |t: &str| Comment { author: "a".into(), text: t.into(), ..Comment::default() };
        let r = m.add("v", &[anon("x"), anon("x"), anon("y")], None);
        assert_eq!(r.comments.len(), 2);
    }

    #[test]
    fn comment_merger_evicts_the_least_recently_used_video() {
        let mut m = CommentMerger::new(5, 2);
        m.add("a", &[c("1")], None);
        m.add("b", &[c("1")], None);
        assert_eq!(m.count("a"), 1); // touches a, so b is now the oldest
        m.add("c", &[c("1")], None);
        assert_eq!(m.count("b"), 0);
        assert_eq!(m.count("a"), 1);
        assert_eq!(m.count("c"), 1);
    }

    #[test]
    fn merge_video_meta_fills_gaps_and_prefers_non_empty_new_values() {
        let a = VideoIngest { video_id: "a".into(), title: "T".into(), description: Some(String::new()), ..VideoIngest::default() };
        let b = VideoIngest { video_id: "a".into(), title: "old".into(), description: Some("d".into()), channel: Some("c".into()), ..VideoIngest::default() };
        let m = merge_video_meta(&a, Some(&b));
        assert_eq!(m.video_id, "a");
        assert_eq!(m.title, "T");
        assert_eq!(m.description.as_deref(), Some("d"));
        assert_eq!(m.channel.as_deref(), Some("c"));
        let only = VideoIngest { video_id: "z".into(), view_count: Some(5), ..VideoIngest::default() };
        let m = merge_video_meta(&only, None);
        assert_eq!((m.video_id.as_str(), m.view_count), ("z", Some(5)));
    }

    #[test]
    fn merge_video_meta_related_numbers_and_flags() {
        let item = |id: &str| Item { video_id: id.into(), title: id.into(), ..Item::default() };
        let with_related = VideoIngest { video_id: "a".into(), title: "T".into(), related: vec![item("r1")], ..VideoIngest::default() };
        let player = VideoIngest { video_id: "a".into(), title: "T".into(), duration_seconds: Some(212), view_count: Some(0), is_live: Some(false), related: vec![], ..VideoIngest::default() };
        let m = merge_video_meta(&player, Some(&with_related));
        assert_eq!(m.related.len(), 1);
        assert_eq!(m.duration_seconds, Some(212));
        assert_eq!(m.view_count, Some(0)); // zero is a value, not a gap
        assert_eq!(m.is_live, Some(false));
        // a's related wins when it has some
        let m = merge_video_meta(&with_related, Some(&player));
        assert_eq!(m.related[0].video_id, "r1");
        assert_eq!(m.duration_seconds, Some(212));
    }

    #[test]
    fn library_quality() {
        assert_eq!(library_quality_for_height(Some(1080)), "720");
        assert_eq!(library_quality_for_height(Some(720)), "720");
        assert_eq!(library_quality_for_height(Some(480)), "480");
        assert_eq!(library_quality_for_height(Some(144)), "360");
        assert_eq!(library_quality_for_height(None), "360");
    }
}
