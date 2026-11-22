//! Explicit, single-owner remote adapter. The desktop router and listener remain
//! unchanged. Only the operations below cross this boundary; native hooks,
//! local-path imports, updater/tool installers and capture proxy are not exposed.
use axum::{
    body::Body,
    extract::{connect_info::ConnectInfo, DefaultBodyLimit, Request, State},
    http::{header, HeaderValue, Method, StatusCode, Uri},
    middleware::{self, Next},
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Json, Router,
};
use muninn_core::{util::{ct_eq, random_token}, Core};
use serde::Deserialize;
use serde_json::json;
use std::{collections::HashMap, net::{IpAddr, SocketAddr}, sync::{Arc, Mutex}, time::{Duration, Instant}};
use tower::ServiceExt;

const SESSION_TTL: Duration = Duration::from_secs(86_400);
const MAX_SESSIONS: usize = 32;
const LOGIN_WINDOW: Duration = Duration::from_secs(60);
const MAX_LOGIN_ATTEMPTS: usize = 10;

struct Authelia {
    proxy_ips: Vec<IpAddr>,
    // Never accept an owner name from a browser, forwarded claim or env var.
    expected_user: &'static str,
}
pub struct Config {
    pub origin: String,
    authority: String,
    secure: bool,
    secret: Option<String>,
    authelia: Option<Authelia>,
}
impl Config {
    /// An exact public origin, never inferred from untrusted forwarded headers.
    fn parse_origin(origin: &str) -> Result<(String, String, bool), String> {
        let uri: Uri = origin.parse().map_err(|_| "invalid public origin")?;
        let authority = uri.authority().ok_or("origin needs a host")?;
        let scheme = uri.scheme_str().ok_or("origin needs http or https")?;
        if authority.as_str().contains('@') || uri.query().is_some()
            || !matches!(uri.path(), "" | "/") || origin.contains('#') {
            return Err("origin must not contain credentials, path, query or fragment".into());
        }
        let host = authority.host().trim_matches(&['[', ']'][..]);
        let loopback = matches!(host, "localhost" | "127.0.0.1" | "::1");
        if scheme != "https" && !(scheme == "http" && loopback) {
            return Err("remote access requires an HTTPS public origin; HTTP is loopback-only".into());
        }
        let authority = authority.as_str().to_ascii_lowercase();
        Ok((format!("{scheme}://{authority}"), authority, scheme == "https"))
    }
    pub fn new(origin: &str, secret: &str) -> Result<Self, String> {
        let (origin, authority, secure) = Self::parse_origin(origin)?;
        let secret = secret.trim();
        if !(32..=256).contains(&secret.len()) || !secret.bytes().all(|b| b.is_ascii_graphic()) {
            return Err("token file must contain a random 32–256 character ASCII secret".into());
        }
        Ok(Self { origin, authority, secure, secret: Some(secret.to_owned()), authelia: None })
    }
    pub fn authelia(origin: &str, proxy_ips: Vec<IpAddr>) -> Result<Self, String> {
        let (origin, authority, secure) = Self::parse_origin(origin)?;
        if !secure { return Err("Authelia requires an exact HTTPS origin".into()); }
        if proxy_ips.is_empty() || proxy_ips.iter().any(IpAddr::is_loopback) {
            return Err("Authelia requires non-loopback resolved Traefik peer addresses".into());
        }
        Ok(Self { origin, authority, secure, secret: None,
            authelia: Some(Authelia { proxy_ips, expected_user: "esben" }) })
    }
    fn auth_mode(&self) -> &'static str {
        if self.authelia.is_some() { "authelia" } else { "token" }
    }
    fn cookie_name(&self) -> &'static str {
        if self.secure { "__Host-muninn_session" } else { "muninn_dev_session" }
    }
    fn cookie(&self, token: &str, age: u32) -> String {
        format!("{}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={age}{}",
            self.cookie_name(), if self.secure { "; Secure" } else { "" })
    }
}

