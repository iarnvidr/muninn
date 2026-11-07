//! Client intents, independent of Core, SQLite, the HTTP router and Tauri.
//! The native host injects the embedded backend. No remote mode is selected
//! implicitly, and this crate does not give a player upstream media URLs.

use muninn_domain::Result;
use muninn_protocol::{CacheSource, WatchBackend, WatchRequest};
use std::sync::Arc;

pub use muninn_navigation as tabs;

#[derive(Clone)]
pub struct Client {
    backend: Arc<dyn WatchBackend>,
}

impl Client {
    pub fn new(backend: Arc<dyn WatchBackend>) -> Self { Self { backend } }

    /// Creating a background watch tab is an acquisition request, not playback
    /// or focus. Reusing a tab has no side effects: its cache/job is already
    /// owned by that destination. Browsing/feeds alone do not download videos.
    /// The host runs this blocking operation away from the native main thread.
    pub fn prepare_background(&self, destination: &tabs::Loc, created: bool) -> Result<bool> {
        if !created { return Ok(false); }
        let video_id = match destination {
            tabs::Loc::Watch(video_id) | tabs::Loc::WatchInPlaylist { video_id, .. } => video_id,
            tabs::Loc::Yt(_) => return Ok(false),
        };
        let request = WatchRequest { video_id: video_id.clone(), audio_only: false };
        request.validate()?;
        let status = self.backend.prepare_background(&request)?;
        CacheSource::from_watch_status(&status)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use muninn_domain::Error;
    use serde_json::{json, Value};
    use std::sync::Mutex;

    struct Backend {
        opens: Mutex<Vec<WatchRequest>>,
        status: Value,
    }
    impl WatchBackend for Backend {
        fn open_watch(&self, _: &WatchRequest) -> Result<Value> {
            panic!("background intents must not take foreground playback slots")
        }
        fn prepare_background(&self, r: &WatchRequest) -> Result<Value> {
            self.opens.lock().unwrap().push(r.clone());
            Ok(self.status.clone())
        }
        fn watch_status(&self, _: &str) -> Result<Value> { panic!("background open must not poll") }
        fn keep_entry(&self, _: &str) -> Result<Value> { Err(Error::other("must not Keep implicitly")) }
    }
    fn client(status: Value) -> (Client, Arc<Backend>) {
        let b = Arc::new(Backend { opens: Mutex::new(vec![]), status });
        (Client::new(b.clone()), b)
    }

    #[test]
    fn a_new_background_watch_requests_one_acquisition_and_no_other_action() {
        let (client, backend) = client(json!({"playback":{"ready":false}}));
        let loc = tabs::Loc::Watch("abcdefghijk".into());
        assert!(client.prepare_background(&loc, true).unwrap());
        assert!(!client.prepare_background(&loc, false).unwrap());
        assert_eq!(*backend.opens.lock().unwrap(), vec![WatchRequest {
            video_id: "abcdefghijk".into(), audio_only: false,
        }]);
    }

    #[test]
    fn browsing_and_reused_tabs_have_no_acquisition_side_effects() {
        let (client, backend) = client(Value::Null);
        assert!(!client.prepare_background(&tabs::Loc::Yt(tabs::HOME_URL.into()), true).unwrap());
        assert!(!client.prepare_background(&tabs::Loc::Watch("abcdefghijk".into()), false).unwrap());
        assert!(backend.opens.lock().unwrap().is_empty());
    }

    #[test]
    fn playlist_background_tabs_keep_the_same_video_identity() {
        let (client, backend) = client(json!({"playback":{"ready":true,"kind":"file",
            "url_path":"/media/abcdefghijk.720"}}));
        let loc = tabs::Loc::WatchInPlaylist { video_id: "abcdefghijk".into(),
            playlist: tabs::PlaylistContext { list_id: "PLtest".into(), index: Some(2) } };
        assert!(client.prepare_background(&loc, true).unwrap());
        assert_eq!(backend.opens.lock().unwrap()[0].video_id, "abcdefghijk");
    }

    #[test]
    fn invalid_ids_and_remote_playback_are_rejected_at_the_boundary() {
        let (client, backend) = client(json!({"playback":{"ready":true,"kind":"file",
            "url_path":"https://example.test/video"}}));
        assert!(client.prepare_background(&tabs::Loc::Watch("bad".into()), true).is_err());
        assert!(backend.opens.lock().unwrap().is_empty());
        assert!(client.prepare_background(&tabs::Loc::Watch("abcdefghijk".into()), true).is_err());
    }
}
