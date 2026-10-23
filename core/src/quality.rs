//! Cache quality versus watch quality.
//!
//! The cache quality is what is on disk; the watch quality is what the user is
//! (or was) watching in the browser. They are tracked separately and compared
//! here to say whether the cached copy can stand in for the playback.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Quality {
    #[serde(rename = "none")]
    None,
    #[serde(rename = "audio")]
    Audio,
    #[serde(rename = "360")]
    P360,
    #[serde(rename = "480")]
    P480,
    #[serde(rename = "720")]
    P720,
    #[serde(rename = "1080")]
    P1080,
}

impl Quality {
    pub fn as_str(self) -> &'static str {
        match self {
            Quality::None => "none",
            Quality::Audio => "audio",
            Quality::P360 => "360",
            Quality::P480 => "480",
            Quality::P720 => "720",
            Quality::P1080 => "1080",
        }
    }

    pub fn parse(value: &str) -> Option<Quality> {
        match value.trim().to_ascii_lowercase().trim_end_matches('p') {
            "none" | "" => Some(Quality::None),
            "audio" | "a" => Some(Quality::Audio),
            "360" => Some(Quality::P360),
            "480" => Some(Quality::P480),
            "720" => Some(Quality::P720),
            "1080" => Some(Quality::P1080),
            _ => None,
        }
    }

    /// Nominal video height; 0 for audio-only and none.
    pub fn height(self) -> u32 {
        match self {
            Quality::None | Quality::Audio => 0,
            Quality::P360 => 360,
            Quality::P480 => 480,
            Quality::P720 => 720,
            Quality::P1080 => 1080,
        }
    }

    pub fn needs_merge(self) -> bool {
        matches!(self, Quality::P480 | Quality::P720 | Quality::P1080)
    }
}

/// Whether a cached copy can serve the playback the user is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Serves {
    /// Cached video is at least as sharp as what is being watched.
    Full,
    /// Cached video is softer than what is being watched.
    Downgrade,
    /// Cached copy has no picture.
    AudioOnly,
    /// Nothing usable is cached.
    None,
    /// Cached, but no watch quality is known to compare against.
    Unknown,
}

/// `cached_height` is the real height of the cached file when known, else the
/// nominal height of its quality. `None` for no cached copy at all.
pub fn serves(cached: Option<(Quality, Option<u32>)>, watch_height: Option<u32>) -> Serves {
    let Some((quality, actual)) = cached else { return Serves::None };
    if quality == Quality::None {
        return Serves::None;
    }
    if quality == Quality::Audio {
        return Serves::AudioOnly;
    }
    let cached_height = actual.filter(|h| *h > 0).unwrap_or_else(|| quality.height());
    match watch_height.filter(|h| *h > 0) {
        None => Serves::Unknown,
        // A 1px tolerance is not needed: heights are standard rungs, but YouTube
        // reports e.g. 358 for some 16:9 360p encodes, so allow a small slack.
        Some(watch) if cached_height + 8 >= watch => Serves::Full,
        Some(_) => Serves::Downgrade,
    }
}

/// Of several cached variants, the best one for playback given the watch quality:
/// the sharpest video (or audio only when nothing else exists).
pub fn best_variant<T: Copy>(variants: &[(T, Quality, Option<u32>)]) -> Option<T> {
    variants
        .iter()
        .filter(|(_, q, _)| *q != Quality::None)
        .max_by_key(|(_, q, actual)| actual.filter(|h| *h > 0).unwrap_or_else(|| q.height()))
        .map(|(t, _, _)| *t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quality_names() {
        assert_eq!(Quality::parse("360"), Some(Quality::P360));
        assert_eq!(Quality::parse("480p"), Some(Quality::P480));
        assert_eq!(Quality::parse("Audio"), Some(Quality::Audio));
        assert_eq!(Quality::parse("none"), Some(Quality::None));
        assert_eq!(Quality::parse("1080"), Some(Quality::P1080));
        assert_eq!(Quality::parse("2160"), None);
        assert_eq!(serde_json::to_string(&Quality::P720).unwrap(), "\"720\"");
    }

    #[test]
    fn cached_quality_vs_watch_quality() {
        assert_eq!(serves(None, Some(720)), Serves::None);
        assert_eq!(serves(Some((Quality::Audio, None)), Some(720)), Serves::AudioOnly);
        assert_eq!(serves(Some((Quality::P360, Some(360))), Some(360)), Serves::Full);
        assert_eq!(serves(Some((Quality::P360, Some(360))), Some(720)), Serves::Downgrade);
        assert_eq!(serves(Some((Quality::P720, Some(720))), Some(360)), Serves::Full);
        assert_eq!(serves(Some((Quality::P360, Some(358))), Some(360)), Serves::Full);
        assert_eq!(serves(Some((Quality::P480, None)), None), Serves::Unknown);
        assert_eq!(serves(Some((Quality::None, None)), Some(360)), Serves::None);
    }

    #[test]
    fn picks_sharpest_variant() {
        let v = [("a", Quality::Audio, None), ("b", Quality::P360, Some(360)), ("c", Quality::P480, None)];
        assert_eq!(best_variant(&v), Some("c"));
        let only_audio = [("a", Quality::Audio, None)];
        assert_eq!(best_variant(&only_audio), Some("a"));
        let empty: [(&str, Quality, Option<u32>); 0] = [];
        assert_eq!(best_variant(&empty), None);
    }
}
