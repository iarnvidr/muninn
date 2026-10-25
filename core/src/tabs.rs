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
    /// Destination key of a page this tab was just turned away from because another tab
    /// already shows it; repeat reports of that page are ignored until the view has moved.
    pub bounce: Option<String>,
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
            bounce: None,
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

    /// The logical destination this tab shows right now (see [`loc_key`]).
    pub fn dest_key(&self) -> String {
        loc_key(self.loc())
    }

    /// Where Back would go, without moving.
    pub fn peek_back(&self) -> Option<&Loc> {
        self.idx.checked_sub(1).and_then(|i| self.hist.get(i))
    }

    /// Where Forward would go, without moving.
    pub fn peek_forward(&self) -> Option<&Loc> {
        self.hist.get(self.idx + 1)
    }

    /// True when `url` is a page this tab was just turned away from (see `bounce`) and
    /// the report must be ignored. Any other report ends the bounce.
    pub fn suppress_report(&mut self, url: &str) -> bool {
        match &self.bounce {
            Some(k) if *k == url_key(url) => true,
            Some(_) => {
                self.bounce = None;
                false
            }
            None => false,
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

/// Query parameters that only track where a link came from; they never change the page.
fn is_tracking_param(k: &str) -> bool {
    let k = k.to_ascii_lowercase();
    k.starts_with("utm_") || matches!(k.as_str(), "si" | "feature" | "pp" | "fbclid" | "gclid" | "dclid" | "msclkid" | "gbraid" | "wbraid")
}

fn encode_component(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Conservative identity of a browsing page: https, `youtube.com` / `www.` / `m.` folded
/// together, fragment and tracking parameters dropped, trailing slash dropped, remaining
/// parameters decoded, sorted and re-encoded. Everything else stays significant: search
/// queries, `sp=` filters, `list=` playlists, channel and account routes (`/feed/*`,
/// `/account`...), and other hosts such as `music.youtube.com`.
pub fn normalize_page_url(url: &str) -> String {
    let Some(u) = parse_url(url) else { return url.trim().to_string() };
    let host = match u.host.as_str() {
        "youtube.com" | "www.youtube.com" | "m.youtube.com" => "www.youtube.com".to_string(),
        h => h.to_string(),
    };
    let scheme = if u.scheme == "http" { "https" } else { u.scheme.as_str() };
    let mut path = u.path.clone();
    while path.len() > 1 && path.ends_with('/') {
        path.pop();
    }
    let mut params: Vec<(String, String)> = crate::extract::parse_query(&u.query).into_iter().filter(|(k, _)| !is_tracking_param(k)).collect();
    params.sort();
    let mut out = format!("{scheme}://{host}");
    if let Some(p) = u.port {
        out.push_str(&format!(":{p}"));
    }
    out.push_str(&path);
    if !params.is_empty() {
        out.push('?');
        out.push_str(&params.iter().map(|(k, v)| format!("{}={}", encode_component(k), encode_component(v))).collect::<Vec<_>>().join("&"));
    }
    out
}

/// Identity of a destination: `v:<id>` for anything that plays one video (watch, Shorts,
/// youtu.be, embed, live forms), `u:<normalised url>` for browsing pages.
pub fn url_key(url: &str) -> String {
    match video_id_of_url(url) {
        Some(v) => format!("v:{v}"),
        None => format!("u:{}", normalize_page_url(url)),
    }
}

pub fn loc_key(loc: &Loc) -> String {
    match loc {
        Loc::Watch(v) => format!("v:{v}"),
        Loc::Yt(u) => url_key(u),
    }
}

/// The tab showing the destination `key`, other than `except`. Sleeping tabs count.
pub fn find_by_dest(tabs: &[Tab], key: &str, except: Option<u64>) -> Option<u64> {
    tabs.iter().find(|t| Some(t.id) != except && t.dest_key() == key).map(|t| t.id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opened {
    /// The destination was already open in this tab; nothing was changed.
    Existing(u64),
    Created(u64),
}

impl Opened {
    pub fn id(self) -> u64 {
        match self {
            Opened::Existing(id) | Opened::Created(id) => id,
        }
    }
}

/// Find-or-open: lookup and allocation in one call over `&mut`, so callers that hold the
/// shell lock make it one critical section and concurrent requests yield one tab.
pub fn find_or_open(tabs: &mut Vec<Tab>, next_id: &mut u64, loc: Loc, now_ms: i64) -> Opened {
    if let Some(id) = find_by_dest(tabs, &loc_key(&loc), None) {
        return Opened::Existing(id);
    }
    let id = (*next_id).max(tabs.iter().map(|t| t.id + 1).max().unwrap_or(1));
    *next_id = id + 1;
    tabs.push(Tab::new(id, loc, now_ms));
    Opened::Created(id)
}

#[derive(Debug)]
pub struct Reconciled {
    pub tabs: Vec<Tab>,
    /// Ids of duplicate tabs that were dropped (remove them from persistence).
    pub removed: Vec<u64>,
    /// Ids of kept tabs that took over state from a dropped duplicate (persist them again).
    pub merged: Vec<u64>,
    pub active: Option<u64>,
}

/// Collapses tabs showing the same destination (restored or imported state). Per
/// destination the winner is the active tab, then a playing one, then the most recently
/// active, then the lowest id; it keeps its place in the strip. A winner without a scroll
/// position or title takes them from the best loser that has one. Deterministic: the
/// result does not depend on the input order beyond the winner's position.
pub fn reconcile(tabs: Vec<Tab>, active: Option<u64>, playing: &[u64]) -> Reconciled {
    let rank = |t: &Tab| (Some(t.id) == active, playing.contains(&t.id), t.last_active_ms, std::cmp::Reverse(t.id));
    let mut best: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (i, t) in tabs.iter().enumerate() {
        let k = t.dest_key();
        match best.get(&k) {
            Some(&b) if rank(&tabs[b]) >= rank(t) => {}
            _ => {
                best.insert(k, i);
            }
        }
    }
    let mut out: Vec<Tab> = Vec::new();
    let mut removed = Vec::new();
    let mut merged = Vec::new();
    let mut winner_of: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
    for (i, t) in tabs.iter().enumerate() {
        let k = t.dest_key();
        if best[&k] == i {
            winner_of.insert(k, t.id);
            out.push(t.clone());
        } else {
            removed.push(t.id);
        }
    }
    for w in out.iter_mut() {
        let mut losers: Vec<&Tab> = tabs.iter().filter(|t| t.id != w.id && t.dest_key() == w.dest_key()).collect();
        if losers.is_empty() {
            continue;
        }
        losers.sort_by_key(|t| (std::cmp::Reverse(t.last_active_ms), t.id));
        let mut changed = false;
        if w.scroll_y <= 0.0 && w.face() == Face::Yt {
            if let Some(l) = losers.iter().find(|l| l.scroll_y > 0.0) {
                w.scroll_y = l.scroll_y;
                changed = true;
            }
        }
        if w.title.is_empty() {
            if let Some(l) = losers.iter().find(|l| !l.title.is_empty()) {
                w.title = l.title.clone();
                changed = true;
            }
        }
        if changed {
            merged.push(w.id);
        }
    }
    let active = active.and_then(|a| {
        if out.iter().any(|t| t.id == a) {
            Some(a)
        } else {
            tabs.iter().find(|t| t.id == a).and_then(|t| winner_of.get(&t.dest_key()).copied())
        }
    });
    Reconciled { tabs: out, removed, merged, active }
}

/// A sleeping tab from its persisted row.
pub fn tab_from_row(id: u64, r: &crate::browsing::TabRow) -> Tab {
    let loc = match video_id_of_url(&r.url) {
        Some(v) => Loc::Watch(v),
        None => Loc::Yt(if r.url.is_empty() { HOME_URL.into() } else { r.url.clone() }),
    };
    let mut tab = Tab::new(id, loc, r.updated_at);
    tab.title = r.title.clone().unwrap_or_default();
    tab.scroll_y = r.scroll_y.unwrap_or(0.0);
    tab
}

/// Rebuilds the tab list from persisted rows (numeric ids, not closed), collapsing
/// duplicates with [`reconcile`]. Returns the result and the next free tab id.
pub fn restore_from_rows(rows: &[crate::browsing::TabRow], active: Option<u64>) -> (Reconciled, u64) {
    let mut rows: Vec<(u64, &crate::browsing::TabRow)> = rows.iter().filter(|r| r.state != "closed").filter_map(|r| r.tab_id.parse::<u64>().ok().map(|id| (id, r))).collect();
    rows.sort_by_key(|(id, _)| *id);
    let next = rows.iter().map(|(id, _)| id + 1).max().unwrap_or(1);
    let tabs = rows.into_iter().map(|(id, r)| tab_from_row(id, r)).collect();
    (reconcile(tabs, active, &[]), next)
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

    fn yt(id: u64, url: &str, at: i64) -> Tab {
        Tab::new(id, Loc::Yt(url.into()), at)
    }

    #[test]
    fn every_form_of_one_video_is_one_destination() {
        let v = "v:aaaaaaaaaaa";
        for u in [
            "https://www.youtube.com/watch?v=aaaaaaaaaaa",
            "https://m.youtube.com/watch?v=aaaaaaaaaaa&t=30s&list=PLxxxx&utm_source=x",
            "https://youtu.be/aaaaaaaaaaa?si=zzz",
            "https://www.youtube.com/shorts/aaaaaaaaaaa",
            "https://www.youtube.com/embed/aaaaaaaaaaa",
            "https://www.youtube.com/live/aaaaaaaaaaa?feature=share",
        ] {
            assert_eq!(url_key(u), v, "{u}");
        }
        assert_ne!(url_key("https://www.youtube.com/watch?v=bbbbbbbbbbb"), v);
        assert_eq!(loc_key(&Loc::Watch("aaaaaaaaaaa".into())), v);
    }

    #[test]
    fn browsing_pages_are_normalised_conservatively() {
        let home = url_key("https://www.youtube.com/");
        assert_eq!(url_key("https://youtube.com"), home);
        assert_eq!(url_key("http://m.youtube.com/?utm_campaign=a&fbclid=b&gclid=c"), home);
        assert_eq!(url_key("https://www.youtube.com/feed/subscriptions/"), url_key("https://www.youtube.com/feed/subscriptions?si=1&feature=x&pp=y#top"));
        // Same query however it is written; parameter order does not matter.
        assert_eq!(url_key("https://www.youtube.com/results?search_query=a+b&sp=EgIQAQ%3D%3D"), url_key("https://www.youtube.com/results?sp=EgIQAQ%3D%3D&search_query=a%20b&utm_source=z"));
        // Distinct destinations stay distinct.
        let d = |a: &str, b: &str| assert_ne!(url_key(a), url_key(b), "{a} vs {b}");
        d("https://www.youtube.com/results?search_query=a", "https://www.youtube.com/results?search_query=b");
        d("https://www.youtube.com/results?search_query=a", "https://www.youtube.com/results?search_query=a&sp=EgIQAQ%3D%3D");
        d("https://www.youtube.com/playlist?list=PLaaa", "https://www.youtube.com/playlist?list=PLbbb");
        d("https://www.youtube.com/feed/subscriptions", "https://www.youtube.com/feed/channels");
        d("https://www.youtube.com/feed/history", "https://www.youtube.com/account");
        d("https://www.youtube.com/@one", "https://www.youtube.com/@two");
        d("https://www.youtube.com/channel/UCaaa/videos", "https://www.youtube.com/channel/UCaaa/playlists");
        d("https://www.youtube.com/", "https://music.youtube.com/");
        d("https://www.youtube.com/", "https://www.youtube.com/feed/subscriptions");
    }

    #[test]
    fn find_or_open_reuses_a_destination_and_allocates_otherwise() {
        let mut tabs = vec![yt(1, A, 0)];
        let mut next = 2;
        assert_eq!(find_or_open(&mut tabs, &mut next, Loc::Yt("https://youtube.com/?utm_source=x".into()), 5), Opened::Existing(1));
        assert_eq!(find_or_open(&mut tabs, &mut next, Loc::Watch("aaaaaaaaaaa".into()), 5), Opened::Created(2));
        assert_eq!(find_or_open(&mut tabs, &mut next, Loc::Watch("aaaaaaaaaaa".into()), 6), Opened::Existing(2));
        assert_eq!((tabs.len(), next), (2, 3));
        let mut stale = 1;
        assert_eq!(find_or_open(&mut tabs, &mut stale, Loc::Yt(B.into()), 7).id(), 3, "a stale counter never reuses an id");
    }

    #[test]
    fn concurrent_opens_of_one_destination_make_one_tab() {
        let state = std::sync::Arc::new(std::sync::Mutex::new((Vec::<Tab>::new(), 1u64)));
        let handles: Vec<_> = (0..16)
            .map(|n| {
                let state = state.clone();
                std::thread::spawn(move || {
                    let url = ["https://www.youtube.com/watch?v=aaaaaaaaaaa", "https://youtu.be/aaaaaaaaaaa", "https://www.youtube.com/shorts/aaaaaaaaaaa", "https://m.youtube.com/watch?v=aaaaaaaaaaa&si=x"][n % 4];
                    let loc = Loc::Watch(video_id_of_url(url).unwrap());
                    let mut g = state.lock().unwrap();
                    let (tabs, next) = &mut *g;
                    find_or_open(tabs, next, loc, n as i64)
                })
            })
            .collect();
        let ids: Vec<u64> = handles.into_iter().map(|h| h.join().unwrap().id()).collect();
        assert_eq!(state.lock().unwrap().0.len(), 1);
        assert!(ids.iter().all(|i| *i == ids[0]));
    }

    #[test]
    fn navigation_peeks_do_not_move() {
        let mut t = Tab::new(1, Loc::Yt(A.into()), 0);
        t.open_video("aaaaaaaaaaa");
        assert_eq!(t.peek_back(), Some(&Loc::Yt(A.into())));
        assert_eq!(t.peek_forward(), None);
        assert_eq!(t.idx, 1);
    }

    #[test]
    fn a_bounced_page_is_ignored_until_the_view_moves_on() {
        let mut t = yt(1, A, 0);
        t.bounce = Some(url_key(B));
        assert!(t.suppress_report("https://m.youtube.com/feed/subscriptions?si=1"));
        assert!(t.suppress_report(B));
        assert!(!t.suppress_report(A));
        assert!(t.bounce.is_none());
        assert!(!t.suppress_report(B));
    }

    #[test]
    fn reconcile_prefers_active_then_playing_then_recent_and_keeps_state() {
        let mut a = yt(1, B, 10);
        a.scroll_y = 300.0;
        a.title = "Subs".into();
        let b = yt(2, "https://www.youtube.com/feed/subscriptions?si=q", 50);
        let c = yt(3, A, 5);
        let d = yt(4, A, 99);
        let vw = |id, at| Tab::new(id, Loc::Watch("aaaaaaaaaaa".into()), at);
        let all = || vec![a.clone(), b.clone(), c.clone(), d.clone(), vw(5, 20), vw(6, 30)];

        // Most recent wins; ties on recency fall to the lowest id.
        let r = reconcile(all(), None, &[]);
        assert_eq!(r.tabs.iter().map(|t| t.id).collect::<Vec<_>>(), vec![2, 4, 6]);
        assert_eq!(r.removed, vec![1, 3, 5]);
        let w = r.tabs.iter().find(|t| t.id == 2).unwrap();
        assert_eq!((w.scroll_y, w.title.as_str()), (300.0, "Subs"), "the winner inherits scroll and title");
        assert_eq!(r.merged, vec![2]);

        // A playing tab beats recency; the active tab beats playing.
        assert_eq!(reconcile(all(), None, &[5]).tabs.iter().map(|t| t.id).collect::<Vec<_>>(), vec![2, 4, 5]);
        let r = reconcile(all(), Some(3), &[5, 4]);
        assert_eq!(r.tabs.iter().map(|t| t.id).collect::<Vec<_>>(), vec![2, 3, 5]);
        assert_eq!(r.active, Some(3));
        // The active tab always wins its destination, so the focus never moves.
        let r = reconcile(all(), Some(1), &[]);
        assert_eq!((r.active, r.tabs.iter().any(|t| t.id == 1), r.removed.contains(&2)), (Some(1), true, true));

        // Deterministic whatever the order the rows arrive in.
        let mut rev = all();
        rev.reverse();
        let mut ids: Vec<u64> = reconcile(rev, None, &[]).tabs.iter().map(|t| t.id).collect();
        ids.sort();
        assert_eq!(ids, vec![2, 4, 6]);
        // Without duplicates nothing changes.
        let r = reconcile(vec![yt(1, A, 0), yt(2, B, 0)], Some(2), &[]);
        assert_eq!((r.tabs.len(), r.removed.len(), r.merged.len(), r.active), (2, 0, 0, Some(2)));
    }

    #[test]
    fn duplicates_in_saved_state_do_not_come_back_after_a_restart() {
        use crate::browsing::BrowsingState;
        use crate::model::TabIngest;
        let b = BrowsingState::in_memory().unwrap();
        let save = |id: &str, url: &str, scroll: f64, at: i64| {
            let t = TabIngest { tab_id: id.into(), url: url.into(), scroll_y: Some(scroll), state: "hidden".into(), ts: Some(at), ..TabIngest::default() };
            b.upsert_tab(&t, at).unwrap();
        };
        save("1", "https://www.youtube.com/watch?v=aaaaaaaaaaa", 0.0, 10);
        save("2", B, 250.0, 20);
        save("3", "https://youtu.be/aaaaaaaaaaa", 0.0, 30);
        save("4", "https://m.youtube.com/feed/subscriptions?si=1", 0.0, 40);
        save("import-x", A, 0.0, 50);
        let (r, next) = restore_from_rows(&b.tabs().unwrap(), Some(3));
        assert_eq!(next, 5);
        assert_eq!(r.tabs.iter().map(|t| t.id).collect::<Vec<_>>(), vec![3, 4], "active video tab and the more recent page win");
        assert_eq!(r.tabs[1].scroll_y, 250.0, "the kept page inherits the scroll position");
        assert_eq!(r.active, Some(3));
        // What the shell does with the result: drop the loser rows, rewrite merged winners.
        for id in &r.removed {
            b.upsert_tab(&TabIngest { tab_id: id.to_string(), state: "closed".into(), ..TabIngest::default() }, 100).unwrap();
        }
        for t in r.tabs.iter().filter(|t| r.merged.contains(&t.id)) {
            save(&t.id.to_string(), &t.url(), t.scroll_y, 101);
        }
        // Restart: the second restore finds the same tabs and nothing left to collapse.
        let (again, _) = restore_from_rows(&b.tabs().unwrap(), r.active);
        assert_eq!(again.tabs.iter().map(|t| t.id).collect::<Vec<_>>(), vec![3, 4]);
        assert!(again.removed.is_empty());
        assert_eq!(again.tabs[1].scroll_y, 250.0);
    }
}
