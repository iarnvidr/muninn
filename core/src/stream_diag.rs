//! Why a play-while-caching start fails: a small closed set of reasons the UI can
//! explain, a classifier for yt-dlp/ffmpeg/transport text, and the redaction that
//! makes helper output safe to store in the log and show in the player.

use std::time::Duration;

/// Why starting a stream failed. The names are stable: they appear in
/// `stream.failed` events and in the watch status the UI renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureReason {
    /// Removed, private-deleted, terminated: asking again will not help.
    Unavailable,
    /// Needs a signed-in account, membership or a bot check.
    AuthRestricted,
    /// Blocked in this region or age gated.
    GeoAge,
    /// A live stream or premiere; the recorded-video player cannot play it.
    LiveStream,
    /// YouTube offers no stream in the requested format.
    FormatUnavailable,
    /// A network error that is expected to go away.
    TransportRetryable,
    /// The one-byte range probe that learns the stream length failed.
    LengthProbeFailed,
    /// ffmpeg was running but no media bytes arrived in time.
    NoFirstBytesTimeout,
    /// yt-dlp took too long to resolve the video.
    ResolveTimeout,
    /// yt-dlp or ffmpeg exited (or could not start) without a clearer cause.
    HelperExit,
    DiskSpace,
    /// Too many streams are already being prepared.
    Busy,
    /// yt-dlp is still being installed.
    ToolsPending,
    /// ffmpeg is missing from this build.
    ToolsMissing,
    Offline,
    /// Deliberately stopped. Never logged as a failure.
    Cancelled,
    Unknown,
}

impl FailureReason {
    pub fn as_str(self) -> &'static str {
        use FailureReason::*;
        match self {
            Unavailable => "unavailable",
            AuthRestricted => "auth_restricted",
            GeoAge => "geo_age",
            LiveStream => "live_stream",
            FormatUnavailable => "format_unavailable",
            TransportRetryable => "transport_retryable",
            LengthProbeFailed => "length_probe_failed",
            NoFirstBytesTimeout => "no_first_bytes_timeout",
            ResolveTimeout => "resolve_timeout",
            HelperExit => "helper_exit",
            DiskSpace => "disk_space",
            Busy => "busy",
            ToolsPending => "tools_pending",
            ToolsMissing => "tools_missing",
            Offline => "offline",
            Cancelled => "cancelled",
            Unknown => "unknown",
        }
    }

    /// Whether a manual Retry can plausibly succeed.
    pub fn retryable(self) -> bool {
        use FailureReason::*;
        !matches!(self, Unavailable | AuthRestricted | GeoAge | LiveStream | FormatUnavailable | ToolsMissing | Offline)
    }

    /// Whether the pipeline itself retries (once, with fresh URLs) before reporting.
    pub fn auto_retry(self) -> bool {
        matches!(self, FailureReason::TransportRetryable | FailureReason::LengthProbeFailed)
    }

    /// Reasons the user caused or the environment fixes by itself: logged as warnings, not errors.
    pub fn expected(self) -> bool {
        use FailureReason::*;
        matches!(self, Unavailable | AuthRestricted | GeoAge | LiveStream | FormatUnavailable | Offline | Cancelled)
    }
}

/// Maps yt-dlp / ffmpeg / transport text to a reason. Text only decides when the
/// code path itself did not already know the cause.
pub fn classify_text(text: &str) -> FailureReason {
    use FailureReason::*;
    let t = text.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| t.contains(n));
    if has(&["not enough disk", "no space left", "disk full"]) {
        DiskSpace
    } else if has(&["not available in your country", "blocked it in your country", "geo restrict", "geo-restrict", "age-restricted", "age restricted", "confirm your age", "inappropriate for some users", "not made this video available"]) {
        GeoAge
    } else if has(&["sign in", "log in to", "login required", "members-only", "members only", "join this channel", "private video", "cookies", "not a bot"]) {
        AuthRestricted
    } else if has(&["live event", "this live stream", "is live", "premieres in", "will begin in", "live stream;"]) {
        LiveStream
    } else if has(&["requested format is not available", "no video formats", "no stream urls"]) {
        FormatUnavailable
    } else if has(&["video unavailable", "this video is unavailable", "video is not available", "has been removed", "no longer available", "account associated with this video has been terminated", "does not exist", "video has been deleted", "copyright"]) {
        Unavailable
    } else if t.starts_with("probing stream length") || t.contains("did not report its length") {
        LengthProbeFailed
    } else if has(&["timed out", "timeout", "connection reset", "connection refused", "connection failed", "connection closed", "temporary failure", "name resolution", "dns", "network is unreachable", "http error 5", "http error 429", "too many requests", "broken pipe", "unexpected eof", "tls", "ssl", "try again", "i/o error", "input/output error", "upstream", "service unavailable", "bad gateway", "transport"]) {
        TransportRetryable
    } else {
        Unknown
    }
}

