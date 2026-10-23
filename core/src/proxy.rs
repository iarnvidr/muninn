//! The in-process HTTP CONNECT proxy that the YouTube web views are pointed at.
//!
//! It exists for three things and listens on 127.0.0.1 only:
//!
//! * **Capture.** `*.youtube.com` (except the sign-in host) is decrypted with a
//!   certificate from Muninn's own CA (see `ca`). The `youtubei/v1` JSON and the
//!   page HTML are copied to the content cache *while they stream to the page*:
//!   the page never waits for the copy, and anything that goes wrong in the copy
//!   is dropped silently.
//! * **Blocking.** `*.googlevideo.com` CONNECTs are refused. Video only ever
//!   plays in Muninn's own player, so YouTube's player downloads nothing.
//! * **Everything else is a plain TCP tunnel**, untouched: Google sign-in
//!   (`accounts.google.com`, `accounts.youtube.com`), images, fonts, ads.

use crate::ca::Ca;
use crate::capture::CaptureInput;
use crate::engine::Core;
use crate::error::{Error, Result};
use bytes::Bytes;
use http_body_util::{combinators::UnsyncBoxBody, BodyExt, Full};
use hyper::body::{Body, Frame, Incoming, SizeHint};
use hyper::header::{HeaderMap, HeaderName, HeaderValue};
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode, Uri};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio_rustls::TlsAcceptor;

type BoxErr = Box<dyn std::error::Error + Send + Sync>;
type RespBody = UnsyncBoxBody<Bytes, BoxErr>;

/// Largest response copied for capture. Bigger responses stream through untouched.
const MAX_CAPTURE_BYTES: usize = 24 * 1024 * 1024;
const MAX_REQUEST_BODY: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HostClass {
    /// Decrypt and inspect.
    Mitm,
    /// Refuse (video bytes).
    Block,
    /// Plain TCP tunnel.
    Tunnel,
}

fn host_is(host: &str, domain: &str) -> bool {
    host == domain || host.strip_suffix(domain).is_some_and(|p| p.ends_with('.'))
}

pub fn classify_host(host: &str) -> HostClass {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host_is(&host, "googlevideo.com") {
        return HostClass::Block;
    }
    // The sign-in flow on youtube.com's domain stays untouched.
    if host_is(&host, "accounts.youtube.com") {
        return HostClass::Tunnel;
    }
    if host_is(&host, "youtube.com") {
        return HostClass::Mitm;
    }
    HostClass::Tunnel
}

#[derive(Debug, Clone, Serialize)]
pub struct ProxyEvent {
    pub ts: i64,
    pub kind: &'static str,
    pub host: String,
    pub detail: String,
    pub status: Option<u16>,
    pub bytes: Option<u64>,
}

#[derive(Default)]
pub struct ProxyStats {
    pub connects: AtomicU64,
    pub mitm_connects: AtomicU64,
    pub tunnels: AtomicU64,
    pub blocked: AtomicU64,
    pub requests: AtomicU64,
    pub captured: AtomicU64,
    pub captured_bytes: AtomicU64,
    pub tls_failures: AtomicU64,
    /// Subset of `tls_failures` where the client rejected our certificate
    /// (the CA is not trusted yet).
    pub tls_untrusted: AtomicU64,
    pub tls_ok: AtomicU64,
    /// Bytes of googlevideo traffic that reached the network. Always 0 by design.
    pub media_bytes_fetched: AtomicU64,
    hosts: Mutex<HashMap<String, u64>>,
    events: Mutex<VecDeque<ProxyEvent>>,
}

impl ProxyStats {
    fn bump_host(&self, host: &str) {
        let mut h = self.hosts.lock().unwrap_or_else(|e| e.into_inner());
        if h.len() < 500 || h.contains_key(host) {
            *h.entry(host.to_string()).or_default() += 1;
        }
    }

    fn event(&self, kind: &'static str, host: &str, detail: String, status: Option<u16>, bytes: Option<u64>) {
        let ev = ProxyEvent { ts: crate::util::now_ms(), kind, host: host.to_string(), detail, status, bytes };
        if std::env::var_os("MUNINN_PROXY_LOG").is_some() {
            eprintln!("muninn-proxy {} {} {} {:?} {:?}", ev.kind, ev.host, ev.detail, ev.status, ev.bytes);
        }
        let mut q = self.events.lock().unwrap_or_else(|e| e.into_inner());
        q.push_back(ev);
        while q.len() > 300 {
            q.pop_front();
        }
    }

