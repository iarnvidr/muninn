//! Trusting Muninn's own certificate authority inside the app's web views.
//!
//! The capture proxy decrypts `*.youtube.com` with leaf certificates from the
//! per-install CA (core/src/ca.rs). The CA is name-constrained to youtube.com and
//! ytimg.com and trusted only for TLS server auth.
//!
//! * macOS: wry has no server-trust delegate method, so `trust_mac` adds one to
//!   its navigation delegate through the Objective-C runtime and answers the
//!   server-trust challenge for chains that end in Muninn's CA. If the proxy
//!   still sees clients rejecting the certificate (WebKit routes proxied
//!   sub-resources elsewhere), the CA is added to the login keychain with
//!   user-scope, SSL-only trust: one system confirmation, from inside the app.
//! * Linux (development/tests only): the exact leaf certificates are approved
//!   per host with WebKitGTK's `allow_tls_certificate_for_host`.

use crate::shell::Shell;
use serde::Serialize;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::Webview;

#[derive(Debug, Clone, Serialize)]
pub struct TrustState {
    /// "unknown", "ok", "needed", "installing" or "failed".
    pub state: String,
    pub detail: Option<String>,
    #[serde(skip)]
    pub attempted: bool,
}

impl Default for TrustState {
    fn default() -> Self {
        TrustState { state: "unknown".into(), detail: None, attempted: false }
    }
}

pub fn status(shell: &Shell) -> TrustState {
    let mut st = shell.trust.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(stats) = shell.core.proxy_stats() {
        if stats.tls_ok.load(Ordering::Relaxed) > 0 && st.state != "installing" {
            st.state = "ok".into();
            st.detail = None;
        } else if (stats.tls_untrusted.load(Ordering::Relaxed) > 0 || stats.tls_failures.load(Ordering::Relaxed) >= 3) && st.state == "unknown" {
            st.state = "needed".into();
        }
    }
    st.clone()
}

/// Called for every new YouTube web view.
pub fn prepare_view(shell: &Arc<Shell>, wv: &Webview) {
    #[cfg(target_os = "linux")]
    linux::prepare(shell, wv);
    #[cfg(target_os = "macos")]
    crate::trust_mac::prepare(shell, wv);
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let _ = (shell, wv);
}

/// The keychain fallback (macOS) and the manual "Allow" button.
pub fn install(shell: &Arc<Shell>) -> Result<(), String> {
    {
        let mut st = shell.trust.lock().unwrap_or_else(|e| e.into_inner());
        st.state = "installing".into();
        st.attempted = true;
    }
    let result = install_platform(shell);
    let mut st = shell.trust.lock().unwrap_or_else(|e| e.into_inner());
    match &result {
        Ok(()) => {
            st.state = "ok".into();
            st.detail = None;
        }
        Err(e) => {
            st.state = "failed".into();
            st.detail = Some(e.clone());
        }
    }
    drop(st);
    if result.is_ok() {
        shell.reload_live_tabs();
    }
    result
}

#[cfg(target_os = "macos")]
fn install_platform(shell: &Arc<Shell>) -> Result<(), String> {
    let info = shell.core.proxy_info().ok_or("the capture proxy is not running")?;
    let home = std::env::var("HOME").map_err(|e| e.to_string())?;
    let keychain = format!("{home}/Library/Keychains/login.keychain-db");
    // User-scope (no -d), SSL policy only, root of trust. macOS asks the user to confirm.
    let out = std::process::Command::new("/usr/bin/security")
        .args(["add-trusted-cert", "-r", "trustRoot", "-p", "ssl", "-k", &keychain])
        .arg(&info.ca_pem_path)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(not(target_os = "macos"))]
fn install_platform(_shell: &Arc<Shell>) -> Result<(), String> {
    Ok(())
}

/// Watches for clients rejecting the certificate and falls back to the keychain once.
pub fn start_monitor(shell: &Arc<Shell>) {
    #[cfg(target_os = "macos")]
    {
        let shell = shell.clone();
        std::thread::Builder::new()
            .name("muninn-trust".into())
            .spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(2));
                let (attempted, untrusted, ok) = {
                    let st = shell.trust.lock().unwrap_or_else(|e| e.into_inner());
                    let stats = shell.core.proxy_stats();
                    (
                        st.attempted,
                        // Rejections with a TLS alert, or several aborted handshakes and not one success.
                        stats.as_ref().map_or(0, |s| s.tls_untrusted.load(Ordering::Relaxed) + s.tls_failures.load(Ordering::Relaxed) / 3),
                        stats.as_ref().map_or(0, |s| s.tls_ok.load(Ordering::Relaxed)),
                    )
                };
                if !attempted && untrusted > 0 && ok == 0 {
                    let _ = install(&shell);
                }
            })
            .ok();
    }
    #[cfg(not(target_os = "macos"))]
    let _ = shell;
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use webkit2gtk::gio;
    use webkit2gtk::{WebContextExt, WebViewExt};

    /// Hosts Muninn decrypts; the leaf certificates for them are approved up front.
    const HOSTS: &[&str] = &["www.youtube.com", "youtube.com", "m.youtube.com", "consent.youtube.com", "music.youtube.com", "studio.youtube.com", "kids.youtube.com", "tv.youtube.com"];

    pub fn prepare(shell: &Arc<Shell>, wv: &Webview) {
        let Some(ca) = shell.core.ca() else { return };
        let pems: Vec<(String, String)> = HOSTS.iter().filter_map(|h| ca.leaf_pem(h).map(|p| (h.to_string(), p))).collect();
        let _ = wv.with_webview(move |w| {
            let view = w.inner();
            let Some(ctx) = view.context() else { return };
            for (host, pem) in &pems {
                if let Ok(cert) = gio::TlsCertificate::from_pem(pem) {
                    #[allow(deprecated)]
                    ctx.allow_tls_certificate_for_host(&cert, host);
                }
            }
            // A view that starts loading before the approvals above ran (the load begins at
            // creation) fails once with a TLS error: approve that exact certificate for the
            // YouTube host and load again.
            view.connect_load_failed_with_tls_errors(|view, uri, cert, _flags| {
                let host = uri.split("://").nth(1).and_then(|r| r.split(['/', ':', '?']).next()).unwrap_or("");
                if host == "youtube.com" || host.ends_with(".youtube.com") {
                    if let Some(ctx) = view.context() {
                        #[allow(deprecated)]
                        ctx.allow_tls_certificate_for_host(cert, host);
                    }
                    view.load_uri(uri);
                    return true;
                }
                false
            });
        });
    }
}
