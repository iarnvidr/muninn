//! Zero-setup helpers: on every launch make sure yt-dlp, ffmpeg and a JS runtime
//! are usable without the user doing anything, and keep yt-dlp current.
//!
//! The app bundle ships all three (see `build/fetch-tools.sh`). yt-dlp is copied
//! to the data dir on first run so that `yt-dlp -U` can replace it in place
//! (bundles are read-only). A build without a bundled yt-dlp (a dev build) downloads
//! the official standalone binary and verifies it against the release's
//! SHA2-256SUMS. Nothing here ever asks the user for anything.

use crate::engine::Core;
use crate::error::Result;
use crate::ytdlp::{self, ytdlp_path};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const UPDATE_EVERY_MS: i64 = 24 * 3600 * 1000;
const KV_LAST_UPDATE: &str = "ytdlp_last_update_ms";

#[derive(Debug, Clone, Serialize, Default)]
pub struct ToolsSetup {
    /// "starting", "installing", "updating", "ready" or "error".
    pub phase: String,
    pub message: String,
    pub progress: Option<f64>,
    pub last_update_log: Option<String>,
}

impl Core {
    pub fn setup_state(&self) -> ToolsSetup {
        self.setup.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn set_setup(&self, phase: &str, message: &str, progress: Option<f64>) {
        let mut s = self.setup.lock().unwrap_or_else(|e| e.into_inner());
        s.phase = phase.into();
        s.message = message.into();
        s.progress = progress;
    }

    /// Runs the one-time setup and the background update loop on its own thread.
    pub fn bootstrap_tools(self: &Arc<Self>) {
        self.set_setup("starting", "Checking tools", None);
        let core = self.clone();
        std::thread::Builder::new()
            .name("muninn-tools".into())
            .spawn(move || {
                core.run_setup_once();
                while !core.shutdown.load(std::sync::atomic::Ordering::SeqCst) {
                    for _ in 0..360 {
                        if core.shutdown.load(std::sync::atomic::Ordering::SeqCst) {
                            return;
                        }
                        std::thread::sleep(Duration::from_secs(10));
                    }
                    core.update_ytdlp_if_due();
                }
            })
            .ok();
    }

    /// Synchronous body of the setup (also used by tests).
    pub fn run_setup_once(&self) {
        if let Some(dir) = ytdlp::bundled_dir() {
            clear_quarantine(&dir);
        }
        match self.ensure_ytdlp() {
            Ok(()) => {}
            Err(e) => {
                self.set_setup("error", &format!("Could not set up yt-dlp: {e}"), None);
                self.refresh_tools();
                return;
            }
        }
        self.refresh_tools();
        // A yt-dlp that cannot report its version is unusable (damaged copy, wrong CPU, blocked by the
        // OS): replace it with the official binary, verified against the release checksums.
        let t = self.tools();
        if t.ytdlp.installed && t.ytdlp.version.is_none() {
            self.set_setup("installing", "yt-dlp would not start; downloading a fresh copy (verified)", Some(0.1));
            let _ = std::fs::remove_file(ytdlp_path(&self.paths.bin_dir));
            match ytdlp::install_ytdlp(self.fetcher.as_ref(), &self.paths.bin_dir, ytdlp::ytdlp_asset_name(), ytdlp::RELEASE_BASE) {
                Ok(log) => self.setup.lock().unwrap_or_else(|e| e.into_inner()).last_update_log = Some(log),
                Err(e) => {
                    self.set_setup("error", &format!("Could not get a working yt-dlp: {e}"), None);
                    self.refresh_tools();
                    return;
                }
            }
            clear_quarantine(&self.paths.bin_dir);
            self.refresh_tools();
        }
        self.wake_scheduler();
        self.update_ytdlp_if_due();
        let t = self.tools();
        if t.ytdlp.installed && t.ffmpeg.found && t.js_runtime.found {
            self.set_setup("ready", "All tools ready", None);
        } else {
            self.set_setup("ready", &t.notes.join(" "), None);
        }
    }

    fn ensure_ytdlp(&self) -> Result<()> {
        let own = ytdlp_path(&self.paths.bin_dir);
        if own.exists() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.paths.bin_dir)?;
        if let Some(bundled) = ytdlp::bundled_dir().map(|d| d.join("yt-dlp")).filter(|p| p.is_file()) {
            self.set_setup("installing", "Setting up yt-dlp", None);
            copy_executable(&bundled, &own)?;
            clear_quarantine(&self.paths.bin_dir);
            return Ok(());
        }
        // A system yt-dlp in a search dir is good enough; leave it alone.
        if ytdlp::find_in(&self.tool_dirs, "yt-dlp").is_some() {
            return Ok(());
        }
        self.set_setup("installing", "Downloading yt-dlp (one time, verified)", Some(0.1));
        let log = ytdlp::install_ytdlp(self.fetcher.as_ref(), &self.paths.bin_dir, ytdlp::ytdlp_asset_name(), ytdlp::RELEASE_BASE)?;
        self.setup.lock().unwrap_or_else(|e| e.into_inner()).last_update_log = Some(log);
        clear_quarantine(&self.paths.bin_dir);
        Ok(())
    }

