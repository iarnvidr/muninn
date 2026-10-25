//! The localhost HTTP server: ingest API (extension), query API (UI), the Range
//! media server and the embedded UI. Everything except `/health` and `/ui/` needs
//! the per-install token; requests whose Host is not localhost are refused.

use crate::engine::{Core, CoreConfig, DEFAULT_PORT};
use crate::error::Error;
use crate::range;
use crate::rules::{normalize, Rule};
use crate::settings::Settings;
use crate::ui_assets;
use crate::ytdlp;
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;

type S = State<Arc<Core>>;
type ApiResult = std::result::Result<Json<Value>, ApiError>;

pub struct ApiError(Error);

impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        ApiError(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            Error::NotFound(_) => StatusCode::NOT_FOUND,
            Error::BadRequest(_) => StatusCode::BAD_REQUEST,
            Error::Conflict(_) => StatusCode::CONFLICT,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(json!({"ok": false, "error": self.0.to_string()}))).into_response()
    }
}

fn parse<T: serde::de::DeserializeOwned>(body: &Bytes) -> Result<T, ApiError> {
    serde_json::from_slice(body).map_err(|e| ApiError(Error::bad(format!("invalid JSON body: {e}"))))
}

fn json_error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"ok": false, "error": message}))).into_response()
}

// --- middleware ------------------------------------------------------------------------------

fn host_allowed(host: &str) -> bool {
    let name = if let Some(rest) = host.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        host.split(':').next().unwrap_or("")
    };
    matches!(name, "127.0.0.1" | "localhost" | "::1")
}

fn cors(headers: &mut HeaderMap) {
    headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    headers.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, HeaderValue::from_static("authorization, content-type, x-muninn-token"));
    headers.insert(header::ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static("GET, POST, PUT, DELETE, HEAD, OPTIONS"));
    headers.insert(header::ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("600"));
}

/// Outermost: DNS-rebinding guard, CORS headers and preflight answers.
async fn outer(req: Request, next: Next) -> Response {
    if let Some(host) = req.headers().get(header::HOST).and_then(|h| h.to_str().ok()) {
        if !host_allowed(host) {
            return json_error(StatusCode::FORBIDDEN, "host not allowed");
        }
    }
    let mut resp = if req.method() == Method::OPTIONS {
        let mut r = Response::new(Body::empty());
        *r.status_mut() = StatusCode::NO_CONTENT;
        r
    } else {
        next.run(req).await
    };
    cors(resp.headers_mut());
    resp
}

fn token_from(headers: &HeaderMap, query: Option<&str>) -> Option<String> {
    if let Some(v) = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) {
        if let Some(t) = v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")) {
            return Some(t.trim().to_string());
        }
    }
    if let Some(v) = headers.get("x-muninn-token").and_then(|v| v.to_str().ok()) {
        return Some(v.trim().to_string());
    }
    query?.split('&').find_map(|kv| kv.strip_prefix("token=")).map(|t| t.to_string())
}

async fn auth(State(core): S, req: Request, next: Next) -> Response {
    let token = token_from(req.headers(), req.uri().query());
    match token {
        Some(t) if core.check_token(&t) => next.run(req).await,
        _ => json_error(StatusCode::UNAUTHORIZED, "unauthorized"),
    }
}

async fn audit(State(core): S, req: Request, next: Next) -> Response {
    let method = req.method().to_string();
    let route = req.extensions().get::<axum::extract::MatchedPath>().map(|m|m.as_str().to_string()).unwrap_or_default();
    let response = next.run(req).await;
    if response.status().is_server_error() || response.status().is_client_error() {
        core.log_api_failure(&method, &route, response.status().as_u16());
    }
    response
}

// --- router -----------------------------------------------------------------------------------

pub fn router(core: Arc<Core>) -> Router {
    let protected = Router::new()
        // Ingest
        .route("/api/ingest/feed", post(ingest_feed))
        .route("/api/ingest/video", post(ingest_video))
        .route("/api/ingest/comments", post(ingest_comments))
        .route("/api/ingest/tab", post(ingest_tab))
        .route("/api/ingest/playback", post(ingest_playback))
        // Extension helpers
        .route("/api/status", get(status))
        .route("/api/logs", get(logs).post(client_log))
        .route("/api/feeds/refresh", post(refresh_feed))
        .route("/api/automation", get(automation).put(put_automation))
        .route("/api/organization", get(organization).put(put_organization).post(change_organization))
        .route("/api/cleanup", get(cleanup_preview).post(cleanup_run))
        .route("/api/subscriptions", get(subscriptions).put(put_subscriptions))
        .route("/api/channels", get(known_channels))
        .route("/api/channels/status", get(channel_discovery))
        .route("/api/channel-groups", get(channel_groups).put(put_channel_groups))
        .route("/api/featured", get(featured))
        .route("/api/channels/{id}/videos", get(channel_videos))
        .route("/api/subscriptions/effective", get(effective_subscriptions))
        .route("/api/entries/{id}/playback", get(local_playback_state).post(local_playback))
        .route("/api/entries/{id}/playback/stop", post(stop_local_playback))
        .route("/api/subscriptions/poll", post(poll_subscriptions))
        .route("/api/import/media", post(import_media))
        .route("/api/import/tabs", post(import_tabs))
        .route("/api/import/links", post(import_links))
        .route("/api/library", get(library))
        .route("/api/library/{id}/remove", post(remove_library_link))
        .route("/api/cache", post(cache_request))
        .route("/api/open", post(open))
        .route("/api/open/pending", get(open_pending))
        // Query / control
        .route("/api/entries", get(entries))
        .route("/api/entries/{id}", get(entry))
        .route("/api/entries/{id}/promote", post(promote))
        .route("/api/entries/{id}/retention", post(retention))
        .route("/api/entries/{id}/evict", post(evict))
        .route("/api/entries/{id}/retry", post(retry))
        .route("/api/entries/{id}/cancel", post(cancel))
        .route("/api/feeds", get(feeds))
        .route("/api/feeds/latest", get(feed_latest))
        .route("/api/feeds/{id}", get(feed))
        .route("/api/video/{id}", get(video))
        .route("/api/rules", get(get_rules).put(put_rules))
        .route("/api/settings", get(get_settings).put(put_settings))
        .route("/api/disk", get(disk))
        .route("/api/jobs", get(jobs))
        .route("/api/tabs", get(tabs))
        .route("/api/history", get(history))
        .route("/api/tools", get(tools))
        .route("/api/tools/ytdlp/install", post(ytdlp_install))
        .route("/api/tools/ytdlp/update", post(ytdlp_update))
        .route("/api/watch/{id}", get(watch_status).post(watch_open))
        .route("/api/playlist", get(playlist_metadata))
        .route("/api/video/{id}/comments/more", post(comments_more))
        .route("/api/video/{id}/comments/{comment}/replies", post(comment_replies))
        .route("/api/proxy", get(proxy_info))
        .route("/api/captures", get(captures))
        .route("/api/setup", get(setup_state))
        .route("/media/{id}", get(media))
        .route("/thumb/{id}", get(thumb))
        .layer(middleware::from_fn_with_state(core.clone(), audit))
        .layer(middleware::from_fn_with_state(core.clone(), auth));
    Router::new()
        .route("/health", get(health))
        .route("/s/{token}/{video}/{index}", get(feeder))
        .route("/h/{token}/{dir}/{file}", get(hls_file))
        .route("/ui", get(|| async { Redirect::permanent("/ui/") }))
        .route("/ui/", get(ui_index))
        .route("/ui/{file}", get(ui_file))
        .merge(protected)
        .layer(DefaultBodyLimit::max(80 * 1024 * 1024))
        .layer(middleware::from_fn(outer))
        .with_state(core)
}

