//! Optional single-user PWA server. The desktop router and localhost trust model
//! stay separate: this adapter exports only browser operations, authenticates
//! with revocable HttpOnly sessions, and serves media from this server's disk.

use super::*;
use axum::extract::Extension;
use std::fs::{File, OpenOptions};
use std::path::{Path as FsPath, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const COOKIE: &str = "muninn_session";
const SESSION_SECONDS: u64 = 7 * 24 * 60 * 60;
const MAX_SESSIONS: usize = 64;

/// Explicit network boundary. HTTPS is terminated by a trusted reverse proxy;
/// it must preserve Host and must not publish the private loopback listener.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub bind: SocketAddr,
    pub public_origin: Option<String>,
}
impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:47872".parse().unwrap(),
            public_origin: None,
        }
    }
}

#[derive(Debug, Clone)]
struct NetworkPolicy {
    origin: String,
    authority: String,
    secure: bool,
}
impl NetworkPolicy {
    fn new(config: &ServerConfig, bound: SocketAddr) -> muninn_core::Result<Self> {
        let raw = match &config.public_origin {
            Some(origin) => origin.clone(),
            None if bound.ip().is_loopback() => format!("http://{bound}"),
            None => {
                return Err(Error::bad(
                    "a non-loopback --bind requires --public-origin https://HOST",
                ))
            }
        };
        let parsed = url::Url::parse(&raw)
            .map_err(|_| Error::bad("public origin must be an absolute HTTP(S) origin"))?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || parsed.path() != "/"
        {
            return Err(Error::bad(
                "public origin must contain only scheme, host and optional port",
            ));
        }
        let is_local_host = parsed.host_str().is_some_and(|host| {
            host == "localhost"
                || host
                    .trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
        if parsed.scheme() == "http" && (!bound.ip().is_loopback() || !is_local_host) {
            return Err(Error::bad(
                "HTTPS public origin is required for remote browser access",
            ));
        }
        let origin = parsed.origin().ascii_serialization();
        let uri: axum::http::Uri = origin
            .parse()
            .map_err(|_| Error::bad("invalid origin authority"))?;
        Ok(Self {
            authority: uri.authority().unwrap().as_str().to_ascii_lowercase(),
            secure: parsed.scheme() == "https",
            origin,
        })
    }
}

struct BrowserState {
    policy: NetworkPolicy,
    sessions: Mutex<HashMap<String, u64>>,
}
fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn cookie_from(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        (name == COOKIE && value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| value.to_string())
    })
}
impl BrowserState {
    fn authorized(&self, headers: &HeaderMap) -> bool {
        let Some(token) = cookie_from(headers) else {
            return false;
        };
        let now = now_seconds();
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.retain(|_, expiry| *expiry > now);
        sessions.contains_key(&token)
    }
    fn cookie(&self, value: &str, age: u64) -> String {
        format!(
            "{COOKIE}={value}; Path=/; HttpOnly; SameSite=Strict; Max-Age={age}{}",
            if self.policy.secure { "; Secure" } else { "" }
        )
    }
}

/// Host and Origin are checked against configured values, never forwarded headers.
async fn browser_boundary(
    State(state): State<Arc<BrowserState>>,
    req: Request,
    next: Next,
) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok());
    if !host.is_some_and(|h| h.eq_ignore_ascii_case(&state.policy.authority)) {
        return json_error(StatusCode::FORBIDDEN, "host not allowed");
    }
    let origin = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|h| h.to_str().ok());
    let mutation = !matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    if origin.is_some_and(|o| o != state.policy.origin)
        || (mutation && origin != Some(state.policy.origin.as_str()))
    {
        return json_error(StatusCode::FORBIDDEN, "same-origin request required");
    }
    if req
        .headers()
        .get("sec-fetch-site")
        .and_then(|h| h.to_str().ok())
        == Some("cross-site")
    {
        return json_error(StatusCode::FORBIDDEN, "cross-site request refused");
    }
    let protected = !req.uri().path().starts_with("/app/") || req.uri().path() == "/app/session";
    let mut response = next.run(req).await;
    response
        .headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response
        .headers_mut()
        .insert("x-frame-options", HeaderValue::from_static("DENY"));
    if protected {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}
async fn session_auth(
    State(state): State<Arc<BrowserState>>,
    req: Request,
    next: Next,
) -> Response {
    if state.authorized(req.headers()) {
        next.run(req).await
    } else {
        json_error(
            StatusCode::UNAUTHORIZED,
            "pair this browser with the server",
        )
    }
}

