//! External tools: yt-dlp (bundled in the app; a copy in the data dir updates itself, and
//! a missing one is downloaded checksum-verified), ffmpeg and a JS runtime (bundled), format
//! selection, the yt-dlp command line and its progress protocol.
//!
//! macOS GUI apps do not inherit the shell PATH, so tools are looked up in fixed
//! directories (the app's own `Resources/bin` first) and child processes get an explicit PATH.

use crate::error::{Error, Result};
use crate::quality::Quality;
use crate::util::hex;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

pub const RELEASE_BASE: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download";

/// The standalone release asset for this OS.
pub fn ytdlp_asset_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "yt-dlp_macos"
    } else if cfg!(target_arch = "aarch64") {
        "yt-dlp_linux_aarch64"
    } else {
        "yt-dlp_linux"
    }
}

// --- HTTP fetching (replaceable in tests) -----------------------------------------------

pub struct HttpResponse {
    pub status: u16,
    pub content_encoding: Option<String>,
    pub body: Vec<u8>,
}

pub trait Fetcher: Send + Sync {
    fn get(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>>;

    /// A general request (the page fetcher and the innertube client use this).
    fn request(&self, _method: &str, url: &str, _headers: &[(&str, String)], _body: Option<&[u8]>, _max_bytes: u64) -> Result<HttpResponse> {
        Err(Error::other(format!("this fetcher cannot request {url}")))
    }
}

pub struct UreqFetcher;

impl Fetcher for UreqFetcher {
    fn get(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>> {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(15))
            .timeout_read(Duration::from_secs(60))
            .user_agent("Muninn/0.1 (+local cache)")
            .build();
        let resp = agent.get(url).call().map_err(|e| Error::other(format!("GET {url}: {e}")))?;
        let mut out = Vec::new();
        resp.into_reader().take(max_bytes + 1).read_to_end(&mut out)?;
        if out.len() as u64 > max_bytes {
            return Err(Error::other(format!("GET {url}: response larger than {max_bytes} bytes")));
        }
        Ok(out)
    }

    fn request(&self, method: &str, url: &str, headers: &[(&str, String)], body: Option<&[u8]>, max_bytes: u64) -> Result<HttpResponse> {
        let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(15)).timeout_read(Duration::from_secs(30)).build();
        let mut req = agent.request(method, url);
        for (k, v) in headers {
            req = req.set(k, v);
        }
        let resp = match body {
            Some(b) => req.send_bytes(b),
            None => req.call(),
        };
        let resp = match resp {
            Ok(r) => r,
            Err(ureq::Error::Status(_, r)) => r,
            Err(e) => return Err(Error::other(format!("{method} {url}: {e}"))),
        };
        let status = resp.status();
        let content_encoding = resp.header("content-encoding").map(str::to_string);
        let mut out = Vec::new();
        resp.into_reader().take(max_bytes + 1).read_to_end(&mut out)?;
        if out.len() as u64 > max_bytes {
            return Err(Error::other(format!("{method} {url}: response larger than {max_bytes} bytes")));
        }
        Ok(HttpResponse { status, content_encoding, body: out })
    }
}

// --- yt-dlp install and update ------------------------------------------------------------

/// Finds the hex digest for `asset` in a `SHA2-256SUMS` file (`<hex>  <name>` lines).
pub fn parse_sha_sums(text: &str, asset: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let (digest, name) = (parts.next()?, parts.next()?);
        (name.trim_start_matches('*') == asset && digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| digest.to_ascii_lowercase())
    })
}

pub fn ytdlp_path(bin_dir: &Path) -> PathBuf {
    bin_dir.join("yt-dlp")
}

/// Directory with the tools shipped inside the app (`Contents/Resources/bin`).
/// Set once at startup; searched before everything else.
static BUNDLED_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

pub fn set_bundled_dir(dir: PathBuf) {
    let _ = BUNDLED_DIR.set(dir);
}

pub fn bundled_dir() -> Option<PathBuf> {
    BUNDLED_DIR.get().cloned()
}

/// The yt-dlp to run: the self-updating copy in the data dir, else one found in `dirs`
/// (the bundled one, or a system install).
pub fn find_ytdlp(bin_dir: &Path, dirs: &[PathBuf]) -> Option<PathBuf> {
    let own = ytdlp_path(bin_dir);
    if own.exists() {
        return Some(own);
    }
    find_in(dirs, "yt-dlp")
}

