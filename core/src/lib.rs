//! Muninn core: caches, rules, scheduler, yt-dlp worker and the localhost server.
//! Plain Rust, no GUI: it builds and tests on Linux; the Tauri app embeds it.

pub mod browsing;
pub mod automation;
pub mod organization;
pub mod logging;
pub mod diagnostics;
pub mod imports;
pub mod library_links;
pub mod local_playback;
pub mod subscriptions;
pub mod channel_groups;
pub mod channel_catalog;
mod offline;
mod featured;
mod account;
pub mod cli;
pub mod capture;
pub mod ca;
pub mod content;
pub mod engine;
pub mod error;
pub mod eviction;
pub mod extract;
pub mod ingest;
pub mod innertube;
pub mod jobs;
pub mod merge;
pub mod migrations;
pub mod model;
pub mod proxy;
pub mod quality;
pub mod range;
pub mod rules;
pub mod server;
pub mod settings;
pub mod stream;
pub mod tabs;
pub mod tools_setup;
pub mod ui_assets;
pub mod util;
pub mod videodb;
pub mod views;
pub mod watch;
pub mod ytdlp;

pub use engine::{Core, CoreConfig, OpenRequest, Paths, DEFAULT_PORT, VERSION};
pub use error::{Error, Result};
pub use server::Service;

#[cfg(test)]
mod version_tests {
    /// The shell's tauri.conf.json, the Cargo workspace and the core report one version.
    #[test]
    fn app_and_core_versions_agree() {
        let conf: serde_json::Value = serde_json::from_str(include_str!("../../src-tauri/tauri.conf.json")).unwrap();
        assert_eq!(conf["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(conf["identifier"], "io.github.iarnvidr.muninn");
        assert_eq!(conf["bundle"]["macOS"]["minimumSystemVersion"], "14.0");
    }
}
