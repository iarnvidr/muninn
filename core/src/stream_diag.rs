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

/// Whole-word match, so "dns" does not fire inside another word.
fn has_word(text: &str, word: &str) -> bool {
    text.match_indices(word).any(|(i, _)| {
        let before = text[..i].chars().next_back().is_none_or(|c| !c.is_ascii_alphanumeric());
        let after = text[i + word.len()..].chars().next().is_none_or(|c| !c.is_ascii_alphanumeric());
        before && after
    })
}

/// Maps yt-dlp / ffmpeg / transport text to a reason. Text only decides when the
/// code path itself did not already know the cause.
pub fn classify_text(text: &str) -> FailureReason {
    use FailureReason::*;
    let t = text.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| t.contains(n));
    if has(&["not enough disk", "no space left", "disk full"]) {
        DiskSpace
    } else if has(&["not available in your country", "not made this video available in your country", "blocked in your country", "blocked it in your country", "geo restrict", "geo-restrict", "age-restricted", "age restricted", "confirm your age", "inappropriate for some users"]) {
        GeoAge
    } else if has(&["sign in", "log in to", "login required", "members-only", "members only", "join this channel", "private video", "--cookies", "cookies are no longer valid", "not a bot"]) {
        AuthRestricted
    } else if has(&["live event", "this live stream", "is live", "premieres in", "will begin in", "live stream;"]) {
        LiveStream
    } else if has(&["requested format is not available", "no video formats", "no stream urls"]) {
        FormatUnavailable
    } else if has(&["video unavailable", "this video is unavailable", "has been removed", "no longer available", "account associated with this video has been terminated", "video does not exist", "video has been deleted", "copyright"]) {
        Unavailable
    } else if t.starts_with("probing stream length") || t.contains("did not report its length") {
        LengthProbeFailed
    } else if has(&["timed out", "timeout", "connection reset", "connection refused", "connection failed", "connection closed", "temporary failure", "name resolution", "network is unreachable", "http error 5", "http error 429", "too many requests", "broken pipe", "unexpected eof", "try again", "i/o error", "input/output error", "upstream", "service unavailable", "bad gateway"]) || ["dns", "tls", "ssl", "transport"].iter().any(|w| has_word(&t, w)) {
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

/// Per-request limits of the upstream length probe (connect + read), see `upstream_agent`.
pub const PROBE_REQUEST_LIMIT: Duration = Duration::from_secs(45);
/// A video is fed to ffmpeg as at most this many inputs (video + audio).
const MAX_INPUTS: u32 = 2;

impl StreamTuning {
    /// Longest a start can legitimately keep working without reporting: every attempt resolves twice,
    /// probes each input on every try and waits for first bytes, and a transient failure repeats all of it.
    pub fn worst_case_start(&self) -> Duration {
        let resolve = self.resolve_timeout * 2 + self.resolve_backoff;
        let tries = self.probe_attempts.max(1);
        let backoffs: Duration = (1..tries).map(|n| self.probe_backoff * n).sum();
        let probe = (PROBE_REQUEST_LIMIT * tries + backoffs) * MAX_INPUTS;
        let attempts = self.max_attempts.max(1);
        (resolve + probe + self.no_first_bytes_timeout) * attempts + self.retry_backoff * (attempts - 1)
    }
}

/// Final labels that are file names, not hosts. Any other alphabetic final label is treated as a host.
const FILE_EXTS: &[&str] = &[
    "txt", "log", "json", "mkv", "webm", "m4a", "mp3", "ogg", "opus", "wav", "flac", "aac", "mov", "avi", "srt", "vtt", "jpg", "jpeg", "png", "gif", "webp", "exe", "dll", "yaml", "yml", "xml", "html", "part", "tmp", "ytdl", "conf", "ini", "toml", "cfg",
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
    if host.split('.').any(|p| p.is_empty()) {
        return false;
    }
    let tld = host.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    tld.len() >= 2 && tld.bytes().all(|b| b.is_ascii_alphabetic()) && !FILE_EXTS.contains(&tld.as_str())
}

fn looks_like_ipv6(s: &str) -> bool {
    let colons = s.bytes().filter(|b| *b == b':').count();
    colons >= 2 && (s.contains("::") || colons >= 7) && s.bytes().all(|b| b.is_ascii_hexdigit() || matches!(b, b':' | b'.' | b'%'))
}

/// `[::1]:8080`-style bracketed IPv6 host inside a word.
fn mask_bracketed_ipv6(word: &str) -> Option<String> {
    let p = word.find('[')?;
    let q = p + word[p..].find(']')?;
    let inner = &word[p + 1..q];
    if !inner.contains(':') || !looks_like_ipv6(inner.split('%').next().unwrap_or(inner)) {
        return None;
    }
    let rest = &word[q + 1..];
    let rest = match rest.strip_prefix(':') {
        Some(port) if port.bytes().take_while(u8::is_ascii_digit).count() > 0 => port.trim_start_matches(|c: char| c.is_ascii_digit()),
        _ => rest,
    };
    Some(format!("{}[ip]{}", &word[..p], rest))
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

/// `name:` forms: the name must end in a credential word ("x-goog-api-key", "access_token").
fn credential_label(label: &str) -> bool {
    let k: String = label.bytes().filter(u8::is_ascii_alphanumeric).map(|b| b.to_ascii_lowercase() as char).collect();
    ["token", "key", "secret", "password", "passwd", "pwd", "authorization", "auth", "cookie", "cookies", "signature", "sig", "session", "sessionid", "credential", "credentials", "apikey"].iter().any(|s| k.ends_with(s))
}

const EDGE: &[char] = &['(', ')', '[', ']', '<', '>', '"', '\'', '`', ',', ';', '{', '}'];
const PUNCT: &[char] = &[':', '.', '!', '?'];
const QUOTES: &[char] = &['"', '\'', '`'];

fn path_start(core: &str) -> bool {
    let b = core.as_bytes();
    core.starts_with('/') || core.starts_with("~/") || core.starts_with("\\\\") || (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/'))
}

/// A word whose last path component carries a file extension (`x.mp4`) ends an unquoted path.
fn ends_with_file_ext(core: &str) -> bool {
    let last = core.rsplit(['/', '\\']).next().unwrap_or("");
    match last.rsplit_once('.') {
        Some((stem, ext)) => !stem.is_empty() && (2..=5).contains(&ext.len()) && ext.bytes().all(|b| b.is_ascii_alphanumeric()) && ext.bytes().any(|b| b.is_ascii_alphabetic()),
        None => false,
    }
}

fn split_word(word: &str) -> (&str, &str, &str) {
    let lead_len = word.len() - word.trim_start_matches(EDGE).len();
    let rest = &word[lead_len..];
    let core = rest.trim_end_matches(|c: char| EDGE.contains(&c) || PUNCT.contains(&c));
    (&word[..lead_len], core, &rest[core.len()..])
}

fn redact_line(line: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    // Inside a path that contained spaces: Some(quoted) until a clear boundary.
    let mut in_path: Option<bool> = None;
    let mut mask_next = false;
    for word in line.split_whitespace() {
        let (lead, core, tail) = split_word(word);
        if let Some(quoted) = in_path {
            let closes = tail.contains(QUOTES);
            let ends = closes || (!quoted && (tail.contains([':', '.', '!', '?', ',', ';']) || ends_with_file_ext(core)));
            if let Some(last) = out.last_mut() {
                if ends {
                    last.push_str(tail);
                }
            }
            if ends {
                in_path = None;
            }
            continue;
        }
        if mask_next {
            mask_next = false;
            out.push(format!("{lead}[redacted]{tail}"));
            continue;
        }
        if core.is_empty() {
            out.push(word.to_string());
            continue;
        }
        let replaced: String = if word.contains("://") || looks_like_host(core) {
            "[url]".into()
        } else if let Some(masked) = mask_bracketed_ipv6(word) {
            out.push(masked);
            continue;
        } else if core.contains('@') {
            "[email]".into()
        } else if looks_like_ipv6(core) {
            "[ip]".into()
        } else if path_start(core) {
            let quoted = lead.contains(QUOTES);
            let ends = if quoted { tail.contains(QUOTES) } else { !tail.is_empty() && tail.contains([':', '.', '!', '?', ',', ';']) || ends_with_file_ext(core) };
            if !ends {
                in_path = Some(quoted);
            }
            "[path]".into()
        } else if core.contains('/') || core.contains('\\') {
            "[path]".into()
        } else if let Some((k, v)) = core.split_once('=') {
            if v.is_empty() && sensitive_key(k) {
                mask_next = true;
                core.to_string()
            } else if sensitive_key(k) || v.len() > 12 {
                format!("{k}=[redacted]")
            } else {
                core.to_string()
            }
        } else if tail.contains(':') && credential_label(core) {
            mask_next = true;
            core.to_string()
        } else if looks_like_id(core) {
            "[id]".into()
        } else if core.len() >= 24 && core.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'%' | b'+')) {
            "[token]".into()
        } else {
            core.to_string()
        };
        out.push(format!("{lead}{replaced}{tail}"));
    }
    out
}

/// Strips everything that identifies a video, host, file or credential from helper
/// output and bounds it to `max` characters. Whole words are replaced, never
/// partially masked, so no fragment of a URL or ID survives. Redaction runs on the
/// complete text; only the result is truncated.
pub fn redact_error(text: &str, video_id: &str, secrets: &[String], home: &str, max: usize) -> String {
    let mut text = text.to_string();
    for s in secrets {
        if s.len() >= 3 {
            text = text.replace(s.as_str(), "[redacted]");
        }
    }
    if video_id.len() >= 6 {
        text = text.replace(video_id, "[id]");
    }
    if home.len() > 1 {
        text = text.replace(home, "~");
    }
    let joined = text.lines().flat_map(redact_line).collect::<Vec<_>>().join(" ");
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
            ("ERROR: This video is blocked in your country", GeoAge),
            ("ERROR: HTTP Error 403: Forbidden", Unknown),
            ("ERROR: This video is not available", Unknown),
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

    fn red(text: &str) -> String {
        redact_error(text, "", &[], "", 400)
    }

    #[test]
    fn paths_with_spaces_are_masked_whole() {
        for (raw, tail_word) in [
            ("open /srv/someone/Videos/My Movie/x.mp4", "Movie"),
            ("Error: '/srv/someone/Videos/My Movie/x.mp4' failed", "Movie"),
            ("cannot read /srv/someone/Videos/My Movie: Permission denied", "Movie"),
            ("cannot read C:\\Users\\Bob\\My Movie\\x.mp4 now", "Movie"),
            ("no such file Movie Night/x.mp4", "x.mp4"),
            ("no such file dir/x.mp4", "x.mp4"),
        ] {
            let out = red(raw);
            assert!(!out.contains(tail_word) && !out.contains("someone") && !out.contains("Bob") && !out.contains("dir/"), "{raw} -> {out}");
        }
        assert_eq!(red("cannot read /srv/someone/Videos/My Movie: Permission denied"), "cannot read [path]: Permission denied");
        assert_eq!(red("open /srv/someone/My Movie/x.mp4 failed"), "open [path] failed");
        assert!(red("file.mp4 not found").contains("file.mp4 not found"));
    }

    #[test]
    fn emails_ipv6_unlisted_hosts_and_colon_credentials_are_masked() {
        for (raw, leaked) in [
            ("mail bob@example.com failed", "bob"),
            ("connect to [::1]:8080 failed", "::1"),
            ("connect to [::1]:8080 failed", "8080"),
            ("connect to [2001:db8::7]:443 failed", "db8"),
            ("connect to 2001:db8::7 failed", "db8"),
            ("reach cdn.example.zzz failed", "example"),
            ("reach cdn.example.museum. failed", "museum"),
            ("x-goog-api-key: v", "key: v"),
            ("Password: pw", "pw"),
            ("access_token: q", "token: q"),
        ] {
            let out = red(raw);
            assert!(!out.contains(leaked), "{leaked} leaked from {raw}: {out}");
        }
        assert_eq!(red("token: abc"), "token: [redacted]");
        assert_eq!(red("x-goog-api-key: v"), "x-goog-api-key: [redacted]");
        assert!(!redact_error("secret abc12 here", "", &["abc12".into()], "", 100).contains("abc12"));
        assert_eq!(red("Error 5: retry in 12:30:45 [youtube] e.g. ok"), "Error 5: retry in 12:30:45 [youtube] e.g. ok");
    }

    #[test]
    fn redaction_happens_before_truncation() {
        // A secret straddling the cut would survive as a fragment if the text were cut first.
        let raw = format!("{} /srv/someone/Secret Place/data.bin", "x ".repeat(95));
        let out = redact_error(&raw, "", &[], "", 200);
        assert!(!out.contains("Secret") && !out.contains("someone"), "{out}");
        let long = format!("{} https://h.example.com/p?sig=abcdefghijklmnop", "word ".repeat(450));
        assert!(!redact_error(&long, "", &[], "", 5000).contains("example"));
    }

    #[test]
    fn loose_classifier_matches_are_tightened() {
        use FailureReason::*;
        for text in ["warning: cookies enabled for example", "the folder does not exist", "undns resolution", "settlsomething", "classlss"] {
            assert_eq!(classify_text(text), Unknown, "{text}");
        }
        assert_eq!(classify_text("DNS lookup failed"), TransportRetryable);
        assert_eq!(classify_text("ERROR: This video does not exist"), Unavailable);
        assert_eq!(classify_text("Use --cookies-from-browser for the authentication"), AuthRestricted);
    }

    #[test]
    fn the_start_budget_covers_the_documented_worst_case() {
        let t = StreamTuning::default();
        // Two resolves of 120 s per attempt alone already exceed the old 150 s UI watchdog.
        assert!(t.worst_case_start() > Duration::from_secs(2 * 120 * 2));
        // ui/lib.js START_WATCHDOG_MS fallback must stay above this.
        assert!(t.worst_case_start() < Duration::from_millis(1_200_000), "{:?}", t.worst_case_start());
    }
}
