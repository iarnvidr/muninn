//! Proxy tests: a fake "youtube" TLS upstream (its own CA, trusted by the proxy
//! through `extra_roots`), a client that speaks CONNECT + TLS like a web view.

use super::*;
use crate::ca::Ca;
use crate::engine::testutil::test_core;
use http_body_util::StreamBody;
use std::io::Write;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn gz(bytes: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    e.write_all(bytes).unwrap();
    e.finish().unwrap()
}

fn feed_json() -> String {
    let v = |id: &str, t: &str| {
        serde_json::json!({"videoRenderer": {"videoId": id, "title": {"runs": [{"text": t}]},
            "ownerText": {"runs": [{"text": "Chan"}]}, "lengthText": {"simpleText": "1:00"}}})
    };
    serde_json::json!({"contents": {"items": [v("aaaaaaaaaaa", "One"), v("bbbbbbbbbbb", "Two")]}}).to_string()
}

struct Upstream {
    addr: SocketAddr,
    root_der: Vec<u8>,
    _dir: tempfile::TempDir,
}

/// A TLS HTTP/1.1 server for www.youtube.com with its own CA.
async fn upstream() -> Upstream {
    let dir = tempfile::tempdir().unwrap();
    let ca = Ca::load_or_create(dir.path()).unwrap();
    let config = ca.server_config("www.youtube.com").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let acceptor = TlsAcceptor::from(config);
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else { continue };
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(tcp).await else { return };
                let svc = service_fn(|req: Request<Incoming>| async move {
                    let path = req.uri().path().to_string();
                    let resp: Response<RespBody> = match path.as_str() {
                        "/youtubei/v1/browse" => {
                            let body = gz(feed_json().as_bytes());
                            Response::builder()
                                .header("content-type", "application/json")
                                .header("content-encoding", "gzip")
                                .header("alt-svc", "h3=\":443\"")
                                .body(boxed_full(body))
                                .unwrap()
                        }
                        "/" => Response::builder()
                            .header("content-type", "text/html; charset=utf-8")
                            .body(boxed_full(format!("<html><script>var ytInitialData = {};</script></html>", feed_json())))
                            .unwrap(),
                        "/slow" => {
                            // First chunk now, second one two seconds later.
                            let stream = futures_util::stream::unfold(0u8, |i| async move {
                                if i >= 2 {
                                    return None;
                                }
                                if i == 1 {
                                    tokio::time::sleep(Duration::from_millis(2000)).await;
                                }
                                Some((Ok::<_, BoxErr>(Frame::data(Bytes::from(vec![b'x'; 1000]))), i + 1))
                            });
                            Response::builder().header("content-type", "text/plain").body(StreamBody::new(stream).boxed_unsync()).unwrap()
                        }
                        _ => Response::builder().status(404).body(boxed_full("nope")).unwrap(),
                    };
                    Ok::<_, Infallible>(resp)
                });
                let _ = hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(tls), svc).await;
            });
        }
    });
    Upstream { addr, root_der: ca.cert_der().to_vec(), _dir: dir }
}

struct Harness {
    core: Arc<Core>,
    proxy: Proxy,
    up: Upstream,
    _dir: tempfile::TempDir,
    _data: tempfile::TempDir,
}

async fn harness() -> Harness {
    let (data, core) = test_core(None, None);
    let up = upstream().await;
    let dir = tempfile::tempdir().unwrap();
    let proxy = Proxy::spawn(
        core.clone(),
        &tokio::runtime::Handle::current(),
        ProxyConfig { ca_dir: dir.path().to_path_buf(), upstream_override: Some(up.addr), extra_roots: vec![up.root_der.clone()] },
    )
    .unwrap();
    Harness { core, proxy, up, _dir: dir, _data: data }
}

async fn connect(h: &Harness, host: &str) -> (u16, TcpStream) {
    let mut s = TcpStream::connect(h.proxy.addr).await.unwrap();
    s.write_all(format!("CONNECT {host}:443 HTTP/1.1\r\nHost: {host}:443\r\n\r\n").as_bytes()).await.unwrap();
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if s.read(&mut b).await.unwrap() == 0 {
            break;
        }
        head.push(b[0]);
    }
    let text = String::from_utf8_lossy(&head).to_string();
    let status = text.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    (status, s)
}

