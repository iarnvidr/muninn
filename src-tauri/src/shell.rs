//! The browser shell: the main window with Muninn's own UI (the "chrome") and one
//! native web view per live YouTube tab laid over the chrome's content area.
//!
//! Hibernating a tab destroys its web view (the web content process goes with
//! it); the URL, scroll position and the video's playback time are kept in the
//! browsing-state DB, and the chrome shows a restore view from the content cache
//! until the live page is back.

use muninn_core::model::TabIngest;
use muninn_core::tabs::{self, classify_input, Face, Input, Loc, Move, Tab, HOME_URL};
use muninn_core::{Core, Service};
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::webview::{NewWindowResponse, WebviewBuilder};
use tauri::{AppHandle, LogicalPosition, LogicalSize, Manager, Url, WebviewUrl};

pub const MAIN: &str = "main";
pub const CHROME: &str = "chrome";
/// A clean, current Safari user agent: no Tauri/app token (Google refuses embedded browsers).
pub const SAFARI_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.4 Safari/605.1.15";
const INJECT_JS: &str = include_str!("../inject.js");
/// WKWebsiteDataStore identifier of the YouTube/Google store (macOS 14+): login lives here.
#[cfg(target_os = "macos")]
const YT_STORE: [u8; 16] = *b"muninn-yt-store1";

#[derive(Debug, Clone, Copy, Default)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Default)]
pub struct Inner {
    pub tabs: Vec<Tab>,
    pub active: Option<u64>,
    pub next_id: u64,
    pub rev: u64,
    pub rect: Rect,
    pub native_visible: bool,
    /// Last outside-open revision, retained until the chrome has observed it.
    pub handoff_rev: u64,
}

pub struct Shell {
    pub app: AppHandle,
    pub core: Arc<Core>,
    pub service: Arc<Service>,
    pub inner: Mutex<Inner>,
    pub trust: Mutex<crate::trust::TrustState>,
    channel_action_lock: Mutex<()>,
}

#[derive(Serialize)]
struct TabView {
    id: u64,
    title: String,
    url: String,
    face: Face,
    video_id: Option<String>,
    playlist: Option<muninn_core::tabs::PlaylistContext>,
    state: &'static str,
    live: bool,
    kind: &'static str,
    query: Option<String>,
    can_back: bool,
    can_forward: bool,
    last_active_ms: i64,
}

fn now_ms() -> i64 {
    muninn_core::util::now_ms()
}

impl Shell {
    pub fn new(app: AppHandle, service: Arc<Service>) -> Arc<Shell> {
        let core = service.core.clone();
        let shell = Arc::new(Shell {
            app,
            core,
            service,
            inner: Mutex::new(Inner { next_id: 1, native_visible: true, ..Inner::default() }),
            trust: Mutex::new(crate::trust::TrustState::default()),
            channel_action_lock: Mutex::new(()),
        });
        let weak = Arc::downgrade(&shell);
        *shell.core.feed_refresh_hook.write().unwrap_or_else(|e|e.into_inner()) = Some(Arc::new(move |kind| {
            let shell = weak.upgrade().ok_or_else(||muninn_core::Error::other("Muninn is closing"))?;
            let kind = kind.to_string();
            let app = shell.app.clone();
            app.run_on_main_thread(move || shell.refresh_feed(&kind)).map_err(|e|muninn_core::Error::other(e.to_string()))
        }));
        shell
    }