// --- handlers: open --------------------------------------------------------------------------------

async fn playlist_metadata(State(core): S, Query(q): Query<HashMap<String,String>>) -> ApiResult {
    let list_id = q.get("list").map(String::as_str).unwrap_or("");
    if !crate::playlist::valid_list_id(list_id) { return Err(Error::bad("Invalid playlist id").into()); }
    let current = q.get("video").filter(|v| crate::util::valid_video_id(v)).map(String::as_str);
    let index = q.get("index").and_then(|s| s.parse::<usize>().ok()).filter(|i| *i > 0);
    let bin = crate::ytdlp::find_ytdlp(&core.paths.bin_dir, &core.tool_dirs).ok_or_else(|| Error::bad("yt-dlp is not installed"))?;
    let url = format!("https://www.youtube.com/playlist?list={list_id}");
    let output = tokio::time::timeout(std::time::Duration::from_secs(20), tokio::process::Command::new(bin)
        .args(["--ignore-config", "--flat-playlist", "--dump-single-json", "--playlist-end", "200", "--no-warnings", "--", &url])
        .env("PATH", crate::ytdlp::search_path()).stdin(std::process::Stdio::null()).output())
        .await.map_err(|_| Error::bad("Playlist lookup timed out"))?.map_err(|e| Error::other(format!("Could not run yt-dlp: {e}")))?;
    if !output.status.success() { return Err(Error::bad("yt-dlp could not read playlist metadata").into()); }
    let raw = String::from_utf8_lossy(&output.stdout);
    let playlist = crate::playlist::parse_flat_json(&raw, list_id, current, index).map_err(Error::bad)?;
    Ok(Json(serde_json::to_value(playlist).unwrap_or_else(|_| json!({}))) )
}

async fn health() -> Json<Value> {
    Json(json!({"ok": true, "app": "muninn", "version": crate::engine::VERSION}))
}