fn client_config(root: &[u8]) -> Arc<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    roots.add(rustls::pki_types::CertificateDer::from(root.to_vec())).unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    Arc::new(
        rustls::ClientConfig::builder_with_provider(provider).with_safe_default_protocol_versions().unwrap().with_root_certificates(roots).with_no_client_auth(),
    )
}

type Sender = hyper::client::conn::http1::SendRequest<RespBody>;

async fn tls_client(h: &Harness, host: &str, root: &[u8]) -> std::result::Result<Sender, String> {
    let (status, tcp) = connect(h, host).await;
    assert_eq!(status, 200, "CONNECT {host}");
    let connector = tokio_rustls::TlsConnector::from(client_config(root));
    let name = rustls::pki_types::ServerName::try_from(host.to_string()).unwrap();
    let tls = connector.connect(name, tcp).await.map_err(|e| e.to_string())?;
    let (sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(tls)).await.map_err(|e| e.to_string())?;
    tokio::spawn(async move {
        let _ = conn.await;
    });
    Ok(sender)
}

fn req(method: &str, host: &str, path: &str, body: &str, extra: &[(&str, &str)]) -> Request<RespBody> {
    let mut r = Request::builder().method(method).uri(path).header("host", host).header("accept-encoding", "gzip, deflate, br");
    for (k, v) in extra {
        r = r.header(*k, *v);
    }
    r.body(boxed_full(body.to_string())).unwrap()
}

async fn wait_for<F: Fn() -> bool>(what: &str, f: F) {
    wait_for_with(what, f, || String::new()).await
}

async fn wait_for_with<F: Fn() -> bool, D: Fn() -> String>(what: &str, f: F, dbg: D) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(5), "timed out waiting for {what}: {}", dbg());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[test]
fn host_classes() {
    use HostClass::*;
    for (h, c) in [
        ("www.youtube.com", Mitm),
        ("YouTube.com", Mitm),
        ("m.youtube.com", Mitm),
        ("rr3---sn-abc.googlevideo.com", Block),
        ("googlevideo.com", Block),
        ("accounts.google.com", Tunnel),
        ("accounts.youtube.com", Tunnel),
        ("i.ytimg.com", Tunnel),
        ("evilyoutube.com", Tunnel),
        ("youtube.com.evil.example", Tunnel),
        ("www.google.com", Tunnel),
    ] {
        assert_eq!(classify_host(h), c, "{h}");
    }
}

