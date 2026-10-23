//! `Core`: the shared state of the service. Stores, paths, tools, the job table
//! and the notification plumbing. Behaviour lives in `ingest`, `jobs` and `views`.

use crate::browsing::BrowsingState;
use crate::content::ContentCache;
use crate::error::Result;
use crate::util::{random_token, DAY_MS};
use crate::videodb::VideoDb;
use crate::ytdlp::{Fetcher, Runner, ToolsStatus, UreqFetcher};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::Duration;

pub const APP_NAME: &str = "muninn";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const DEFAULT_PORT: u16 = 47871;

#[derive(Debug, Clone)]
pub struct Paths {
    pub data_dir: PathBuf,
    /// Evictable: video files.
    pub cache_dir: PathBuf,
    pub video_dir: PathBuf,
    /// Persistent: promoted ("library") videos. Never evicted.
    pub library_dir: PathBuf,
    pub db_dir: PathBuf,
    pub bin_dir: PathBuf,
}

impl Paths {
    pub fn new(data_dir: &Path) -> Paths {
        let cache_dir = data_dir.join("cache");
        Paths {
            data_dir: data_dir.to_path_buf(),
            video_dir: cache_dir.join("video"),
            cache_dir,
            library_dir: data_dir.join("library"),
            db_dir: data_dir.join("db"),
            bin_dir: data_dir.join("bin"),
        }
    }