async fn logs(State(core): S, Query(q): Query<HashMap<String,String>>) -> Json<Value> {
    Json(core.logs(q.get("limit").and_then(|v|v.parse().ok()).unwrap_or(200)))
}
async fn refresh_feed(State(core): S, body: Bytes) -> ApiResult {
    core.require_online()?;
    let value: Value = parse(&body)?;
    let kind = value["kind"].as_str().unwrap_or("subscriptions");
    if !matches!(kind,"home"|"subscriptions"|"channels") { return Err(Error::bad("Refresh supports home, subscriptions or channels").into()); }
    let hook = core.feed_refresh_hook.read().unwrap_or_else(|e|e.into_inner()).clone()
        .ok_or_else(||Error::bad("Feed refresh requires the Muninn desktop app"))?;
    let requested_at = core.now();
    if kind=="channels" {core.video.kv_put("channel-discovery",&json!({"phase":"loading","count":0,"updated_at":requested_at}).to_string())?;}
    hook(kind)?;
    core.log_event("info","feed.refresh.requested",json!({"kind":kind}));
    Ok(Json(json!({"queued":true,"kind":kind,"requested_at":requested_at})))
}
async fn client_log(State(core): S, body: Bytes) -> ApiResult {
    let value: Value = parse(&body)?;
    let event = value["event"].as_str().unwrap_or("ui.error");
    if !matches!(event,"ui.error"|"ui.unhandled"|"ui.channels.rendered"|"ui.playback.error"|"ui.playback.stall"|"ui.playback.playing"|"ui.playback.state"|"ui.player.theatre"|"player.pip") { return Err(Error::bad("Unsupported client log event").into()); }
    let message: String = value["message"].as_str().unwrap_or("").chars().take(4000).collect();
    core.log_event(if matches!(event,"ui.error"|"ui.unhandled"|"ui.playback.error"|"ui.playback.stall"){"warn"}else{"info"},event,json!({"message":message}));
    Ok(Json(json!({"ok":true})))
}
async fn automation(State(core): S) -> Json<Value> {Json(json!(core.automation()))}
async fn organization(State(core):S)->Json<Value> {Json(json!(core.organization()))}
async fn put_organization(State(core):S,body:Bytes)->ApiResult {
    let org:crate::organization::Organization=parse(&body)?;core.put_organization(&org)?;Ok(Json(json!(org)))
}
async fn change_organization(State(core):S,body:Bytes)->ApiResult {Ok(Json(json!(core.change_organization(parse(&body)?)?)))}
async fn put_automation(State(core): S, body:Bytes) -> ApiResult {
    let config:crate::automation::Automation=parse(&body)?;core.put_automation(&config)?;Ok(Json(json!(config)))
}
async fn cleanup_preview(State(core): S) -> ApiResult {Ok(Json(core.cleanup_preview()?))}
async fn cleanup_run(State(core): S) -> ApiResult {
    tokio::task::spawn_blocking(move||core.cleanup_completed()).await.map_err(|e|ApiError(Error::other(e.to_string())))??;
    Ok(Json(json!({"ok":true})))
}
async fn subscriptions(State(core): S) -> Json<Value> {Json(json!(core.subscriptions()))}
async fn known_channels(State(core): S) -> Json<Value> {Json(core.known_channels())}
async fn channel_discovery(State(core): S) -> Json<Value> {Json(core.channel_discovery())}
async fn channel_groups(State(core):S)->Json<Value>{Json(json!(core.channel_groups()))}
async fn featured(State(core):S)->Json<Value>{Json(core.featured_uploads())}
async fn channel_videos(State(core):S,Path(id):Path<String>,Query(q):Query<HashMap<String,String>>)->ApiResult {
    if q.get("source").is_some_and(|s|s=="library"){return Ok(Json(core.channel_library(&id)?));}
    let offset=q.get("offset").map(|s|s.parse::<usize>().map_err(|_|Error::bad("invalid page"))).transpose()?.unwrap_or(0);
    let refresh=q.get("refresh").is_some_and(|s|s=="1");
    Ok(Json(tokio::task::spawn_blocking(move||core.channel_uploads(&id,offset,refresh)).await.map_err(|e|ApiError(Error::other(e.to_string())))??))
}
async fn effective_subscriptions(State(core):S)->Json<Value>{Json(json!(core.effective_subscriptions()))}
async fn put_channel_groups(State(core):S,body:Bytes)->ApiResult {let groups:Vec<crate::channel_groups::ChannelGroup>=parse(&body)?;core.put_channel_groups(&groups)?;Ok(Json(json!(groups)))}
async fn local_playback(State(core): S, Path(id): Path<String>, Query(q): Query<HashMap<String, String>>) -> ApiResult {
    let position=q.get("position").map(|p|p.parse::<f64>().ok().filter(|p|p.is_finite()&&*p>=0.0).ok_or_else(||Error::bad("invalid position"))).transpose()?;
    let seq=q.get("seq").map(|s|s.parse::<u64>().map_err(|_|Error::bad("invalid seq"))).transpose()?;
    let client=q.get("client").cloned().unwrap_or_default();
    Ok(Json(tokio::task::spawn_blocking(move||core.prepare_local_playback_for(&id,position,seq.map(|s|(client.as_str(),s)))).await.map_err(|e|ApiError(Error::other(e.to_string())))??))
}
async fn local_playback_state(State(core): S, Path(id): Path<String>) -> ApiResult {core.resolve_media_entry(&id)?;Ok(Json(core.local_playback_status(&id).ok_or_else(||Error::not_found("no local playback"))?))}
async fn stop_local_playback(State(core): S, Path(id): Path<String>, Query(q): Query<HashMap<String, String>>) -> ApiResult {
    let generation=q.get("generation").map(|g|g.parse::<u64>().map_err(|_|Error::bad("invalid generation"))).transpose()?;
    tokio::task::spawn_blocking(move||core.stop_local_playback_generation(&id,generation)).await.map_err(|e|ApiError(Error::other(e.to_string())))??;
    Ok(Json(json!({"ok":true})))
}
async fn put_subscriptions(State(core): S, body:Bytes) -> ApiResult {
    let channels:Vec<crate::subscriptions::Channel>=parse(&body)?;core.put_subscriptions(&channels)?;Ok(Json(json!(channels)))
}
async fn poll_subscriptions(State(core): S) -> ApiResult {
    core.check_channels_now()?;Ok(Json(json!({"ok":true,"scheduled":true})))
}
async fn import_media(State(core): S, body:Bytes) -> ApiResult {
    let req=parse(&body)?;
    let value=tokio::task::spawn_blocking(move||core.import_media(req)).await.map_err(|e|ApiError(Error::other(e.to_string())))??;
    Ok(Json(value))
}
async fn import_tabs(State(core): S, body:Bytes) -> ApiResult {Ok(Json(core.import_tabs(parse(&body)?)?))}
async fn import_links(State(core):S,body:Bytes)->ApiResult {Ok(Json(core.import_links(parse(&body)?)?))}
async fn library(State(core):S)->ApiResult {Ok(Json(json!(core.library_view()?)))}
async fn remove_library_link(State(core):S,Path(id):Path<String>)->ApiResult {Ok(Json(core.remove_library_link(&id)?))}

fn ui_response(name: &str) -> Response {
    if name == "raven.png" {
        return Response::builder().header(header::CONTENT_TYPE,"image/png").header(header::CACHE_CONTROL,"no-cache").header("x-content-type-options","nosniff").body(Body::from(ui_assets::RAVEN_PNG)).unwrap();
    }
    if let Some(bytes) = ui_assets::font(name) {
        return Response::builder().header(header::CONTENT_TYPE,"font/ttf").header("x-content-type-options","nosniff").body(Body::from(bytes)).unwrap();
    }
    match ui_assets::asset(name) {
        Some((ct, body)) => Response::builder()
            .header(header::CONTENT_TYPE, ct)
            .header(header::CACHE_CONTROL, "no-cache")
            // The UI loads only from its own origin; media and thumbnails come from the same server.
            .header("x-content-type-options", "nosniff")
            .body(Body::from(body))
            .unwrap(),
        None => json_error(StatusCode::NOT_FOUND, "no such file"),
    }
}

async fn ui_index() -> Response {
    ui_response("")
}

async fn ui_file(Path(file): Path<String>) -> Response {
    ui_response(&file)
}

// --- handlers: ingest ------------------------------------------------------------------------------

async fn ingest_feed(State(core): S, body: Bytes) -> ApiResult {
    Ok(Json(core.ingest_feed(parse(&body)?)?))
}
async fn ingest_video(State(core): S, body: Bytes) -> ApiResult {
    Ok(Json(core.ingest_video(parse(&body)?)?))
}
async fn ingest_comments(State(core): S, body: Bytes) -> ApiResult {
    Ok(Json(core.ingest_comments(parse(&body)?)?))
}
async fn ingest_tab(State(core): S, body: Bytes) -> ApiResult {
    Ok(Json(core.ingest_tab(parse(&body)?)?))
}
async fn ingest_playback(State(core): S, body: Bytes) -> ApiResult {
    Ok(Json(core.ingest_playback(parse(&body)?)?))
}
// --- handlers: extension helpers ------------------------------------------------------------------

async fn status(State(core): S) -> Json<Value> {
    Json(core.status_view())
}

async fn cache_request(State(core): S, body: Bytes) -> ApiResult {
    let (entry, decision) = core.request_cache(parse(&body)?)?;
    Ok(Json(json!({"ok": true, "entry": entry.map(|e| core.entry_view(&e)), "decision": decision})))
}

