//! Eviction policy and disk caps. Pure functions, no I/O.
//!
//! Order of eviction, first to go first:
//!  0. expired entries (past their retention)
//!  1. failed, canceled or stalled partial downloads
//!  2. speculative entries never played (prefetched by a rule)
//!  3. entries already watched to the end
//!  4. entries the user asked for manually and has not played yet
//!  5. partially watched videos
//! Within a class the least recently used goes first, bigger before smaller on
//! ties. Protected (library) entries and downloads in progress are never evicted.

#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub id: String,
    pub protected: bool,
    /// A download is running right now.
    pub active: bool,
    /// Download finished and the file is whole.
    pub complete: bool,
    /// "rule", "manual", "prefetch" or "capture".
    pub origin: String,
    pub size_bytes: u64,
    pub last_access_at: i64,
    pub expires_at: Option<i64>,
    /// Watch progress 0..1, `None` when never played.
    pub watched: Option<f64>,
}

pub const WATCHED_TO_END: f64 = 0.95;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    Expired = 0,
    Junk = 1,
    Speculative = 2,
    Watched = 3,
    ManualUnplayed = 4,
    PartiallyWatched = 5,
}

/// `None` means "never evict this".
pub fn classify(c: &Candidate, now: i64) -> Option<Class> {
    if c.protected || c.active {
        return None;
    }
    if c.expires_at.is_some_and(|t| t <= now) {
        return Some(Class::Expired);
    }
    if !c.complete {
        return Some(Class::Junk);
    }
    Some(match c.watched {
        None if c.origin == "manual" => Class::ManualUnplayed,
        None => Class::Speculative,
        Some(w) if w >= WATCHED_TO_END => Class::Watched,
        Some(_) => Class::PartiallyWatched,
    })
}

#[derive(Debug, Clone, Copy)]
pub struct Caps {
    /// Cap for the cache (evictable) bytes.
    pub max_disk_bytes: u64,
    /// Keep at least this much free on the filesystem.
    pub min_free_bytes: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub evict: Vec<String>,
    /// Bytes still missing after evicting everything allowed (0 when satisfied).
    pub shortfall: u64,
}

/// Bytes that must be freed so that `incoming` more bytes fit under both caps.
pub fn bytes_needed(used_cache: u64, free: Option<u64>, incoming: u64, caps: Caps) -> u64 {
    let over_cap = (used_cache + incoming).saturating_sub(caps.max_disk_bytes);
    let over_free = match free {
        Some(free) => (caps.min_free_bytes + incoming).saturating_sub(free),
        None => 0,
    };
    over_cap.max(over_free)
}

