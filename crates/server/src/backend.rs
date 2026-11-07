//! Embedded implementation of the client watch contract. This owns acquisition
//! and local cache operations, not tab focus or player state.

use muninn_core::{Core, Error, Result};
use muninn_protocol::{CacheSource, WatchBackend, WatchRequest};
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Clone)]
pub struct EmbeddedBackend {
    core: Arc<Core>,
}

impl EmbeddedBackend {
    pub fn new(core: Arc<Core>) -> Self { Self { core } }
}

impl WatchBackend for EmbeddedBackend {
    fn open_watch(&self, request: &WatchRequest) -> Result<Value> {
        request.validate()?;
        let status = self.core.watch_open(&request.video_id, request.audio_only)?;
        CacheSource::from_watch_status(&status)?;
        Ok(status)
    }

    fn prepare_background(&self, request: &WatchRequest) -> Result<Value> {
        request.validate()?;
        let status = self.core.prepare_background_watch(&request.video_id, request.audio_only)?;
        CacheSource::from_watch_status(&status)?;
        Ok(status)
    }

    fn watch_status(&self, video_id: &str) -> Result<Value> {
        if !muninn_core::util::valid_video_id(video_id) {
            return Err(Error::bad("invalid video id"));
        }
        let status = self.core.watch_status(video_id);
        CacheSource::from_watch_status(&status)?;
        Ok(status)
    }

    fn keep_entry(&self, entry_id: &str) -> Result<Value> {
        let entry = self.core.promote(entry_id)?;
        Ok(json!({"ok": true, "entry": self.core.entry_view(&entry)}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use muninn_core::{CoreConfig, videodb::{EntryRow, STATUS_COMPLETE}};

    fn cached_backend() -> (tempfile::TempDir, Arc<Core>, EmbeddedBackend) {
        let dir = tempfile::tempdir().unwrap();
        let core = Core::open(CoreConfig { data_dir: dir.path().into(),
            tool_dirs: Some(vec![]), spawn_workers: false, ..Default::default() }).unwrap();
        let mut settings = core.video.settings();
        settings.offline_mode = true;
        core.apply_settings(&settings).unwrap();
        let mut entry = EntryRow::new("abcdefghijk", "720", "720", "play", 1);
        let path = core.paths.video_dir.join("abcdefghijk.720.mp4");
        std::fs::write(&path, b"synthetic local cache").unwrap();
        entry.status = STATUS_COMPLETE.into();
        entry.file_path = Some(path.display().to_string());
        entry.ext = Some("mp4".into());
        entry.size_bytes = 21;
        core.video.insert_entry(&entry).unwrap();
        let backend = EmbeddedBackend::new(core.clone());
        (dir, core, backend)
    }

    #[test]
    fn embedded_contract_uses_the_existing_cache_and_does_not_report_a_play() {
        let (_dir, core, backend) = cached_backend();
        let status = backend.open_watch(&WatchRequest { video_id: "abcdefghijk".into(), audio_only: false }).unwrap();
        let source = CacheSource::from_watch_status(&status).unwrap().unwrap();
        assert_eq!(source.path(), "/media/abcdefghijk.720");
        assert!(core.stream_job("abcdefghijk").is_none());
        assert!(core.content.get_progress("abcdefghijk").unwrap().is_none());
        assert_eq!(core.video.entries_for_video("abcdefghijk").unwrap().len(), 1);
        assert_eq!(backend.watch_status("abcdefghijk").unwrap()["playback"], status["playback"]);
    }

    #[test]
    fn keep_uses_the_exact_existing_copy_and_existing_lifecycle() {
        let (_dir, core, backend) = cached_backend();
        let result = backend.keep_entry("abcdefghijk.720").unwrap();
        assert_eq!(result["entry"]["state"], "protected");
        assert_eq!(core.video.entries_for_video("abcdefghijk").unwrap().len(), 1);
        assert!(core.stream_job("abcdefghijk").is_none());
        assert!(core.content.get_progress("abcdefghijk").unwrap().is_none());
        assert_eq!(CacheSource::from_watch_status(&backend.watch_status("abcdefghijk").unwrap())
            .unwrap().unwrap().path(), "/media/abcdefghijk.720");
    }
}