async fn open(State(core): S, body: Bytes) -> ApiResult {
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct Req {
        video_id: String,
        quality: Option<String>,
    }
    let req: Req = parse(&body)?;
    let (entry, decision) = core.open_video(&req.video_id, req.quality.as_deref())?;
    Ok(Json(json!({"ok": true, "entry": entry.map(|e| core.entry_view(&e)), "decision": decision})))
}

async fn open_pending(State(core): S, Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    let after = q.get("after").and_then(|v| v.parse().ok()).unwrap_or(0);
    let (seq, items) = core.pending_opens(after);
    Json(json!({"seq": seq, "items": items}))
}

// --- handlers: query / control ------------------------------------------------------------------------

async fn entries(State(core): S, Query(q): Query<HashMap<String, String>>) -> ApiResult {
    Ok(Json(Value::Array(core.entries_view(q.get("state").map(|s| s.as_str()))?)))
}

async fn entry(State(core): S, Path(id): Path<String>) -> ApiResult {
    let e = core.video.get_entry(&id)?.ok_or_else(|| Error::not_found("no such entry"))?;
    Ok(Json(core.entry_view(&e)))
}

async fn promote(State(core): S, Path(id): Path<String>) -> ApiResult {
    let e = tokio::task::spawn_blocking(move || core.promote(&id).map(|e| (core.entry_view(&e), ())))
        .await
        .map_err(|e| ApiError(Error::other(e.to_string())))??;
    Ok(Json(json!({"ok": true, "entry": e.0})))
}

async fn retention(State(core): S, Path(id): Path<String>, body:Bytes) -> ApiResult {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Req {protect:bool,retention_days:Option<u32>}
    let req:Req=parse(&body)?;
    let value=tokio::task::spawn_blocking(move|| {
        let e=if req.protect {core.promote(&id)?} else {core.release(&id,req.retention_days.unwrap_or(0))?};
        Ok::<_,Error>(core.entry_view(&e))
    }).await.map_err(|e|ApiError(Error::other(e.to_string())))??;
    Ok(Json(json!({"ok":true,"entry":value})))
}

async fn evict(State(core): S, Path(id): Path<String>, Query(q): Query<HashMap<String, String>>) -> ApiResult {
    let force = q.get("force").is_some_and(|v| v == "1" || v == "true");
    tokio::task::spawn_blocking(move || core.evict(&id, force)).await.map_err(|e| ApiError(Error::other(e.to_string())))??;
    Ok(Json(json!({"ok": true})))
}

async fn retry(State(core): S, Path(id): Path<String>) -> ApiResult {
    core.retry(&id)?;
    Ok(Json(json!({"ok": true})))
}

async fn cancel(State(core): S, Path(id): Path<String>) -> ApiResult {
    core.cancel(&id)?;
    Ok(Json(json!({"ok": true})))
}

async fn feeds(State(core): S, Query(q): Query<HashMap<String, String>>) -> ApiResult {
    let limit = q.get("limit").and_then(|v| v.parse().ok()).unwrap_or(50).clamp(1, 500);
    let kind = q.get("kind").filter(|k| !k.is_empty()).map(|k| k.as_str());
    Ok(Json(serde_json::to_value(core.content.list_snapshots(kind, limit)?).map_err(Error::from)?))
}

async fn feed_latest(State(core): S, Query(q): Query<HashMap<String, String>>) -> ApiResult {
    let kind = q.get("kind").cloned().unwrap_or_else(|| "home".into());
    let snap = core.content.latest_snapshot(&kind)?.ok_or_else(|| Error::not_found(format!("no {kind} snapshot captured yet")))?;
    Ok(Json(core.feed_view(snap)))
}

async fn feed(State(core): S, Path(id): Path<i64>) -> ApiResult {
    let snap = core.content.get_snapshot(id)?.ok_or_else(|| Error::not_found("no such snapshot"))?;
    Ok(Json(core.feed_view(snap)))
}

async fn video(State(core): S, Path(id): Path<String>) -> ApiResult {
    Ok(Json(core.video_detail(&id)?))
}

async fn get_rules(State(core): S) -> Json<Value> {
    Json(serde_json::to_value(core.video.rules()).unwrap_or(Value::Null))
}

async fn put_rules(State(core): S, body: Bytes) -> ApiResult {
    let value: Value = parse(&body)?;
    let list = if value.is_array() { value } else { value.get("rules").cloned().unwrap_or(Value::Null) };
    let rules: Vec<Rule> = serde_json::from_value(list).map_err(|e| ApiError(Error::bad(format!("invalid rules: {e}"))))?;
    let rules = normalize(rules).map_err(Error::BadRequest)?;
    core.video.put_rules(&rules)?;
    Ok(Json(serde_json::to_value(rules).map_err(Error::from)?))
}

async fn get_settings(State(core): S) -> Json<Value> {
    Json(serde_json::to_value(core.video.settings()).unwrap_or(Value::Null))
}

async fn put_settings(State(core): S, body: Bytes) -> ApiResult {
    let patch: Value = parse(&body)?;
    let mut current = serde_json::to_value(core.video.settings()).map_err(Error::from)?;
    if let (Some(cur), Some(p)) = (current.as_object_mut(), patch.as_object()) {
        for (k, v) in p {
            cur.insert(k.clone(), v.clone());
        }
    }
    let settings: Settings = serde_json::from_value(current).map_err(|e| ApiError(Error::bad(format!("invalid settings: {e}"))))?;
    core.apply_settings(&settings)?;
    Ok(Json(serde_json::to_value(core.video.settings()).map_err(Error::from)?))
}

async fn disk(State(core): S) -> Json<Value> {
    Json(serde_json::to_value(core.disk_usage()).unwrap_or(Value::Null))
}

async fn jobs(State(core): S) -> ApiResult {
    Ok(Json(Value::Array(core.jobs_view()?)))
}

async fn tabs(State(core): S) -> ApiResult {
    Ok(Json(serde_json::to_value(core.browsing.tabs()?).map_err(Error::from)?))
}

async fn history(State(core): S, Query(q): Query<HashMap<String, String>>) -> ApiResult {
    let limit = q.get("limit").and_then(|v| v.parse().ok()).unwrap_or(100).clamp(1, 1000);
    Ok(Json(serde_json::to_value(core.browsing.history(limit)?).map_err(Error::from)?))
}

async fn tools(State(core): S) -> Json<Value> {
    Json(serde_json::to_value(core.refresh_tools()).unwrap_or(Value::Null))
}

async fn ytdlp_install(State(core): S) -> Response {
    run_tool_task(core, |core| ytdlp::install_ytdlp(core.fetcher.as_ref(), &core.paths.bin_dir, ytdlp::ytdlp_asset_name(), ytdlp::RELEASE_BASE)).await
}

