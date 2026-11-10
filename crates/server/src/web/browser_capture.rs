//! Container-private ingestion bridge. This listener is loopback-only, opt-in,
//! token authenticated, and is never mounted on the public PWA router.
use axum::{body::Body, extract::{DefaultBodyLimit, Request, State}, http::{header, StatusCode},
    middleware::{self, Next}, response::{IntoResponse, Response}, routing::post, Json, Router};
use muninn_core::{capture::CaptureInput, model::TabIngest, Core};
use serde::Deserialize;
use serde_json::json;
use std::{path::PathBuf, sync::Arc};

const MAX_BODY: usize = 4 * 1024 * 1024;

async fn authenticate(State(core): State<Arc<Core>>, request: Request, next: Next) -> Response {
    let host = request.headers().get(header::HOST).and_then(|h| h.to_str().ok());
    let token = request.headers().get(header::AUTHORIZATION).and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "));
    if host != Some("127.0.0.1:47873") || !token.is_some_and(|t| core.check_token(t)) {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error":"private capture authentication required"}))).into_response();
    }
    let mut response = next.run(request).await;
    response.headers_mut().insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
}

pub fn router(core: Arc<Core>) -> Router {
    Router::new().route("/capture", post(capture)).route("/tab", post(tab))
        .layer(DefaultBodyLimit::max(MAX_BODY * 2 + 65536))
        .layer(middleware::from_fn_with_state(core.clone(), authenticate)).with_state(core)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CapturedResponse {
    url: String,
    referer: Option<String>,
    request_body: Option<String>,
    body: String,
    content_type: String,
}
fn youtube(url: &str) -> bool {
    ["https://www.youtube.com/", "https://youtube.com/", "https://m.youtube.com/"]
        .iter().any(|prefix| url.starts_with(prefix)) && !url.chars().any(char::is_control)
}
async fn capture(State(core): State<Arc<Core>>, Json(input): Json<CapturedResponse>) -> Response {
    if !youtube(&input.url) || input.url.len() > 4096 || input.body.len() > MAX_BODY
        || input.referer.as_ref().is_some_and(|r| !youtube(r) || r.len() > 4096)
        || input.request_body.as_ref().is_some_and(|r| r.len() > 65536)
        || !matches!(input.content_type.as_str(), "application/json" | "text/html") {
        return (StatusCode::BAD_REQUEST, Json(json!({"error":"invalid capture envelope"}))).into_response();
    }
    // Patchright returns decoded bodies. Never apply Content-Encoding again.
    let result = core.capture(CaptureInput { url: &input.url, referer: input.referer.as_deref(),
        request_body: input.request_body.as_deref().map(str::as_bytes), body: input.body.as_bytes(),
        content_type: Some(&input.content_type), content_encoding: None });
    Json(result).into_response()
}
async fn tab(State(core): State<Arc<Core>>, Json(input): Json<TabIngest>) -> Response {
    if !input.tab_id.starts_with("server-") || input.tab_id.len() > 80 || !youtube(&input.url) {
        return (StatusCode::BAD_REQUEST, Json(json!({"error":"invalid server tab"}))).into_response();
    }
    match core.ingest_tab(input) {
        Ok(value) => Json(value).into_response(),
        Err(_) => (StatusCode::BAD_REQUEST, Json(json!({"error":"tab capture failed"}))).into_response(),
    }
}

// Credentials have no Debug/Serialize implementation and never enter HTTP output.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredCookie {
    domain: String, path: String, name: String, value: String,
    secure: bool, http_only: bool, expires: Option<i64>,
}
pub fn cookie_reader(path: PathBuf) -> muninn_core::youtube_session::CookieReader {
    Arc::new(move || {
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(_) => return Err(muninn_core::Error::other("server browser session unavailable")),
        };
        if !metadata.is_file() || metadata.len() > 65536 {
            return Err(muninn_core::Error::other("invalid server browser session file"));
        }
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(muninn_core::Error::other("server browser session file must be private"));
            }
        }
        let data = std::fs::read(&path).map_err(|_| muninn_core::Error::other("server browser session unavailable"))?;
        let cookies: Vec<StoredCookie> = serde_json::from_slice(&data)
            .map_err(|_| muninn_core::Error::other("invalid server browser session file"))?;
        if cookies.len() > 512 { return Err(muninn_core::Error::other("too many server browser cookies")); }
        Ok(cookies.into_iter().filter(|c| muninn_core::youtube_session::youtube_domain(&c.domain))
            .map(|c| muninn_core::youtube_session::YoutubeCookie { domain:c.domain,path:c.path,name:c.name,value:c.value,
                secure:c.secure,http_only:c.http_only,expires:c.expires }).collect())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use muninn_core::CoreConfig;
    use tower::ServiceExt;
    use http_body_util::BodyExt;
    #[tokio::test]
    async fn authenticated_capture_reuses_native_feed_parser() {
        let dir = tempfile::tempdir().unwrap();
        let core = Core::open(CoreConfig {data_dir:dir.path().into(),spawn_workers:false,tool_dirs:Some(vec![]),..Default::default()}).unwrap();
        let app = router(core.clone());
        let envelope = json!({"url":"https://www.youtube.com/youtubei/v1/browse","referer":"https://www.youtube.com/",
            "request_body":"{\"browseId\":\"FEwhat_to_watch\"}","content_type":"application/json",
            "body":json!({"contents":{"items":[{"videoRenderer":{"videoId":"aaaaaaaaaaa","title":{"simpleText":"Captured in server Chromium"}}}]}}).to_string()}).to_string();
        let request = |auth: bool| {
            let mut b = Request::builder().method("POST").uri("/capture").header("Host","127.0.0.1:47873").header("Content-Type","application/json");
            if auth { b = b.header("Authorization",format!("Bearer {}",core.token())); }
            b.body(Body::from(envelope.clone())).unwrap()
        };
        assert_eq!(app.clone().oneshot(request(false)).await.unwrap().status(),StatusCode::UNAUTHORIZED);
        let response = app.clone().oneshot(request(true)).await.unwrap();
        assert_eq!(response.status(),StatusCode::OK);
        let value:serde_json::Value=serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!(value["items"],1);
        assert_eq!(core.content.latest_snapshot("home").unwrap().unwrap().items.len(),1);
        let bad = Request::builder().method("POST").uri("/api/settings").header("Host","127.0.0.1:47873")
            .header("Authorization",format!("Bearer {}",core.token())).body(Body::empty()).unwrap();
        assert_eq!(app.oneshot(bad).await.unwrap().status(),StatusCode::NOT_FOUND);
    }
    #[test]
    fn cookie_reader_scopes_credentials_and_fails_closed() {
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("cookies");
        let reader=cookie_reader(path.clone());assert!(reader().unwrap().is_empty());
        std::fs::write(&path,r#"[{"domain":"accounts.google.com","path":"/","name":"SID","value":"fixture-private","secure":true,"http_only":true,"expires":null},{"domain":".youtube.com","path":"/","name":"SID","value":"fixture-private","secure":true,"http_only":true,"expires":null}]"#).unwrap();
        #[cfg(unix)]{use std::os::unix::fs::PermissionsExt;std::fs::set_permissions(&path,std::fs::Permissions::from_mode(0o600)).unwrap();}
        assert_eq!(reader().unwrap().len(),1);
        std::fs::write(&path,"bad credential JSON").unwrap();
        assert!(reader().is_err());
    }
}