    pub fn recent_events(&self, limit: usize) -> Vec<ProxyEvent> {
        self.events.lock().unwrap_or_else(|e| e.into_inner()).iter().rev().take(limit).cloned().collect()
    }

    pub fn snapshot(&self) -> serde_json::Value {
        let g = |a: &AtomicU64| a.load(Ordering::Relaxed);
        let mut hosts: Vec<(String, u64)> = self.hosts.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(k, v)| (k.clone(), *v)).collect();
        hosts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        hosts.truncate(40);
        serde_json::json!({
            "connects": g(&self.connects), "mitm_connects": g(&self.mitm_connects), "tunnels": g(&self.tunnels),
            "blocked": g(&self.blocked), "requests": g(&self.requests), "captured": g(&self.captured),
            "captured_bytes": g(&self.captured_bytes), "tls_ok": g(&self.tls_ok), "tls_failures": g(&self.tls_failures),
            "tls_untrusted": g(&self.tls_untrusted), "media_bytes_fetched": g(&self.media_bytes_fetched),
            "hosts": hosts.into_iter().map(|(h, n)| serde_json::json!({"host": h, "count": n})).collect::<Vec<_>>(),
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ProxyInfo {
    pub port: u16,
    pub url: String,
    pub ca_pem_path: std::path::PathBuf,
}

#[derive(Default, Clone)]
pub struct ProxyConfig {
    pub ca_dir: std::path::PathBuf,
    /// Tests: send every upstream connection to this address instead of resolving the host.
    pub upstream_override: Option<SocketAddr>,
    /// Tests: extra trusted roots (DER) for the upstream TLS connection.
    pub extra_roots: Vec<Vec<u8>>,
}

pub struct Proxy {
    pub addr: SocketAddr,
    pub stats: Arc<ProxyStats>,
    pub ca: Arc<Ca>,
}

struct State {
    core: Arc<Core>,
    ca: Arc<Ca>,
    stats: Arc<ProxyStats>,
    client: Client<hyper_rustls::HttpsConnector<HttpConnector<Resolver>>, RespBody>,
    /// Plain HTTP to loopback only (a web view that shares this proxy and talks to Muninn's own server).
    local: Client<HttpConnector, RespBody>,
    upstream_override: Option<SocketAddr>,
}

/// DNS resolver that can pin every host to one address (tests).
#[derive(Clone)]
struct Resolver(Option<SocketAddr>);

impl tower_service::Service<hyper_util::client::legacy::connect::dns::Name> for Resolver {
    type Response = std::vec::IntoIter<SocketAddr>;
    type Error = std::io::Error;
    type Future = Pin<Box<dyn std::future::Future<Output = std::io::Result<Self::Response>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<std::result::Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, name: hyper_util::client::legacy::connect::dns::Name) -> Self::Future {
        let pinned = self.0;
        Box::pin(async move {
            if let Some(addr) = pinned {
                return Ok(vec![addr].into_iter());
            }
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((name.as_str(), 0)).await?.collect();
            Ok(addrs.into_iter())
        })
    }
}

fn boxed_full(bytes: impl Into<Bytes>) -> RespBody {
    Full::new(bytes.into()).map_err(|never| match never {}).boxed_unsync()
}

fn text_response(status: StatusCode, text: &str) -> Response<RespBody> {
    let mut r = Response::new(boxed_full(text.to_string()));
    *r.status_mut() = status;
    r.headers_mut().insert(hyper::header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
    r.headers_mut().insert(hyper::header::CONNECTION, HeaderValue::from_static("close"));
    r
}

impl Proxy {
    /// Binds 127.0.0.1:<random> and serves on `rt`.
    pub fn spawn(core: Arc<Core>, rt: &tokio::runtime::Handle, cfg: ProxyConfig) -> Result<Proxy> {
        let ca = Arc::new(Ca::load_or_create(&cfg.ca_dir)?);
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let stats = Arc::new(ProxyStats::default());

        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut roots = rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        for der in &cfg.extra_roots {
            roots.add(rustls::pki_types::CertificateDer::from(der.clone())).map_err(|e| Error::other(e.to_string()))?;
        }
        let tls = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| Error::other(e.to_string()))?
            .with_root_certificates(roots)
            .with_no_client_auth();
        let mut http = HttpConnector::new_with_resolver(Resolver(cfg.upstream_override));
        http.enforce_http(false);
        http.set_connect_timeout(Some(std::time::Duration::from_secs(15)));
        http.set_nodelay(true);
        let https = hyper_rustls::HttpsConnectorBuilder::new().with_tls_config(tls).https_only().enable_http1().wrap_connector(http);
        let client = Client::builder(TokioExecutor::new()).pool_max_idle_per_host(8).build(https);

        let local = Client::builder(TokioExecutor::new()).build(HttpConnector::new());
        let state = Arc::new(State { core: core.clone(), ca: ca.clone(), stats: stats.clone(), client, local, upstream_override: cfg.upstream_override });
        rt.spawn(async move {
            let listener = match tokio::net::TcpListener::from_std(listener) {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("muninn: proxy listener: {e}");
                    return;
                }
            };
            loop {
                let Ok((stream, _)) = listener.accept().await else { continue };
                let _ = stream.set_nodelay(true);
                let state = state.clone();
                tokio::spawn(async move {
                    let io = TokioIo::new(stream);
                    let svc = service_fn(move |req| handle_outer(state.clone(), req));
                    let _ = hyper::server::conn::http1::Builder::new().preserve_header_case(false).serve_connection(io, svc).with_upgrades().await;
                });
            }
        });
        *core.proxy_info.write().unwrap_or_else(|e| e.into_inner()) =
            Some(ProxyInfo { port: addr.port(), url: format!("http://127.0.0.1:{}", addr.port()), ca_pem_path: Ca::cert_path(&cfg.ca_dir) });
        core.set_proxy(stats.clone(), ca.clone());
        Ok(Proxy { addr, stats, ca })
    }
}

fn split_authority(uri: &Uri) -> Option<(String, u16)> {
    let a = uri.authority()?;
    Some((a.host().trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase(), a.port_u16().unwrap_or(443)))
}

async fn handle_outer(state: Arc<State>, req: Request<Incoming>) -> std::result::Result<Response<RespBody>, Infallible> {
    if state.core.is_offline() && req.method() == Method::CONNECT {
        return Ok(text_response(StatusCode::SERVICE_UNAVAILABLE, "Muninn offline mode"));
    }
    if req.method() != Method::CONNECT {
        // Only plain HTTP to this machine is forwarded (Muninn's own UI server); the rest is refused.
        let local_host = req.uri().host().is_some_and(|h| matches!(h, "127.0.0.1" | "localhost" | "[::1]" | "::1"));
        if local_host {
            let (parts, body) = req.into_parts();
            let mut up = Request::new(body.map_err(|e| Box::new(e) as BoxErr).boxed_unsync());
            *up.method_mut() = parts.method;
            *up.uri_mut() = parts.uri;
            *up.headers_mut() = parts.headers;
            return Ok(match state.local.request(up).await {
                Ok(r) => {
                    let (p, b) = r.into_parts();
                    Response::from_parts(p, b.map_err(|e| Box::new(e) as BoxErr).boxed_unsync())
                }
                Err(e) => text_response(StatusCode::BAD_GATEWAY, &format!("Muninn proxy: local forward failed: {e}")),
            });
        }
        return Ok(text_response(StatusCode::NOT_IMPLEMENTED, "Muninn proxy: HTTPS only"));
    }
    let Some((host, port)) = split_authority(req.uri()) else {
        return Ok(text_response(StatusCode::BAD_REQUEST, "bad CONNECT target"));
    };
    state.stats.connects.fetch_add(1, Ordering::Relaxed);
    state.stats.bump_host(&host);
    match classify_host(&host) {
        HostClass::Block => {
            state.stats.blocked.fetch_add(1, Ordering::Relaxed);
            state.stats.event("blocked", &host, "CONNECT refused (video stays in Muninn's player)".into(), Some(403), None);
            Ok(text_response(StatusCode::FORBIDDEN, "blocked by Muninn: video plays in Muninn's own player"))
        }
        HostClass::Tunnel => {
            let target = match state.upstream_override {
                Some(a) => TcpStream::connect(a).await,
                None => TcpStream::connect((host.as_str(), port)).await,
            };
            let mut upstream = match target {
                Ok(s) => s,
                Err(e) => return Ok(text_response(StatusCode::BAD_GATEWAY, &format!("cannot reach {host}: {e}"))),
            };
            let _ = upstream.set_nodelay(true);
            state.stats.tunnels.fetch_add(1, Ordering::Relaxed);
            state.stats.event("tunnel", &host, format!(":{port}"), None, None);
            let on_upgrade = hyper::upgrade::on(req);
            tokio::spawn(async move {
                if let Ok(upgraded) = on_upgrade.await {
                    let mut client = TokioIo::new(upgraded);
                    tokio::select! {
                        _ = tokio::io::copy_bidirectional(&mut client, &mut upstream) => {},
                        _ = async { while !state.core.is_offline() { tokio::time::sleep(std::time::Duration::from_millis(100)).await; } } => {},
                    }
                    let _ = upstream.shutdown().await;
                }
            });
            Ok(Response::new(boxed_full(Bytes::new())))
        }
        HostClass::Mitm => {
            state.stats.mitm_connects.fetch_add(1, Ordering::Relaxed);
            let config = match state.ca.server_config(&host) {
                Ok(c) => c,
                Err(e) => return Ok(text_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())),
            };
            let on_upgrade = hyper::upgrade::on(req);
            let st = state.clone();
            tokio::spawn(async move {
                let Ok(upgraded) = on_upgrade.await else { return };
                let acceptor = TlsAcceptor::from(config);
                match acceptor.accept(TokioIo::new(upgraded)).await {
                    Ok(tls) => {
                        st.stats.tls_ok.fetch_add(1, Ordering::Relaxed);
                        let svc_state = st.clone();
                        let host2 = host.clone();
                        let svc = service_fn(move |r| handle_mitm(svc_state.clone(), host2.clone(), port, r));
                        let _ = hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(tls), svc).await;
                    }
                    Err(e) => {
                        st.stats.tls_failures.fetch_add(1, Ordering::Relaxed);
                        let text = e.to_string();
                        // The client aborts the handshake with an alert when it does not trust the CA.
                        if text.contains("UnknownCA") || text.contains("BadCertificate") || text.contains("CertificateUnknown") || text.contains("UnknownIssuer") {
                            st.stats.tls_untrusted.fetch_add(1, Ordering::Relaxed);
                        }
                        st.stats.event("tls_fail", &host, text, None, None);
                    }
                }
            });
            Ok(Response::new(boxed_full(Bytes::new())))
        }
    }
}

