//! Persisted navigation data shared by storage and client navigation.
//! The representation is independent of SQLite and the native webview.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TabRow {
    pub tab_id: String,
    pub url: String,
    pub title: Option<String>,
    pub video_id: Option<String>,
    pub scroll_y: Option<f64>,
    pub position_seconds: Option<f64>,
    pub duration_seconds: Option<f64>,
    pub watch_height: Option<u32>,
    pub state: String,
    pub updated_at: i64,
}