struct Sessions {
    active: HashMap<String, Instant>,
    attempts: Vec<Instant>,
}
struct WebState {
    core: Arc<Core>,
    config: Config,
    inner: Router,
    sessions: Mutex<Sessions>,
}
type App = Arc<WebState>;
fn error(status: StatusCode, text: &str) -> Response {
    (status, Json(json!({"ok": false, "error": text}))).into_response()
}
fn session_token(req: &Request, state: &WebState) -> Option<String> {
    req.headers().get(header::COOKIE)?.to_str().ok()?.split(';').find_map(|part| {
        let (key, value) = part.trim().split_once('=')?;
        (key == state.config.cookie_name() && value.len() == 64).then(|| value.to_owned())
    })
}
fn authenticated(req: &Request, state: &WebState) -> bool {
    if state.config.authelia.is_some() { return authelia_authorized(req, &state.config); }
    let Some(token) = session_token(req, state) else { return false };
    let mut sessions = state.sessions.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    sessions.active.retain(|_, until| *until > now);
    sessions.active.contains_key(&token)
}

// Authentication is tied to the actual inbound socket peer, not a spoofable
// X-Forwarded-For header. Traefik ForwardAuth replaces Remote-User and strips
// any untrusted browser supplied identity before forwarding.
fn authelia_authorized(req: &Request, config: &Config) -> bool {
    let Some(auth) = config.authelia.as_ref() else { return false };
    let peer = req.extensions().get::<ConnectInfo<SocketAddr>>().map(|v| v.0);
    let Some(peer) = peer else { return false };
    let headers = req.headers();
    auth.proxy_ips.contains(&peer.ip())
        && headers.get_all(header::HOST).iter().count() == 1
        && headers.get(header::HOST).is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(config.authority.as_bytes()))
        && headers.get_all("remote-user").iter().count() == 1
        && headers.get("remote-user").is_some_and(|v| v.as_bytes() == auth.expected_user.as_bytes())
        && headers.get_all("x-forwarded-proto").iter().count() == 1
        && headers.get("x-forwarded-proto").is_some_and(|v| v == "https")
        && headers.get_all(header::ORIGIN).iter().count() <= 1
        && (matches!(*req.method(), Method::GET | Method::HEAD)
            || headers.get(header::ORIGIN).is_some_and(|v| v == config.origin.as_str()))
}

/// No CORS. Exact Host + Origin and a custom header protect cookie-authenticated
/// writes, including login/logout. A proxy must preserve the configured Host.
async fn boundary(State(state): State<App>, req: Request, next: Next) -> Response {
    let host = req.headers().get(header::HOST).and_then(|v| v.to_str().ok());
    let origin = req.headers().get(header::ORIGIN).and_then(|v| v.to_str().ok());
    let read = matches!(*req.method(), Method::GET | Method::HEAD);
    let valid = host.is_some_and(|h| h.eq_ignore_ascii_case(&state.config.authority))
        && origin.is_none_or(|o| o == state.config.origin)
        && (read || (origin == Some(state.config.origin.as_str())
            && req.headers().get("x-muninn-client").is_some_and(|v| v == "pwa")));
    let authelia_ok = req.uri().path() == "/health" || state.config.authelia.is_none()
        || authelia_authorized(&req, &state.config);
    let mut response = if !valid || !authelia_ok {
        error(StatusCode::FORBIDDEN, "request origin or trusted identity not allowed")
    } else { next.run(req).await };
    let headers = response.headers_mut();
    for name in [header::ACCESS_CONTROL_ALLOW_ORIGIN, header::ACCESS_CONTROL_ALLOW_HEADERS,
        header::ACCESS_CONTROL_ALLOW_METHODS, header::ACCESS_CONTROL_MAX_AGE] { headers.remove(name); }
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert("content-security-policy", HeaderValue::from_static(
        "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data: blob:; media-src 'self' blob:; connect-src 'self'; worker-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'"));
    response
}