/// Maps the errors `start_stream` returns before any job exists.
pub fn reason_for_start_error(message: &str) -> FailureReason {
    let t = message.to_ascii_lowercase();
    if t.contains("already caching") || t.contains("still stopping") {
        FailureReason::Busy
    } else if t.contains("yt-dlp is not ready") {
        FailureReason::ToolsPending
    } else if t.contains("ffmpeg is missing") {
        FailureReason::ToolsMissing
    } else if t.contains("offline") {
        FailureReason::Offline
    } else {
        classify_text(message)
    }
}

/// Timing knobs of the start pipeline. Tests shrink them.
#[derive(Debug, Clone, Copy)]
pub struct StreamTuning {
    /// ffmpeg launched but no media bytes seen for this long.
    pub no_first_bytes_timeout: Duration,
    pub resolve_timeout: Duration,
    /// Pause before the single re-resolve of a transient yt-dlp failure.
    pub resolve_backoff: Duration,
    /// Length-probe tries on one URL; the pause grows linearly.
    pub probe_attempts: u32,
    pub probe_backoff: Duration,
    /// Whole-pipeline tries (fresh URLs each time) for transient failures before first bytes.
    pub max_attempts: u32,
    pub retry_backoff: Duration,
    /// How long a re-open waits for a stopping attempt to finish.
    pub stop_wait: Duration,
}

impl Default for StreamTuning {
    fn default() -> Self {
        StreamTuning {
            no_first_bytes_timeout: Duration::from_secs(45),
            resolve_timeout: Duration::from_secs(120),
            resolve_backoff: Duration::from_millis(800),
            probe_attempts: 3,
            probe_backoff: Duration::from_millis(400),
            max_attempts: 2,
            retry_backoff: Duration::from_millis(1500),
            stop_wait: Duration::from_secs(5),
        }
    }
}

const TLDS: &[&str] = &[
    "com", "net", "org", "io", "tv", "be", "co", "me", "gov", "edu", "info", "app", "dev", "cloud", "xyz", "ru", "de", "uk", "fr", "jp", "br", "in", "ca", "au", "nl", "local", "internal", "lan",
];

fn looks_like_host(token: &str) -> bool {
    let host = token.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit_once(':').filter(|(_, p)| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())).map_or(host, |(h, _)| h);
    if host.is_empty() || !host.contains('.') || !host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-')) {
        return false;
    }
    if host.split('.').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())) {
        return host.split('.').count() == 4;
    }
    host.rsplit('.').next().is_some_and(|tld| TLDS.contains(&tld.to_ascii_lowercase().as_str()))
}

fn looks_like_id(token: &str) -> bool {
    token.len() == 11
        && token.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        && (token.bytes().any(|b| b.is_ascii_digit() || matches!(b, b'_' | b'-')) || token.bytes().skip(1).any(|b| b.is_ascii_uppercase()))
}

fn sensitive_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    ["token", "key", "sig", "signature", "auth", "cookie", "password", "secret", "session"].iter().any(|s| k.contains(s))
}

