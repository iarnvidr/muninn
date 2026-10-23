//! HTTP Range serving for media files, including files that are still being
//! written by the download worker.
//!
//! * Complete file: ordinary 200/206/416 semantics.
//! * Growing file whose final size is known: the advertised total is the final
//!   size; the body tails the file, waiting for bytes the writer has not produced
//!   yet, and is aborted (so the client notices) if the writer dies or stalls.
//! * Growing file of unknown size: ranges are answered with what is on disk now
//!   (`Content-Range: bytes a-b/*`); a range that starts beyond the written part
//!   waits briefly for data.

use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::Response;
use std::io::SeekFrom;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

const CHUNK: usize = 256 * 1024;

#[derive(Clone)]
pub struct MediaFile {
    pub path: PathBuf,
    pub content_type: String,
    /// Final size when known although the file is still growing.
    pub final_size: Option<u64>,
    /// True while the writer may still append.
    pub growing: Arc<dyn Fn() -> bool + Send + Sync>,
    /// How long a request may wait for the first byte it asked for.
    pub first_byte_wait: Duration,
    /// How long a tailing body tolerates no growth before it is aborted.
    pub stall_timeout: Duration,
}

impl MediaFile {
    pub fn complete(path: PathBuf, content_type: &str) -> MediaFile {
        MediaFile {
            path,
            content_type: content_type.into(),
            final_size: None,
            growing: Arc::new(|| false),
            first_byte_wait: Duration::from_secs(15),
            stall_timeout: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteRange {
    /// `bytes=a-` or `bytes=a-b`
    From { start: u64, end: Option<u64> },
    /// `bytes=-n`
    Suffix(u64),
}

/// Parses a single-range `Range` header. Anything unparseable or multi-range
/// returns `None`, which means "ignore the header and send the whole file".
pub fn parse_range(header: &str) -> Option<ByteRange> {
    let spec = header.trim().strip_prefix("bytes")?.trim_start().strip_prefix('=')?.trim();
    if spec.contains(',') {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    let (a, b) = (a.trim(), b.trim());
    match (a.is_empty(), b.is_empty()) {
        (true, true) => None,
        (true, false) => Some(ByteRange::Suffix(b.parse().ok()?)),
        (false, true) => Some(ByteRange::From { start: a.parse().ok()?, end: None }),
        (false, false) => {
            let (start, end): (u64, u64) = (a.parse().ok()?, b.parse().ok()?);
            if end < start {
                return None;
            }
            Some(ByteRange::From { start, end: Some(end) })
        }
    }
}

/// Resolves a range against a known total. `Err(())` = unsatisfiable (416).
pub fn resolve(range: ByteRange, total: u64) -> Result<(u64, u64), ()> {
    if total == 0 {
        return Err(());
    }
    match range {
        ByteRange::From { start, end } => {
            if start >= total {
                return Err(());
            }
            Ok((start, end.map_or(total - 1, |e| e.min(total - 1))))
        }
        ByteRange::Suffix(n) => {
            if n == 0 {
                return Err(());
            }
            Ok((total.saturating_sub(n), total - 1))
        }
    }
}

fn base(status: StatusCode, media: &MediaFile, growing: bool) -> axum::http::response::Builder {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, media.content_type.as_str())
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CACHE_CONTROL, if growing { "no-store" } else { "private, no-cache" })
}

fn plain(status: StatusCode, text: &'static str) -> Response {
    Response::builder().status(status).header(header::CONTENT_TYPE, "text/plain").body(Body::from(text)).unwrap()
}

fn unsatisfiable(total: Option<u64>, media: &MediaFile) -> Response {
    let value = match total {
        Some(t) => format!("bytes */{t}"),
        None => "bytes */*".to_string(),
    };
    base(StatusCode::RANGE_NOT_SATISFIABLE, media, false)
        .header(header::CONTENT_RANGE, value)
        .header(header::CONTENT_LENGTH, "0")
        .body(Body::empty())
        .unwrap()
}

pub async fn serve(method: &Method, headers: &HeaderMap, media: MediaFile) -> Response {
    let len_now = |path: &PathBuf| std::fs::metadata(path).map(|m| m.len());
    let mut cur = match len_now(&media.path) {
        Ok(n) => n,
        Err(_) => return plain(StatusCode::NOT_FOUND, "media file missing"),
    };
    let growing = (media.growing)();
    // A complete file ignores `final_size`; a growing one advertises it when known.
    let total: Option<u64> = if growing { media.final_size } else { Some(cur) };
    let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok()).and_then(parse_range);
    let head = method == Method::HEAD;

    // Decide (start, end_inclusive, content_range_total, advertised_length, tail)
    let (start, end, range_total, status) = match (range, total) {
        (None, Some(t)) => {
            if t == 0 {
                return base(StatusCode::OK, &media, growing).header(header::CONTENT_LENGTH, "0").body(Body::empty()).unwrap();
            }
            (0, t - 1, Some(t), StatusCode::OK)
        }
        (Some(r), Some(t)) => match resolve(r, t) {
            Ok((s, e)) => (s, e, Some(t), StatusCode::PARTIAL_CONTENT),
            Err(()) => return unsatisfiable(Some(t), &media),
        },
        // Growing, final size unknown.
        (r, None) => {
            let wanted_start = match r {
                Some(ByteRange::From { start, .. }) => start,
                _ => 0,
            };
            let deadline = Instant::now() + media.first_byte_wait;
            while cur <= wanted_start && (media.growing)() && Instant::now() < deadline && !head {
                tokio::time::sleep(Duration::from_millis(50)).await;
                cur = len_now(&media.path).unwrap_or(cur);
            }
            if cur <= wanted_start && (r.is_some() || cur == 0) {
                return if r.is_some() { unsatisfiable(None, &media) } else { plain(StatusCode::SERVICE_UNAVAILABLE, "no data yet") };
            }
            let resolved = match r {
                Some(ByteRange::Suffix(n)) => resolve(ByteRange::Suffix(n), cur),
                Some(range) => resolve(range, cur),
                None => resolve(ByteRange::From { start: 0, end: None }, cur),
            };
            match resolved {
                Ok((s, e)) => (s, e, None, if r.is_some() { StatusCode::PARTIAL_CONTENT } else { StatusCode::OK }),
                Err(()) => return unsatisfiable(None, &media),
            }
        }
    };

    let length = end - start + 1;
    let mut builder = base(status, &media, growing);
    if status == StatusCode::PARTIAL_CONTENT {
        let total_text = range_total.map_or("*".to_string(), |t| t.to_string());
        builder = builder.header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{total_text}"));
    }
    builder = builder.header(header::CONTENT_LENGTH, length.to_string());
    if head {
        return builder.body(Body::empty()).unwrap();
    }
    let file = match tokio::fs::File::open(&media.path).await {
        Ok(f) => f,
        Err(_) => return plain(StatusCode::NOT_FOUND, "media file missing"),
    };
    builder.body(Body::from_stream(body_stream(file, start, end, media))).unwrap_or_else(|_| {
        let mut r = Response::new(Body::empty());
        *r.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
        r.headers_mut().insert(header::CONTENT_LENGTH, HeaderValue::from_static("0"));
        r
    })
}

struct StreamState {
    file: tokio::fs::File,
    pos: u64,
    end: u64,
    media: MediaFile,
    last_growth: Instant,
    seeked: bool,
}

fn body_stream(
    file: tokio::fs::File,
    start: u64,
    end: u64,
    media: MediaFile,
) -> impl futures_util::Stream<Item = Result<Vec<u8>, std::io::Error>> {
    let state = StreamState { file, pos: start, end, media, last_growth: Instant::now(), seeked: false };
    futures_util::stream::unfold(state, |mut st| async move {
        loop {
            if st.pos > st.end {
                return None;
            }
            let available = std::fs::metadata(&st.media.path).map(|m| m.len()).unwrap_or(0);
            if st.pos < available {
                if !st.seeked {
                    if let Err(e) = st.file.seek(SeekFrom::Start(st.pos)).await {
                        return Some((Err(e), st));
                    }
                    st.seeked = true;
                }
                let want = ((available - st.pos).min(st.end - st.pos + 1)).min(CHUNK as u64) as usize;
                let mut buf = vec![0u8; want];
                match st.file.read_exact(&mut buf).await {
                    Ok(_) => {
                        st.pos += want as u64;
                        st.last_growth = Instant::now();
                        return Some((Ok(buf), st));
                    }
                    Err(e) => return Some((Err(e), st)),
                }
            }
            // Need bytes the writer has not produced yet.
            if !(st.media.growing)() {
                // The writer finished; re-check once to close the race with its last write.
                let available = std::fs::metadata(&st.media.path).map(|m| m.len()).unwrap_or(0);
                if st.pos < available {
                    continue;
                }
                let e = std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "download ended before the advertised size");
                st.pos = st.end + 1;
                return Some((Err(e), st));
            }
            if st.last_growth.elapsed() > st.media.stall_timeout {
                let e = std::io::Error::new(std::io::ErrorKind::TimedOut, "download stalled");
                st.pos = st.end + 1;
                return Some((Err(e), st));
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    })
}

pub fn content_type_for(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "m4a" => "audio/mp4",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "mp3" => "audio/mpeg",
        "opus" | "ogg" => "audio/ogg",
        "aac" => "audio/aac",
        "wav" => "audio/wav",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn pattern(from: usize, to: usize) -> Vec<u8> {
        (from..to).map(|i| (i % 251) as u8).collect()
    }

    fn headers(range: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Some(r) = range {
            h.insert(header::RANGE, HeaderValue::from_str(r).unwrap());
        }
        h
    }

    async fn body(resp: Response) -> Result<Vec<u8>, String> {
        resp.into_body().collect().await.map(|c| c.to_bytes().to_vec()).map_err(|e| e.to_string())
    }

    fn header_str(resp: &Response, name: header::HeaderName) -> String {
        resp.headers().get(name).map(|v| v.to_str().unwrap().to_string()).unwrap_or_default()
    }

    fn complete_file(len: usize) -> (tempfile::TempDir, MediaFile) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v.mp4");
        std::fs::write(&path, pattern(0, len)).unwrap();
        (dir, MediaFile::complete(path, "video/mp4"))
    }

    #[test]
    fn parses_range_headers() {
        assert_eq!(parse_range("bytes=0-99"), Some(ByteRange::From { start: 0, end: Some(99) }));
        assert_eq!(parse_range("bytes=100-"), Some(ByteRange::From { start: 100, end: None }));
        assert_eq!(parse_range("bytes=-500"), Some(ByteRange::Suffix(500)));
        assert_eq!(parse_range(" bytes = 5-6 "), Some(ByteRange::From { start: 5, end: Some(6) }));
        assert_eq!(parse_range("bytes=0-1,5-6"), None, "multi-range is ignored");
        assert_eq!(parse_range("bytes=9-3"), None);
        assert_eq!(parse_range("bytes=-"), None);
        assert_eq!(parse_range("items=0-1"), None);
        assert_eq!(parse_range("bytes=a-b"), None);
        assert_eq!(parse_range(""), None);
    }

    #[test]
    fn resolves_against_total() {
        assert_eq!(resolve(ByteRange::From { start: 0, end: Some(9999) }, 100), Ok((0, 99)));
        assert_eq!(resolve(ByteRange::From { start: 100, end: None }, 100), Err(()));
        assert_eq!(resolve(ByteRange::Suffix(30), 100), Ok((70, 99)));
        assert_eq!(resolve(ByteRange::Suffix(300), 100), Ok((0, 99)));
        assert_eq!(resolve(ByteRange::Suffix(0), 100), Err(()));
        assert_eq!(resolve(ByteRange::From { start: 0, end: None }, 0), Err(()));
    }

    #[tokio::test]
    async fn complete_file_full_partial_and_suffix() {
        let (_d, media) = complete_file(1000);
        let r = serve(&Method::GET, &headers(None), media.clone()).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(header_str(&r, header::CONTENT_LENGTH), "1000");
        assert_eq!(header_str(&r, header::ACCEPT_RANGES), "bytes");
        assert_eq!(body(r).await.unwrap(), pattern(0, 1000));

        let r = serve(&Method::GET, &headers(Some("bytes=100-199")), media.clone()).await;
        assert_eq!(r.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(header_str(&r, header::CONTENT_RANGE), "bytes 100-199/1000");
        assert_eq!(header_str(&r, header::CONTENT_LENGTH), "100");
        assert_eq!(body(r).await.unwrap(), pattern(100, 200));

        let r = serve(&Method::GET, &headers(Some("bytes=900-")), media.clone()).await;
        assert_eq!(header_str(&r, header::CONTENT_RANGE), "bytes 900-999/1000");
        assert_eq!(body(r).await.unwrap(), pattern(900, 1000));

        let r = serve(&Method::GET, &headers(Some("bytes=-10")), media.clone()).await;
        assert_eq!(header_str(&r, header::CONTENT_RANGE), "bytes 990-999/1000");
        assert_eq!(body(r).await.unwrap(), pattern(990, 1000));

        // End beyond the file is clamped.
        let r = serve(&Method::GET, &headers(Some("bytes=990-5000")), media.clone()).await;
        assert_eq!(header_str(&r, header::CONTENT_RANGE), "bytes 990-999/1000");
    }

    #[tokio::test]
    async fn unsatisfiable_invalid_and_head() {
        let (_d, media) = complete_file(1000);
        let r = serve(&Method::GET, &headers(Some("bytes=1000-")), media.clone()).await;
        assert_eq!(r.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(header_str(&r, header::CONTENT_RANGE), "bytes */1000");
        // Garbage Range header: serve the whole file.
        let r = serve(&Method::GET, &headers(Some("bytes=oops")), media.clone()).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(body(r).await.unwrap().len(), 1000);
        // HEAD: headers only.
        let r = serve(&Method::HEAD, &headers(Some("bytes=0-9")), media.clone()).await;
        assert_eq!(r.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(header_str(&r, header::CONTENT_LENGTH), "10");
        assert!(body(r).await.unwrap().is_empty());
        // Missing file.
        let missing = MediaFile::complete("/nonexistent/x.mp4".into(), "video/mp4");
        assert_eq!(serve(&Method::GET, &headers(None), missing).await.status(), StatusCode::NOT_FOUND);
    }

    /// Spawns a writer that appends the pattern in steps until `total` bytes exist.
    fn spawn_writer(path: PathBuf, from: usize, total: usize, flag: Arc<AtomicBool>, delay_ms: u64) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
            let mut at = from;
            while at < total {
                let next = (at + 700).min(total);
                f.write_all(&pattern(at, next)).unwrap();
                f.flush().unwrap();
                at = next;
                std::thread::sleep(Duration::from_millis(delay_ms));
            }
            flag.store(false, Ordering::SeqCst);
        })
    }

    #[tokio::test]
    async fn growing_file_with_known_size_streams_as_it_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("g.mp4");
        std::fs::write(&path, pattern(0, 1000)).unwrap();
        let growing = Arc::new(AtomicBool::new(true));
        let flag = growing.clone();
        let mut media = MediaFile::complete(path.clone(), "video/mp4");
        media.final_size = Some(5000);
        media.growing = Arc::new(move || flag.load(Ordering::SeqCst));
        let writer = spawn_writer(path, 1000, 5000, growing, 60);

        // Total is the final size even though only 1000 bytes exist.
        let r = serve(&Method::GET, &headers(Some("bytes=500-")), media.clone()).await;
        assert_eq!(r.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(header_str(&r, header::CONTENT_RANGE), "bytes 500-4999/5000");
        assert_eq!(header_str(&r, header::CONTENT_LENGTH), "4500");
        // The body arrives complete and correct although most of it was not on disk yet.
        assert_eq!(body(r).await.unwrap(), pattern(500, 5000));
        writer.join().unwrap();

        // A seek past the written part waits for it rather than failing.
        let r = serve(&Method::GET, &headers(Some("bytes=4990-4999")), media.clone()).await;
        assert_eq!(body(r).await.unwrap(), pattern(4990, 5000));
    }

    #[tokio::test]
    async fn growing_file_whose_writer_dies_aborts_the_body() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("g.mp4");
        std::fs::write(&path, pattern(0, 1000)).unwrap();
        let growing = Arc::new(AtomicBool::new(true));
        let flag = growing.clone();
        let mut media = MediaFile::complete(path, "video/mp4");
        media.final_size = Some(5000);
        media.growing = Arc::new(move || flag.load(Ordering::SeqCst));
        let r = serve(&Method::GET, &headers(Some("bytes=0-")), media).await;
        let killer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            growing.store(false, Ordering::SeqCst);
        });
        let result = body(r).await;
        killer.join().unwrap();
        assert!(result.is_err(), "a truncated download must not look like a complete body");
    }

    #[tokio::test]
    async fn growing_file_of_unknown_size_uses_star_total() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("g.m4a");
        std::fs::write(&path, pattern(0, 1000)).unwrap();
        let growing = Arc::new(AtomicBool::new(true));
        let flag = growing.clone();
        let mut media = MediaFile::complete(path.clone(), "audio/mp4");
        media.first_byte_wait = Duration::from_millis(300);
        media.growing = Arc::new(move || flag.load(Ordering::SeqCst));

        let r = serve(&Method::GET, &headers(Some("bytes=0-")), media.clone()).await;
        assert_eq!(r.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(header_str(&r, header::CONTENT_RANGE), "bytes 0-999/*");
        assert_eq!(body(r).await.unwrap(), pattern(0, 1000));

        // Starting past what exists, with no writer progress: 416 after the wait.
        let r = serve(&Method::GET, &headers(Some("bytes=2000-")), media.clone()).await;
        assert_eq!(r.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(header_str(&r, header::CONTENT_RANGE), "bytes */*");

        // ... but data that arrives during the wait is served.
        let mut slow = media.clone();
        slow.first_byte_wait = Duration::from_secs(5);
        let writer = spawn_writer(path, 1000, 3000, growing, 80);
        let r = serve(&Method::GET, &headers(Some("bytes=1500-")), slow).await;
        assert_eq!(r.status(), StatusCode::PARTIAL_CONTENT);
        let got = body(r).await.unwrap();
        assert_eq!(got[0..10], pattern(1500, 1510)[..]);
        writer.join().unwrap();
    }

    #[test]
    fn content_types() {
        assert_eq!(content_type_for("mp4"), "video/mp4");
        assert_eq!(content_type_for("M4A"), "audio/mp4");
        assert_eq!(content_type_for("mov"), "video/quicktime");
        assert_eq!(content_type_for("aac"), "audio/aac");
        assert_eq!(content_type_for("wav"), "audio/wav");
        assert_eq!(content_type_for("xyz"), "application/octet-stream");
    }
}