const HOP_BY_HOP: &[&str] = &["connection", "keep-alive", "proxy-authenticate", "proxy-authorization", "proxy-connection", "te", "trailer", "transfer-encoding", "upgrade"];

fn strip_hop_by_hop(h: &mut HeaderMap) {
    let listed: Vec<String> = h
        .get_all(hyper::header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(',').map(|s| s.trim().to_ascii_lowercase()).collect::<Vec<_>>())
        .collect();
    for name in HOP_BY_HOP {
        h.remove(*name);
    }
    for name in listed {
        if let Ok(n) = HeaderName::from_bytes(name.as_bytes()) {
            h.remove(n);
        }
    }
}

fn is_api_post(method: &Method, path: &str) -> bool {
    method == Method::POST && path.starts_with("/youtubei/v1/")
}

fn wants_html(method: &Method, headers: &HeaderMap) -> bool {
    method == Method::GET && headers.get(hyper::header::ACCEPT).and_then(|v| v.to_str().ok()).is_some_and(|a| a.contains("text/html"))
}

async fn handle_mitm(state: Arc<State>, host: String, port: u16, req: Request<Incoming>) -> std::result::Result<Response<RespBody>, Infallible> {
    if state.core.is_offline() {
        return Ok(text_response(StatusCode::SERVICE_UNAVAILABLE, "Muninn offline mode"));
    }
    state.stats.requests.fetch_add(1, Ordering::Relaxed);
    let (mut parts, body) = req.into_parts();
    let path_and_query = parts.uri.path_and_query().map(|p| p.as_str().to_string()).unwrap_or_else(|| "/".into());
    let path = parts.uri.path().to_string();
    let api = is_api_post(&parts.method, &path);
    let html = wants_html(&parts.method, &parts.headers);
    let authority = if port == 443 { host.clone() } else { format!("{host}:{port}") };
    let uri: Uri = match format!("https://{authority}{path_and_query}").parse() {
        Ok(u) => u,
        Err(_) => return Ok(text_response(StatusCode::BAD_REQUEST, "bad request target")),
    };
    strip_hop_by_hop(&mut parts.headers);
    let referer = parts.headers.get(hyper::header::REFERER).and_then(|v| v.to_str().ok()).map(str::to_string);
    let capture_worthy = api || html;
    if capture_worthy {
        // Ask for gzip only: the copy is decoded with flate2, the page gets the same bytes.
        parts.headers.insert(hyper::header::ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
    }
    let mut request_body_copy: Option<Bytes> = None;
    let up_body: RespBody = if api {
        match http_body_util::Limited::new(body, MAX_REQUEST_BODY).collect().await {
            Ok(c) => {
                let b = c.to_bytes();
                request_body_copy = Some(b.clone());
                boxed_full(b)
            }
            Err(e) => return Ok(text_response(StatusCode::BAD_REQUEST, &format!("request body: {e}"))),
        }
    } else {
        body.map_err(|e| Box::new(e) as BoxErr).boxed_unsync()
    };
    let mut up_req = Request::new(up_body);
    *up_req.method_mut() = parts.method.clone();
    *up_req.uri_mut() = uri.clone();
    *up_req.headers_mut() = parts.headers;
    if let Some(b) = &request_body_copy {
        up_req.headers_mut().insert(hyper::header::CONTENT_LENGTH, HeaderValue::from(b.len()));
    }
    let resp = match state.client.request(up_req).await {
        Ok(r) => r,
        Err(e) => {
            state.stats.event("mitm_error", &host, format!("{} {}: {e}", parts.method, path), Some(502), None);
            return Ok(text_response(StatusCode::BAD_GATEWAY, &format!("Muninn proxy: upstream error: {e}")));
        }
    };
    let (mut rparts, rbody) = resp.into_parts();
    let status = rparts.status;
    strip_hop_by_hop(&mut rparts.headers);
    rparts.headers.remove("alt-svc");
    state.stats.event("mitm", &host, format!("{} {}", parts.method, path.chars().take(120).collect::<String>()), Some(status.as_u16()), None);
    let content_type = rparts.headers.get(hyper::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(str::to_string);
    let content_encoding = rparts.headers.get(hyper::header::CONTENT_ENCODING).and_then(|v| v.to_str().ok()).map(str::to_string);
    let ct_l = content_type.clone().unwrap_or_default().to_ascii_lowercase();
    let teeable = capture_worthy && status == StatusCode::OK && (ct_l.contains("json") || ct_l.contains("text/html"));
    let inner: RespBody = rbody.map_err(|e| Box::new(e) as BoxErr).boxed_unsync();
    let out_body: RespBody = if teeable {
        let st = state.clone();
        let url = format!("https://{authority}{path_and_query}");
        let on_done = Box::new(move |bytes: Vec<u8>| {
            st.stats.captured.fetch_add(1, Ordering::Relaxed);
            st.stats.captured_bytes.fetch_add(bytes.len() as u64, Ordering::Relaxed);
            let core = st.core.clone();
            // Parsing never runs on the proxy's I/O threads.
            std::thread::spawn(move || {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    core.capture(CaptureInput {
                        url: &url,
                        referer: referer.as_deref(),
                        request_body: request_body_copy.as_deref(),
                        body: &bytes,
                        content_type: content_type.as_deref(),
                        content_encoding: content_encoding.as_deref(),
                    })
                }));
            });
        });
        TeeBody { inner, buf: Vec::new(), cap: MAX_CAPTURE_BYTES, overflow: false, on_done: Some(on_done) }.boxed_unsync()
    } else {
        inner
    };
    Ok(Response::from_parts(rparts, out_body))
}