/// Downloads the standalone yt-dlp, verifies it against the release's
/// SHA2-256SUMS and installs it atomically. Returns a log of what happened.
pub fn install_ytdlp(fetcher: &dyn Fetcher, bin_dir: &Path, asset: &str, base_url: &str) -> Result<String> {
    std::fs::create_dir_all(bin_dir)?;
    let mut log = String::new();
    let sums_url = format!("{base_url}/SHA2-256SUMS");
    log.push_str(&format!("fetching {sums_url}\n"));
    let sums = fetcher.get(&sums_url, 1024 * 1024)?;
    let expected = parse_sha_sums(&String::from_utf8_lossy(&sums), asset)
        .ok_or_else(|| Error::other(format!("SHA2-256SUMS has no entry for {asset}")))?;
    let url = format!("{base_url}/{asset}");
    log.push_str(&format!("downloading {url}\n"));
    let bytes = fetcher.get(&url, 400 * 1024 * 1024)?;
    let actual = hex(&Sha256::digest(&bytes));
    if actual != expected {
        return Err(Error::other(format!("checksum mismatch for {asset}: expected {expected}, got {actual}; nothing was installed")));
    }
    log.push_str(&format!("sha256 verified: {actual}\n"));
    let target = ytdlp_path(bin_dir);
    let tmp = bin_dir.join("yt-dlp.download");
    std::fs::write(&tmp, &bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&tmp, &target)?;
    log.push_str(&format!("installed {}\n", target.display()));
    Ok(log)
}

/// `yt-dlp -U`: the standalone binary updates itself in place.
pub fn update_ytdlp(bin_dir: &Path) -> Result<String> {
    let bin = ytdlp_path(bin_dir);
    if !bin.exists() {
        return Err(Error::other("yt-dlp is not installed yet"));
    }
    let out = Command::new(&bin).arg("-U").env("PATH", search_path()).stdin(Stdio::null()).output()?;
    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    if out.status.success() {
        Ok(text)
    } else {
        Err(Error::other(format!("yt-dlp -U failed: {}", text.trim())))
    }
}

// --- discovery --------------------------------------------------------------------------------

/// Directories searched for tools: the bundled ones first. GUI apps on macOS have a minimal PATH,
/// so system prefixes are listed explicitly (useful for development builds).
pub fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = bundled_dir().into_iter().collect();
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"].iter().map(PathBuf::from));
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(&home).join(".deno/bin"));
        dirs.push(PathBuf::from(&home).join(".local/bin"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            if !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
    }
    dirs
}

pub fn search_path() -> std::ffi::OsString {
    std::env::join_paths(search_dirs()).unwrap_or_default()
}

