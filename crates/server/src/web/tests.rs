use super::*;
use http_body_util::BodyExt;
use muninn_core::CoreConfig;

const ORIGIN: &str = "https://muninn.test";
const SECRET: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
struct Harness { _dir: tempfile::TempDir, core: Arc<Core>, app: Router }
fn harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::open(CoreConfig { data_dir:dir.path().into(), spawn_workers:false,
        tool_dirs:Some(vec![]), ..CoreConfig::default() }).unwrap();
    let app = router(core.clone(),Config::new(ORIGIN,SECRET).unwrap());
    Harness { _dir:dir, core, app }
}
fn request(method: &str, path: &str, cookie: Option<&str>, body: &str) -> Request {
    let mut builder = axum::http::Request::builder().method(method).uri(path)
        .header(header::HOST,"muninn.test").header(header::ORIGIN,ORIGIN)
        .header("x-muninn-client","pwa");
    if let Some(cookie)=cookie { builder=builder.header(header::COOKIE,cookie); }
    if !body.is_empty() { builder=builder.header(header::CONTENT_TYPE,"application/json"); }
    builder.body(Body::from(body.to_owned())).unwrap()
}
impl Harness {
    async fn send(&self, req:Request) -> Response { self.app.clone().oneshot(req).await.unwrap() }
    async fn login(&self) -> String {
        let response=self.send(request("POST","/api/session",None,&json!({"token":SECRET}).to_string())).await;
        assert_eq!(response.status(),StatusCode::OK);
        let cookie=response.headers().get(header::SET_COOKIE).unwrap().to_str().unwrap();
        assert!(cookie.contains("HttpOnly")&&cookie.contains("Secure")&&cookie.contains("SameSite=Strict"));
        assert!(!cookie.contains(SECRET)); assert!(!cookie.contains(&self.core.token()));
        cookie.split(';').next().unwrap().to_string()
    }
}
#[test]
fn configuration_requires_exact_secure_origin_and_a_strong_token() {
    for origin in ["http://muninn.test","https://muninn.test/path","https://user@muninn.test",
        "https://muninn.test?x=1","https://muninn.test#token","file:///app/"] {
        assert!(Config::new(origin,SECRET).is_err(),"{origin}");
    }
    for origin in ["https://muninn.test","http://localhost:47872","http://127.0.0.1:47872","http://[::1]:47872"] {
        assert!(Config::new(origin,SECRET).is_ok(),"{origin}");
    }
    assert!(Config::new(ORIGIN,"weak").is_err());
}
#[tokio::test]
async fn shell_is_public_and_api_and_media_are_not() {
    let h=harness();
    for path in ["/app/","/app/manifest.webmanifest","/app/sw.js","/app/desktop.css","/app/raven.png",
        "/ui/Cinzel.ttf","/ui/Inter.ttf","/ui/IBMPlexMono.ttf","/app/icon-192.png","/health"] {
        assert_eq!(h.send(request("GET",path,None,"")).await.status(),StatusCode::OK,"{path}");
    }
    for path in ["/api/entries","/api/jobs","/api/session","/media/example","/thumb/aaaaaaaaaaa"] {
        assert_eq!(h.send(request("GET",path,None,"")).await.status(),StatusCode::UNAUTHORIZED,"{path}");
    }
    let mut legacy=request("GET","/api/entries",None,"");
    legacy.headers_mut().insert(header::AUTHORIZATION,format!("Bearer {}",h.core.token()).parse().unwrap());
    assert_eq!(h.send(legacy).await.status(),StatusCode::UNAUTHORIZED);
}
#[tokio::test]
async fn login_is_revocable_and_tokens_never_appear_in_responses() {
    let h=harness();let cookie=h.login().await;
    let response=h.send(request("GET","/api/entries",Some(&cookie),"")).await;
    assert_eq!(response.status(),StatusCode::OK);
    assert!(!response.headers().contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN));
    assert_eq!(response.headers()[header::CACHE_CONTROL],"no-store");
    let body=response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.as_ref(),b"[]");
    assert_eq!(h.send(request("DELETE","/api/session",Some(&cookie),"")).await.status(),StatusCode::OK);
    assert_eq!(h.send(request("GET","/api/entries",Some(&cookie),"")).await.status(),StatusCode::UNAUTHORIZED);
}
#[tokio::test]
async fn host_origin_and_csrf_checks_apply_to_login_and_writes() {
    let h=harness();let body=json!({"token":SECRET}).to_string();
    let mut req=request("POST","/api/session",None,&body);
    req.headers_mut().insert(header::ORIGIN,HeaderValue::from_static("https://evil.test"));
    assert_eq!(h.send(req).await.status(),StatusCode::FORBIDDEN);
    let mut req=request("POST","/api/session",None,&body);req.headers_mut().remove("x-muninn-client");
    assert_eq!(h.send(req).await.status(),StatusCode::FORBIDDEN);
    let mut req=request("GET","/app/",None,"");req.headers_mut().insert(header::HOST,HeaderValue::from_static("evil.test"));
    assert_eq!(h.send(req).await.status(),StatusCode::FORBIDDEN);
    let mut req=request("POST","/api/session",None,&body);req.headers_mut().remove(header::ORIGIN);
    assert_eq!(h.send(req).await.status(),StatusCode::FORBIDDEN);
}
#[tokio::test]
async fn login_attempts_are_bounded() {
    let h=harness();
    for _ in 0..MAX_LOGIN_ATTEMPTS {
        assert_eq!(h.send(request("POST","/api/session",None,r#"{"token":"wrong"}"#)).await.status(),StatusCode::UNAUTHORIZED);
    }
    assert_eq!(h.send(request("POST","/api/session",None,r#"{"token":"wrong"}"#)).await.status(),StatusCode::TOO_MANY_REQUESTS);
}
#[tokio::test]
async fn native_admin_and_incomplete_playback_are_not_exposed() {
    let h=harness();let cookie=h.login().await;
    for (method,path) in [("GET","/ui/app.js"),("GET","/ui/index.html"),
        ("POST","/api/import/media"),("POST","/api/tools/ytdlp/install"),
        ("GET","/api/proxy"),("POST","/api/open"),("POST","/api/watch/aaaaaaaaaaa"),("GET","/ui/"),
        ("GET","/s/secret/video/0"),("GET","/api/settings")] {
        assert_eq!(h.send(request(method,path,Some(&cookie),"{}")).await.status(),StatusCode::NOT_FOUND,"{path}");
    }
    let response=h.send(request("POST","/api/cache",Some(&cookie),
        r#"{"video_id":"aaaaaaaaaaa","quality":"720","protect":true}"#)).await;
    assert_eq!(response.status(),StatusCode::OK);
    let bytes=response.into_body().collect().await.unwrap().to_bytes();
    let json:serde_json::Value=serde_json::from_slice(&bytes).unwrap();
    let id=json["entry"]["id"].as_str().expect("cache request should queue an entry");
    assert_eq!(h.send(request("GET",&format!("/media/{id}"),Some(&cookie),"")).await.status(),StatusCode::CONFLICT);
    assert_eq!(h.send(request("GET","/api/entries?token=not-supported",Some(&cookie),"")).await.status(),StatusCode::BAD_REQUEST);
    assert_eq!(h.send(request("POST","/api/cache",Some(&cookie),&"x".repeat(17*1024))).await.status(),StatusCode::PAYLOAD_TOO_LARGE);
}
#[test]
fn route_allowlist_is_narrow() {
    for path in ["/api/entries","/api/feeds/1","/media/aaaaaaaaaaa.720","/thumb/aaaaaaaaaaa"] { assert!(allowed(&Method::GET,path)); }
    for path in ["/api/entries/../settings","//api/entries","/media/%2fetc","/api/settings","/api/feeds/1/extra","/media/..","/media/."] { assert!(!allowed(&Method::GET,path),"{path}"); }
    assert!(!allowed(&Method::DELETE,"/api/entries/abc"));
}

#[tokio::test]
async fn completed_files_keep_auth_and_streaming_range_semantics() {
    use muninn_core::videodb::{EntryRow, STATUS_COMPLETE};
    let h=harness();let cookie=h.login().await;
    let data:Vec<u8>=(0..512).map(|n|(n%251) as u8).collect();
    let path=h.core.paths.video_dir.join("aaaaaaaaaaa.360.mp4");
    std::fs::write(&path,&data).unwrap();
    let mut entry=EntryRow::new("aaaaaaaaaaa","360","360","manual",1);
    entry.status=STATUS_COMPLETE.into();entry.file_path=Some(path.display().to_string());
    entry.ext=Some("mp4".into());entry.size_bytes=data.len() as u64;entry.actual_height=Some(360);
    h.core.video.insert_entry(&entry).unwrap();
    let url=format!("/media/{}",entry.id);
    assert!(entry.id.contains('.'),"Use the real Core entry-ID format");
    assert_eq!(h.send(request("GET",&url,None,"")).await.status(),StatusCode::UNAUTHORIZED);
    let response=h.send(request("GET",&url,Some(&cookie),"")).await;
    assert_eq!(response.status(),StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE],"video/mp4");
    assert_eq!(response.headers()[header::CONTENT_LENGTH],"512");
    assert_eq!(response.into_body().collect().await.unwrap().to_bytes().as_ref(),data.as_slice());
    let mut req=request("GET",&url,Some(&cookie),"");
    req.headers_mut().insert(header::RANGE,HeaderValue::from_static("bytes=100-199"));
    let response=h.send(req).await;
    assert_eq!(response.status(),StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.headers()[header::CONTENT_RANGE],"bytes 100-199/512");
    assert!(!response.headers().contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN));
    assert_eq!(response.into_body().collect().await.unwrap().to_bytes().as_ref(),&data[100..200]);
    let response=h.send(request("HEAD",&url,Some(&cookie),"")).await;
    assert_eq!(response.status(),StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_LENGTH],"512");
    assert!(response.into_body().collect().await.unwrap().to_bytes().is_empty());
}

fn authelia_harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::open(CoreConfig { data_dir:dir.path().into(), spawn_workers:false,
        tool_dirs:Some(vec![]), ..CoreConfig::default() }).unwrap();
    let app = router(core.clone(), Config::authelia(ORIGIN,
        vec!["172.29.0.2".parse().unwrap()]).unwrap());
    Harness { _dir:dir, core, app }
}
fn authelia_request(method: &str, path: &str, peer: Option<&str>, user: Option<&str>) -> Request {
    let mut req = request(method, path, None, if method == "POST" {"{}"} else {""});
    req.headers_mut().insert("x-forwarded-proto", HeaderValue::from_static("https"));
    if let Some(user) = user { req.headers_mut().insert("remote-user", user.parse().unwrap()); }
    if let Some(peer) = peer {
        req.extensions_mut().insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
    }
    req
}
#[test]
fn authelia_requires_a_real_nonloopback_proxy_and_https() {
    assert!(Config::authelia("http://localhost:47872", vec!["172.29.0.2".parse().unwrap()]).is_err());
    assert!(Config::authelia(ORIGIN, vec![]).is_err());
    assert!(Config::authelia(ORIGIN, vec!["127.0.0.1".parse().unwrap()]).is_err());
    assert!(Config::authelia(ORIGIN, vec!["172.29.0.2".parse().unwrap()]).is_ok());
}
#[tokio::test]
async fn authelia_owner_has_one_login_and_real_private_library_access() {
    let h = authelia_harness();
    let good = || authelia_request("GET","/api/entries",Some("172.29.0.2:45454"),Some("esben"));
    let response = h.send(good()).await;
    assert_eq!(response.status(),StatusCode::OK);
    assert_eq!(response.into_body().collect().await.unwrap().to_bytes().as_ref(),b"[]");
    let response = h.send(authelia_request("GET","/api/session",
        Some("172.29.0.2:45454"),Some("esben"))).await;
    assert_eq!(response.status(),StatusCode::OK);
    let bytes=response.into_body().collect().await.unwrap().to_bytes();
    let value:serde_json::Value=serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["auth_mode"],"authelia");
    assert_eq!(value["login_required"],false);
    let response=h.send(authelia_request("GET","/app/",
        Some("172.29.0.2:45454"),Some("esben"))).await;
    assert_eq!(response.status(),StatusCode::OK);
    let html=response.into_body().collect().await.unwrap().to_bytes();
    let html=String::from_utf8_lossy(&html);
    assert!(html.contains("name=\"muninn-auth-mode\" content=\"authelia\""));
    assert!(!html.contains("__MUNINN_AUTH_MODE__"));
    let mut post = authelia_request("POST","/api/session",Some("172.29.0.2:45454"),Some("esben"));
    *post.body_mut()=Body::from(json!({"token":SECRET}).to_string());
    assert_eq!(h.send(post).await.status(),StatusCode::NOT_FOUND);
    assert_eq!(h.send(authelia_request("DELETE","/api/session",
        Some("172.29.0.2:45454"),Some("esben"))).await.status(),StatusCode::NOT_FOUND);
}
#[tokio::test]
async fn authelia_rejects_direct_forged_identity_and_malformed_forwarding() {
    let h=authelia_harness();
    for (peer,user) in [(None,Some("esben")),(Some("127.0.0.1:1234"),Some("esben")),
        (Some("172.29.0.3:1234"),Some("esben")),
        (Some("172.29.0.2:1234"),Some("alice")),
        (Some("172.29.0.2:1234"),None)] {
        let response=h.send(authelia_request("GET","/api/entries",peer,user)).await;
        assert_eq!(response.status(),StatusCode::FORBIDDEN,"peer {peer:?}, user {user:?}");
    }
    let mut duplicate=authelia_request("GET","/api/entries",Some("172.29.0.2:1234"),Some("esben"));
    duplicate.headers_mut().append("remote-user",HeaderValue::from_static("esben"));
    assert_eq!(h.send(duplicate).await.status(),StatusCode::FORBIDDEN);
    let mut downgraded=authelia_request("GET","/api/entries",Some("172.29.0.2:1234"),Some("esben"));
    downgraded.headers_mut().insert("x-forwarded-proto",HeaderValue::from_static("http"));
    assert_eq!(h.send(downgraded).await.status(),StatusCode::FORBIDDEN);
    let mut foreign=authelia_request("POST","/api/cache",Some("172.29.0.2:1234"),Some("esben"));
    foreign.headers_mut().insert(header::ORIGIN,HeaderValue::from_static("https://evil.test"));
    assert_eq!(h.send(foreign).await.status(),StatusCode::FORBIDDEN);
    let mut no_origin=authelia_request("POST","/api/cache",Some("172.29.0.2:1234"),Some("esben"));
    no_origin.headers_mut().remove(header::ORIGIN);
    assert_eq!(h.send(no_origin).await.status(),StatusCode::FORBIDDEN);
    let mut no_custom=authelia_request("POST","/api/cache",Some("172.29.0.2:1234"),Some("esben"));
    no_custom.headers_mut().remove("x-muninn-client");
    assert_eq!(h.send(no_custom).await.status(),StatusCode::FORBIDDEN);
}