    pub fn create_all(&self) -> Result<()> {
        for dir in [&self.video_dir, &self.library_dir, &self.db_dir, &self.bin_dir] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct CoreConfig {
    pub data_dir: PathBuf,
    /// Replaces the yt-dlp runner (tests).
    pub runner: Option<Arc<dyn Runner>>,
    /// Replaces the HTTP fetcher used for yt-dlp, thumbnails (tests).
    pub fetcher: Option<Arc<dyn Fetcher>>,
    /// Replaces the tool search directories (tests).
    pub tool_dirs: Option<Vec<PathBuf>>,
    /// Start the scheduler and thumbnail threads.
    pub spawn_workers: bool,
    /// Tools shipped inside the app bundle (`Contents/Resources/bin`).
    pub bundled_bin: Option<PathBuf>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct OpenRequest {
    pub seq: u64,
    pub video_id: String,
    pub ts: i64,
}

pub type OpenHook = Arc<dyn Fn(&OpenRequest) + Send + Sync>;
pub type FeedRefreshHook = Arc<dyn Fn(&str) -> Result<()> + Send + Sync>;

pub struct RunningJob {
    pub cancel: AtomicBool,
    pub started_at: i64,
}

pub struct Core {
    pub(crate) account: RwLock<crate::account::Account>,
    pub(crate) offline_paused: Mutex<HashSet<String>>,
    pub(crate) catalog_running: Mutex<HashSet<String>>,
    pub(crate) local_playbacks: Mutex<crate::local_playback::LocalPlaybacks>,
    pub(crate) local_playback_gate: Mutex<()>,
    pub(crate) channel_lock: Mutex<()>,
    pub feed_refresh_hook: RwLock<Option<FeedRefreshHook>>,
    pub(crate) log_lock: Mutex<()>,
    pub(crate) log_coalesce: Mutex<HashMap<String, (i64, u64)>>,
    pub(crate) completion_lock: Mutex<()>,
    pub(crate) entry_lock: Mutex<()>,
    pub(crate) organization_lock: Mutex<()>,
    pub(crate) playback_leases: Mutex<HashMap<String,i64>>,
    pub(crate) subscription_polling: AtomicBool,
    pub paths: Paths,
    pub video: VideoDb,
    pub content: ContentCache,
    pub browsing: BrowsingState,
    pub(crate) token: RwLock<String>,
    pub(crate) tools: RwLock<ToolsStatus>,
    pub(crate) tool_dirs: Vec<PathBuf>,
    pub(crate) runner: Option<Arc<dyn Runner>>,
    pub(crate) fetcher: Arc<dyn Fetcher>,
    pub(crate) running: Mutex<HashMap<String, Arc<RunningJob>>>,
    pub(crate) opens: Mutex<(u64, VecDeque<OpenRequest>)>,
    pub(crate) open_hook: RwLock<Option<OpenHook>>,
    pub(crate) wake: (Mutex<bool>, Condvar),
    pub(crate) thumbs: (Mutex<(VecDeque<String>, HashSet<String>)>, Condvar),
    pub(crate) shutdown: AtomicBool,
    pub(crate) port: AtomicU16,
    pub(crate) now_override: Mutex<Option<i64>>,
    pub(crate) capture_log: Mutex<VecDeque<crate::capture::Capture>>,
    pub(crate) proxy_info: RwLock<Option<crate::proxy::ProxyInfo>>,
    pub(crate) proxy_stats: RwLock<Option<Arc<crate::proxy::ProxyStats>>>,
    pub(crate) ca: RwLock<Option<Arc<crate::ca::Ca>>>,
    pub(crate) setup: Mutex<crate::tools_setup::ToolsSetup>,
    pub streams: crate::stream::Streams,
    pub(crate) meta_fetches: Mutex<HashMap<String, String>>,
}

impl Core {
    pub fn open(config: CoreConfig) -> Result<Arc<Core>> {
        let paths = Paths::new(&config.data_dir);
        paths.create_all()?;
        let token = load_or_create_token(&paths.data_dir.join("token"))?;
        let video = VideoDb::open(&paths.db_dir.join("video.sqlite"))?;
        let content = ContentCache::open(&paths.db_dir.join("content.sqlite"))?;
        let browsing = BrowsingState::open(&paths.db_dir.join("browsing.sqlite"))?;
        if let Some(dir) = &config.bundled_bin {
            crate::ytdlp::set_bundled_dir(dir.clone());
        }
        let tool_dirs = config.tool_dirs.clone().unwrap_or_else(crate::ytdlp::search_dirs);
        let tools = ToolsStatus::detect(&paths.bin_dir, &tool_dirs);
        let core = Arc::new_cyclic(|weak| Core {
            account: RwLock::new(Default::default()),
            offline_paused: Mutex::new(HashSet::new()),
            channel_lock: Mutex::new(()),
            catalog_running: Mutex::new(HashSet::new()),
            local_playbacks: Mutex::new(Default::default()),
            local_playback_gate: Mutex::new(()),
            feed_refresh_hook: RwLock::new(None),
            log_lock: Mutex::new(()),
            log_coalesce: Mutex::new(HashMap::new()),
            completion_lock: Mutex::new(()),
            entry_lock: Mutex::new(()),
            organization_lock: Mutex::new(()),
            playback_leases: Mutex::new(HashMap::new()),
            subscription_polling: AtomicBool::new(false),
            paths,
            video,
            content,
            browsing,
            token: RwLock::new(token),
            tools: RwLock::new(tools),
            tool_dirs,
            runner: config.runner,
            fetcher: Arc::new(crate::offline::OfflineFetcher {
                core: weak.clone(),
                inner: config.fetcher.unwrap_or_else(|| Arc::new(UreqFetcher)),
            }),
            running: Mutex::new(HashMap::new()),
            opens: Mutex::new((0, VecDeque::new())),
            open_hook: RwLock::new(None),
            wake: (Mutex::new(false), Condvar::new()),
            thumbs: (Mutex::new((VecDeque::new(), HashSet::new())), Condvar::new()),
            shutdown: AtomicBool::new(false),
            port: AtomicU16::new(0),
            now_override: Mutex::new(None),
            capture_log: Mutex::new(VecDeque::new()),
            proxy_info: RwLock::new(None),
            proxy_stats: RwLock::new(None),
            ca: RwLock::new(None),
            setup: Mutex::new(Default::default()),
            streams: Default::default(),
            meta_fetches: Mutex::new(HashMap::new()),
        });
        core.clear_hls();
        core.migrate_watch_retention()?;
        core.video.requeue_interrupted()?;
        core.reconcile()?;
        core.log_event("info", "app.started", serde_json::json!({"version":VERSION,"platform":std::env::consts::OS}));
        if config.spawn_workers {
            core.clone().spawn_workers();
            core.bootstrap_tools();
        }
        Ok(core)
    }

    /// Current time in ms; tests can pin it.
    pub fn now(&self) -> i64 {
        self.now_override.lock().ok().and_then(|g| *g).unwrap_or_else(crate::util::now_ms)
    }

    pub fn set_now_for_tests(&self, now: Option<i64>) {
        *self.now_override.lock().unwrap() = now;
    }

    pub fn token(&self) -> String {
        self.token.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn check_token(&self, candidate: &str) -> bool {
        crate::util::ct_eq(&self.token(), candidate)
    }

    pub fn port(&self) -> u16 {
        self.port.load(Ordering::SeqCst)
    }

    pub fn set_port(&self, port: u16) {
        self.port.store(port, Ordering::SeqCst);
        let _ = std::fs::write(
            self.paths.data_dir.join("service.json"),
            serde_json::json!({"port": port, "version": VERSION, "pid": std::process::id()}).to_string(),
        );
    }

    pub fn set_proxy(&self, stats: Arc<crate::proxy::ProxyStats>, ca: Arc<crate::ca::Ca>) {
        *self.proxy_stats.write().unwrap_or_else(|e| e.into_inner()) = Some(stats);
        *self.ca.write().unwrap_or_else(|e| e.into_inner()) = Some(ca);
    }

    pub fn proxy_info(&self) -> Option<crate::proxy::ProxyInfo> {
        self.proxy_info.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn proxy_stats(&self) -> Option<Arc<crate::proxy::ProxyStats>> {
        self.proxy_stats.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn ca(&self) -> Option<Arc<crate::ca::Ca>> {
        self.ca.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn tools(&self) -> ToolsStatus {
        self.tools.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn refresh_tools(&self) -> ToolsStatus {
        let fresh = ToolsStatus::detect(&self.paths.bin_dir, &self.tool_dirs);
        *self.tools.write().unwrap_or_else(|e| e.into_inner()) = fresh.clone();
        fresh
    }

    pub fn set_open_hook(&self, hook: Option<OpenHook>) {
        *self.open_hook.write().unwrap_or_else(|e| e.into_inner()) = hook;
    }

    pub fn wake_scheduler(&self) {
        let (lock, cv) = &self.wake;
        *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
        cv.notify_all();
    }

    pub fn shutdown(&self) {
        self.log_event("info", "app.stopped", serde_json::json!({}));
        self.shutdown.store(true, Ordering::SeqCst);
        self.wake_scheduler();
        self.thumbs.1.notify_all();
        let running: Vec<_> = self.running.lock().unwrap().values().cloned().collect();
        for job in running {
            job.cancel.store(true, Ordering::SeqCst);
        }
    }

    fn spawn_workers(self: Arc<Self>) {
        let core = self.clone();
        std::thread::Builder::new()
            .name("muninn-scheduler".into())
            .spawn(move || {
                while !core.shutdown.load(Ordering::SeqCst) {
                    core.tick();
                    let (lock, cv) = &core.wake;
                    let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
                    let (mut guard, _) = cv.wait_timeout_while(guard, Duration::from_secs(5), |woken| !*woken).unwrap_or_else(|e| e.into_inner());
                    *guard = false;
                }
            })
            .ok();
        let core = self;
        std::thread::Builder::new().name("muninn-thumbs".into()).spawn(move || core.thumb_loop()).ok();
    }

    /// Brings the index and the disk back into agreement after a crash or a manual delete.
    fn reconcile(&self) -> Result<()> {
        for e in self.video.list_entries()? {
            match (&e.file_path, e.status.as_str()) {
                (Some(path), "complete") => match std::fs::metadata(path) {
                    Ok(m) => {
                        if m.len() != e.size_bytes {
                            self.video.set_progress(&e.id, m.len(), Some(m.len()), 1.0, None)?;
                        }
                    }
                    Err(_) => self.video.delete_entry(&e.id)?,
                },
                (_, "complete") => self.video.delete_entry(&e.id)?,
                _ => {}
            }
        }
        Ok(())
    }

    pub(crate) fn retention_expiry(&self, now: i64, days: u32) -> Option<i64> {
        (days > 0).then(|| now + days as i64 * DAY_MS)
    }
}

/// A new random token every launch: it only lets Muninn's own UI and its helper
/// processes talk to the local server, so there is nothing to keep or to pair.
fn load_or_create_token(path: &Path) -> Result<String> {
    let fresh = random_token();
    write_token(path, &fresh)?;
    Ok(fresh)
}

fn write_token(path: &Path, token: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, token)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;

    pub fn test_core(runner: Option<Arc<dyn Runner>>, fetcher: Option<Arc<dyn Fetcher>>) -> (tempfile::TempDir, Arc<Core>) {
        let dir = tempfile::tempdir().unwrap();
        let core = Core::open(CoreConfig {
            data_dir: dir.path().to_path_buf(),
            runner,
            fetcher,
            tool_dirs: Some(vec![]),
            spawn_workers: false,
            bundled_bin: None,
        })
        .unwrap();
        (dir, core)
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;

    #[test]
    fn token_is_fresh_every_launch_and_private() {
        let (dir, core) = test_core(None, None);
        let t = core.token();
        assert_eq!(t.len(), 64);
        let perms = std::fs::metadata(dir.path().join("token")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(perms.permissions().mode() & 0o777, 0o600, "token file must be private");
        }
        let _ = perms;
        drop(core);
        let again = Core::open(CoreConfig { data_dir: dir.path().to_path_buf(), tool_dirs: Some(vec![]), ..CoreConfig::default() }).unwrap();
        assert_ne!(again.token(), t, "a new token every launch");
        assert!(!again.check_token(&t));
        assert!(again.check_token(&again.token()));
    }

    #[test]
    fn data_layout_has_cache_and_library() {
        let (dir, core) = test_core(None, None);
        assert!(dir.path().join("cache/video").is_dir());
        assert!(dir.path().join("library").is_dir());
        assert_eq!(core.paths.video_dir, dir.path().join("cache/video"));
    }
}