pub fn find_in(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    dirs.iter().map(|d| d.join(name)).find(|p| is_executable(p))
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

fn first_line_of(cmd: &Path, arg: &str) -> Option<String> {
    let out = Command::new(cmd).arg(arg).env("PATH", search_path()).stdin(Stdio::null()).output().ok()?;
    String::from_utf8_lossy(&out.stdout).lines().next().map(|l| l.trim().to_string()).filter(|l| !l.is_empty())
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct YtDlpStatus {
    pub installed: bool,
    pub path: Option<String>,
    pub version: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct FfmpegStatus {
    pub found: bool,
    pub path: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct JsRuntimeStatus {
    pub found: bool,
    pub name: Option<String>,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct ToolsStatus {
    pub ytdlp: YtDlpStatus,
    pub ffmpeg: FfmpegStatus,
    pub js_runtime: JsRuntimeStatus,
    pub can_merge: bool,
    pub notes: Vec<String>,
}

impl ToolsStatus {
    pub fn detect(bin_dir: &Path, dirs: &[PathBuf]) -> ToolsStatus {
        let found = find_ytdlp(bin_dir, dirs);
        let ytdlp = if let Some(bin) = found {
            match first_line_of(&bin, "--version") {
                Some(v) => YtDlpStatus { installed: true, path: Some(bin.display().to_string()), version: Some(v), error: None },
                None => YtDlpStatus {
                    installed: true,
                    path: Some(bin.display().to_string()),
                    version: None,
                    error: Some("installed but did not report a version".into()),
                },
            }
        } else {
            YtDlpStatus::default()
        };
        let ffmpeg = match find_in(dirs, "ffmpeg") {
            Some(p) => FfmpegStatus { found: true, version: first_line_of(&p, "-version"), path: Some(p.display().to_string()) },
            None => FfmpegStatus::default(),
        };
        let js_runtime = ["deno", "node", "bun", "qjs"]
            .iter()
            .find_map(|name| find_in(dirs, name).map(|p| (name.to_string(), p)))
            .map(|(name, p)| JsRuntimeStatus { found: true, name: Some(name), path: Some(p.display().to_string()) })
            .unwrap_or_default();
        let mut notes = Vec::new();
        if !ytdlp.installed {
            notes.push("yt-dlp is missing: Muninn is downloading it now (this app build ships without it).".into());
        }
        if !ffmpeg.found {
            notes.push("ffmpeg is missing from this build. Without it Muninn can only cache progressive formats (format 18 / m4a 140) and cannot stream-play merged video.".into());
        }
        if !js_runtime.found {
            notes.push("No JavaScript runtime (deno) is available; current yt-dlp needs one for most YouTube videos.".into());
        }
        ToolsStatus { can_merge: ffmpeg.found, ytdlp, ffmpeg, js_runtime, notes }
    }

    pub fn ffmpeg_dir(&self) -> Option<PathBuf> {
        self.ffmpeg.path.as_ref().and_then(|p| Path::new(p).parent().map(Path::to_path_buf))
    }
}

// --- format selection --------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct FormatPlan {
    /// The variant actually cached; may be lower than requested without ffmpeg.
    pub variant: Quality,
    pub selector: String,
    pub merge: bool,
    pub downgraded: bool,
}

/// Picks yt-dlp format selectors. Without ffmpeg only unmerged formats are used:
/// progressive mp4 format 18 for 360p and m4a format 140 for audio.
pub fn plan_format(requested: Quality, has_ffmpeg: bool) -> Option<FormatPlan> {
    let (variant, downgraded) = match requested {
        Quality::None => return None,
        Quality::P480 | Quality::P720 | Quality::P1080 if !has_ffmpeg => (Quality::P360, true),
        q => (q, false),
    };
    let (selector, merge) = match variant {
        Quality::Audio => ("140/bestaudio[ext=m4a]/bestaudio".to_string(), false),
        Quality::P360 if !has_ffmpeg => ("18/b[height<=360][ext=mp4]".to_string(), false),
        Quality::P360 => ("18/b[height<=360][ext=mp4]/bv*[height<=360][vcodec^=avc1]+ba[ext=m4a]".to_string(), true),
        q => {
            let h = q.height();
            (format!("bv*[height<={h}][vcodec^=avc1]+ba[ext=m4a]/b[height<={h}][ext=mp4]/18"), true)
        }
    };
    Some(FormatPlan { variant, selector, merge, downgraded })
}

// --- command line and progress protocol -----------------------------------------------------------

#[derive(Debug, Clone)]
pub struct JobSpec {
    pub entry_id: String,
    pub video_id: String,
    pub selector: String,
    pub merge: bool,
    pub out_dir: PathBuf,
    pub limit_rate_kbps: Option<u32>,
    pub ffmpeg_dir: Option<PathBuf>,
    pub js_runtime: Option<(String, String)>,
}

pub fn build_args(spec: &JobSpec) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "--ignore-config".into(),
        "--no-playlist".into(),
        "--no-part".into(),
        "--no-mtime".into(),
        "--no-warnings".into(),
        "--newline".into(),
        "--progress".into(),
        "--no-simulate".into(),
        "-f".into(),
        spec.selector.clone(),
        "-o".into(),
        spec.out_dir.join(format!("{}.%(ext)s", spec.entry_id)).display().to_string(),
        "--progress-template".into(),
        "download:MUNINN_PROGRESS\t%(progress.downloaded_bytes)s\t%(progress.total_bytes)s\t%(progress.total_bytes_estimate)s\t%(progress.speed)s".into(),
        "--print".into(),
        "before_dl:MUNINN_INFO\t%(format_id)s\t%(height)s\t%(ext)s\t%(filesize)s".into(),
        "--print".into(),
        "after_move:MUNINN_DONE\t%(height)s\t%(ext)s\t%(filepath)s".into(),
    ];
    if spec.merge {
        a.push("--merge-output-format".into());
        a.push("mp4".into());
    }
    if let Some(kbps) = spec.limit_rate_kbps.filter(|k| *k > 0) {
        a.push("--limit-rate".into());
        a.push(format!("{kbps}K"));
    }
    if let Some(dir) = &spec.ffmpeg_dir {
        a.push("--ffmpeg-location".into());
        a.push(dir.display().to_string());
    }
    if let Some((name, path)) = &spec.js_runtime {
        a.push("--js-runtimes".into());
        a.push(format!("{name}:{path}"));
    }
    a.push("--".into());
    a.push(format!("https://www.youtube.com/watch?v={}", spec.video_id));
    a
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Info { format_id: String, height: Option<u32>, ext: String, expected_bytes: Option<u64> },
    Progress { downloaded: u64, total: Option<u64>, estimate: Option<u64>, speed: Option<u64> },
    Done { height: Option<u32>, ext: String, path: String },
}

fn num(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() || s == "NA" || s == "None" {
        return None;
    }
    s.parse::<u64>().ok().or_else(|| s.parse::<f64>().ok().filter(|f| *f >= 0.0).map(|f| f as u64))
}

pub fn parse_line(line: &str) -> Option<Event> {
    let mut parts = line.trim_end().split('\t');
    match parts.next()? {
        "MUNINN_INFO" => {
            let format_id = parts.next()?.to_string();
            let height = num(parts.next()?).map(|h| h as u32);
            let ext = parts.next()?.to_string();
            let expected_bytes = parts.next().and_then(num);
            Some(Event::Info { format_id, height, ext, expected_bytes })
        }
        "MUNINN_PROGRESS" => {
            let downloaded = num(parts.next()?)?;
            let total = parts.next().and_then(num);
            let estimate = parts.next().and_then(num);
            let speed = parts.next().and_then(num);
            Some(Event::Progress { downloaded, total, estimate, speed })
        }
        "MUNINN_DONE" => {
            let height = num(parts.next()?).map(|h| h as u32);
            let ext = parts.next()?.to_string();
            let path = parts.next()?.to_string();
            Some(Event::Done { height, ext, path })
        }
        _ => None,
    }
}

// --- running -----------------------------------------------------------------------------------------

pub trait Runner: Send + Sync {
    /// Runs one download. Calls `events` as the job reports progress; returns
    /// `Err(message)` when it fails or is canceled.
    fn run(&self, spec: &JobSpec, events: &mut dyn FnMut(Event), cancel: &AtomicBool) -> std::result::Result<(), String>;
}

pub struct YtDlpRunner {
    pub bin: PathBuf,
}

impl Runner for YtDlpRunner {
    fn run(&self, spec: &JobSpec, events: &mut dyn FnMut(Event), cancel: &AtomicBool) -> std::result::Result<(), String> {
        let mut child = Command::new(&self.bin)
            .args(build_args(spec))
            .env("PATH", search_path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("could not start yt-dlp at {}: {e}", self.bin.display()))?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stderr = child.stderr.take().ok_or("no stderr")?;
        let (tx, rx) = mpsc::channel::<String>();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(|l| l.ok()) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let err_reader = std::thread::spawn(move || {
            let mut tail: Vec<String> = Vec::new();
            for line in BufReader::new(stderr).lines().map_while(|l| l.ok()) {
                tail.push(line);
                if tail.len() > 12 {
                    tail.remove(0);
                }
            }
            tail
        });
        let mut canceled = false;
        loop {
            if cancel.load(Ordering::SeqCst) {
                canceled = true;
                let _ = child.kill();
                break;
            }
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(line) => {
                    if let Some(ev) = parse_line(&line) {
                        events(ev);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        let status = child.wait().map_err(|e| e.to_string())?;
        let _ = reader.join();
        let tail = err_reader.join().unwrap_or_default();
        // Drain anything still queued (events printed just before exit).
        while let Ok(line) = rx.try_recv() {
            if let Some(ev) = parse_line(&line) {
                events(ev);
            }
        }
        if canceled {
            return Err("canceled".into());
        }
        if status.success() {
            Ok(())
        } else {
            let message = tail.iter().rev().find(|l| l.contains("ERROR")).or(tail.last()).cloned().unwrap_or_else(|| format!("yt-dlp exited with {status}"));
            Err(message)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct FakeFetcher(Mutex<HashMap<String, Vec<u8>>>);

    impl FakeFetcher {
        fn new(files: &[(&str, &[u8])]) -> Self {
            FakeFetcher(Mutex::new(files.iter().map(|(k, v)| (k.to_string(), v.to_vec())).collect()))
        }
    }

    impl Fetcher for FakeFetcher {
        fn get(&self, url: &str, _max: u64) -> Result<Vec<u8>> {
            self.0.lock().unwrap().get(url).cloned().ok_or_else(|| Error::other(format!("404 {url}")))
        }
    }

    #[test]
    fn parses_sha_sums() {
        let text = format!("{}  yt-dlp\n{}  yt-dlp_macos\n{} *yt-dlp.exe\n", "a".repeat(64), "B".repeat(64), "c".repeat(64));
        assert_eq!(parse_sha_sums(&text, "yt-dlp_macos"), Some("b".repeat(64)));
        assert_eq!(parse_sha_sums(&text, "yt-dlp.exe"), Some("c".repeat(64)));
        assert_eq!(parse_sha_sums(&text, "missing"), None);
        assert_eq!(parse_sha_sums("short  yt-dlp_macos", "yt-dlp_macos"), None);
    }

    #[test]
    fn installs_after_verifying_the_checksum() {
        let dir = tempfile::tempdir().unwrap();
        let binary = b"#!/bin/sh\necho 2099.01.01\n";
        let digest = hex(&Sha256::digest(binary));
        let base = "https://example.test/dl";
        let fetcher = FakeFetcher::new(&[
            (&format!("{base}/SHA2-256SUMS"), format!("{digest}  yt-dlp_macos\n").as_bytes()),
            (&format!("{base}/yt-dlp_macos"), binary),
        ]);
        let log = install_ytdlp(&fetcher, dir.path(), "yt-dlp_macos", base).unwrap();
        assert!(log.contains("sha256 verified"));
        assert_eq!(std::fs::read(ytdlp_path(dir.path())).unwrap(), binary);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(ytdlp_path(dir.path())).unwrap().permissions().mode() & 0o111, 0o111);
            // And the installed tool is detected and reports its version.
            let status = ToolsStatus::detect(dir.path(), &[]);
            assert!(status.ytdlp.installed);
            assert_eq!(status.ytdlp.version.as_deref(), Some("2099.01.01"));
        }
    }

    #[test]
    fn refuses_a_binary_whose_checksum_does_not_match() {
        let dir = tempfile::tempdir().unwrap();
        let base = "https://example.test/dl";
        let fetcher = FakeFetcher::new(&[
            (&format!("{base}/SHA2-256SUMS"), format!("{}  yt-dlp_macos\n", "0".repeat(64)).as_bytes()),
            (&format!("{base}/yt-dlp_macos"), b"tampered"),
        ]);
        let err = install_ytdlp(&fetcher, dir.path(), "yt-dlp_macos", base).unwrap_err();
        assert!(err.to_string().contains("checksum mismatch"), "{err}");
        assert!(!ytdlp_path(dir.path()).exists(), "nothing may be installed after a mismatch");
        assert!(!dir.path().join("yt-dlp.download").exists());
    }

    #[test]
    fn refuses_when_the_sums_file_lacks_the_asset() {
        let dir = tempfile::tempdir().unwrap();
        let base = "https://example.test/dl";
        let fetcher = FakeFetcher::new(&[(&format!("{base}/SHA2-256SUMS"), b"x"), (&format!("{base}/yt-dlp_macos"), b"y")]);
        assert!(install_ytdlp(&fetcher, dir.path(), "yt-dlp_macos", base).is_err());
    }

    #[test]
    fn selectors_without_ffmpeg_need_no_merging() {
        let p = plan_format(Quality::P360, false).unwrap();
        assert!(p.selector.starts_with("18/") && !p.merge && !p.downgraded);
        let p = plan_format(Quality::Audio, false).unwrap();
        assert!(p.selector.starts_with("140/") && !p.merge);
        // 480/720 cannot be produced without ffmpeg: honest downgrade to 360p.
        for q in [Quality::P480, Quality::P720] {
            let p = plan_format(q, false).unwrap();
            assert_eq!(p.variant, Quality::P360);
            assert!(p.downgraded && !p.merge);
        }
        assert!(plan_format(Quality::None, true).is_none());
    }

    #[test]
    fn selectors_with_ffmpeg_merge_higher_qualities() {
        let p = plan_format(Quality::P720, true).unwrap();
        assert_eq!(p.variant, Quality::P720);
        assert!(p.merge && !p.downgraded);
        assert!(p.selector.contains("height<=720") && p.selector.contains("avc1"));
        let p = plan_format(Quality::P480, true).unwrap();
        assert!(p.selector.contains("height<=480"));
        // 360p still prefers the progressive format 18 first.
        assert!(plan_format(Quality::P360, true).unwrap().selector.starts_with("18/"));
    }

    #[test]
    fn builds_the_command_line() {
        let spec = JobSpec {
            entry_id: "dQw4w9WgXcQ.360".into(),
            video_id: "dQw4w9WgXcQ".into(),
            selector: "18/b".into(),
            merge: false,
            out_dir: PathBuf::from("/data/cache/video"),
            limit_rate_kbps: Some(500),
            ffmpeg_dir: Some(PathBuf::from("/opt/homebrew/bin")),
            js_runtime: Some(("deno".into(), "/opt/homebrew/bin/deno".into())),
        };
        let a = build_args(&spec);
        let joined = a.join(" ");
        assert!(joined.contains("--no-part") && joined.contains("--no-playlist") && joined.contains("--ignore-config"));
        assert!(joined.contains("-o /data/cache/video/dQw4w9WgXcQ.360.%(ext)s"));
        assert!(joined.contains("--limit-rate 500K"));
        assert!(joined.contains("--ffmpeg-location /opt/homebrew/bin"));
        assert!(joined.contains("--js-runtimes deno:/opt/homebrew/bin/deno"));
        assert!(!joined.contains("--merge-output-format"));
        assert_eq!(a[a.len() - 2], "--");
        assert_eq!(a.last().unwrap(), "https://www.youtube.com/watch?v=dQw4w9WgXcQ");
    }

    #[test]
    fn parses_progress_protocol() {
        assert_eq!(
            parse_line("MUNINN_INFO\t18\t360\tmp4\t1234567"),
            Some(Event::Info { format_id: "18".into(), height: Some(360), ext: "mp4".into(), expected_bytes: Some(1234567) })
        );
        assert_eq!(
            parse_line("MUNINN_INFO\t140\tNA\tm4a\tNA"),
            Some(Event::Info { format_id: "140".into(), height: None, ext: "m4a".into(), expected_bytes: None })
        );
        assert_eq!(
            parse_line("MUNINN_PROGRESS\t1000\tNA\t5000\t250000.5"),
            Some(Event::Progress { downloaded: 1000, total: None, estimate: Some(5000), speed: Some(250000) })
        );
        assert_eq!(
            parse_line("MUNINN_DONE\t360\tmp4\t/data/cache/video/x.360.mp4"),
            Some(Event::Done { height: Some(360), ext: "mp4".into(), path: "/data/cache/video/x.360.mp4".into() })
        );
        assert_eq!(parse_line("[youtube] Extracting URL"), None);
        assert_eq!(parse_line(""), None);
    }

    #[test]
    fn discovers_tools_in_given_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for name in ["ffmpeg", "deno"] {
                let p = bin.join(name);
                std::fs::write(&p, "#!/bin/sh\necho tool 1.0\n").unwrap();
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            let s = ToolsStatus::detect(dir.path(), &[bin.clone()]);
            assert!(s.ffmpeg.found && s.can_merge);
            assert_eq!(s.ffmpeg_dir(), Some(bin.clone()));
            assert_eq!(s.js_runtime.name.as_deref(), Some("deno"));
            assert!(s.notes.iter().any(|n| n.contains("yt-dlp is missing")));
        }
        #[cfg(unix)]
        {
            // A bundled yt-dlp is found; the self-updating copy in the data dir wins over it.
            use std::os::unix::fs::PermissionsExt;
            let bundled = bin.join("yt-dlp");
            std::fs::write(&bundled, "#!/bin/sh\necho bundled\n").unwrap();
            std::fs::set_permissions(&bundled, std::fs::Permissions::from_mode(0o755)).unwrap();
            let s = ToolsStatus::detect(dir.path(), &[bin.clone()]);
            assert_eq!(s.ytdlp.version.as_deref(), Some("bundled"));
            let own = ytdlp_path(dir.path());
            std::fs::write(&own, "#!/bin/sh\necho updated\n").unwrap();
            std::fs::set_permissions(&own, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert_eq!(ToolsStatus::detect(dir.path(), &[bin.clone()]).ytdlp.version.as_deref(), Some("updated"));
            std::fs::remove_file(&own).unwrap();
        }
        let none = ToolsStatus::detect(dir.path(), &[]);
        assert!(!none.ffmpeg.found && !none.can_merge);
        assert!(none.notes.iter().any(|n| n.contains("ffmpeg is missing")));
    }
}