/// Decides what to evict. Expired entries always go; beyond that entries go in
/// policy order until the caps hold for `incoming` additional bytes.
pub fn plan(candidates: &[Candidate], now: i64, used_cache: u64, free: Option<u64>, incoming: u64, caps: Caps) -> Plan {
    let mut ranked: Vec<(Class, &Candidate)> =
        candidates.iter().filter_map(|c| classify(c, now).map(|class| (class, c))).collect();
    ranked.sort_by(|(ca, a), (cb, b)| {
        ca.cmp(cb)
            .then(a.last_access_at.cmp(&b.last_access_at))
            .then(b.size_bytes.cmp(&a.size_bytes))
            .then(a.id.cmp(&b.id))
    });
    let mut needed = bytes_needed(used_cache, free, incoming, caps);
    let mut evict = Vec::new();
    for (class, c) in ranked {
        if class == Class::Expired {
            evict.push(c.id.clone());
            needed = needed.saturating_sub(c.size_bytes);
            continue;
        }
        if needed == 0 {
            // Classes are sorted, so nothing after this needs to go either.
            break;
        }
        evict.push(c.id.clone());
        needed = needed.saturating_sub(c.size_bytes);
    }
    Plan { evict, shortfall: needed }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_000_000_000;

    fn cand(id: &str, size: u64, last: i64) -> Candidate {
        Candidate {
            id: id.into(),
            protected: false,
            active: false,
            complete: true,
            origin: "rule".into(),
            size_bytes: size,
            last_access_at: last,
            expires_at: Some(NOW + 1_000_000),
            watched: None,
        }
    }

    fn caps(max: u64, min_free: u64) -> Caps {
        Caps { max_disk_bytes: max, min_free_bytes: min_free }
    }

    #[test]
    fn protected_and_active_are_never_evicted() {
        let mut lib = cand("lib", 100, 0);
        lib.protected = true;
        lib.expires_at = Some(NOW - 1); // even expired
        let mut active = cand("dl", 100, 0);
        active.active = true;
        active.complete = false;
        let p = plan(&[lib, active], NOW, 10_000, Some(0), 5_000, caps(100, 1_000_000));
        assert!(p.evict.is_empty());
        assert!(p.shortfall > 0);
    }

    #[test]
    fn speculative_goes_before_partially_watched() {
        let mut spec_new = cand("spec-new", 100, 900);
        spec_new.watched = None;
        let mut partial_old = cand("partial-old", 100, 1); // much older, but partially watched
        partial_old.watched = Some(0.4);
        let mut watched = cand("done", 100, 500);
        watched.watched = Some(1.0);
        let mut manual = cand("manual", 100, 2);
        manual.origin = "manual".into();
        // Need to free 300 of 400 used.
        let p = plan(&[partial_old, manual, watched, spec_new], NOW, 400, None, 0, caps(100, 0));
        assert_eq!(p.evict, vec!["spec-new", "done", "manual"]);
        assert_eq!(p.shortfall, 0);
    }

    #[test]
    fn lru_within_a_class_and_stops_when_satisfied() {
        let a = cand("a", 100, 30);
        let b = cand("b", 100, 10);
        let c = cand("c", 100, 20);
        let p = plan(&[a, b, c], NOW, 300, None, 0, caps(250, 0));
        assert_eq!(p.evict, vec!["b"]);
    }

    #[test]
    fn expired_and_junk_go_first_and_expired_always_go() {
        let mut expired = cand("expired", 10, 999);
        expired.expires_at = Some(NOW - 1);
        let mut junk = cand("junk", 10, 999);
        junk.complete = false;
        let spec = cand("spec", 10, 1);
        // Under the cap: only the expired entry is removed.
        let p = plan(&[spec.clone(), junk.clone(), expired.clone()], NOW, 30, None, 0, caps(1_000, 0));
        assert_eq!(p.evict, vec!["expired"]);
        // Over the cap by one entry: junk goes next, before the older speculative one.
        let p = plan(&[spec, junk, expired], NOW, 30, None, 0, caps(15, 0));
        assert_eq!(p.evict, vec!["expired", "junk"]);
    }

    #[test]
    fn disk_caps_account_for_incoming_bytes_and_min_free() {
        // Cap: 1000 total, we use 900, a 300 byte download is coming -> free 200.
        assert_eq!(bytes_needed(900, Some(u64::MAX / 2), 300, caps(1000, 0)), 200);
        // Free space: 500 free, must keep 400, download 300 -> free 200.
        assert_eq!(bytes_needed(0, Some(500), 300, caps(1_000_000, 400)), 200);
        // Both limits: the larger need wins.
        assert_eq!(bytes_needed(900, Some(500), 300, caps(1000, 400)), 200);
        assert_eq!(bytes_needed(900, Some(100), 300, caps(1000, 400)), 600);
        // Unknown free space only applies the cap.
        assert_eq!(bytes_needed(10, None, 10, caps(100, 400)), 0);
        // Everything fits.
        assert_eq!(bytes_needed(10, Some(10_000), 10, caps(100, 400)), 0);
    }

    #[test]
    fn plan_frees_for_min_free_even_under_cap() {
        let a = cand("a", 400, 1);
        let b = cand("b", 400, 2);
        // Plenty under the cap, but the disk only has 100 free and we must keep 500.
        let p = plan(&[a, b], NOW, 800, Some(100), 0, caps(10_000, 500));
        assert_eq!(p.evict, vec!["a"]);
        assert_eq!(p.shortfall, 0);
    }

    #[test]
    fn shortfall_reported_when_only_protected_remain() {
        let mut lib = cand("lib", 1000, 0);
        lib.protected = true;
        let p = plan(&[lib], NOW, 1000, None, 500, caps(1000, 0));
        assert!(p.evict.is_empty());
        assert_eq!(p.shortfall, 500);
    }

    #[test]
    fn classification_table() {
        let mut c = cand("x", 1, 1);
        assert_eq!(classify(&c, NOW), Some(Class::Speculative));
        c.watched = Some(0.5);
        assert_eq!(classify(&c, NOW), Some(Class::PartiallyWatched));
        c.watched = Some(0.97);
        assert_eq!(classify(&c, NOW), Some(Class::Watched));
        c.complete = false;
        assert_eq!(classify(&c, NOW), Some(Class::Junk));
        c.protected = true;
        assert_eq!(classify(&c, NOW), None);
    }
}
