//! Global limits and TTL settings.

use serde::{Deserialize, Serialize};

pub const GB: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all="snake_case")]
pub enum PipMode { #[default] Manual, Automatic, Disabled }

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all="snake_case")]
pub enum Theme { #[default] System, Classified, ClassifiedLight }
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all="snake_case")]
pub enum Accent { #[default] Blue, Red, Amber, Teal }
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all="snake_case")]
pub enum TabLayout { #[default] Horizontal, Vertical }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Test the cached experience without contacting remote services.
    pub offline_mode: bool,
    pub channel_list_ttl_hours: u32,
    pub favourite_downloads: bool,
    pub favourite_carousel: bool,
    pub favourite_carousel_days: u32,
    pub favourite_hide_started: bool,
    pub theme: Theme,
    pub accent: Accent,
    pub tab_layout: TabLayout,
    pub pip_mode: PipMode,
    pub background_playback: bool,
    pub pip_return_to_inline: bool,
    pub pause_when_pip_closes: bool,
    /// Cap for the evictable video cache.
    pub max_disk_bytes: u64,
    /// Never let the filesystem drop below this much free space because of us.
    pub min_free_bytes: u64,
    pub download_concurrency: u32,
    /// Speculative prefetch bandwidth, shared by running prefetches. 0 = unlimited.
    pub prefetch_bandwidth_kbps: u32,
    /// Master switch for rule-driven (automatic) prefetch.
    pub auto_prefetch: bool,
    pub feed_refresh_minutes: u32,
    pub comments_ttl_hours: u32,
    pub search_ttl_hours: u32,
    pub metadata_ttl_days: u32,
    /// Height Muninn's player asks for when a video is not cached yet (watch
    /// quality). Independent of the rules' cache quality.
    pub watch_height: u32,
    /// Browser tabs that keep a live web view; the rest hibernate.
    pub warm_tabs: u32,
    /// Seconds a tab must stay in the background before it may hibernate.
    pub hibernate_after_secs: u32,
    /// Shows technical panels (Diagnostics, raw status, logs) and the manual
    /// diagnostics export. Presentation only: breadcrumbs are recorded either way.
    pub developer_mode: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            offline_mode: false,
            channel_list_ttl_hours: 24,
            favourite_downloads: true,
            favourite_carousel: true,
            favourite_carousel_days: 5,
            favourite_hide_started: true,
            theme: Theme::System,
            accent: Accent::Blue,
            tab_layout: TabLayout::Horizontal,
            pip_mode: PipMode::Manual,
            background_playback: false,
            pip_return_to_inline: true,
            pause_when_pip_closes: true,
            max_disk_bytes: 20 * GB,
            min_free_bytes: 10 * GB,
            download_concurrency: 2,
            prefetch_bandwidth_kbps: 0,
            auto_prefetch: true,
            feed_refresh_minutes: 45,
            comments_ttl_hours: 24,
            search_ttl_hours: 2,
            metadata_ttl_days: 21,
            watch_height: 720,
            warm_tabs: 3,
            hibernate_after_secs: 120,
            developer_mode: false,
        }
    }
}

impl Settings {
    /// Clamps to sane values so a typo cannot disable the safety limits.
    pub fn sanitized(mut self) -> Settings {
        self.channel_list_ttl_hours = self.channel_list_ttl_hours.clamp(1, 24 * 30);
        self.favourite_carousel_days = self.favourite_carousel_days.clamp(1, 30);
        self.download_concurrency = self.download_concurrency.clamp(1, 8);
        self.feed_refresh_minutes = self.feed_refresh_minutes.clamp(5, 24 * 60);
        self.comments_ttl_hours = self.comments_ttl_hours.clamp(1, 24 * 30);
        self.search_ttl_hours = self.search_ttl_hours.clamp(1, 24 * 30);
        self.metadata_ttl_days = self.metadata_ttl_days.clamp(1, 365);
        self.max_disk_bytes = self.max_disk_bytes.max(GB / 4);
        self.watch_height = self.watch_height.clamp(144, 2160);
        self.warm_tabs = self.warm_tabs.clamp(1, 12);
        self.hibernate_after_secs = self.hibernate_after_secs.clamp(5, 7 * 86400);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_design() {
        let s = Settings::default();
        assert_eq!(s.comments_ttl_hours, 24);
        assert_eq!(s.search_ttl_hours, 2);
        assert!((30..=60).contains(&s.feed_refresh_minutes));
        assert!(s.metadata_ttl_days >= 14);
        assert_eq!((s.warm_tabs, s.watch_height), (3, 720));
        assert_eq!(s.pip_mode,PipMode::Manual);
        assert!(!s.background_playback);
        assert!(s.pip_return_to_inline && s.pause_when_pip_closes);
        assert!(!s.developer_mode);
    }

    #[test]
    fn developer_mode_defaults_off_for_older_settings_and_round_trips() {
        // Settings persisted before the field existed (no developer_mode key).
        let old = r#"{"offline_mode":true,"watch_height":1080,"warm_tabs":5}"#;
        let loaded: Settings = serde_json::from_str(old).unwrap();
        assert!(!loaded.developer_mode);
        assert!(loaded.offline_mode);
        assert_eq!(loaded.watch_height, 1080);
        let on: Settings = serde_json::from_str(r#"{"developer_mode":true}"#).unwrap();
        assert!(on.developer_mode);
        let text = serde_json::to_string(&on.clone().sanitized()).unwrap();
        let back: Settings = serde_json::from_str(&text).unwrap();
        assert_eq!(back, on);
        assert_eq!(serde_json::to_value(Settings::default()).unwrap()["developer_mode"], false);
    }

    #[test]
    fn sanitizing_clamps() {
        let s = Settings { download_concurrency: 0, max_disk_bytes: 1, metadata_ttl_days: 0, ..Settings::default() }.sanitized();
        assert_eq!(s.download_concurrency, 1);
        assert!(s.max_disk_bytes >= GB / 4);
        assert_eq!(s.metadata_ttl_days, 1);
    }

    #[test]
    fn partial_json_fills_defaults() {
        let s: Settings = serde_json::from_str(r#"{"download_concurrency":4}"#).unwrap();
        assert_eq!(s.download_concurrency, 4);
        assert_eq!(s.search_ttl_hours, 2);
        assert_eq!(s.pip_mode,PipMode::Manual);
        assert_eq!(s.tab_layout, TabLayout::Horizontal);
        let vertical: Settings = serde_json::from_str(r#"{"tab_layout":"vertical"}"#).unwrap();
        assert_eq!(vertical.tab_layout, TabLayout::Vertical);
        assert_eq!(serde_json::from_value::<Settings>(serde_json::to_value(vertical.clone()).unwrap()).unwrap(), vertical);
        let configured:Settings=serde_json::from_value(serde_json::json!({"pip_mode":"automatic","background_playback":true})).unwrap();
        assert_eq!(configured.pip_mode,PipMode::Automatic);
        assert!(configured.background_playback);
        assert!(serde_json::from_value::<Settings>(serde_json::json!({"pip_mode":"invalid"})).is_err());
    }
}