async fn ytdlp_update(State(core): S) -> Response {
    run_tool_task(core, |core| ytdlp::update_ytdlp(&core.paths.bin_dir)).await
}

async fn run_tool_task(core: Arc<Core>, f: impl FnOnce(&Core) -> crate::error::Result<String> + Send + 'static) -> Response {
    let c = core.clone();
    let result = tokio::task::spawn_blocking(move || f(&c)).await;
    let tools = core.refresh_tools();
    match result {
        Ok(Ok(log)) => {
            core.wake_scheduler();
            Json(json!({"ok": true, "log": log, "tools": tools})).into_response()
        }
        Ok(Err(e)) => (StatusCode::BAD_GATEWAY, Json(json!({"ok": false, "error": e.to_string(), "log": e.to_string(), "tools": tools}))).into_response(),
        Err(e) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

// --- handlers: watch view ----------------------------------------------------------------------

async fn watch_open(State(core): S, Path(id): Path<String>, Query(q): Query<HashMap<String, String>>) -> ApiResult {
    let audio = q.get("audio").is_some_and(|v| v == "1");
    let c = core.clone();
    let out = tokio::task::spawn_blocking(move || c.watch_open(&id, audio)).await.map_err(|e| Error::other(e.to_string()))??;
    Ok(Json(out))
}

async fn watch_status(State(core): S, Path(id): Path<String>) -> ApiResult {
    if !crate::util::valid_video_id(&id) {
        return Err(Error::bad("invalid video id").into());
    }
    Ok(Json(core.watch_status(&id)))
}

async fn comments_more(State(core): S, Path(id): Path<String>, Query(q):Query<HashMap<String,String>>) -> ApiResult {
    if !crate::util::valid_video_id(&id) {
        return Err(Error::bad("invalid video id").into());
    }
    let c = core.clone();
    let id2 = id.clone();
    let refresh=q.get("refresh").is_some_and(|v|v=="1");
    let stored = tokio::task::spawn_blocking(move || if refresh {c.fetch_watch(&id2).map(|f|f.comments)} else {c.load_more_comments(&id2)}).await.map_err(|e| Error::other(e.to_string()))??;
    Ok(Json(json!({"ok": true, "stored": stored, "has_more": core.has_more_comments(&id)})))
}

async fn comment_replies(State(core):S, Path((id,comment)):Path<(String,String)>, Query(q):Query<HashMap<String,String>>) -> ApiResult {
    let more=q.get("more").is_some_and(|s|s=="1");
    let result=tokio::task::spawn_blocking(move||core.load_replies(&id,&comment,more)).await.map_err(|e|Error::other(e.to_string()))??;
    Ok(Json(result))
}

async fn proxy_info(State(core): S) -> Json<Value> {
    let events = core.proxy_stats().map(|s| s.recent_events(60)).unwrap_or_default();
    Json(json!({
        "info": core.proxy_info(),
        "stats": core.proxy_stats().map(|s| s.snapshot()),
        "events": events,
        "ca_pem": core.ca().map(|c| c.cert_pem.clone()),
    }))
}

async fn captures(State(core): S, Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    let limit = q.get("limit").and_then(|v| v.parse().ok()).unwrap_or(50usize).min(200);
    Json(serde_json::to_value(core.recent_captures(limit)).unwrap_or(Value::Null))
}

async fn setup_state(State(core): S) -> Json<Value> {
    Json(json!({"setup": core.setup_state(), "tools": core.tools()}))
}

// --- handlers: stream feeder and HLS (token in the path: players follow relative URLs) --------------

async fn feeder(State(core): S, Path((token, video, index)): Path<(String, String, String)>, headers: HeaderMap) -> Response {
    if !core.check_token(&token) {
        return json_error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    if core.is_offline() { return json_error(StatusCode::SERVICE_UNAVAILABLE, "Offline mode is on"); }
    let Some(source) = core.source(&format!("{video}/{index}")) else {
        return json_error(StatusCode::NOT_FOUND, "no such stream");
    };
    let len = source.len;
    let (start, end, partial) = match headers.get(header::RANGE).and_then(|v| v.to_str().ok()).and_then(parse_range) {
        Some((s, e)) if s < len => (s, e.unwrap_or(len - 1).min(len - 1), true),
        Some(_) => return json_error(StatusCode::RANGE_NOT_SATISFIABLE, "range not satisfiable"),
        None => (0, len.saturating_sub(1), false),
    };
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(4);
    std::thread::spawn(move || {
        let mut send = |chunk: Vec<u8>| !core.is_offline() && tx.blocking_send(Ok(Bytes::from(chunk))).is_ok();
        if let Err(e) = crate::stream::pump_range(&source, start, end, &mut send) {
            if e != "consumer closed" {
                let _ = tx.blocking_send(Err(std::io::Error::other(e)));
            }
        }
    });
    let stream = futures_util::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|item| (item, rx)) });
    let mut resp = Response::builder()
        .status(if partial { StatusCode::PARTIAL_CONTENT } else { StatusCode::OK })
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, end - start + 1);
    if partial {
        resp = resp.header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}"));
    }
    resp.body(Body::from_stream(stream)).unwrap_or_else(|_| json_error(StatusCode::INTERNAL_SERVER_ERROR, "response"))
}

fn parse_range(v: &str) -> Option<(u64, Option<u64>)> {
    let spec = v.trim().strip_prefix("bytes=")?;
    let (a, b) = spec.split_once('-')?;
    Some((a.trim().parse().ok()?, if b.trim().is_empty() { None } else { Some(b.trim().parse().ok()?) }))
}