#[tokio::test]
async fn youtubei_is_decrypted_streamed_to_the_page_and_captured() {
    let h = harness().await;
    let ca_der = h.proxy.ca.cert_der().to_vec();
    let mut tx = tls_client(&h, "www.youtube.com", &ca_der).await.expect("the Muninn CA is trusted by this client");
    let r = tx
        .send_request(req("POST", "www.youtube.com", "/youtubei/v1/browse?prettyPrint=false", r#"{"browseId":"FEwhat_to_watch"}"#, &[("referer", "https://www.youtube.com/"), ("content-type", "application/json")]))
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.headers().get("alt-svc").is_none(), "alt-svc is stripped so the page never tries QUIC");
    assert_eq!(r.headers()["content-encoding"], "gzip", "the page gets the upstream bytes unchanged");
    let body = r.into_body().collect().await.unwrap().to_bytes();
    let plain = crate::capture::decode_body(&body, Some("gzip")).unwrap();
    assert_eq!(String::from_utf8(plain).unwrap(), feed_json());
    wait_for_with("the feed snapshot", || h.core.content.latest_snapshot("home").unwrap().is_some(), || format!("{:?} {:?}", h.core.recent_captures(3), h.proxy.stats.recent_events(5))).await;
    let snap = h.core.content.latest_snapshot("home").unwrap().unwrap();
    assert_eq!(snap.items.len(), 2);
    assert_eq!(h.proxy.stats.captured.load(Ordering::Relaxed), 1);
    assert_eq!(h.proxy.stats.tls_ok.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn page_html_is_captured_and_other_requests_pass_through() {
    let h = harness().await;
    let ca_der = h.proxy.ca.cert_der().to_vec();
    let mut tx = tls_client(&h, "www.youtube.com", &ca_der).await.unwrap();
    let r = tx.send_request(req("GET", "www.youtube.com", "/", "", &[("accept", "text/html,application/xhtml+xml")])).await.unwrap();
    let _ = r.into_body().collect().await.unwrap();
    wait_for("html capture", || h.core.content.latest_snapshot("home").unwrap().is_some()).await;
    let r = tx.send_request(req("GET", "www.youtube.com", "/nothing", "", &[])).await.unwrap();
    assert_eq!(r.status(), 404);
    assert_eq!(r.into_body().collect().await.unwrap().to_bytes(), "nope");
    assert_eq!(h.proxy.stats.captured.load(Ordering::Relaxed), 1, "a 404 is not copied");
}

#[tokio::test]
async fn the_copy_never_delays_the_page() {
    let h = harness().await;
    let ca_der = h.proxy.ca.cert_der().to_vec();
    let mut tx = tls_client(&h, "www.youtube.com", &ca_der).await.unwrap();
    let t = Instant::now();
    let r = tx.send_request(req("GET", "www.youtube.com", "/slow", "", &[("accept", "text/html")])).await.unwrap();
    let mut body = r.into_body();
    let first = body.frame().await.unwrap().unwrap();
    assert!(first.into_data().unwrap().len() > 0);
    assert!(t.elapsed() < Duration::from_millis(1200), "first chunk streams before the upstream finished: {:?}", t.elapsed());
}

#[tokio::test]
async fn video_hosts_are_refused_and_nothing_is_fetched() {
    let h = harness().await;
    let (status, _) = connect(&h, "rr1---sn-aaaa.googlevideo.com").await;
    assert_eq!(status, 403);
    assert_eq!(h.proxy.stats.blocked.load(Ordering::Relaxed), 1);
    assert_eq!(h.proxy.stats.media_bytes_fetched.load(Ordering::Relaxed), 0);
    assert_eq!(h.proxy.stats.recent_events(1)[0].kind, "blocked");
}

#[tokio::test]
async fn sign_in_hosts_are_tunneled_untouched() {
    let h = harness().await;
    // The tunnel ends at the fake upstream; the client sees the upstream's own certificate chain, not Muninn's.
    let mut tx = tls_client(&h, "www.youtube.com", &h.up.root_der.clone()).await;
    assert!(tx.is_err(), "www.youtube.com is MITM'd, so the upstream root alone must not validate it");
    let (status, tcp) = connect(&h, "accounts.google.com").await;
    assert_eq!(status, 200);
    let name = rustls::pki_types::ServerName::try_from("www.youtube.com".to_string()).unwrap();
    let tls = tokio_rustls::TlsConnector::from(client_config(&h.up.root_der)).connect(name, tcp).await;
    assert!(tls.is_ok(), "accounts.google.com was not decrypted: {:?}", tls.err());
    assert_eq!(h.proxy.stats.tunnels.load(Ordering::Relaxed), 1);
    assert_eq!(h.proxy.stats.mitm_connects.load(Ordering::Relaxed), 1);
    let _ = &mut tx;
}

#[tokio::test]
async fn an_untrusting_client_is_counted_so_the_app_can_offer_trust() {
    let h = harness().await;
    let other = tempfile::tempdir().unwrap();
    let stranger = Ca::load_or_create(other.path()).unwrap();
    let r = tls_client(&h, "www.youtube.com", stranger.cert_der()).await;
    assert!(r.is_err());
    wait_for("the failure to be counted", || h.proxy.stats.tls_failures.load(Ordering::Relaxed) >= 1).await;
    assert_eq!(h.proxy.stats.tls_ok.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn loopback_http_is_forwarded_for_views_that_share_the_proxy() {
    let h = harness().await;
    let local = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = local.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (mut s, _) = local.accept().await.unwrap();
        let mut buf = [0u8; 1024];
        let _ = s.read(&mut buf).await;
        let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").await;
    });
    let mut s = TcpStream::connect(h.proxy.addr).await.unwrap();
    s.write_all(format!("GET http://127.0.0.1:{port}/ui/ HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").as_bytes()).await.unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out).await;
    assert!(out.starts_with("HTTP/1.1 200") && out.ends_with("ok"), "{out}");
}

#[tokio::test]
async fn plain_http_proxying_is_refused() {
    let h = harness().await;
    let mut s = TcpStream::connect(h.proxy.addr).await.unwrap();
    s.write_all(b"GET http://example.org/ HTTP/1.1\r\nHost: example.org\r\n\r\n").await.unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out).await;
    assert!(out.starts_with("HTTP/1.1 501"), "{out}");
}