/// Native failures can contain tool stderr, local paths or upstream credentials.
/// Scrub JSON diagnostics at the browser boundary, including errors stored on
/// entries/jobs. Media bodies keep their streaming/range behavior unchanged.
fn scrub_browser_json(value: &mut Value, secrets: &[String]) {
    match value {
        Value::Object(map) => {
            for (key, item) in map {
                if key == "error" || key.ends_with("_error") {
                    if let Some(message) = item.as_str() {
                        *item = Value::String(muninn_core::stream_diag::redact_error(
                            message, "", secrets, "", 2000,
                        ));
                        continue;
                    }
                }
                scrub_browser_json(item, secrets);
            }
        }
        Value::Array(items) => {
            for item in items {
                scrub_browser_json(item, secrets);
            }
        }
        Value::String(text) => {
            // The process master token must never leave the private listener,
            // even if a future response adds another field carrying a path.
            *text = text.replace(&secrets[0], "[redacted]");
        }
        _ => {}
    }
}
async fn browser_json(State(core): S, req: Request, next: Next) -> Response {
    let head = req.method() == Method::HEAD;
    let response = next.run(req).await;
    if head
        || !response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .is_some_and(|mime| mime.starts_with("application/json"))
    {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, 64 * 1024 * 1024).await else {
        return json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "browser response could not be read",
        );
    };
    let Ok(mut value) = serde_json::from_slice::<Value>(&bytes) else {
        return json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "browser response was invalid",
        );
    };
    scrub_browser_json(
        &mut value,
        &[core.token(), core.paths.data_dir.display().to_string()],
    );
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(parts, Body::from(value.to_string()))
}
async fn login(
    State(core): S,
    Extension(state): Extension<Arc<BrowserState>>,
    headers: HeaderMap,
) -> Response {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if !token.is_some_and(|v| core.check_token(v)) {
        return json_error(StatusCode::UNAUTHORIZED, "invalid pairing token");
    }
    let now = now_seconds();
    let mut sessions = state.sessions.lock().unwrap_or_else(|e| e.into_inner());
    sessions.retain(|_, expiry| *expiry > now);
    if let Some(old) = cookie_from(&headers) {
        sessions.remove(&old);
    }
    if sessions.len() >= MAX_SESSIONS {
        return json_error(
            StatusCode::TOO_MANY_REQUESTS,
            "browser session limit reached; disconnect another browser or restart the server",
        );
    }
    let session = muninn_core::util::random_token();
    sessions.insert(session.clone(), now + SESSION_SECONDS);
    let mut response =
        Json(json!({"authenticated":true,"version":muninn_core::engine::VERSION,"mode":"server"}))
            .into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        state.cookie(&session, SESSION_SECONDS).parse().unwrap(),
    );
    response
}
async fn session(Extension(state): Extension<Arc<BrowserState>>, headers: HeaderMap) -> Response {
    if !state.authorized(&headers) {
        return json_error(
            StatusCode::UNAUTHORIZED,
            "pair this browser with the server",
        );
    }
    Json(json!({"authenticated":true,"version":muninn_core::engine::VERSION,"mode":"server"}))
        .into_response()
}
async fn logout(Extension(state): Extension<Arc<BrowserState>>, headers: HeaderMap) -> Response {
    if let Some(token) = cookie_from(&headers) {
        state
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&token);
    }
    let mut response = Json(json!({"authenticated":false})).into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, state.cookie("", 0).parse().unwrap());
    response
}

fn pwa_response(name: &str) -> Response {
    match ui_assets::pwa_asset(name) {
        Some((mime, bytes)) => Response::builder().header(header::CONTENT_TYPE, mime)
            .header(header::CACHE_CONTROL, "no-cache")
            .header("content-security-policy", "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; media-src 'self' blob:; connect-src 'self'; worker-src 'self'; manifest-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'")
            .body(Body::from(bytes)).unwrap(),
        None => json_error(StatusCode::NOT_FOUND, "no such app asset"),
    }
}
async fn pwa_index() -> Response {
    pwa_response("")
}
async fn pwa_file(Path(name): Path<String>) -> Response {
    pwa_response(&name)
}