async fn hls_file(State(core): S, Path((token, dir, file)): Path<(String, String, String)>, method: Method, headers: HeaderMap) -> Response {
    if !core.check_token(&token) {
        return json_error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    let dir_ok = dir.len() >= 12 && dir.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')) && !dir.contains("..");
    let content_type = match file.as_str() {
        "index.m3u8" => "application/vnd.apple.mpegurl",
        "init.mp4" => "video/mp4",
        f if f.starts_with("seg_") && f.ends_with(".m4s") && f[4..f.len() - 4].bytes().all(|b| b.is_ascii_digit()) => "video/iso.segment",
        _ => return json_error(StatusCode::NOT_FOUND, "no such file"),
    };
    if !dir_ok {
        return json_error(StatusCode::BAD_REQUEST, "bad directory");
    }
    let path = core.paths.cache_dir.join("hls").join(&dir).join(&file);
    let mut resp = if file=="index.m3u8" {
        match tokio::fs::read_to_string(path).await {
            Ok(playlist)=>{
                let playlist=crate::stream::recorded_playlist_start(&playlist);
                Response::builder().header(header::CONTENT_TYPE,content_type).header(header::CONTENT_LENGTH,playlist.len())
                    .body(if method==Method::HEAD{Body::empty()}else{Body::from(playlist)}).unwrap()
            }
            Err(error) if error.kind()==std::io::ErrorKind::NotFound=>json_error(StatusCode::NOT_FOUND,"no such playlist"),
            Err(_)=>json_error(StatusCode::INTERNAL_SERVER_ERROR,"could not read playlist"),
        }
    } else {range::serve(&method, &headers, range::MediaFile::complete(path, content_type)).await};
    if file == "index.m3u8" {
        resp.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    core.log_event("info","hls.served",json!({"directory":dir,"file":file,"status":resp.status().as_u16(),"method":method.as_str(),"content_type":content_type,
        "range":headers.get(header::RANGE).and_then(|h|h.to_str().ok()),"length":resp.headers().get(header::CONTENT_LENGTH).and_then(|h|h.to_str().ok())}));
    resp
}

// --- handlers: media and thumbnails --------------------------------------------------------------

async fn media(State(core): S, Path(id): Path<String>, method: Method, headers: HeaderMap) -> Response {
    let response=match core.media_file(&id) {
        Ok(file) => range::serve(&method, &headers, file).await,
        Err(e) => ApiError(e).into_response(),
    };
    core.log_event("info","media.served",json!({"entry_id":id,"method":method.as_str(),"status":response.status().as_u16(),"range":headers.get(header::RANGE).and_then(|h|h.to_str().ok()),"content_type":response.headers().get(header::CONTENT_TYPE).and_then(|h|h.to_str().ok()),"content_length":response.headers().get(header::CONTENT_LENGTH).and_then(|h|h.to_str().ok())}));
    response
}

async fn thumb(State(core): S, Path(id): Path<String>) -> Response {
    if !crate::util::valid_video_id(&id) {
        return json_error(StatusCode::BAD_REQUEST, "invalid video id");
    }
    match core.thumb_bytes(&id) {
        Some(bytes) => Response::builder()
            .header(header::CONTENT_TYPE, "image/jpeg")
            .header(header::CACHE_CONTROL, "private, max-age=86400")
            .body(Body::from(bytes))
            .unwrap(),
        None => json_error(StatusCode::NOT_FOUND, "thumbnail not cached yet"),
    }
}

// --- running it ----------------------------------------------------------------------------------------

/// The service: core state plus the HTTP server on its own runtime.
pub struct Service {
    pub core: Arc<Core>,
    pub addr: SocketAddr,
    pub proxy: Option<crate::proxy::Proxy>,
    _runtime: tokio::runtime::Runtime,
}

impl Service {
    /// Opens the stores, starts the scheduler and serves HTTP on 127.0.0.1.
    /// `port` 0 means the default port, falling back to a random free one.
    pub fn start(mut config: CoreConfig, port: u16) -> crate::error::Result<Service> {
        config.spawn_workers = true;
        let core = Core::open(config)?;
        let wanted = if port == 0 { DEFAULT_PORT } else { port };
        let listener = TcpListener::bind(("127.0.0.1", wanted)).or_else(|_| TcpListener::bind(("127.0.0.1", 0)))?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        core.set_port(addr.port());
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().thread_name("muninn-http").build()?;
        let app = router(core.clone());
        runtime.spawn(async move {
            let listener = tokio::net::TcpListener::from_std(listener).expect("listener");
            if let Err(e) = axum::serve(listener, app).await {
                eprintln!("muninn: http server stopped: {e}");
            }
        });
        let proxy = match crate::proxy::Proxy::spawn(core.clone(), runtime.handle(), crate::proxy::ProxyConfig { ca_dir: core.paths.data_dir.join("ca"), ..Default::default() }) {
            Ok(p) => Some(p),
            Err(e) => {
                eprintln!("muninn: proxy could not start: {e}");
                None
            }
        };
        Ok(Service { core, addr, proxy, _runtime: runtime })
    }

    /// `http://127.0.0.1:<port>` of the capture proxy the YouTube web views use.
    pub fn proxy_url(&self) -> Option<String> {
        self.proxy.as_ref().map(|p| format!("http://127.0.0.1:{}", p.addr.port()))
    }

    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.addr.port())
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.core.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::testutil::test_core;
    use crate::videodb::{EntryRow, STATUS_COMPLETE};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    struct Harness {
        _dir: tempfile::TempDir,
        core: Arc<Core>,
        app: Router,
    }

    fn harness() -> Harness {
        let (dir, core) = test_core(None, None);
        let app = router(core.clone());
        Harness { _dir: dir, core, app }
    }

    impl Harness {
        async fn send(&self, req: axum::http::Request<Body>) -> (StatusCode, HeaderMap, Vec<u8>) {
            let resp = self.app.clone().oneshot(req).await.unwrap();
            let (parts, body) = resp.into_parts();
            (parts.status, parts.headers, body.collect().await.unwrap().to_bytes().to_vec())
        }

        fn req(&self, method: &str, uri: &str, token: Option<&str>, body: &str) -> axum::http::Request<Body> {
            let mut b = axum::http::Request::builder().method(method).uri(uri).header(header::HOST, "127.0.0.1:47871");
            if let Some(t) = token {
                b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
            }
            if !body.is_empty() {
                b = b.header(header::CONTENT_TYPE, "application/json");
            }
            b.body(Body::from(body.to_string())).unwrap()
        }

        async fn json(&self, method: &str, uri: &str, body: &str) -> (StatusCode, Value) {
            let token = self.core.token();
            let (s, _, b) = self.send(self.req(method, uri, Some(&token), body)).await;
            (s, serde_json::from_slice(&b).unwrap_or(Value::Null))
        }
    }

    #[tokio::test]
    async fn ingest_requires_the_token() {
        let h = harness();
        let body = r#"{"kind":"home","items":[{"video_id":"aaaaaaaaaaa","title":"A"}]}"#;
        // No token, wrong token, truncated token.
        for token in [None, Some("wrong"), Some(&h.core.token()[..63])] {
            let (s, _, b) = h.send(h.req("POST", "/api/ingest/feed", token, body)).await;
            assert_eq!(s, StatusCode::UNAUTHORIZED, "{token:?}");
            assert!(String::from_utf8_lossy(&b).contains("unauthorized"));
        }
        assert!(h.core.content.list_snapshots(None, 10).unwrap().is_empty(), "nothing may be stored without auth");
        // Right token: Bearer, X-Muninn-Token and ?token= all work.
        let t = h.core.token();
        let (s, _, _) = h.send(h.req("POST", "/api/ingest/feed", Some(&t), body)).await;
        assert_eq!(s, StatusCode::OK);
        let mut r = h.req("POST", "/api/ingest/feed", None, body);
        r.headers_mut().insert("x-muninn-token", t.parse().unwrap());
        assert_eq!(h.send(r).await.0, StatusCode::OK);
        let (s, _, _) = h.send(h.req("POST", &format!("/api/ingest/feed?token={t}"), None, body)).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(h.core.content.list_snapshots(None, 10).unwrap().len(), 3);
    }

    #[tokio::test]
    async fn feed_refresh_uses_desktop_hook_and_client_errors_are_redacted() {
        let h = harness();
        assert_eq!(h.json("POST","/api/feeds/refresh",r#"{"kind":"subscriptions"}"#).await.0,StatusCode::BAD_REQUEST);
        let seen = Arc::new(std::sync::Mutex::new(String::new()));
        let seen_hook = seen.clone();
        *h.core.feed_refresh_hook.write().unwrap() = Some(Arc::new(move |kind| { *seen_hook.lock().unwrap() = kind.into(); Ok(()) }));
        let (status,result) = h.json("POST","/api/feeds/refresh",r#"{"kind":"subscriptions"}"#).await;
        assert_eq!(status,StatusCode::OK);
        assert_eq!(result["queued"],true);
        assert_eq!(*seen.lock().unwrap(),"subscriptions");
        assert_eq!(h.json("POST","/api/feeds/refresh",r#"{"kind":"https://evil.test"}"#).await.0,StatusCode::BAD_REQUEST);
        let message = json!({"event":"ui.error","message":format!("failed https://cdn.test/media?sig=private {}",h.core.token())});
        assert_eq!(h.json("POST","/api/logs",&message.to_string()).await.0,StatusCode::OK);
        let logs = h.core.logs(1).to_string();
        assert!(logs.contains("ui.error"));
        assert!(!logs.contains("sig=private"));
        assert!(!logs.contains(&h.core.token()));
    }

    #[tokio::test]
    async fn health_and_ui_are_open_everything_else_is_not() {
        let h = harness();
        assert_eq!(h.send(h.req("GET", "/health", None, "")).await.0, StatusCode::OK);
        let (s, headers, body) = h.send(h.req("GET", "/ui/", None, "")).await;
        assert_eq!(s, StatusCode::OK);
        assert!(headers[header::CONTENT_TYPE].to_str().unwrap().starts_with("text/html"));
        assert!(String::from_utf8_lossy(&body).to_lowercase().contains("<html"));
        assert_eq!(h.send(h.req("GET", "/ui/app.js", None, "")).await.0, StatusCode::OK);
        let (font_status,font_headers,font_bytes)=h.send(h.req("GET","/ui/Cinzel.ttf",None,"")).await;
        assert_eq!(font_status,StatusCode::OK);
        assert_eq!(font_headers[header::CONTENT_TYPE],"font/ttf");
        assert!(font_bytes.len()>1000);
        assert_eq!(h.send(h.req("GET", "/ui/nope.js", None, "")).await.0, StatusCode::NOT_FOUND);
        for uri in ["/api/entries", "/api/rules", "/media/aaaaaaaaaaa", "/thumb/aaaaaaaaaaa", "/api/status", "/api/logs", "/api/automation", "/api/cleanup", "/api/subscriptions", "/api/organization"] {
            assert_eq!(h.send(h.req("GET", uri, None, "")).await.0, StatusCode::UNAUTHORIZED, "{uri}");
        }
    }

    #[tokio::test]
    async fn foreign_host_headers_are_refused() {
        let h = harness();
        let t = h.core.token();
        let mut r = h.req("GET", "/api/status", Some(&t), "");
        r.headers_mut().insert(header::HOST, "evil.example:47871".parse().unwrap());
        assert_eq!(h.send(r).await.0, StatusCode::FORBIDDEN);
        for ok in ["localhost:1234", "127.0.0.1", "[::1]:80"] {
            let mut r = h.req("GET", "/health", None, "");
            r.headers_mut().insert(header::HOST, ok.parse().unwrap());
            assert_eq!(h.send(r).await.0, StatusCode::OK, "{ok}");
        }
    }

    #[tokio::test]
    async fn preflight_and_cors() {
        let h = harness();
        let r = axum::http::Request::builder()
            .method("OPTIONS")
            .uri("/api/ingest/feed")
            .header(header::HOST, "127.0.0.1:47871")
            .header(header::ORIGIN, "moz-extension://abc")
            .header("access-control-request-headers", "authorization,content-type")
            .body(Body::empty())
            .unwrap();
        let (s, headers, _) = h.send(r).await;
        assert_eq!(s, StatusCode::NO_CONTENT);
        assert_eq!(headers[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        assert!(headers[header::ACCESS_CONTROL_ALLOW_HEADERS].to_str().unwrap().contains("authorization"));
        // Errors carry CORS too, so the extension can read the 401.
        let (_, headers, _) = h.send(h.req("GET", "/api/status", None, "")).await;
        assert_eq!(headers[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
    }

    #[tokio::test]
    async fn bad_json_is_a_400_not_a_crash() {
        let h = harness();
        let (s, v) = h.json("POST", "/api/ingest/feed", "{not json").await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(v["ok"], false);
        let (s, _) = h.json("POST", "/api/ingest/video", r#"{"video_id":"nope","title":"x"}"#).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn the_ingest_flow_end_to_end_over_http() {
        let h = harness();
        let (s, v) = h.json("POST", "/api/ingest/feed", r#"{"kind":"home","source_url":"https://www.youtube.com/","items":[{"video_id":"aaaaaaaaaaa","title":"First","channel":"Ch","duration_seconds":100}]}"#).await;
        assert_eq!((s, v["ok"].clone()), (StatusCode::OK, json!(true)));
        let (_, feed) = h.json("GET", "/api/feeds/latest?kind=home", "").await;
        assert_eq!(feed["items"][0]["title"], "First");
        assert_eq!(feed["items"][0]["thumb_url"], "/thumb/aaaaaaaaaaa");
        let (_, list) = h.json("GET", "/api/feeds?kind=home", "").await;
        assert_eq!(list.as_array().unwrap().len(), 1);
        let (s, _) = h.json("GET", "/api/feeds/latest?kind=subscriptions", "").await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        // Tab + playback.
        let (s, _) = h.json("POST", "/api/ingest/tab", r#"{"tab_id":3,"url":"https://www.youtube.com/watch?v=aaaaaaaaaaa","video_id":"aaaaaaaaaaa","state":"open"}"#).await;
        assert_eq!(s, StatusCode::OK);
        let (_, tabs) = h.json("GET", "/api/tabs", "").await;
        assert_eq!(tabs[0]["video_id"], "aaaaaaaaaaa");
        let (_, p) = h.json("POST", "/api/ingest/playback", r#"{"video_id":"aaaaaaaaaaa","position_seconds":12.5,"duration_seconds":100,"watch_height":720}"#).await;
        assert_eq!(p["first_play"], true);
    }

    #[tokio::test]
    async fn rules_and_settings_roundtrip() {
        let h = harness();
        let (_, rules) = h.json("GET", "/api/rules", "").await;
        assert_eq!(rules.as_array().unwrap().len(), 5);
        let mut edited = rules.clone();
        edited[0]["retention_days"] = json!(9);
        edited.as_array_mut().unwrap().push(json!({"name":"New","trigger":"manual","quality":"480","retention_days":2,"match":{"title_keywords":["x"]}}));
        let (s, saved) = h.json("PUT", "/api/rules", &edited.to_string()).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(saved.as_array().unwrap().len(), 6);
        assert!(!saved[5]["id"].as_str().unwrap().is_empty());
        assert_eq!(h.json("GET", "/api/rules", "").await.1[0]["retention_days"], 9);
        let (s, _) = h.json("PUT", "/api/rules", r#"[{"name":"","trigger":"manual","quality":"360","retention_days":1}]"#).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        // Settings merge: a partial body keeps the other values.
        let (s, st) = h.json("PUT", "/api/settings", r#"{"download_concurrency":3,"min_free_bytes":5000000000}"#).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!((st["download_concurrency"].clone(), st["comments_ttl_hours"].clone()), (json!(3), json!(24)));
        assert_eq!(h.json("GET", "/api/settings", "").await.1["min_free_bytes"], 5_000_000_000u64);
        let (_, disk) = h.json("GET", "/api/disk", "").await;
        assert!(disk["max_disk_bytes"].as_u64().unwrap() > 0);
    }

    fn seed_media(h: &Harness, len: usize) -> String {
        let path = h.core.paths.video_dir.join("aaaaaaaaaaa.360.mp4");
        std::fs::write(&path, (0..len).map(|i| (i % 251) as u8).collect::<Vec<_>>()).unwrap();
        let mut e = EntryRow::new("aaaaaaaaaaa", "360", "360", "manual", 1);
        e.status = STATUS_COMPLETE.into();
        e.file_path = Some(path.display().to_string());
        e.ext = Some("mp4".into());
        e.size_bytes = len as u64;
        e.actual_height = Some(360);
        h.core.video.insert_entry(&e).unwrap();
        e.id
    }

    #[tokio::test]
    async fn media_endpoint_serves_ranges_with_the_token() {
        let h = harness();
        let id = seed_media(&h, 5000);
        let t = h.core.token();
        let mut r = h.req("GET", &format!("/media/{id}?token={t}"), None, "");
        r.headers_mut().insert(header::RANGE, "bytes=100-199".parse().unwrap());
        let (s, headers, body) = h.send(r).await;
        assert_eq!(s, StatusCode::PARTIAL_CONTENT);
        assert_eq!(headers[header::CONTENT_RANGE], "bytes 100-199/5000");
        assert_eq!(body, (100..200).map(|i| (i % 251) as u8).collect::<Vec<_>>());
        assert_eq!(headers[header::CONTENT_TYPE], "video/mp4");
        // A bare video id resolves to the best cached variant.
        let (s, _, body) = h.send(h.req("GET", &format!("/media/aaaaaaaaaaa?token={t}"), None, "")).await;
        assert_eq!((s, body.len()), (StatusCode::OK, 5000));
        let (s, _, _) = h.send(h.req("GET", &format!("/media/bbbbbbbbbbb?token={t}"), None, "")).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        let (s, _, _) = h.send(h.req("GET", &format!("/media/..%2F..%2Fetc%2Fpasswd?token={t}"), None, "")).await;
        assert!(s == StatusCode::BAD_REQUEST || s == StatusCode::NOT_FOUND);
        // Entry actions through the API.
        let (s, v) = h.json("POST", &format!("/api/entries/{id}/promote"), "").await;
        assert_eq!((s, v["entry"]["state"].clone()), (StatusCode::OK, json!("protected")));
        let (s, _) = h.json("POST", &format!("/api/entries/{id}/evict"), "").await;
        assert_eq!(s, StatusCode::CONFLICT);
        let (s, _) = h.json("POST", &format!("/api/entries/{id}/evict?force=1"), "").await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(h.json("GET", "/api/entries", "").await.1, json!([]));
    }

    #[tokio::test]
    async fn cache_and_open_endpoints() {
        let h = harness();
        let (s, v) = h.json("POST", "/api/cache", r#"{"video_id":"aaaaaaaaaaa","quality":"audio","title":"T"}"#).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["entry"]["id"], "aaaaaaaaaaa.audio");
        assert_eq!(v["entry"]["status"], "queued");
        let (_, v) = h.json("POST", "/api/cache", r#"{"video_id":"bbbbbbbbbbb"}"#).await;
        assert!(v["entry"].is_null(), "the catch-all rule caches nothing on request");
        let (s, v) = h.json("POST", "/api/open", r#"{"video_id":"ccccccccccc"}"#).await;
        assert_eq!((s, v["entry"]["id"].clone()), (StatusCode::OK, json!("ccccccccccc.360")));
        let (_, p) = h.json("GET", "/api/open/pending?after=0", "").await;
        assert_eq!(p["items"][0]["video_id"], "ccccccccccc");
        let (_, jobs) = h.json("GET", "/api/jobs", "").await;
        assert_eq!(jobs.as_array().unwrap().len(), 2);
        let (_, entries) = h.json("GET", "/api/entries?state=partial", "").await;
        assert_eq!(entries.as_array().unwrap().len(), 2);
    }
}