    /// `yt-dlp -U` on our own copy, at most once a day, never fatal.
    pub fn update_ytdlp_if_due(&self) {
        if self.is_offline() { return; }
        let own = ytdlp_path(&self.paths.bin_dir);
        if !own.exists() {
            return;
        }
        let now = self.now();
        let last = self.video.kv_get(KV_LAST_UPDATE).ok().flatten().and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
        if now - last < UPDATE_EVERY_MS {
            return;
        }
        let prev = self.setup_state();
        self.set_setup("updating", "Updating yt-dlp in the background", None);
        let result = ytdlp::update_ytdlp(&self.paths.bin_dir);
        let _ = self.video.kv_put(KV_LAST_UPDATE, &now.to_string());
        {
            let mut s = self.setup.lock().unwrap_or_else(|e| e.into_inner());
            s.last_update_log = Some(match &result {
                Ok(t) => t.trim().chars().take(400).collect(),
                Err(e) => format!("update failed: {e}"),
            });
            s.phase = if prev.phase == "error" { "error".into() } else { "ready".into() };
            s.message = prev.message;
        }
        self.refresh_tools();
    }
}

fn copy_executable(from: &Path, to: &Path) -> Result<()> {
    let tmp: PathBuf = to.with_extension("copy");
    std::fs::copy(from, &tmp)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&tmp, to)?;
    Ok(())
}

/// Files in a downloaded zip carry the quarantine attribute; helpers started by
/// the app must not trip Gatekeeper's first-run check.
pub fn clear_quarantine(dir: &Path) {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("/usr/bin/xattr").args(["-dr", "com.apple.quarantine"]).arg(dir).output();
    }
    let _ = dir;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::testutil::test_core;

    #[cfg(unix)]
    #[test]
    fn the_bundled_ytdlp_is_copied_to_the_data_dir_once() {
        use std::os::unix::fs::PermissionsExt;
        let (_d, core) = test_core(None, None);
        let bundle = tempfile::tempdir().unwrap();
        let tool = bundle.path().join("yt-dlp");
        std::fs::write(&tool, "#!/bin/sh\necho 2026.01.01\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        ytdlp::set_bundled_dir(bundle.path().to_path_buf());
        core.ensure_ytdlp().unwrap();
        let own = ytdlp_path(&core.paths.bin_dir);
        assert!(own.exists());
        assert_ne!(std::fs::metadata(&own).unwrap().permissions().mode() & 0o111, 0);
        std::fs::write(&own, "#!/bin/sh\necho newer\n").unwrap();
        core.ensure_ytdlp().unwrap();
        assert_eq!(std::fs::read_to_string(&own).unwrap(), "#!/bin/sh\necho newer\n", "an updated copy is never overwritten by the bundled one");
    }
}