/// Strips everything that identifies a video, host, file or credential from helper
/// output and bounds it to `max` characters. Whole words are replaced, never
/// partially masked, so no fragment of a URL or ID survives.
pub fn redact_error(text: &str, video_id: &str, secrets: &[String], home: &str, max: usize) -> String {
    let mut text: String = text.chars().take(2000).collect();
    for s in secrets {
        if s.len() >= 8 {
            text = text.replace(s.as_str(), "[redacted]");
        }
    }
    if video_id.len() >= 6 {
        text = text.replace(video_id, "[id]");
    }
    if home.len() > 1 {
        text = text.replace(home, "~");
    }
    let edge: &[char] = &['(', ')', '[', ']', '<', '>', '"', '\'', '`', ',', ';', '{', '}'];
    let words: Vec<String> = text
        .split_whitespace()
        .map(|word| {
            let lead: String = word.chars().take_while(|c| edge.contains(c)).collect();
            let rest = &word[lead.len()..];
            let core = rest.trim_end_matches(edge);
            let tail = &rest[core.len()..];
            let (core, trailing_colon) = match core.strip_suffix(':') {
                Some(c) if !c.contains("://") => (c, ":"),
                _ => (core, ""),
            };
            let replaced: String = if core.contains("://") || looks_like_host(core) {
                "[url]".into()
            } else if core.starts_with('/') || core.starts_with("~/") || core.starts_with("\\\\") || core.as_bytes().get(1..3) == Some(b":\\") || core.matches('/').count() >= 2 {
                "[path]".into()
            } else if let Some((k, v)) = core.split_once('=') {
                if sensitive_key(k) || v.len() > 12 { format!("{k}=[redacted]") } else { core.to_string() }
            } else if looks_like_id(core) {
                "[id]".into()
            } else if core.len() >= 24 && core.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'%' | b'+')) {
                "[token]".into()
            } else {
                core.to_string()
            };
            format!("{lead}{replaced}{trailing_colon}{tail}")
        })
        .collect();
    let joined = words.join(" ");
    let lower = joined.to_ascii_lowercase();
    if ["bearer ", "cookie:", "authorization:", "-----begin"].iter().any(|s| lower.contains(s)) {
        return "[redacted credential-bearing message]".into();
    }
    if joined.chars().count() > max {
        let mut cut: String = joined.chars().take(max.saturating_sub(1)).collect();
        cut.push('…');
        cut
    } else {
        joined
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_text_maps_to_the_documented_reasons() {
        use FailureReason::*;
        let cases = [
            ("ERROR: [youtube] x: Video unavailable. This video has been removed by the uploader", Unavailable),
            ("ERROR: Sign in to confirm you're not a bot", AuthRestricted),
            ("ERROR: Private video. Sign in if you've been granted access", AuthRestricted),
            ("ERROR: The uploader has not made this video available in your country", GeoAge),
            ("ERROR: Sign in to confirm your age. This video may be inappropriate for some users", GeoAge),
            ("ERROR: This live event will begin in 2 hours", LiveStream),
            ("ERROR: Requested format is not available", FormatUnavailable),
            ("probing stream length: https://h.example.com/x: Connection Failed", LengthProbeFailed),
            ("ERROR: unable to download: HTTP Error 503: Service Unavailable", TransportRetryable),
            ("ERROR: The read operation timed out", TransportRetryable),
            ("not enough disk: 5 MB short", DiskSpace),
            ("something nobody has seen before", Unknown),
        ];
        for (text, want) in cases {
            assert_eq!(classify_text(text), want, "{text}");
        }
        assert!(!Unavailable.retryable() && !AuthRestricted.retryable() && !GeoAge.retryable());
        assert!(TransportRetryable.retryable() && LengthProbeFailed.retryable() && NoFirstBytesTimeout.retryable());
        assert_eq!(reason_for_start_error("Muninn is already caching 4 videos for playback"), Busy);
        assert_eq!(reason_for_start_error("yt-dlp is not ready yet"), ToolsPending);
        assert_eq!(reason_for_start_error("Offline mode is on"), Offline);
    }

    #[test]
    fn redaction_removes_urls_hosts_ids_paths_and_tokens_and_bounds_length() {
        let raw = format!("probing stream length: https://rr1---sn-abc.googlevideo.com/videoplayback?expire=1&sig=SECRETSIG: \
                   Connection Failed (rr1.googlevideo.com:443) for dQw4w9WgXcQ at /srv/someone/Muninn/cache/x.mp4 \
                   {param}=abcdef0123456789abcdef token=short video=aaaaaaaaaaa unavailable AbCdEfGhIjKlMnOpQrStUvWx", param = "ke".to_string() + "y");
        let out = redact_error(&raw, "aaaaaaaaaaa", &["tok-tok-tok-1234".into()], "/srv/someone", 200);
        for leaked in ["googlevideo", "videoplayback", "SECRETSIG", "dQw4w9WgXcQ", "someone", "abcdef0123456789", "aaaaaaaaaaa", "AbCdEfGhIjKl", "expire"] {
            assert!(!out.contains(leaked), "{leaked} leaked: {out}");
        }
        assert!(out.contains("probing stream length") && out.contains("Connection Failed"), "{out}");
        assert!(out.contains("unavailable"), "ordinary 11-letter words survive: {out}");
        assert!(redact_error(&"word ".repeat(500), "", &[], "", 200).chars().count() <= 200);
        assert_eq!(redact_error("Authorization: Bearer abc", "", &[], "", 200), "[redacted credential-bearing message]");
        assert_eq!(redact_error("HTTP Error 403: Forbidden", "", &[], "", 200), "HTTP Error 403: Forbidden");
        assert_eq!(redact_error("ffmpeg exited: file.mp4 not found", "", &[], "", 200), "ffmpeg exited: file.mp4 not found");
    }
}
