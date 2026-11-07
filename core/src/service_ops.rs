//! Application operations used by the HTTP adapter.
//!
//! Helper discovery and browser credentials stay inside Core. The server only
//! receives the bounded playlist metadata, never the credential jar or fetcher.

use crate::playlist::{self, Playlist};
use crate::{Core, Error, Result};

impl Core {
    /// Resolve the same bounded playlist metadata used by the existing UI.
    /// The temporary cookie jar lives for this helper call and is removed on
    /// completion or timeout, exactly as for other Core-owned helper operations.
    pub async fn playlist_metadata(
        &self,
        list_id: &str,
        current_video: Option<&str>,
        requested_index: Option<usize>,
    ) -> Result<Playlist> {
        if !playlist::valid_list_id(list_id) {
            return Err(Error::bad("Invalid playlist id"));
        }
        let bin = crate::ytdlp::find_ytdlp(&self.paths.bin_dir, &self.tool_dirs)
            .ok_or_else(|| Error::bad("yt-dlp is not installed"))?;
        let url = format!("https://www.youtube.com/playlist?list={list_id}");
        let cookies = self.youtube_cookie_file()?;
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            tokio::process::Command::new(bin)
                .args(cookies.as_ref().map(|jar| jar.args()).into_iter().flatten())
                .args([
                    "--ignore-config", "--flat-playlist", "--dump-single-json",
                    "--playlist-end", "200", "--no-warnings", "--", &url,
                ])
                .kill_on_drop(true)
                .env("PATH", crate::ytdlp::search_path())
                .stdin(std::process::Stdio::null())
                .output(),
        )
        .await
        .map_err(|_| Error::bad("Playlist lookup timed out"))?
        .map_err(|e| Error::other(format!("Could not run yt-dlp: {e}")))?;
        if !output.status.success() {
            return Err(Error::bad("yt-dlp could not read playlist metadata"));
        }
        let raw = String::from_utf8_lossy(&output.stdout);
        playlist::parse_flat_json(&raw, list_id, current_video, requested_index).map_err(Error::bad)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::testutil::test_core;

    #[tokio::test]
    async fn playlist_adapter_rejects_invalid_ids_before_helper_lookup() {
        let (_dir, core) = test_core(None, None);
        let error = core.playlist_metadata("../invalid", None, None).await.unwrap_err();
        assert!(matches!(error, Error::BadRequest(_)));
        assert_eq!(error.to_string(), "bad request: Invalid playlist id");
    }

    #[tokio::test]
    async fn playlist_adapter_reports_missing_helper_without_network() {
        let (_dir, core) = test_core(None, None);
        let error = core.playlist_metadata("PLsynthetic", None, None).await.unwrap_err();
        assert!(matches!(error, Error::BadRequest(_)));
        assert_eq!(error.to_string(), "bad request: yt-dlp is not installed");
    }
}