pub fn router(core: Arc<Core>, config: Config) -> Router {
    let state = Arc::new(WebState { inner: muninn_server::router(core.clone()), core, config,
        sessions: Mutex::new(Sessions { active: HashMap::new(), attempts: Vec::new() }) });
    Router::new()
        .route("/", get(|| async { Redirect::temporary("/app/") }))
        .route("/health", get(|| async { Json(json!({"ok":true,"mode":"server-pwa","version":muninn_core::VERSION})) }))
        .route("/app", get(|| async { Redirect::permanent("/app/") }))
        .route("/app/", get(|State(state): State<App>| async move { index(&state.config) }))
        .route("/app/{file}", get(|State(state): State<App>, axum::extract::Path(file): axum::extract::Path<String>| async move {
            if file == "index.html" { index(&state.config) } else { asset(&file) }
        }))
        .route("/api/session", get(session).post(login).delete(logout))
        .fallback(bridge)
        .layer(DefaultBodyLimit::max(16 * 1024))
        .layer(middleware::from_fn_with_state(state.clone(), boundary))
        .with_state(state)
}

fn index(config: &Config) -> Response {
    // The server stamps auth mode before JavaScript executes, so a failed
    // Authelia API request can never cause a fallback access-token prompt.
    let html = include_str!("../../../../ui/pwa/index.html")
        .replace("__MUNINN_AUTH_MODE__", config.auth_mode());
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], Body::from(html)).into_response()
}
fn asset(file: &str) -> Response {
    let (mime, bytes): (&str, &[u8]) = match file {
        "index.html" => ("text/html; charset=utf-8", include_bytes!("../../../../ui/pwa/index.html")),
        "app.js" => ("text/javascript; charset=utf-8", include_bytes!("../../../../ui/pwa/app.js")),
        "model.js" => ("text/javascript; charset=utf-8", include_bytes!("../../../../ui/pwa/model.js")),
        "store.js" => ("text/javascript; charset=utf-8", include_bytes!("../../../../ui/pwa/store.js")),
        "style.css" => ("text/css; charset=utf-8", include_bytes!("../../../../ui/pwa/style.css")),
        "sw.js" => ("text/javascript; charset=utf-8", include_bytes!("../../../../ui/pwa/sw.js")),
        "manifest.webmanifest" => ("application/manifest+json", include_bytes!("../../../../ui/pwa/manifest.webmanifest")),
        "icon.svg" => ("image/svg+xml", include_bytes!("../../../../ui/pwa/icon.svg")),
        "icon-192.png" => ("image/png", include_bytes!("../../../../ui/pwa/icon-192.png")),
        "icon-512.png" => ("image/png", include_bytes!("../../../../ui/pwa/icon-512.png")),
        _ => return error(StatusCode::NOT_FOUND, "asset not found"),
    };
    ([(header::CONTENT_TYPE, mime)], Body::from(bytes)).into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Login { token: String }
async fn login(State(state): State<App>, Json(login): Json<Login>) -> Response {
    if state.config.authelia.is_some() {
        return error(StatusCode::NOT_FOUND, "a second browser login is disabled");
    }
    let mut sessions = state.sessions.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    sessions.attempts.retain(|t| now.duration_since(*t) < LOGIN_WINDOW);
    if sessions.attempts.len() >= MAX_LOGIN_ATTEMPTS {
        let mut response = error(StatusCode::TOO_MANY_REQUESTS, "too many attempts; try again in a minute");
        response.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from_static("60"));
        return response;
    }
    sessions.attempts.push(now);
    if !state.config.secret.as_deref().is_some_and(|secret| ct_eq(login.token.trim(), secret)) {
        return error(StatusCode::UNAUTHORIZED, "invalid token");
    }
    sessions.active.retain(|_, until| *until > now);
    if sessions.active.len() >= MAX_SESSIONS {
        return error(StatusCode::TOO_MANY_REQUESTS, "session limit reached; sign out another device or restart the server");
    }
    let token = random_token();
    sessions.active.insert(token.clone(), now + SESSION_TTL);
    let mut response = Json(json!({"ok":true,"mode":"server-pwa"})).into_response();
    response.headers_mut().insert(header::SET_COOKIE,
        state.config.cookie(&token, SESSION_TTL.as_secs() as u32).parse().expect("generated ASCII cookie"));
    response
}
async fn session(State(state): State<App>, req: Request) -> Response {
    if !authenticated(&req, &state) { return error(StatusCode::UNAUTHORIZED, "sign in to the server"); }
    Json(json!({"ok":true,"mode":"server-pwa","auth_mode":state.config.auth_mode(),
        "login_required":false,"version":muninn_core::VERSION})).into_response()
}
async fn logout(State(state): State<App>, req: Request) -> Response {
    if state.config.authelia.is_some() {
        return error(StatusCode::NOT_FOUND, "use Authelia to sign out");
    }
    if let Some(token) = session_token(&req, &state) {
        state.sessions.lock().unwrap_or_else(|e| e.into_inner()).active.remove(&token);
    }
    let mut response = Json(json!({"ok":true})).into_response();
    response.headers_mut().insert(header::SET_COOKIE, state.config.cookie("", 0).parse().expect("ASCII cookie"));
    response
}

