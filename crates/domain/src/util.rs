//! Small shared helpers: clocks, hex, ids, free-space and directory sizes.
//! Filesystem helpers are native; this crate does not promise a WASM target.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub const DAY_MS: i64 = 86_400_000;
pub const HOUR_MS: i64 = 3_600_000;
pub const MINUTE_MS: i64 = 60_000;

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 15) as usize] as char);
    }
    out
}

/// A 244-bit random token (two v4 UUIDs, hex, no dashes).
pub fn random_token() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

pub fn random_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..12].to_string()
}

/// Constant-time equality for tokens.
pub fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Legacy YouTube IDs remain unchanged; TikTok uses a separate, filesystem-safe
/// namespace so numeric TikTok IDs cannot collide with YouTube or entry variants.
pub fn valid_video_id(id: &str) -> bool {
    (id.len() == 11 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
        || id.strip_prefix("tt_").is_some_and(|n| (15..=22).contains(&n.len()) && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The download worker accepts provider IDs, never a user-supplied arbitrary URL.
/// yt-dlp's TikTok extractor accepts the `@_` placeholder if the real handle
/// was not captured (and substitutes the same placeholder internally).
pub fn source_video_url(id: &str) -> Option<String> {
    if let Some(n) = id.strip_prefix("tt_") {
        return valid_video_id(id).then(|| format!("https://www.tiktok.com/@_/video/{n}"));
    }
    valid_video_id(id).then(|| format!("https://www.youtube.com/watch?v={id}"))
}

/// Free bytes available to unprivileged users on the filesystem holding `path`.
pub fn free_space(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let c = CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: `c` is a valid NUL-terminated path and `stat` is a valid out pointer.
        let rc = unsafe { libc::statvfs(c.as_ptr(), &mut stat) };
        if rc != 0 {
            return None;
        }
        Some((stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// Total size of regular files below `path` (0 when it does not exist).
pub fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    let Ok(read) = std::fs::read_dir(path) else { return 0 };
    for entry in read.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            total += dir_size(&entry.path());
        } else {
            total += meta.len();
        }
    }
    total
}

pub fn clamp_f(value: f64, lo: f64, hi: f64) -> f64 {
    if value.is_nan() {
        lo
    } else {
        value.max(lo).min(hi)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_ids() {
        assert!(valid_video_id("dQw4w9WgXcQ"));
        assert!(valid_video_id("a_b-c_d-e_f"));
        assert!(!valid_video_id("short"));
        assert!(!valid_video_id("../etc/passwd"));
        assert!(!valid_video_id("dQw4w9WgXc!"));
        assert!(valid_video_id("tt_7668090902816017671"));
        assert!(!valid_video_id("tt_1234567890/../../etc"));
        assert_eq!(source_video_url("tt_7668090902816017671").as_deref(),Some("https://www.tiktok.com/@_/video/7668090902816017671"));
        assert!(source_video_url("tt_1").is_none());
    }

    #[test]
    fn tokens_are_long_and_unique() {
        let (a, b) = (random_token(), random_token());
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
        assert!(ct_eq(&a, &a.clone()));
        assert!(!ct_eq(&a, &b));
        assert!(!ct_eq("abc", "abcd"));
    }

    #[test]
    fn free_space_reports_something() {
        assert!(free_space(&std::env::temp_dir()).unwrap_or(0) > 0);
    }
}