/// Passes frames through untouched and keeps a bounded copy of the data; when the
/// stream ends cleanly the copy goes to `on_done`.
struct TeeBody {
    inner: RespBody,
    buf: Vec<u8>,
    cap: usize,
    overflow: bool,
    on_done: Option<Box<dyn FnOnce(Vec<u8>) + Send>>,
}

impl TeeBody {
    fn finish(&mut self) {
        if let Some(cb) = self.on_done.take() {
            if !self.overflow {
                cb(std::mem::take(&mut self.buf));
            }
        }
    }
}

impl Body for TeeBody {
    type Data = Bytes;
    type Error = BoxErr;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<std::result::Result<Frame<Bytes>, BoxErr>>> {
        let this = &mut *self;
        match Pin::new(&mut this.inner).poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(d) = frame.data_ref() {
                    if !this.overflow {
                        if this.buf.len() + d.len() > this.cap {
                            this.overflow = true;
                            this.buf = Vec::new();
                        } else {
                            this.buf.extend_from_slice(d);
                        }
                    }
                }
                // hyper does not poll again once the body says it is finished.
                if this.inner.is_end_stream() {
                    this.finish();
                }
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Ready(None) => {
                this.finish();
                Poll::Ready(None)
            }
            other => other,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

#[cfg(test)]
mod tests;