    /// Called on a blocking task: JavaScript runs on the native main thread,
    /// and only a sanitized acknowledgement returns from the YouTube origin.
    pub fn unsubscribe_channel(self: &Arc<Self>, channel_id: &str) -> Result<(), String> {
        let _action=self.channel_action_lock.lock().unwrap_or_else(|e|e.into_inner());
        self.core.require_online().map_err(|e|e.to_string())?;
        if channel_id.len()!=24 || !channel_id.starts_with("UC") || !channel_id.bytes().all(|b|b.is_ascii_alphanumeric()||matches!(b,b'_'|b'-')) {
            return Err("Invalid channel ID".into());
        }
        let account_key=self.core.unsubscribe_account(channel_id).map_err(|e|e.to_string())?;
        let page={let inner=self.lock();inner.tabs.iter().filter(|t|t.live&&t.yt_url.starts_with("https://www.youtube.com/"))
            .max_by_key(|t|(inner.active==Some(t.id),t.last_active_ms)).map(|t|t.id)};
        let Some(id)=page else {return Err("Open YouTube in Browse and sign in before unsubscribing.".into())};
        let view=self.app.get_webview(&label(id)).ok_or("YouTube page is no longer open")?;
        let nonce=muninn_core::util::random_token();
        let key=serde_json::to_string(&nonce).map_err(|e|e.to_string())?;
        let channel=serde_json::to_string(channel_id).map_err(|e|e.to_string())?;
        let account=serde_json::to_string(&account_key).map_err(|e|e.to_string())?;
        let script=format!("{};window.__muninnChannelResult={{key:{key},pending:true}};window.__muninnUnsubscribe(window,{channel},{account}).then(()=>{{window.__muninnChannelResult={{key:{key},ok:true}};}},e=>{{window.__muninnChannelResult={{key:{key},error:String(e.message||'YouTube unsubscribe failed')}};}});", include_str!("../channel-action.js"));
        view.eval(script).map_err(|e|e.to_string())?;
        let outcome=(|| {
            for _ in 0..50 {
                self.core.require_online().map_err(|e|e.to_string())?;
                std::thread::sleep(Duration::from_millis(500));
                let (tx,rx)=std::sync::mpsc::channel();
                view.eval_with_callback("window.__muninnChannelResult || null",move |value|{let _=tx.send(value);}).map_err(|e|e.to_string())?;
                let value=rx.recv_timeout(Duration::from_secs(2)).map_err(|_|"YouTube did not respond. Refresh subscriptions to check its state.")?;
                let result:Value=serde_json::from_str(&value).map_err(|_|"Invalid response from YouTube")?;
                if result["key"].as_str()!=Some(&nonce){return Err("YouTube page changed during unsubscribe".into());}
                if let Some(error)=result["error"].as_str(){return Err(error.to_string());}
                if result["ok"]==true {
                    self.core.confirm_channel_unsubscribed(channel_id,&account_key).map_err(|e|e.to_string())?;
                    return Ok(());
                }
            }
            Err("YouTube did not confirm unsubscribe. Refresh subscriptions before retrying.".into())
        })();
        let _=view.eval(format!("if(window.__muninnChannelResult?.key==={key})delete window.__muninnChannelResult;"));
        outcome
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn bump(inner: &mut Inner) {
        inner.rev += 1;
    }

    // --- state for the chrome -----------------------------------------------------------------

    pub fn tabs_state(&self) -> Value {
        let settings = self.core.video.settings();
        let inner = self.lock();
        let tabs: Vec<TabView> = inner
            .tabs
            .iter()
            .map(|t| {
                let (_, query) = t.kind();
                let state = if t.restoring {
                    "restoring"
                } else if !t.live {
                    "sleeping"
                } else if t.loading {
                    "loading"
                } else {
                    "live"
                };
                let title = match t.face() {
                    Face::Watch => t
                        .video_id
                        .as_deref()
                        .and_then(|v| self.core.content.get_json(muninn_core::content::KIND_META, v, self.core.now()).ok().flatten())
                        .and_then(|m| m["title"].as_str().map(str::to_string))
                        .unwrap_or_else(|| "Video".into()),
                    Face::Yt if t.title.is_empty() => "YouTube".into(),
                    Face::Yt => t.title.clone(),
                };
                TabView {
                    id: t.id,
                    title,
                    url: t.shown_url(),
                    face: t.face(),
                    video_id: t.video_id.clone(),
                    playlist: t.playlist.clone(),
                    state,
                    live: t.live && !t.restoring,
                    kind: t.kind_str(),
                    query,
                    can_back: t.can_back(),
                    can_forward: t.can_forward(),
                    last_active_ms: t.last_active_ms,
                }
            })
            .collect();
        json!({"rev": inner.rev, "active": inner.active, "handoff_rev": inner.handoff_rev, "warm_tabs": settings.warm_tabs, "hibernate_after_secs": settings.hibernate_after_secs, "tabs": tabs})
    }

    // --- persistence ---------------------------------------------------------------------------

    fn persist(&self, tab: &Tab, state: &str) {
        let position = tab.video_id.as_deref().and_then(|v| self.core.content.get_progress(v).ok().flatten()).map(|p| p.position_seconds);
        let _ = self.core.ingest_tab(TabIngest {
            tab_id: tab.id.to_string(),
            url: tab.url(),
            title: Some(tab.title.clone()).filter(|t| !t.is_empty()),
            video_id: if tab.face() == Face::Watch { tab.video_id.clone() } else { None },
            scroll_y: Some(tab.scroll_y),
            position_seconds: position,
            duration_seconds: None,
            watch_height: None,
            state: state.into(),
            ts: None,
        });
    }

    /// Restores the tab list from the browsing DB (all tabs start asleep). Tabs that show
    /// the same destination are collapsed to one and the duplicates leave the DB.
    pub fn restore_tabs(self: &Arc<Self>) {
        let rows = self.core.browsing.tabs().unwrap_or_default();
        let active_id = self.core.video.kv_get("active_tab").ok().flatten().and_then(|v| v.parse::<u64>().ok());
        let (restored, next_id) = tabs::restore_from_rows(&rows, active_id);
        let active_id = restored.active;
        let kept: Vec<Tab> = {
            let mut inner = self.lock();
            inner.next_id = inner.next_id.max(next_id);
            inner.tabs = restored.tabs;
            Self::bump(&mut inner);
            inner.tabs.clone()
        };
        for id in &restored.removed {
            let _ = self.core.browsing.upsert_tab(&TabIngest { tab_id: id.to_string(), state: "closed".into(), ..Default::default() }, now_ms());
        }
        for tab in kept.iter().filter(|t| restored.merged.contains(&t.id)) {
            self.persist(tab, "hidden");
        }
        if !restored.removed.is_empty() {
            self.core.log_event("info", "tab.restore.deduplicated", json!({"removed": restored.removed.len()}));
        }
        self.adopt_imported_tabs();
        let first = {
            let inner = self.lock();
            active_id.filter(|a| inner.tabs.iter().any(|t| t.id == *a)).or_else(|| inner.tabs.first().map(|t| t.id))
        };
        match first {
            Some(id) => self.activate(id),
            None => {
                self.new_tab(None);
            }
        }
    }

    // --- tab operations --------------------------------------------------------------------------

    /// The one way to open a destination: finds the tab already showing it (sleeping tabs
    /// included) or allocates one, inside a single critical section so concurrent requests
    /// yield one tab. An existing tab is left exactly as it is (playback, scroll, history).
    /// Returns the id and whether it was created.
    fn find_or_open(&self, loc: Loc) -> (u64, bool) {
        let mut inner = self.lock();
        let inner = &mut *inner;
        let opened = tabs::find_or_open(&mut inner.tabs, &mut inner.next_id, loc, now_ms());
        if let tabs::Opened::Created(id) = opened {
            // Created and shown right away: nothing to restore for a new tab.
            if let Some(t) = inner.tabs.iter_mut().find(|t| t.id == id) {
                t.restoring = false;
            }
            Self::bump(inner);
        }
        (opened.id(), matches!(opened, tabs::Opened::Created(_)))
    }

    /// Opens a destination by find-or-open. `background` never takes the focus.
    fn open_dest(self: &Arc<Self>, loc: Loc, background: bool) -> u64 {
        let (id, created) = self.find_or_open(loc);
        if background {
            if created {
                if let Some(t) = self.lock().tabs.iter().find(|t| t.id == id).cloned() {
                    self.persist(&t, "hidden");
                }
            }
        } else {
            self.activate(id);
        }
        id
    }

    /// Load through the existing persistent YouTube session without switching away
    /// from the feed page or the video playing in picture-in-picture.
    fn refresh_feed(self: &Arc<Self>, kind: &str) {
        if self.core.is_offline() { return; }
        let url = match kind { "channels" => "https://www.youtube.com/feed/channels", "subscriptions" => "https://www.youtube.com/feed/subscriptions", _ => HOME_URL };
        let (id, _) = self.find_or_open(Loc::Yt(url.into()));
        {
            let mut inner = self.lock();
            if let Some(tab) = inner.tabs.iter_mut().find(|t| t.id == id) {
                tab.loading = true;
                tab.last_active_ms = now_ms();
            }
            Self::bump(&mut inner);
        }
        if self.view_exists(id) { self.eval_tab(id,"location.reload()"); }
        else { self.create_view(id); }
        if let Some(tab) = self.lock().tabs.iter().find(|t|t.id==id).cloned() { self.persist(&tab,"hidden"); }
        self.sync_native();
    }

    /// Opens a destination in a tab: the existing tab for it when there is one (activated),
    /// else a new one. A new blank tab is the Home destination.
    pub fn new_tab(self: &Arc<Self>, url: Option<&str>) -> u64 {
        self.core.log_event("info", "tab.new", json!({"has_url":url.is_some()}));
        let loc = match url.map(classify_input) {
            Some(Input::Video(v)) => Loc::Watch(v),
            Some(Input::VideoInPlaylist { video_id, playlist }) => Loc::WatchInPlaylist { video_id, playlist },
            Some(Input::Yt(u)) => Loc::Yt(u),
            Some(Input::Search(q)) => Loc::Yt(tabs::search_url(&q)),
            Some(Input::External(u)) => {
                let _ = crate::commands::open_in_browser(&u);
                Loc::Yt(HOME_URL.into())
            }
            None => Loc::Yt(HOME_URL.into()),
        };
        self.open_dest(loc, false)
    }

    /// The chrome may still show Library/Settings, or may not have loaded yet.
    /// Retain the focus request in polled state instead of a one-shot JS event.
    pub fn open_from_link(self: &Arc<Self>, target: &Url) {
        let id = self.new_tab(Some(target.as_str()));
        let mut inner = self.lock();
        Self::bump(&mut inner);
        inner.handoff_rev = inner.rev;
        self.core.log_event("info", "app.link.opened", json!({"tab_id": id}));
    }

    pub fn activate(self: &Arc<Self>, id: u64) {
        let need_view = {
            let mut inner = self.lock();
            let Some(tab) = inner.tabs.iter_mut().find(|t| t.id == id) else { return };
            tab.last_active_ms = now_ms();
            let need = tab.face() == Face::Yt && !tab.live;
            if need {
                tab.restoring = tab.scroll_y > 0.0 || !tab.title.is_empty();
                tab.loading = true;
            }
            inner.active = Some(id);
            Self::bump(&mut inner);
            need
        };
        let _ = self.core.video.kv_put("active_tab", &id.to_string());
        if need_view {
            self.create_view(id);
        }
        self.sync_native();
        if let Some(t) = self.lock().tabs.iter().find(|t| t.id == id).cloned() {
            self.persist(&t, "open");
        }
    }

    pub fn close_tab(self: &Arc<Self>, id: u64) {
        self.destroy_view(id);
        let (removed, next) = {
            let mut inner = self.lock();
            let pos = inner.tabs.iter().position(|t| t.id == id);
            let removed = pos.map(|p| inner.tabs.remove(p));
            let next = if inner.active == Some(id) {
                let n = pos.and_then(|p| inner.tabs.get(p).or_else(|| inner.tabs.get(p.saturating_sub(1)))).map(|t| t.id);
                inner.active = n;
                n
            } else {
                None
            };
            Self::bump(&mut inner);
            (removed, next)
        };
        if let Some(t) = removed {
            self.persist(&t, "closed");
        }
        match next {
            Some(n) => self.activate(n),
            None => {
                if self.lock().tabs.is_empty() {
                    self.new_tab(None);
                }
            }
        }
    }

    /// A video is opened from `tab_id`. From a browsing page (or in the background) it gets
    /// its own player tab; a player tab navigating to another video changes in place.
    /// Either way a video already open in another tab is reused instead of duplicated: that
    /// tab is focused (unless `background`) and playback in it is not touched. The tab that
    /// asked stays where it was.
    pub fn open_video(self: &Arc<Self>, tab_id: u64, video_id: &str, background: bool) {
        self.open_video_loc(tab_id, Loc::Watch(video_id.to_string()), background);
    }

    pub fn open_video_loc(self: &Arc<Self>, tab_id: u64, loc: Loc, background: bool) {
        let video_id = match &loc { Loc::Watch(v) | Loc::WatchInPlaylist { video_id: v, .. } => v.clone(), _ => return };
        let browsing = self.lock().tabs.iter().any(|tab|tab.id==tab_id && tab.face()==Face::Yt);
        if background || browsing {
            self.open_dest(loc.clone(), background);
            return;
        }
        let key = tabs::loc_key(&loc);
        let other = {
            let mut inner = self.lock();
            let other = tabs::find_by_dest(&inner.tabs, &key, Some(tab_id));
            if other.is_none() {
                let Some(tab) = inner.tabs.iter_mut().find(|t| t.id == tab_id) else { return };
                if let Loc::WatchInPlaylist { playlist, .. } = &loc { tab.playlist = Some(playlist.clone()); }
                tab.open_video(&video_id);
                Self::bump(&mut inner);
            }
            other
        };
        if let Some(other) = other {
            self.activate(other);
            return;
        }
        self.activate(tab_id);
        // The page behind the player can rest: hide it now (it keeps its state while warm).
        self.sync_native();
    }

    pub fn go(self: &Arc<Self>, tab_id: u64, input: &str) {
        match classify_input(input) {
            Input::Video(v) => self.open_video(tab_id, &v, false),
            Input::VideoInPlaylist { video_id, playlist } => self.open_video_loc(tab_id, Loc::WatchInPlaylist { video_id, playlist }, false),
            Input::External(u) => {
                let _ = crate::commands::open_in_browser(&u);
            }
            Input::Yt(url) => self.go_yt(tab_id, &url),
            Input::Search(q) => self.go_yt(tab_id, &tabs::search_url(&q)),
        }
    }

    fn go_yt(self: &Arc<Self>, tab_id: u64, url: &str) {
        let key = tabs::loc_key(&Loc::Yt(url.to_string()));
        let (live, restoring_needed) = {
            let mut inner = self.lock();
            let other = tabs::find_by_dest(&inner.tabs, &key, Some(tab_id));
            if let Some(other) = other {
                // Already open elsewhere: show that tab, leave this one where it is.
                drop(inner);
                self.activate(other);
                return;
            }
            let Some(tab) = inner.tabs.iter_mut().find(|t| t.id == tab_id) else { return };
            tab.go_yt(url);
            tab.scroll_y = 0.0;
            tab.loading = true;
            tab.restoring = false;
            let live = tab.live;
            Self::bump(&mut inner);
            (live, !live)
        };
        if live {
            self.navigate_view(tab_id, url);
        }
        self.activate(tab_id);
        let _ = restoring_needed;
    }

    pub fn back(self: &Arc<Self>, tab_id: u64) {
        let (mv, other) = {
            let mut inner = self.lock();
            let other = inner.tabs.iter().find(|t| t.id == tab_id).and_then(|t| t.peek_back()).and_then(|l| tabs::find_by_dest(&inner.tabs, &tabs::loc_key(l), Some(tab_id)));
            let mv = if other.is_some() { None } else { inner.tabs.iter_mut().find(|t| t.id == tab_id).and_then(|t| t.back()) };
            Self::bump(&mut inner);
            (mv, other)
        };
        if let Some(other) = other {
            // The destination is open in another tab: show that one instead of duplicating it.
            self.activate(other);
        } else if let Some(mv) = mv {
            self.apply_move(tab_id, mv);
        }
    }

    pub fn forward(self: &Arc<Self>, tab_id: u64) {
        let (mv, other) = {
            let mut inner = self.lock();
            let other = inner.tabs.iter().find(|t| t.id == tab_id).and_then(|t| t.peek_forward()).and_then(|l| tabs::find_by_dest(&inner.tabs, &tabs::loc_key(l), Some(tab_id)));
            let mv = if other.is_some() { None } else { inner.tabs.iter_mut().find(|t| t.id == tab_id).and_then(|t| t.forward()) };
            Self::bump(&mut inner);
            (mv, other)
        };
        if let Some(other) = other {
            // The destination is open in another tab: show that one instead of duplicating it.
            self.activate(other);
        } else if let Some(mv) = mv {
            self.apply_move(tab_id, mv);
        }
    }

    fn apply_move(self: &Arc<Self>, tab_id: u64, mv: Move) {
        self.run_move(tab_id, mv);
        self.activate(tab_id);
    }

    fn run_move(self: &Arc<Self>, tab_id: u64, mv: Move) {
        match mv {
            Move::HistoryBack => self.eval_tab(tab_id, "history.back()"),
            Move::HistoryForward => self.eval_tab(tab_id, "history.forward()"),
            Move::Navigate(u) => {
                if self.view_exists(tab_id) {
                    self.navigate_view(tab_id, &u);
                }
            }
            Move::ShowWatch(_) | Move::ShowYt => {}
        }
    }

    /// The page in `dup` moved by itself (a single-page navigation) to a destination that
    /// `keep` already shows. `keep` is focused and `dup` is sent back to where it was; if it
    /// has nowhere to go back to, it is closed. Reports of the unwanted page are ignored
    /// while the view is still on it.
    fn yield_to(self: &Arc<Self>, dup: u64, keep: u64) {
        let mv = {
            let mut inner = self.lock();
            let mv = inner.tabs.iter_mut().find(|t| t.id == dup).and_then(|t| {
                t.bounce = Some(t.dest_key());
                t.back()
            });
            Self::bump(&mut inner);
            mv
        };
        match mv {
            Some(mv) => self.run_move(dup, mv),
            None => self.close_tab(dup),
        }
        self.activate(keep);
    }

    pub fn reload(self: &Arc<Self>, tab_id: u64) {
        let (face, live) = {
            let inner = self.lock();
            match inner.tabs.iter().find(|t| t.id == tab_id) {
                Some(t) => (t.face(), t.live),
                None => return,
            }
        };
        match (face, live) {
            (Face::Yt, true) => self.eval_tab(tab_id, "location.reload()"),
            _ => self.activate(tab_id),
        }
    }

    // --- page reports ----------------------------------------------------------------------------------

    pub fn page_state(self: &Arc<Self>, tab_id: u64, url: &str, title: &str, scroll_y: f64) {
        let Ok(parsed) = url.parse::<Url>() else { return };
        if !parsed.host_str().is_some_and(tabs::is_youtube_host) {
            return;
        }
        // The scroll position of a tab whose view is hidden behind the player is still worth keeping.
        let (persist, duplicate_of) = {
            let mut inner = self.lock();
            Self::bump(&mut inner);
            let Some(tab) = inner.tabs.iter_mut().find(|t| t.id == tab_id) else { return };
            if tab.suppress_report(url) {
                return;
            }
            let moved = tab.yt_reported(url);
            if moved {
                tab.scroll_y = 0.0;
            } else if !tab.restoring {
                // While waking up the saved position must survive the new page's first reports.
                tab.scroll_y = scroll_y;
            }
            let clean = title.trim_end_matches(" - YouTube").trim().to_string();
            if !clean.is_empty() {
                tab.title = clean;
            }
            let tab = tab.clone();
            let duplicate_of = if moved { tabs::find_by_dest(&inner.tabs, &tab.dest_key(), Some(tab_id)) } else { None };
            (tab, duplicate_of)
        };
        if let Some(keep) = duplicate_of {
            self.yield_to(tab_id, keep);
            return;
        }
        self.persist(&persist, "open");
    }

    pub fn page_loaded(self: &Arc<Self>, tab_id: u64, finished: bool) {
        let scroll = {
            let mut inner = self.lock();
            Self::bump(&mut inner);
            let Some(tab) = inner.tabs.iter_mut().find(|t| t.id == tab_id) else { return };
            tab.loading = !finished;
            let was_restoring = tab.restoring;
            if finished {
                tab.restoring = false;
            }
            (finished && was_restoring).then_some(tab.scroll_y).filter(|y| *y > 0.0)
        };
        if finished {
            self.sync_native();
        }
        if let Some(y) = scroll {
            // YouTube fills its feeds in after load; try a few times.
            let shell = self.clone();
            std::thread::spawn(move || {
                for delay in [700u64, 1500, 3000] {
                    std::thread::sleep(Duration::from_millis(delay));
                    shell.eval_tab(tab_id, &format!("window.scrollTo(0,{y})"));
                }
            });
        }
    }

    /// A load started or finished at `url`: remember pages outside YouTube (the Google
    /// sign-in) so the address bar is honest about where the tab is.
    pub fn page_url(self: &Arc<Self>, tab_id: u64, url: &Url) {
        let outside = !url.host_str().is_some_and(tabs::is_youtube_host);
        let mut inner = self.lock();
        if let Some(tab) = inner.tabs.iter_mut().find(|t| t.id == tab_id) {
            let new = outside.then(|| url.to_string());
            if tab.other_url != new {
                tab.other_url = new;
                Self::bump(&mut inner);
            }
        }
    }

    pub fn set_title(self: &Arc<Self>, tab_id: u64, title: &str) {
        let mut inner = self.lock();
        if let Some(tab) = inner.tabs.iter_mut().find(|t| t.id == tab_id) {
            let clean = title.trim_end_matches(" - YouTube").trim();
            if !clean.is_empty() && tab.title != clean {
                tab.title = clean.to_string();
                Self::bump(&mut inner);
            }
        }
    }

    // --- layout ------------------------------------------------------------------------------------------

    pub fn set_rect(self: &Arc<Self>, rect: Rect) {
        self.lock().rect = rect;
        self.sync_native();
    }

    pub fn set_viewport_rect(self:&Arc<Self>,rect:Rect,width:Option<f64>,height:Option<f64>) {
        // One mapping (see geometry.rs): the chrome web view's own logical size over its CSS
        // viewport gives a single uniform scale; the window's inner size is not used because
        // it can include the title bar. The origin is the chrome web view's real position
        // (logical points, same scale factor as size()); if it cannot be read, fall back to (0,0).
        let chrome=self.app.get_webview(CHROME).and_then(|c|{
            let scale=c.window().scale_factor().ok()?;
            let size=c.size().ok()?.to_logical::<f64>(scale);
            let (x,y)=match c.position() {
                Ok(p)=>{let l=p.to_logical::<f64>(scale);(l.x,l.y)}
                Err(e)=>{eprintln!("muninn: could not read the chrome web view position, using (0,0): {e}");(0.0,0.0)}
            };
            Some(crate::geometry::Bounds{x,y,w:size.width,h:size.height})
        });
        let Some(m)=crate::geometry::map_viewport_rect(crate::geometry::Bounds{x:rect.x,y:rect.y,w:rect.w,h:rect.h},width,height,chrome) else {return};
        let next=Rect{x:m.rect.x,y:m.rect.y,w:m.rect.w,h:m.rect.h};
        self.core.log_event("info","webview.layout",json!({"x":next.x,"y":next.y,"w":next.w,"h":next.h,"css_x":rect.x,"css_y":rect.y,"css_w":rect.w,"css_h":rect.h,"scale":m.scale,"clamped":m.clamped,"chrome_x":chrome.map(|c|c.x),"chrome_y":chrome.map(|c|c.y),"chrome_w":chrome.map(|c|c.w),"chrome_h":chrome.map(|c|c.h),"viewport_width":width,"viewport_height":height}));
        self.set_rect(next);
    }

    pub fn set_native_visible(self: &Arc<Self>, visible: bool) {
        self.lock().native_visible = visible;
        self.sync_native();
    }

    /// Shows the active tab's web view over the content area, hides all others.
    pub fn sync_native(self: &Arc<Self>) {
        let online=!self.core.is_offline();
        let (plan, rect) = {
            let inner = self.lock();
            let plan: Vec<(u64, bool)> = inner
                .tabs
                .iter()
                .filter(|t| t.live)
                .map(|t| (t.id, online && inner.active == Some(t.id) && t.face() == Face::Yt && !t.restoring && inner.native_visible))
                .collect();
            (plan, inner.rect)
        };
        for (id, visible) in plan {
            let Some(wv) = self.app.get_webview(&label(id)) else { continue };
            #[cfg(target_os = "linux")]
            {
                crate::linux_overlay::place(&wv, rect.x, rect.y, rect.w, rect.h, visible && rect.w > 1.0 && rect.h > 1.0);
                continue;
            }
            #[allow(unreachable_code)]
            if visible && rect.w > 1.0 && rect.h > 1.0 {
                // One bounds update: separate async position/size updates can overwrite each other.
                if wv.set_bounds(tauri::Rect {
                    position: LogicalPosition::new(rect.x, rect.y).into(),
                    size: LogicalSize::new(rect.w, rect.h).into(),
                }).is_err() {
                    self.core.log_event("error", "webview.layout_failed", json!({"tab_id":id}));
                }
                let _ = wv.show();
            } else {
                let _ = wv.hide();
            }
        }
    }

    // --- native views --------------------------------------------------------------------------------------

    fn view_exists(&self, id: u64) -> bool {
        self.app.get_webview(&label(id)).is_some()
    }

    fn eval_tab(&self, id: u64, js: &str) {
        if let Some(wv) = self.app.get_webview(&label(id)) {
            let _ = wv.eval(js);
        }
    }

    fn navigate_view(&self, id: u64, url: &str) {
        if let (Some(wv), Ok(u)) = (self.app.get_webview(&label(id)), url.parse::<Url>()) {
            let _ = wv.navigate(u);
        }
    }

    fn create_view(self: &Arc<Self>, id: u64) {
        if self.core.is_offline() {
            let mut inner=self.lock();
            if let Some(tab)=inner.tabs.iter_mut().find(|t|t.id==id){tab.restoring=false;tab.loading=false;}
            Self::bump(&mut inner);
            return;
        }
        if self.view_exists(id) {
            self.lock().tabs.iter_mut().filter(|t| t.id == id).for_each(|t| t.live = true);
            return;
        }
        let (url, rect) = {
            let inner = self.lock();
            let Some(t) = inner.tabs.iter().find(|t| t.id == id) else { return };
            (t.yt_url.clone(), inner.rect)
        };
        let Some(window) = self.app.get_window(MAIN) else { return };
        let Ok(parsed) = url.parse::<Url>() else { return };
        // Without the capture proxy a YouTube view would play video in YouTube's own player and
        // nothing would be cached: better no view than that.
        let Some(proxy) = self.service.proxy_url().and_then(|p| p.parse::<Url>().ok()) else {
            eprintln!("muninn: the capture proxy is not running; not opening a YouTube view for tab {id}");
            let mut inner = self.lock();
            if let Some(t) = inner.tabs.iter_mut().find(|t| t.id == id) {
                t.loading = false;
                t.restoring = false;
                t.title = "Capture proxy unavailable".into();
            }
            Self::bump(&mut inner);
            return;
        };
        let init = format!("{INJECT_JS}\n;window.__MUNINN_TAB_ID__ = {id};");
        let (s_nav, s_load, s_title, s_new) = (self.clone(), self.clone(), self.clone(), self.clone());
        let mut builder = WebviewBuilder::new(label(id), WebviewUrl::External(parsed))
            .user_agent(SAFARI_UA)
            .initialization_script(init)
            .on_navigation(move |u| {
                if u.host_str().is_some_and(tabs::is_youtube_host) {
                    if let Some(v) = tabs::video_id_of_url(u.as_str()) {
                        // A full page load of a video (typed link, redirect): Muninn's player takes it.
                        let shell = s_nav.clone();
                        let loc = match tabs::playlist_context_of_url(u.as_str()) { Some(playlist) => Loc::WatchInPlaylist { video_id: v, playlist }, None => Loc::Watch(v) };
                        tauri::async_runtime::spawn(async move { shell.open_video_loc(id, loc, false) });
                        return false;
                    }
                }
                true
            })
            .on_page_load(move |_wv, payload| {
                let finished = matches!(payload.event(), tauri::webview::PageLoadEvent::Finished);
                s_load.page_url(id, payload.url());
                s_load.page_loaded(id, finished);
            })
            .on_document_title_changed(move |_wv, title| s_title.set_title(id, &title))
            .on_new_window(move |u, _features| {
                // A requested new YouTube window becomes a new tab in Muninn.
                if u.host_str().is_some_and(tabs::is_youtube_host) {
                    let shell = s_new.clone();
                    let target = u.to_string();
                    tauri::async_runtime::spawn(async move { shell.new_tab(Some(&target)); });
                } else {
                    let _ = crate::commands::open_in_browser(u.as_str());
                }
                NewWindowResponse::Deny
            });
        builder = builder.proxy_url(proxy);
        #[cfg(target_os = "macos")]
        {
            builder = builder.data_store_identifier(YT_STORE);
        }
        #[cfg(target_os = "linux")]
        if let Ok(dir) = self.app.path().app_data_dir() {
            builder = builder.data_directory(dir.join("webdata-youtube"));
        }
        match window.add_child(builder, LogicalPosition::new(rect.x, rect.y), LogicalSize::new(rect.w.max(10.0), rect.h.max(10.0))) {
            Ok(wv) => {
                crate::trust::prepare_view(self, &wv);
                #[cfg(target_os = "linux")]
                crate::linux_overlay::adopt(&wv);
                let _ = wv.hide();
                let mut inner = self.lock();
                if let Some(t) = inner.tabs.iter_mut().find(|t| t.id == id) {
                    t.live = true;
                    t.native_depth = 0;
                    t.native_fwd = 0;
                }
                Self::bump(&mut inner);
            }
            Err(e) => {
                eprintln!("muninn: could not create the web view for tab {id}: {e}");
                let mut inner = self.lock();
                if let Some(t) = inner.tabs.iter_mut().find(|t| t.id == id) {
                    t.loading = false;
                    t.restoring = false;
                }
            }
        }
    }

    fn destroy_view(&self, id: u64) {
        if let Some(wv) = self.app.get_webview(&label(id)) {
            let _ = wv.close();
        }
        let mut inner = self.lock();
        if let Some(t) = inner.tabs.iter_mut().find(|t| t.id == id) {
            t.live = false;
            t.loading = false;
            t.restoring = false;
        }
        Self::bump(&mut inner);
    }

    pub fn reload_live_tabs(self: &Arc<Self>) {
        let ids: Vec<u64> = self.lock().tabs.iter().filter(|t| t.live).map(|t| t.id).collect();
        for id in ids {
            self.eval_tab(id, "location.reload()");
        }
    }

    /// Google sign-in in a plain window on the same data store, for when the in-tab
    /// flow is refused. Closing it reloads the YouTube tabs so they pick the login up.
    pub fn open_signin_window(self: &Arc<Self>) -> Result<(), String> {
        self.core.require_online().map_err(|e|e.to_string())?;
        self.core.invalidate_youtube_account();
        if let Some(w) = self.app.get_webview_window("signin") {
            let _ = w.set_focus();
            return Ok(());
        }
        let url: Url = "https://accounts.google.com/ServiceLogin?service=youtube&continue=https%3A%2F%2Fwww.youtube.com%2F".parse().map_err(|e: url::ParseError| e.to_string())?;
        #[allow(unused_mut)]
        let mut b = tauri::WebviewWindowBuilder::new(&self.app, "signin", WebviewUrl::External(url)).title("Sign in to YouTube").inner_size(520.0, 720.0).user_agent(SAFARI_UA);
        if let Some(p) = self.service.proxy_url().and_then(|p| p.parse::<Url>().ok()) {
            b = b.proxy_url(p);
        }
        #[cfg(target_os = "macos")]
        {
            b = b.data_store_identifier(YT_STORE);
        }
        #[cfg(target_os = "linux")]
        if let Ok(dir) = self.app.path().app_data_dir() {
            b = b.data_directory(dir.join("webdata-youtube"));
        }
        let shell = self.clone();
        let w = b.build().map_err(|e| e.to_string())?;
        w.on_window_event(move |ev| {
            if matches!(ev, tauri::WindowEvent::Destroyed) {
                shell.reload_live_tabs();
            }
        });
        Ok(())
    }

    // --- hibernation ---------------------------------------------------------------------------------------------

    pub fn hibernate(self: &Arc<Self>, id: u64) {
        let tab = {
            let inner = self.lock();
            inner.tabs.iter().find(|t| t.id == id).cloned()
        };
        let Some(tab) = tab else { return };
        if !tab.live {
            return;
        }
        self.persist(&tab, "hidden");
        self.destroy_view(id);
        self.sync_native();
    }

    /// One pass of the hibernation policy; returns the tabs put to sleep.
    pub fn hibernate_pass(self: &Arc<Self>) -> Vec<u64> {
        let settings = self.core.video.settings();
        let now = now_ms();
        let live: Vec<(u64, u64, bool)> = {
            let inner = self.lock();
            inner.tabs.iter().filter(|t| t.live).map(|t| (t.id, ((now - t.last_active_ms).max(0) / 1000) as u64, inner.active == Some(t.id))).collect()
        };
        let victims = if settings.offline_mode { live.iter().map(|t|t.0).collect() }
            else { tabs::pick_hibernation(&live, settings.warm_tabs as usize, settings.hibernate_after_secs as u64) };
        for id in &victims {
            self.hibernate(*id);
        }
        victims
    }

    pub fn start_hibernation_thread(self: &Arc<Self>) {
        let shell = self.clone();
        std::thread::Builder::new()
            .name("muninn-hibernate".into())
            .spawn(move || loop {
                std::thread::sleep(Duration::from_secs(3));
                shell.adopt_imported_tabs();
                shell.hibernate_pass();
            })
            .ok();
    }

    /// CLI imports arrive as `import-*` rows: each becomes a sleeping tab unless its
    /// destination is already open, in which case the existing tab is kept untouched.
    fn adopt_imported_tabs(&self) {
        for row in self.core.browsing.tabs().unwrap_or_default().into_iter().filter(|r|r.tab_id.starts_with("import-") && r.state!="closed") {
            let loc=match tabs::video_id_of_url(&row.url) {Some(v)=>match tabs::playlist_context_of_url(&row.url){Some(playlist)=>Loc::WatchInPlaylist{video_id:v,playlist},None=>Loc::Watch(v)},None=>Loc::Yt(row.url.clone())};
            let (id, created) = self.find_or_open(loc);
            if created {
                let tab = {
                    let mut inner=self.lock();
                    let Some(tab)=inner.tabs.iter_mut().find(|t|t.id==id) else {continue};
                    tab.title=row.title.clone().unwrap_or_default();
                    tab.last_active_ms=row.updated_at;
                    tab.clone()
                };
                self.persist(&tab,"hidden");
            }
            let _=self.core.browsing.upsert_tab(&TabIngest{tab_id:row.tab_id,url:row.url,state:"closed".into(),..Default::default()},now_ms());
        }
    }
}

pub fn label(id: u64) -> String {
    format!("tab-{id}")
}

pub fn tab_id_of(label: &str) -> Option<u64> {
    label.strip_prefix("tab-")?.parse().ok()
}

pub fn build_chrome(shell: &Arc<Shell>) -> Result<(), Box<dyn std::error::Error>> {
    let window = shell.app.get_window(MAIN).ok_or("main window missing")?;
    let ui: Url = format!("{}/ui/", shell.service.base_url()).parse()?;
    let size = window.inner_size()?.to_logical::<f64>(window.scale_factor()?);
    #[allow(unused_mut)]
    let mut builder = WebviewBuilder::new(CHROME, WebviewUrl::External(ui)).auto_resize().disable_drag_drop_handler();
    #[cfg(target_os = "linux")]
    if let Ok(dir) = shell.app.path().app_data_dir() {
        builder = builder.data_directory(dir.join("webdata-chrome"));
    }
    let chrome = window.add_child(builder, LogicalPosition::new(0.0, 0.0), LogicalSize::new(size.width, size.height))?;
    #[cfg(target_os = "linux")]
    crate::linux_overlay::wrap_chrome(&chrome);
    #[cfg(not(target_os = "linux"))]
    let _ = chrome;
    Ok(())
}
