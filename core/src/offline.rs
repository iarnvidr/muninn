//! Network policy shared by CLI, UI, workers and the embedded browser proxy.
use crate::{Core, Error, Result};
use crate::settings::Settings;
use crate::ytdlp::{Fetcher, HttpResponse};
use crate::videodb::{STATUS_CANCELED, STATUS_FAILED, STATUS_QUEUED};
use std::sync::{Arc, Weak};
use std::sync::atomic::Ordering;

pub(crate) struct OfflineFetcher {
    pub core: Weak<Core>,
    pub inner: Arc<dyn Fetcher>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{testutil::test_core, RunningJob};
    use crate::videodb::{EntryRow, STATUS_DOWNLOADING};
    use serde_json::json;
    use std::sync::atomic::AtomicUsize;

    struct Network(AtomicUsize);
    impl Fetcher for Network {
        fn get(&self, _: &str, _: u64) -> Result<Vec<u8>> { self.0.fetch_add(1,Ordering::SeqCst); Ok(vec![]) }
    }
    #[test]
    fn offline_blocks_remote_work_but_preserves_cached_channel_pages_and_local_playback() {
        let network=Arc::new(Network(AtomicUsize::new(0)));
        let (_dir,core)=test_core(None,Some(network.clone()));
        core.set_now_for_tests(Some(100_000_000));
        core.video.kv_put("channel-videos:UCabcdefghijklmnopqrstuv:0",&json!({"updated_at":1,"items":[{"video_id":"aaaaaaaaaaa","title":"Cached upload"}]}).to_string()).unwrap();
        let mut settings=core.video.settings();settings.offline_mode=true;core.apply_settings(&settings).unwrap();
        assert!(core.fetcher.get("https://example.com",1).is_err());
        assert!(core.fetcher.request("POST","https://example.com",&[],None,1).is_err());
        assert!(core.start_stream("aaaaaaaaaaa",false).is_err());
        core.poll_channels().unwrap();
        let page=core.channel_uploads("UCabcdefghijklmnopqrstuv",0,true).unwrap();
        assert_eq!(page["items"][0]["title"],"Cached upload");assert_eq!(page["offline"],true);
        assert!(core.channel_uploads("UCabcdefghijklmnopqrstuv",25,false).is_err());
        assert_eq!(network.0.load(Ordering::SeqCst),0);
        let path=core.paths.video_dir.join("aaaaaaaaaaa.720.mp4");std::fs::write(&path,b"local bytes").unwrap();
        let mut entry=EntryRow::new("aaaaaaaaaaa","720","720","manual",core.now());
        entry.status="complete".into();entry.ext=Some("mp4".into());entry.file_path=Some(path.display().to_string());entry.size_bytes=11;
        core.video.insert_entry(&entry).unwrap();
        let playback=core.watch_open("aaaaaaaaaaa",false).unwrap();
        assert_eq!(playback["playback"]["kind"],"file");assert_eq!(playback["playback"]["ready"],true);
        settings.offline_mode=false;core.apply_settings(&settings).unwrap();
        core.fetcher.get("https://example.com",1).unwrap();assert_eq!(network.0.load(Ordering::SeqCst),1);
    }
    #[test]
    fn offline_cancellation_requeues_after_worker_stops_but_explicit_cancel_does_not() {
        let (_dir,core)=test_core(None,None);
        let mut entry=EntryRow::new("aaaaaaaaaaa","720","720","manual",core.now());entry.status=STATUS_DOWNLOADING.into();
        core.video.insert_entry(&entry).unwrap();
        let job=Arc::new(RunningJob{cancel:false.into(),started_at:core.now()});
        core.running.lock().unwrap().insert(entry.id.clone(),job.clone());
        let mut settings=core.video.settings();settings.offline_mode=true;core.apply_settings(&settings).unwrap();
        assert!(job.cancel.load(Ordering::SeqCst));
        core.requeue_offline_jobs();assert_eq!(core.video.get_entry(&entry.id).unwrap().unwrap().status,STATUS_DOWNLOADING);
        core.video.set_status(&entry.id,STATUS_CANCELED,None,core.now()).unwrap();core.running.lock().unwrap().remove(&entry.id);
        core.requeue_offline_jobs();assert_eq!(core.video.get_entry(&entry.id).unwrap().unwrap().status,STATUS_QUEUED);
        core.cancel(&entry.id).unwrap();core.requeue_offline_jobs();assert_eq!(core.video.get_entry(&entry.id).unwrap().unwrap().status,STATUS_CANCELED);
    }
}
impl Fetcher for OfflineFetcher {
    fn get(&self, url: &str, max: u64) -> Result<Vec<u8>> {
        self.core.upgrade().ok_or_else(|| Error::other("service stopped"))?.require_online()?;
        self.inner.get(url, max)
    }
    fn request(&self, method: &str, url: &str, headers: &[(&str, String)], body: Option<&[u8]>, max: u64) -> Result<HttpResponse> {
        self.core.upgrade().ok_or_else(|| Error::other("service stopped"))?.require_online()?;
        self.inner.request(method, url, headers, body, max)
    }
}
impl Core {
    pub fn is_offline(&self) -> bool { self.video.settings().offline_mode }
    pub fn require_online(&self) -> Result<()> {
        if self.is_offline() { Err(Error::Conflict("Offline mode is on. Reconnect in Settings to fetch new content.".into())) }
        else { Ok(()) }
    }
    pub fn apply_settings(&self, settings: &Settings) -> Result<()> {
        let was_offline=self.is_offline();
        self.video.put_settings(settings)?;
        if self.is_offline()&&!was_offline {self.invalidate_youtube_account();}
        if self.is_offline() {
            let mut paused = self.offline_paused.lock().unwrap_or_else(|e| e.into_inner());
            for (id, job) in self.running.lock().unwrap_or_else(|e| e.into_inner()).iter() {
                paused.insert(id.clone());
                job.cancel.store(true, Ordering::SeqCst);
            }
        }
        self.wake_scheduler();
        Ok(())
    }
    /// Cancellation releases network processes; queue their incomplete entries
    /// again once they have stopped. The offline scheduler leaves them queued.
    pub(crate) fn requeue_offline_jobs(&self) {
        let mut paused = self.offline_paused.lock().unwrap_or_else(|e| e.into_inner());
        paused.retain(|id| {
            if self.is_running(id) { return true; }
            if let Ok(Some(row)) = self.video.get_entry(id) {
                if matches!(row.status.as_str(), STATUS_CANCELED | STATUS_FAILED) {
                    return self.video.set_status(id, STATUS_QUEUED, None, self.now()).is_err();
                }
            }
            false
        });
    }
}
