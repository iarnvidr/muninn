//! Application operations used by the HTTP adapter.
//!
//! Helper discovery and browser credentials stay inside Core. The server only
//! receives the bounded playlist metadata, never the credential jar or fetcher.

use crate::playlist::{self, Playlist};
use crate::{Core, Error, Result};

/// Every attempt is metadata-only, tightly bounded and kept on the local backend.
fn playlist_attempts(list_id: &str, current_video: Option<&str>, have_cookies: bool) -> Vec<(String, bool)> {
    let url = format!("https://www.youtube.com/playlist?list={list_id}");
    let watch = current_video.filter(|id| crate::util::valid_video_id(id))
        .map(|id| format!("https://www.youtube.com/watch?v={id}&list={list_id}"));
    let mut attempts = vec![(url.clone(), have_cookies)];
    if let Some(watch) = &watch { attempts.push((watch.clone(), have_cookies)); }
    // A rotated signed-in YouTube session must not prevent access to a public playlist.
    if have_cookies {
        attempts.push((url, false));
        if let Some(watch) = watch { attempts.push((watch, false)); }
    }
    attempts
}

fn saved_playlist(core: &Core, list_id: &str, current_video: Option<&str>,
    requested_index: Option<usize>) -> Option<Playlist> {
    let text = core.video.kv_get(&format!("playlist-queue:{list_id}")).ok().flatten()?;
    let mut queue: Playlist = serde_json::from_str(&text).ok()?;
    if queue.list_id != list_id || queue.items.len() > playlist::MAX_ITEMS { return None; }
    queue.select(current_video, requested_index);
    queue.warning = Some("Using your saved playlist order because YouTube could not refresh it.".into());
    Some(queue)
}

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
        if current_video.is_some_and(|id| !crate::util::valid_video_id(id)) {
            return Err(Error::bad("Invalid current video id"));
        }
        if self.is_offline() {
            return saved_playlist(self, list_id, current_video, requested_index)
                .ok_or_else(|| Error::bad("Playlist not available offline until it has been saved."));
        }
        let bin = crate::ytdlp::find_ytdlp(&self.paths.bin_dir, &self.tool_dirs)
            .ok_or_else(|| Error::bad("yt-dlp is not installed"))?;
        // Private native cookie material never leaves Core or enters an error response.
        let cookies = self.youtube_cookie_file()?;
        let mut last_error = "YouTube did not return playlist metadata.".to_string();
        for (url, with_cookies) in playlist_attempts(list_id, current_video, cookies.is_some()) {
            let mut command = tokio::process::Command::new(&bin);
            command.args(["--ignore-config", "--flat-playlist", "--skip-download",
                "--yes-playlist", "--dump-single-json", "--playlist-end", "200",
                "--socket-timeout", "12", "--retries", "1", "--no-warnings"]);
            if with_cookies {
                command.args(cookies.as_ref().map(|jar| jar.args()).into_iter().flatten());
            }
            let output = tokio::time::timeout(std::time::Duration::from_secs(18),
                command.arg("--").arg(&url).kill_on_drop(true)
                    .env("PATH", crate::ytdlp::search_path())
                    .stdin(std::process::Stdio::null()).output()).await;
            match output {
                Err(_) => { last_error = "Playlist metadata lookup timed out.".into(); }
                Ok(Err(_)) => { last_error = "Could not start playlist metadata lookup.".into(); }
                Ok(Ok(output)) => {
                    if !output.status.success() {
                        // Classify only public error categories; never echo stderr/cookies.
                        let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
                        last_error = if stderr.contains("sign in") || stderr.contains("private") {
                            "The playlist may require signing in to YouTube.".into()
                        } else if stderr.contains("http error 400") {
                            "YouTube rejected the playlist request (HTTP 400).".into()
                        } else if stderr.contains("not available") {
                            "The playlist is unavailable from YouTube.".into()
                        } else {
                            "yt-dlp could not read playlist metadata.".into()
                        };
                    } else if output.stdout.len() > 8 * 1024 * 1024 {
                        last_error = "Playlist metadata response exceeded the size limit.".into();
                    } else {
                        match playlist::parse_flat_json(&String::from_utf8_lossy(&output.stdout),
                            list_id, current_video, requested_index) {
                            Ok(queue) if !queue.items.is_empty() => {
                                // Safe, validated metadata only; no browser credentials persisted.
                                let _ = self.video.kv_put(&format!("playlist-queue:{list_id}"),
                                    &serde_json::to_string(&queue).unwrap_or_default());
                                return Ok(queue);
                            }
                            Ok(_) => { last_error = "YouTube returned an empty playlist.".into(); }
                            Err(_) => { last_error = "YouTube returned invalid playlist metadata.".into(); }
                        }
                    }
                }
            }
        }
        if let Some(queue) = saved_playlist(self, list_id, current_video, requested_index) {
            return Ok(queue);
        }
        Err(Error::bad(last_error))
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

    #[test]
    fn public_playlists_retry_safely_with_watch_url_and_without_rotated_cookies() {
        let x = playlist_attempts("PLknown", Some("aaaaaaaaaaa"), true);
        assert_eq!(x.len(), 4);
        assert!(x[0].0.contains("/playlist?list=PLknown") && x[0].1);
        assert!(x[1].0.contains("watch?v=aaaaaaaaaaa&list=PLknown") && x[1].1);
        assert!(!x[2].1 && !x[3].1);
        let anonymous = playlist_attempts("PLknown", None, false);
        assert_eq!(anonymous.len(), 1);
        assert!(!anonymous[0].1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn playlist_falls_back_to_watch_extraction_then_saved_metadata() {
        use std::os::unix::fs::PermissionsExt;
        let (_temp, core) = test_core(None, None);
        let tool = core.paths.bin_dir.join("yt-dlp");
        std::fs::create_dir_all(&core.paths.bin_dir).unwrap();
        let script = r#"#!/bin/sh
case "$*" in
  *"watch?v=aaaaaaaaaaa&list=PLsynthetic"*)
    printf '%s' '{"entries":[{"id":"aaaaaaaaaaa","title":"First"},{"id":"bbbbbbbbbbb","title":"Second"}]}'
    ;;
  *) exit 1 ;;
esac
"#;
        std::fs::write(&tool, script).unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let queue = core.playlist_metadata("PLsynthetic", Some("aaaaaaaaaaa"), None).await.unwrap();
        assert_eq!(queue.items.len(), 2);
        assert_eq!(queue.active_index, Some(0));
        assert!(queue.warning.is_none());
        std::fs::write(&tool, "#!/bin/sh\nexit 1\n").unwrap();
        let cached = core.playlist_metadata("PLsynthetic", Some("bbbbbbbbbbb"), None).await.unwrap();
        assert_eq!(cached.active_index, Some(1));
        assert_eq!(cached.items.len(), 2);
        assert!(cached.warning.is_some());
        assert!(core.video.list_entries().unwrap().is_empty(), "metadata lookup must not download video");
    }

    #[tokio::test]
    async fn playlist_adapter_reports_missing_helper_without_network() {
        let (_dir, core) = test_core(None, None);
        let error = core.playlist_metadata("PLsynthetic", None, None).await.unwrap_err();
        assert!(matches!(error, Error::BadRequest(_)));
        assert_eq!(error.to_string(), "bad request: yt-dlp is not installed");
    }
}