/// The native API includes master-token HLS paths for its trusted webview. Keep
/// that contract unchanged and rewrite only the PWA boundary's response copy.
fn browser_playback(mut value: Value, core: &Core) -> muninn_core::Result<Value> {
    fn visit(value: &mut Value, master: &str) -> muninn_core::Result<()> {
        match value {
            Value::Object(map) => {
                for (key, item) in map {
                    if key == "url_path" {
                        if let Some(path) = item.as_str() {
                            if let Some(rest) = path.strip_prefix(&format!("/h/{master}/")) {
                                *item = Value::String(format!("/stream/{rest}"));
                            } else if !path.starts_with("/media/") {
                                return Err(Error::bad("unexpected playback source"));
                            }
                        }
                    } else {
                        visit(item, master)?;
                    }
                }
            }
            Value::Array(items) => {
                for item in items {
                    visit(item, master)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    visit(&mut value, &core.token())?;
    Ok(value)
}
async fn browser_watch_open(
    State(core): S,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult {
    let backend = EmbeddedBackend::new(core.clone());
    let request = WatchRequest {
        video_id: id,
        audio_only: q.get("audio").is_some_and(|v| v == "1"),
    };
    let value = tokio::task::spawn_blocking(move || backend.open_watch(&request))
        .await
        .map_err(|e| Error::other(e.to_string()))??;
    Ok(Json(browser_playback(value, &core)?))
}
async fn browser_watch_status(State(core): S, Path(id): Path<String>) -> ApiResult {
    Ok(Json(browser_playback(
        EmbeddedBackend::new(core.clone()).watch_status(&id)?,
        &core,
    )?))
}
async fn browser_local_playback(
    State(core): S,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult {
    let Json(value) = local_playback(State(core.clone()), Path(id), Query(q)).await?;
    Ok(Json(browser_playback(value, &core)?))
}
async fn browser_local_state(State(core): S, Path(id): Path<String>) -> ApiResult {
    let Json(value) = local_playback_state(State(core.clone()), Path(id)).await?;
    Ok(Json(browser_playback(value, &core)?))
}
async fn browser_hls(
    State(core): S,
    Path((dir, file)): Path<(String, String)>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    let token = core.token();
    hls_file(State(core), Path((token, dir, file)), method, headers).await
}

/// Feed provenance belongs next to the feed title. Resolve a recorded parent
/// from local metadata only, including expired metadata retained on disk; never
/// guess a parent from its recommendations or fetch anything to label a card.
fn enrich_feed_source(value: &mut Value, core: &Core) {
    let Some(id) = value["source_url"]
        .as_str()
        .and_then(muninn_core::extract::video_id_from_url)
    else {
        return;
    };
    let mut title = None;
    for kind in [
        muninn_core::content::KIND_META,
        muninn_core::content::KIND_ITEM,
    ] {
        title = core
            .content
            .get_json(kind, &id, 0)
            .ok()
            .flatten()
            .and_then(|item| {
                item["title"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            });
        if title.is_some() {
            break;
        }
    }
    if title.is_none() {
        title = core
            .video
            .entries_for_video(&id)
            .unwrap_or_default()
            .into_iter()
            .find_map(|entry| entry.title.filter(|s| !s.trim().is_empty()));
    }
    value["source_video_id"] = json!(id);
    if let Some(title) = title {
        value["source_title"] = json!(title);
    }
}
async fn browser_feeds(State(core): S, Query(q): Query<HashMap<String, String>>) -> ApiResult {
    let Json(mut value) = feeds(State(core.clone()), Query(q)).await?;
    if let Some(rows) = value.as_array_mut() {
        for row in rows {
            enrich_feed_source(row, &core);
        }
    }
    Ok(Json(value))
}
async fn browser_feed(State(core): S, Path(id): Path<i64>) -> ApiResult {
    let Json(mut value) = feed(State(core.clone()), Path(id)).await?;
    enrich_feed_source(&mut value, &core);
    Ok(Json(value))
}
async fn browser_feed_latest(
    State(core): S,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult {
    let Json(mut value) = feed_latest(State(core.clone()), Query(q)).await?;
    enrich_feed_source(&mut value, &core);
    Ok(Json(value))
}

fn browser_router(core: Arc<Core>, state: Arc<BrowserState>) -> Router {
    let protected = Router::new()
        .route("/api/library", get(library))
        .route("/api/entries", get(entries))
        .route("/api/entries/{id}", get(entry))
        .route("/api/feeds", get(browser_feeds))
        .route("/api/feeds/latest", get(browser_feed_latest))
        .route("/api/feeds/{id}", get(browser_feed))
        .route("/api/video/{id}", get(video))
        .route("/api/jobs", get(jobs))
        .route("/api/search", post(online_search))
        .route("/api/cache", post(cache_request))
        .route(
            "/api/watch/{id}",
            get(browser_watch_status).post(browser_watch_open),
        )
        .route("/api/entries/{id}/promote", post(promote))
        .route(
            "/api/entries/{id}/playback",
            get(browser_local_state).post(browser_local_playback),
        )
        .route("/api/entries/{id}/playback/stop", post(stop_local_playback))
        .route("/api/ingest/playback", post(ingest_playback))
        .route("/media/{id}", get(media))
        .route("/thumb/{id}", get(thumb))
        .route("/stream/{dir}/{file}", get(browser_hls))
        .layer(middleware::from_fn_with_state(core.clone(), browser_json))
        .layer(middleware::from_fn_with_state(state.clone(), session_auth));
    Router::new()
        .route("/", get(|| async { Redirect::temporary("/app/") }))
        .route("/health", get(health))
        .route("/app", get(|| async { Redirect::permanent("/app/") }))
        .route("/app/", get(pwa_index))
        .route("/app/session", get(session).post(login).delete(logout))
        .route("/app/{*file}", get(pwa_file))
        .merge(protected)
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(Extension(state.clone()))
        .layer(middleware::from_fn_with_state(state, browser_boundary))
        .with_state(core)
}

/// OS-owned lock, automatically released on clean exit or a crash. Used by
/// BOTH entry points so a server cannot overwrite a running desktop's token.
pub(crate) fn lock_data_dir(dir: &FsPath) -> muninn_core::Result<File> {
    std::fs::create_dir_all(dir)?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join("service.lock"))?;
    file.try_lock().map_err(|e| {
        Error::other(format!(
            "data directory already in use or cannot be locked: {} ({e})",
            dir.display()
        ))
    })?;
    Ok(file)
}

pub struct ServerService {
    pub core: Arc<Core>,
    pub addr: SocketAddr,
    pub public_origin: String,
    _runtime: tokio::runtime::Runtime,
    _data_lock: File,
}
impl ServerService {
    pub fn start(mut config: CoreConfig, network: ServerConfig) -> muninn_core::Result<Self> {
        config.spawn_workers = true;
        Self::start_configured(config, network)
    }
    fn start_configured(config: CoreConfig, network: ServerConfig) -> muninn_core::Result<Self> {
        // Validate before opening stores/rotating the process token.
        NetworkPolicy::new(&network, network.bind)?;
        let remote = TcpListener::bind(network.bind)?;
        let addr = remote.local_addr()?;
        let policy = NetworkPolicy::new(&network, addr)?;
        let internal = TcpListener::bind(("127.0.0.1", 0))?;
        remote.set_nonblocking(true)?;
        internal.set_nonblocking(true)?;
        let data_lock = lock_data_dir(&config.data_dir)?;
        let core = Core::open(config)?;
        // Acquisition's ffmpeg helper needs /s locally. This listener is NEVER
        // the public address and keeps the existing native token/Host boundary.
        core.set_port(internal.local_addr()?.port());
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .thread_name("muninn-server")
            .build()?;
        let state = Arc::new(BrowserState {
            policy: policy.clone(),
            sessions: Mutex::new(HashMap::new()),
        });
        let remote_app = browser_router(core.clone(), state);
        let private_app = router(core.clone());
        runtime.spawn(async move {
            if let Err(e) = axum::serve(
                tokio::net::TcpListener::from_std(remote).expect("listener"),
                remote_app,
            )
            .await
            {
                eprintln!("muninn-server: browser server stopped: {e}");
            }
        });
        runtime.spawn(async move {
            if let Err(e) = axum::serve(
                tokio::net::TcpListener::from_std(internal).expect("listener"),
                private_app,
            )
            .await
            {
                eprintln!("muninn-server: internal server stopped: {e}");
            }
        });
        Ok(Self {
            core,
            addr,
            public_origin: policy.origin,
            _runtime: runtime,
            _data_lock: data_lock,
        })
    }
    pub fn wait_for_shutdown(self) -> muninn_core::Result<()> {
        self._runtime.block_on(async {
            #[cfg(unix)]
            {
                let mut terminate =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
                tokio::select! { result=tokio::signal::ctrl_c()=>result?, _=terminate.recv()=>{} }
            }
            #[cfg(not(unix))]
            tokio::signal::ctrl_c().await?;
            Ok::<(), std::io::Error>(())
        })?;
        Ok(())
    }
}
impl Drop for ServerService {
    fn drop(&mut self) {
        self.core.shutdown();
    }
}

pub fn default_server_data_dir() -> PathBuf {
    if let Some(base) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(base).join("muninn-server");
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local/share/muninn-server")
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;
    use muninn_core::videodb::{EntryRow, STATUS_COMPLETE};
    use tower::ServiceExt;

    struct Harness {
        _dir: tempfile::TempDir,
        core: Arc<Core>,
        app: Router,
        state: Arc<BrowserState>,
    }
    fn harness() -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let core = Core::open(CoreConfig {
            data_dir: dir.path().into(),
            tool_dirs: Some(vec![]),
            spawn_workers: false,
            ..Default::default()
        })
        .unwrap();
        let policy = NetworkPolicy::new(
            &ServerConfig {
                bind: "127.0.0.1:47872".parse().unwrap(),
                public_origin: Some("https://muninn.example.com".into()),
            },
            "127.0.0.1:47872".parse().unwrap(),
        )
        .unwrap();
        let state = Arc::new(BrowserState {
            policy,
            sessions: Mutex::new(HashMap::new()),
        });
        Harness {
            _dir: dir,
            app: browser_router(core.clone(), state.clone()),
            core,
            state,
        }
    }
    fn request(method: &str, path: &str, cookie: Option<&str>, body: &str) -> Request {
        let mut builder = axum::http::Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, "muninn.example.com");
        if method != "GET" && method != "HEAD" {
            builder = builder.header(header::ORIGIN, "https://muninn.example.com");
        }
        if let Some(cookie) = cookie {
            builder = builder.header(header::COOKIE, cookie);
        }
        builder.body(Body::from(body.to_string())).unwrap()
    }
    async fn send(h: &Harness, req: Request) -> (StatusCode, HeaderMap, Vec<u8>) {
        let response = h.app.clone().oneshot(req).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        (
            status,
            headers,
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
        )
    }
    async fn pair(h: &Harness) -> String {
        let mut req = request("POST", "/app/session", None, "");
        req.headers_mut().insert(
            header::AUTHORIZATION,
            format!("Bearer {}", h.core.token()).parse().unwrap(),
        );
        let (status, headers, body) = send(h, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(!String::from_utf8_lossy(&body).contains(&h.core.token()));
        let set = headers[header::SET_COOKIE].to_str().unwrap();
        for flag in ["HttpOnly", "SameSite=Strict", "Secure", "Max-Age=604800"] {
            assert!(set.contains(flag));
        }
        set.split(';').next().unwrap().to_string()
    }
    #[test]
    fn remote_network_configuration_fails_closed() {
        let remote: SocketAddr = "0.0.0.0:47872".parse().unwrap();
        assert!(NetworkPolicy::new(
            &ServerConfig {
                bind: remote,
                public_origin: None
            },
            remote
        )
        .is_err());
        for origin in [
            "http://muninn.example.com",
            "https://u:p@muninn.example.com",
            "https://muninn.example.com/path",
            "https://muninn.example.com/?token=x",
            "file:///tmp",
        ] {
            assert!(
                NetworkPolicy::new(
                    &ServerConfig {
                        bind: remote,
                        public_origin: Some(origin.into())
                    },
                    remote
                )
                .is_err(),
                "{origin}"
            );
        }
        assert!(
            NetworkPolicy::new(&ServerConfig::default(), "127.0.0.1:47872".parse().unwrap())
                .is_ok()
        );
        let good = NetworkPolicy::new(
            &ServerConfig {
                bind: remote,
                public_origin: Some("https://muninn.example.com:8443".into()),
            },
            remote,
        )
        .unwrap();
        assert_eq!(good.authority, "muninn.example.com:8443");
        assert!(good.secure);
    }
    #[tokio::test]
    async fn session_pairing_csrf_and_revocation() {
        let h = harness();
        assert_eq!(
            send(&h, request("GET", "/api/library", None, "")).await.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            send(&h, request("POST", "/app/session", None, "")).await.0,
            StatusCode::UNAUTHORIZED
        );
        let cookie = pair(&h).await;
        let (s, headers, _) = send(&h, request("GET", "/api/library", Some(&cookie), "")).await;
        assert_eq!(s, StatusCode::OK);
        assert!(!headers.contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN));
        assert_eq!(headers[header::CACHE_CONTROL], "no-store");
        let mut foreign = request(
            "POST",
            "/api/cache",
            Some(&cookie),
            r#"{"video_id":"aaaaaaaaaaa","quality":"360"}"#,
        );
        foreign
            .headers_mut()
            .insert(header::ORIGIN, "https://evil.example.com".parse().unwrap());
        assert_eq!(send(&h, foreign).await.0, StatusCode::FORBIDDEN);
        let mut missing = request("POST", "/api/cache", Some(&cookie), "{}");
        missing.headers_mut().remove(header::ORIGIN);
        assert_eq!(send(&h, missing).await.0, StatusCode::FORBIDDEN);
        for host in ["evil.example.com", "localhost", "muninn.example.com.evil"] {
            let mut req = request("GET", "/health", None, "");
            req.headers_mut()
                .insert(header::HOST, host.parse().unwrap());
            assert_eq!(send(&h, req).await.0, StatusCode::FORBIDDEN);
        }
        let mut no_host = request("GET", "/health", None, "");
        no_host.headers_mut().remove(header::HOST);
        assert_eq!(send(&h, no_host).await.0, StatusCode::FORBIDDEN);
        assert_eq!(
            send(&h, request("DELETE", "/app/session", Some(&cookie), ""))
                .await
                .0,
            StatusCode::OK
        );
        assert_eq!(
            send(&h, request("GET", "/app/session", Some(&cookie), ""))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
    }
    #[tokio::test]
    async fn native_routes_and_query_tokens_are_not_exported() {
        let h = harness();
        let cookie = pair(&h).await;
        let path = format!("/api/library?token={}", h.core.token());
        assert_eq!(
            send(&h, request("GET", &path, None, "")).await.0,
            StatusCode::UNAUTHORIZED
        );
        for (method, path) in [
            ("GET", "/api/settings"),
            ("GET", "/api/tools"),
            ("POST", "/api/import/media"),
            ("POST", "/api/feeds/refresh"),
            ("GET", "/api/proxy"),
            ("GET", "/ui/"),
            ("GET", "/s/token/vid/1"),
            ("GET", "/h/token/dir/index.m3u8"),
        ] {
            let status = send(&h, request(method, path, Some(&cookie), "")).await.0;
            assert!(
                status == StatusCode::NOT_FOUND || status == StatusCode::METHOD_NOT_ALLOWED,
                "{path}: {status}"
            );
        }
        let mut bearer = request("GET", "/api/library", None, "");
        bearer.headers_mut().insert(
            header::AUTHORIZATION,
            format!("Bearer {}", h.core.token()).parse().unwrap(),
        );
        assert_eq!(send(&h, bearer).await.0, StatusCode::UNAUTHORIZED);
    }
    #[tokio::test]
    async fn cached_media_ranges_and_hls_require_cookie_without_exposing_master_token() {
        let h = harness();
        let cookie = pair(&h).await;
        let path = h.core.paths.video_dir.join("aaaaaaaaaaa.360.mp4");
        std::fs::write(&path, b"0123456789").unwrap();
        let mut entry = EntryRow::new("aaaaaaaaaaa", "360", "360", "manual", 1);
        entry.status = STATUS_COMPLETE.into();
        entry.file_path = Some(path.display().to_string());
        entry.ext = Some("mp4".into());
        entry.size_bytes = 10;
        h.core.video.insert_entry(&entry).unwrap();
        let mut r = request("GET", "/media/aaaaaaaaaaa.360", Some(&cookie), "");
        r.headers_mut()
            .insert(header::RANGE, "bytes=2-4".parse().unwrap());
        let (s, headers, body) = send(&h, r).await;
        assert_eq!(s, StatusCode::PARTIAL_CONTENT);
        assert_eq!(body, b"234");
        assert_eq!(headers[header::CONTENT_RANGE], "bytes 2-4/10");
        assert_eq!(
            send(&h, request("HEAD", "/media/aaaaaaaaaaa.360", None, ""))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        let (_, _, body) = send(
            &h,
            request("HEAD", "/media/aaaaaaaaaaa.360", Some(&cookie), ""),
        )
        .await;
        assert!(body.is_empty());
        let dir = "aaaaaaaaaaa.360.local.1";
        let hls = h.core.paths.cache_dir.join("hls").join(dir);
        std::fs::create_dir_all(&hls).unwrap();
        std::fs::write(
            hls.join("index.m3u8"),
            "#EXTM3U\n#EXTINF:3,\nseg_00000.m4s\n#EXT-X-ENDLIST\n",
        )
        .unwrap();
        std::fs::write(hls.join("seg_00000.m4s"), b"segment").unwrap();
        let uri = format!("/stream/{dir}/index.m3u8");
        assert_eq!(
            send(&h, request("GET", &uri, None, "")).await.0,
            StatusCode::UNAUTHORIZED
        );
        let (s, _, body) = send(&h, request("GET", &uri, Some(&cookie), "")).await;
        assert_eq!(s, StatusCode::OK);
        assert!(!String::from_utf8_lossy(&body).contains(&h.core.token()));
        let value = browser_playback(
            json!({"playback":{"url_path":format!("/h/{}/{dir}/index.m3u8",h.core.token())}}),
            &h.core,
        )
        .unwrap();
        assert_eq!(value["playback"]["url_path"], uri);
        assert!(browser_playback(
            json!({"playback":{"url_path":"https://upstream.example/video"}}),
            &h.core
        )
        .is_err());
        assert_eq!(
            send(
                &h,
                request("GET", "/stream/..%2F..%2Fetc/passwd", Some(&cookie), "")
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }
    #[tokio::test]
    async fn sessions_expire_and_are_bounded() {
        let h = harness();
        let cookie = pair(&h).await;
        let value = cookie.split_once('=').unwrap().1.to_string();
        h.state.sessions.lock().unwrap().insert(value, 0);
        assert_eq!(
            send(&h, request("GET", "/app/session", Some(&cookie), ""))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        for _ in 0..MAX_SESSIONS {
            h.state
                .sessions
                .lock()
                .unwrap()
                .insert(muninn_core::util::random_token(), now_seconds() + 10);
        }
        let mut r = request("POST", "/app/session", None, "");
        r.headers_mut().insert(
            header::AUTHORIZATION,
            format!("Bearer {}", h.core.token()).parse().unwrap(),
        );
        assert_eq!(send(&h, r).await.0, StatusCode::TOO_MANY_REQUESTS);
    }
    #[tokio::test]
    async fn browser_diagnostics_remove_master_token_and_helper_paths() {
        let h = harness();
        let cookie = pair(&h).await;
        let diagnostic=format!("ffmpeg failed opening '/srv/private cache/video.mp4': https://media.example.invalid/file?sig=secret {}",h.core.token());
        let mut row = EntryRow::new("aaaaaaaaaaa", "360", "360", "manual", 1);
        row.status = "failed".into();
        row.error = Some(diagnostic.clone());
        h.core.video.insert_entry(&row).unwrap();
        for endpoint in ["/api/library", "/api/entries", "/api/jobs"] {
            let (status, _, body) = send(&h, request("GET", endpoint, Some(&cookie), "")).await;
            assert_eq!(status, StatusCode::OK);
            let text = String::from_utf8(body).unwrap();
            for secret in [
                &h.core.token(),
                "/srv/private",
                "video.mp4",
                "media.example.invalid",
                "sig=secret",
            ] {
                assert!(!text.contains(secret));
            }
            assert!(text.contains("ffmpeg failed"));
        }
        assert_eq!(
            h.core
                .video
                .get_entry(&row.id)
                .unwrap()
                .unwrap()
                .error
                .as_deref(),
            Some(diagnostic.as_str()),
            "native diagnostics remain unchanged"
        );
        let mut status = json!({"meta_error":diagnostic,"playback":{"error":diagnostic,"new_field":h.core.token()}});
        scrub_browser_json(
            &mut status,
            &[h.core.token(), h.core.paths.data_dir.display().to_string()],
        );
        let text = status.to_string();
        assert!(!text.contains(&h.core.token()));
        assert!(!text.contains("/srv/private"));
        assert!(!text.contains("sig=secret"));
        assert_eq!(
            send(&h, request("HEAD", "/api/library", Some(&cookie), ""))
                .await
                .0,
            StatusCode::OK
        );
    }
    #[tokio::test]
    async fn related_feed_names_its_recorded_parent_from_cached_metadata() {
        let h = harness();
        let cookie = pair(&h).await;
        let item = muninn_core::model::Item {
            video_id: "bbbbbbbbbbb".into(),
            title: "Recommendation".into(),
            ..Default::default()
        };
        // Expired content still identifies a historical capture; labeling a feed
        // must not require an online metadata refresh.
        h.core
            .content
            .put_json(
                muninn_core::content::KIND_META,
                "aaaaaaaaaaa",
                &json!({"title":"How ravens remember"}),
                Some(1),
                1,
            )
            .unwrap();
        let id = h
            .core
            .content
            .add_snapshot(
                "related",
                Some("https://www.youtube.com/watch?v=aaaaaaaaaaa"),
                None,
                20,
                &[item.clone()],
            )
            .unwrap();
        let spoof = h
            .core
            .content
            .add_snapshot(
                "related",
                Some("https://youtube.com.evil.invalid/watch?v=aaaaaaaaaaa"),
                None,
                10,
                &[item],
            )
            .unwrap();
        let (_, _, body) = send(&h, request("GET", "/api/feeds", Some(&cookie), "")).await;
        let feeds: Value = serde_json::from_slice(&body).unwrap();
        let row = feeds
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["id"] == id)
            .unwrap();
        assert_eq!(row["source_title"], "How ravens remember");
        assert_eq!(row["source_video_id"], "aaaaaaaaaaa");
        assert!(feeds
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["id"] == spoof)
            .unwrap()["source_title"]
            .is_null());
        for path in [
            format!("/api/feeds/{id}"),
            "/api/feeds/latest?kind=related".into(),
        ] {
            let (status, _, body) = send(&h, request("GET", &path, Some(&cookie), "")).await;
            assert_eq!(status, StatusCode::OK);
            let value: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(value["source_title"], "How ravens remember");
        }
        assert!(
            h.core
                .feed_view(h.core.content.get_snapshot(id).unwrap().unwrap())["source_title"]
                .is_null(),
            "native response remains unchanged"
        );
    }
    #[test]
    fn live_server_keeps_internal_listener_private_and_releases_its_data_lock() {
        use std::io::{Read, Write};
        use std::time::Duration;
        fn tcp(addr: SocketAddr, method: &str, path: &str, extra: &str) -> String {
            let mut socket =
                std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(2)).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            write!(socket,"{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Length: 0\r\n{extra}\r\n").unwrap();
            let mut body = String::new();
            socket.read_to_string(&mut body).unwrap();
            body
        }
        let dir = tempfile::tempdir().unwrap();
        let config = || CoreConfig {
            data_dir: dir.path().into(),
            tool_dirs: Some(vec![]),
            spawn_workers: false,
            ..Default::default()
        };
        let network = || ServerConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            public_origin: None,
        };
        // Exercise production listener setup with acquisition disabled: this
        // regression never installs tools or contacts an upstream service.
        let server = ServerService::start_configured(config(), network()).unwrap();
        let public = server.addr;
        assert_ne!(public.port(), 0);
        assert_eq!(server.public_origin, format!("http://{public}"));
        let info: Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("service.json")).unwrap())
                .unwrap();
        let private: SocketAddr = format!("127.0.0.1:{}", info["port"].as_u64().unwrap())
            .parse()
            .unwrap();
        assert_ne!(private, public);
        assert_eq!(server.core.port(), private.port());
        assert!(server.core.proxy_info().is_none());
        let token = server.core.token();
        assert!(tcp(public, "GET", "/app/", "").starts_with("HTTP/1.1 200"));
        assert!(tcp(public, "GET", "/api/library", "").starts_with("HTTP/1.1 401"));
        assert!(tcp(private, "GET", "/api/status", "").starts_with("HTTP/1.1 401"));
        assert!(tcp(
            private,
            "GET",
            "/api/status",
            &format!("Authorization: Bearer {token}\r\n")
        )
        .starts_with("HTTP/1.1 200"));
        let paired = tcp(
            public,
            "POST",
            "/app/session",
            &format!(
                "Origin: {}\r\nAuthorization: Bearer {token}\r\n",
                server.public_origin
            ),
        );
        assert!(paired.starts_with("HTTP/1.1 200"));
        assert!(!paired.contains(&token));
        let cookie = paired
            .lines()
            .find_map(|line| line.strip_prefix("set-cookie: "))
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        assert!(tcp(
            public,
            "GET",
            "/api/library",
            &format!("Cookie: {cookie}\r\n")
        )
        .starts_with("HTTP/1.1 200"));
        assert!(tcp(
            public,
            "GET",
            &format!("/s/{token}/anything"),
            &format!("Cookie: {cookie}\r\n")
        )
        .starts_with("HTTP/1.1 404"));
        assert!(Service::start(config(), 0).is_err());
        assert!(ServerService::start_configured(config(), network()).is_err());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("token")).unwrap(),
            token
        );
        drop(server);
        assert!(std::net::TcpStream::connect_timeout(&public, Duration::from_secs(1)).is_err());
        assert!(std::net::TcpStream::connect_timeout(&private, Duration::from_secs(1)).is_err());
        let restarted = ServerService::start_configured(config(), network()).unwrap();
        assert_ne!(restarted.core.token(), token);
        assert!(tcp(
            restarted.addr,
            "GET",
            "/api/library",
            &format!("Cookie: {cookie}\r\n")
        )
        .starts_with("HTTP/1.1 401"));
    }
    #[test]
    fn desktop_and_server_data_lock_prevents_two_owners_and_recovers() {
        let d = tempfile::tempdir().unwrap();
        let first = lock_data_dir(d.path()).unwrap();
        assert!(lock_data_dir(d.path()).is_err());
        drop(first);
        assert!(lock_data_dir(d.path()).is_ok());
    }
}