fn allowed(method: &Method, path: &str) -> bool {
    let read = matches!(*method, Method::GET | Method::HEAD);
    if !path.starts_with('/') || path.starts_with("//") { return false; }
    let parts: Vec<_> = path[1..].split('/').collect();
    let id = |s: &str| !s.is_empty() && s.len() <= 160 && s != "." && !s.contains("..")
        // Existing Core IDs are video_id.variant (for example aaaaaaaaaaa.720).
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b':' | b'.'));
    match parts.as_slice() {
        ["api", "entries" | "feeds" | "jobs" | "tools"] => read,
        ["api", "entries" | "feeds", entry] => read && id(entry),
        ["api", "cache"] => *method == Method::POST,
        ["api", "entries", entry, "retry" | "cancel" | "promote"] => *method == Method::POST && id(entry),
        ["media", entry] => read && id(entry),
        ["thumb", video] => read && muninn_core::util::valid_video_id(video),
        _ => false,
    }
}
async fn bridge(State(state): State<App>, mut req: Request) -> Response {
    if !authenticated(&req, &state) { return error(StatusCode::UNAUTHORIZED, "sign in to the server"); }
    if !allowed(req.method(), req.uri().path()) {
        return error(StatusCode::NOT_FOUND, "operation not exposed by this server");
    }
    // The remote adapter never accepts URL credentials and never exposes the
    // core token. Re-enter the unchanged localhost adapter only after auth.
    if req.uri().query().is_some_and(|q| q.split('&').any(|p| p.starts_with("token="))) {
        return error(StatusCode::BAD_REQUEST, "URL credentials are not accepted");
    }
    if let Some(id) = req.uri().path().strip_prefix("/media/") {
        match state.core.video.get_entry(id) {
            Ok(Some(entry)) if entry.is_complete() => (),
            Ok(Some(_)) => return error(StatusCode::CONFLICT, "wait for the server download to complete"),
            Ok(None) => return error(StatusCode::NOT_FOUND, "entry not found"),
            Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "could not read entry"),
        }
    }
    if req.method() == Method::POST {
        let (parts, body) = req.into_parts();
        let bytes = match axum::body::to_bytes(body, 16 * 1024).await {
            Ok(bytes) => bytes,
            Err(_) => return error(StatusCode::PAYLOAD_TOO_LARGE, "request body too large"),
        };
        req = Request::from_parts(parts, Body::from(bytes));
    }
    req.headers_mut().insert(header::HOST, HeaderValue::from_static("127.0.0.1"));
    req.headers_mut().insert(header::AUTHORIZATION,
        format!("Bearer {}", state.core.token()).parse().expect("core token is ASCII"));
    req.headers_mut().remove(header::COOKIE);
    req.headers_mut().remove("x-muninn-token");
    state.inner.clone().oneshot(req).await.unwrap_or_else(|never| match never {})
}

#[cfg(test)]
mod tests;
