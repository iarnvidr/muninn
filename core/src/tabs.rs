//! Pure tab model for the browser shell: what each tab shows, its history, the
//! address-bar rules and the hibernation policy. No GUI types in here, so it is
//! unit tested on any machine; `src-tauri` applies the results to real web views.

use crate::extract::{page_context, parse_url, Kind};
use serde::Serialize;

pub const HOME_URL: &str = "https://www.youtube.com/";

#[derive(Debug, Clone, PartialEq)]
pub enum Loc {
    Yt(String),
    Watch(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Face {
    Yt,
    Watch,
}

#[derive(Debug, Clone)]
pub struct Tab {
    pub id: u64,
    pub title: String,
    /// The YouTube page of this tab (what the native view shows or will show).
    pub yt_url: String,
    pub video_id: Option<String>,
    pub hist: Vec<Loc>,
    pub idx: usize,
    /// A native web view exists for this tab.
    pub live: bool,
    pub loading: bool,
    /// Waking from hibernation: the restore view is shown until the live page loaded.
    pub restoring: bool,
    pub scroll_y: f64,
    /// Single-page navigations inside the current native view (history.back() works that many times).
    pub native_depth: usize,
    /// ...and how many times history.forward() works.
    pub native_fwd: usize,
    /// A page outside YouTube the web view is on right now (Google sign-in): shown in the
    /// address bar, never persisted.
    pub other_url: Option<String>,
    pub last_active_ms: i64,
}

impl Tab {
    pub fn new(id: u64, loc: Loc, now_ms: i64) -> Tab {
        let (yt_url, video_id) = match &loc {
            Loc::Yt(u) => (u.clone(), None),
            Loc::Watch(v) => (HOME_URL.to_string(), Some(v.clone())),
        };
        Tab {
            id,
            title: String::new(),
            yt_url,
            video_id,
            hist: vec![loc],
            idx: 0,
            live: false,
            loading: false,
            restoring: false,
            scroll_y: 0.0,
            native_depth: 0,
            native_fwd: 0,
            other_url: None,
            last_active_ms: now_ms,
        }
    }

    pub fn loc(&self) -> &Loc {
        &self.hist[self.idx]
    }

    pub fn face(&self) -> Face {
        match self.loc() {
            Loc::Yt(_) => Face::Yt,
            Loc::Watch(_) => Face::Watch,
        }
    }

    pub fn can_back(&self) -> bool {
        self.idx > 0
    }

    pub fn can_forward(&self) -> bool {
        self.idx + 1 < self.hist.len()
    }

    /// What the address bar shows.
    pub fn shown_url(&self) -> String {
        match (&self.other_url, self.face()) {
            (Some(u), Face::Yt) => u.clone(),
            _ => self.url(),
        }
    }

    /// URL to persist (always a YouTube page).
    pub fn url(&self) -> String {
        match self.loc() {
            Loc::Yt(u) => u.clone(),
            Loc::Watch(v) => format!("https://www.youtube.com/watch?v={v}"),
        }
    }

    fn push(&mut self, loc: Loc) {
        self.hist.truncate(self.idx + 1);
        self.hist.push(loc);
        self.idx += 1;
        if self.hist.len() > 200 {
            self.hist.remove(0);
            self.idx -= 1;
        }
    }

    pub fn open_video(&mut self, video_id: &str) {
        if self.loc() == &Loc::Watch(video_id.to_string()) {
            return;
        }
        self.push(Loc::Watch(video_id.to_string()));
        self.video_id = Some(video_id.to_string());
    }

    /// The tab was sent to a YouTube page by the user (address bar, channel link).
    pub fn go_yt(&mut self, url: &str) {
        self.push(Loc::Yt(url.to_string()));
        self.yt_url = url.to_string();
        self.video_id = None;
        self.native_depth += 1;
        self.native_fwd = 0;
    }

    /// The native view reported a new URL (single-page navigation or a load).
    /// Returns true when this was a real move (history updated).
    pub fn yt_reported(&mut self, url: &str) -> bool {
        self.other_url = None;
        self.yt_url = url.to_string();
        if self.face() != Face::Yt {
            // The page moved while Muninn's player is showing (e.g. a background load): remember it only.
            return false;
        }
        if self.loc() == &Loc::Yt(url.to_string()) {
            return false;
        }
        if self.idx > 0 && self.hist[self.idx - 1] == Loc::Yt(url.to_string()) {
            self.idx -= 1;
            self.native_depth = self.native_depth.saturating_sub(1);
            self.native_fwd += 1;
        } else if self.idx + 1 < self.hist.len() && self.hist[self.idx + 1] == Loc::Yt(url.to_string()) {
            self.idx += 1;
            self.native_depth += 1;
            self.native_fwd = self.native_fwd.saturating_sub(1);
        } else {
            self.push(Loc::Yt(url.to_string()));
            self.native_depth += 1;
            self.native_fwd = 0;
        }
        true
    }

    /// Moves back; returns what the shell must do to show it.
    pub fn back(&mut self) -> Option<Move> {
        if !self.can_back() {
            return None;
        }
        let from_yt = self.face() == Face::Yt;
        self.idx -= 1;
        Some(self.apply_move(from_yt, true))
    }

    pub fn forward(&mut self) -> Option<Move> {
        if !self.can_forward() {
            return None;
        }
        let from_yt = self.face() == Face::Yt;
        self.idx += 1;
        Some(self.apply_move(from_yt, false))
    }

    fn apply_move(&mut self, from_yt: bool, back: bool) -> Move {
        match self.loc().clone() {
            Loc::Watch(v) => {
                self.video_id = Some(v.clone());
                Move::ShowWatch(v)
            }
            Loc::Yt(u) => {
                self.video_id = None;
                if from_yt {
                    // yt -> yt: the page's own history if this view has one, else a full load.
                    if back && self.native_depth > 0 {
                        self.native_depth -= 1;
                        self.native_fwd += 1;
                        Move::HistoryBack
                    } else if !back && self.native_fwd > 0 {
                        self.native_fwd -= 1;
                        self.native_depth += 1;
                        Move::HistoryForward
                    } else {
                        self.yt_url = u.clone();
                        self.native_depth = 0;
                        self.native_fwd = 0;
                        Move::Navigate(u)
                    }
                } else if self.yt_url == u {
                    // Coming back from the player: the page is still where it was.
                    Move::ShowYt
                } else {
                    self.yt_url = u.clone();
                    Move::Navigate(u)
                }
            }
        }
    }

    pub fn kind(&self) -> (Kind, Option<String>) {
        let ctx = page_context(&self.yt_url);
        (ctx.kind, ctx.query)
    }

    pub fn kind_str(&self) -> &'static str {
        match self.kind().0 {
            Kind::Home => "home",
            Kind::Subscriptions => "subscriptions",
            Kind::Search => "search",
            Kind::Channel => "channel",
            _ => "other",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Move {
    ShowWatch(String),
    ShowYt,
    Navigate(String),
    HistoryBack,
    HistoryForward,
}

/// What the address bar text means.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    Video(String),
    Yt(String),
    /// A page outside YouTube; Muninn hands it to the default browser.
    External(String),
    Search(String),
}

pub fn video_id_of_url(url: &str) -> Option<String> {
    let u = parse_url(url)?;
    let host = u.host.to_ascii_lowercase();
    let id = if host == "youtu.be" {
        u.path.trim_start_matches('/').split('/').next().map(str::to_string)
    } else if host == "youtube.com" || host.ends_with(".youtube.com") {
        if u.path == "/watch" {
            crate::extract::parse_query(&u.query).into_iter().find(|(k, _)| k == "v").map(|(_, v)| v)
        } else {
            ["/shorts/", "/live/", "/embed/", "/v/"].iter().find_map(|p| u.path.strip_prefix(p).map(|r| r.split('/').next().unwrap_or("").to_string()))
        }
    } else {
        None
    }?;
    crate::util::valid_video_id(&id).then_some(id)
}

pub fn is_youtube_host(host: &str) -> bool {
    let h = host.to_ascii_lowercase();
    h == "youtube.com" || h.ends_with(".youtube.com")
}

pub fn classify_input(text: &str) -> Input {
    let t = text.trim();
    if t.is_empty() {
        return Input::Yt(HOME_URL.into());
    }
    let with_scheme = if t.starts_with("http://") || t.starts_with("https://") {
        Some(t.to_string())
    } else if !t.contains(' ') && (t.starts_with("www.") || t.starts_with("youtube.com") || t.starts_with("m.youtube.com") || t.starts_with("youtu.be/")) {
        Some(format!("https://{t}"))
    } else {
        None
    };
    if let Some(u) = with_scheme {
        if let Some(id) = video_id_of_url(&u) {
            return Input::Video(id);
        }
        return match parse_url(&u) {
            Some(p) if is_youtube_host(&p.host) => Input::Yt(u.replace("http://", "https://")),
            Some(_) => Input::External(u),
            None => Input::Search(t.to_string()),
        };
    }
    Input::Search(t.to_string())
}

pub fn search_url(q: &str) -> String {
    let mut enc = String::new();
    for b in q.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => enc.push(b as char),
            b' ' => enc.push('+'),
            _ => enc.push_str(&format!("%{b:02X}")),
        }
    }
    format!("https://www.youtube.com/results?search_query={enc}")
}

/// Which live, inactive tabs should hibernate now.
/// `live` = (tab id, idle seconds, is active). Keeps at most `warm` live tabs
/// (the active one included) and only sleeps tabs idle for at least `after_secs`.
pub fn pick_hibernation(live: &[(u64, u64, bool)], warm: usize, after_secs: u64) -> Vec<u64> {
    let mut count = live.len();
    let mut idle: Vec<(u64, u64)> = live.iter().filter(|(_, _, active)| !active).map(|(id, secs, _)| (*id, *secs)).collect();
    idle.sort_by(|a, b| b.1.cmp(&a.1));
    let mut out = Vec::new();
    for (id, secs) in idle {
        if count <= warm.max(1) {
            break;
        }
        if secs >= after_secs {
            out.push(id);
            count -= 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "https://www.youtube.com/";
    const B: &str = "https://www.youtube.com/feed/subscriptions";

    #[test]
    fn address_bar_classification() {
        assert_eq!(classify_input("https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=5"), Input::Video("dQw4w9WgXcQ".into()));
        assert_eq!(classify_input("youtu.be/dQw4w9WgXcQ"), Input::Video("dQw4w9WgXcQ".into()));
        assert_eq!(classify_input("https://www.youtube.com/shorts/dQw4w9WgXcQ"), Input::Video("dQw4w9WgXcQ".into()));
        assert_eq!(classify_input("www.youtube.com/@some"), Input::Yt("https://www.youtube.com/@some".into()));
        assert_eq!(classify_input("http://www.youtube.com/feed/subscriptions"), Input::Yt("https://www.youtube.com/feed/subscriptions".into()));
        assert_eq!(classify_input("https://example.org/x"), Input::External("https://example.org/x".into()));
        assert_eq!(classify_input("rust async tutorial"), Input::Search("rust async tutorial".into()));
        assert_eq!(classify_input("dQw4w9WgXcQ"), Input::Search("dQw4w9WgXcQ".into()), "bare ids are searched like any text");
        assert_eq!(classify_input("  "), Input::Yt(HOME_URL.into()));
        assert_eq!(search_url("a b&c"), "https://www.youtube.com/results?search_query=a+b%26c");
    }

    #[test]
    fn opening_a_video_and_going_back_returns_to_the_page_as_it_was() {
        let mut t = Tab::new(1, Loc::Yt(A.into()), 0);
        t.open_video("aaaaaaaaaaa");
        assert_eq!((t.face(), t.can_back(), t.can_forward()), (Face::Watch, true, false));
        assert_eq!(t.url(), "https://www.youtube.com/watch?v=aaaaaaaaaaa");
        assert_eq!(t.back(), Some(Move::ShowYt), "the web view never left the page");
        assert_eq!(t.face(), Face::Yt);
        assert_eq!(t.forward(), Some(Move::ShowWatch("aaaaaaaaaaa".into())));
        // Related video, then back twice.
        t.open_video("bbbbbbbbbbb");
        assert_eq!(t.back(), Some(Move::ShowWatch("aaaaaaaaaaa".into())));
        assert_eq!(t.back(), Some(Move::ShowYt));
        assert_eq!(t.back(), None);
    }

    #[test]
    fn single_page_navigations_and_the_pages_own_history() {
        let mut t = Tab::new(1, Loc::Yt(A.into()), 0);
        assert!(t.yt_reported(B));
        assert!(!t.yt_reported(B), "same url twice is not a move");
        assert_eq!((t.idx, t.native_depth), (1, 1));
        assert_eq!(t.back(), Some(Move::HistoryBack));
        assert_eq!(t.native_depth, 0);
        assert_eq!(t.forward(), Some(Move::HistoryForward));
        // A fresh view (after hibernation) has no history of its own: go back by loading.
        t.native_depth = 0;
        t.native_fwd = 0;
        assert_eq!(t.back(), Some(Move::Navigate(A.into())));
        // The page itself reports it went back/forward (user pressed a YouTube control).
        let mut t2 = Tab::new(2, Loc::Yt(A.into()), 0);
        t2.yt_reported(B);
        assert!(t2.yt_reported(A));
        assert_eq!(t2.idx, 0);
    }

    #[test]
    fn a_page_report_while_the_player_shows_does_not_rewrite_history() {
        let mut t = Tab::new(1, Loc::Yt(A.into()), 0);
        t.open_video("aaaaaaaaaaa");
        assert!(!t.yt_reported("https://www.youtube.com/?bp=x"));
        assert_eq!(t.hist.len(), 2);
        assert_eq!(t.yt_url, "https://www.youtube.com/?bp=x");
    }

    #[test]
    fn typed_destinations_cut_the_forward_history() {
        let mut t = Tab::new(1, Loc::Yt(A.into()), 0);
        t.open_video("aaaaaaaaaaa");
        t.back();
        t.go_yt(B);
        assert!(!t.can_forward());
        assert_eq!(t.kind_str(), "subscriptions");
    }

    #[test]
    fn kinds_come_from_the_url() {
        let t = Tab::new(1, Loc::Yt("https://www.youtube.com/results?search_query=rust+lang".into()), 0);
        assert_eq!(t.kind_str(), "search");
        assert_eq!(t.kind().1.as_deref(), Some("rust lang"));
        assert_eq!(Tab::new(2, Loc::Yt(A.into()), 0).kind_str(), "home");
    }

    #[test]
    fn hibernation_keeps_the_warm_tabs_and_never_the_active_one() {
        // (id, idle seconds, active)
        let live = [(1, 0, true), (2, 30, false), (3, 500, false), (4, 400, false), (5, 90, false)];
        // warm = 3: five live tabs, two must go, longest idle first, and only if idle long enough.
        assert_eq!(pick_hibernation(&live, 3, 120), vec![3, 4]);
        assert_eq!(pick_hibernation(&live, 3, 450), vec![3], "only tabs idle past the timeout");
        assert_eq!(pick_hibernation(&live, 5, 0), Vec::<u64>::new(), "within the warm count nothing sleeps");
        assert_eq!(pick_hibernation(&live, 1, 0), vec![3, 4, 5, 2], "everything but the active tab");
        assert_eq!(pick_hibernation(&[(1, 0, true)], 1, 0), Vec::<u64>::new());
    }
}
