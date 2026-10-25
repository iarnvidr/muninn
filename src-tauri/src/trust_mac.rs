//! macOS: answer WebKit's server-trust challenge for Muninn's own CA (see trust.rs).
//!
//! wry's navigation delegate has no `webView:didReceiveAuthenticationChallenge:
//! completionHandler:`. This adds one to that delegate class at runtime (once per
//! process). For a challenge on a youtube.com host whose certificate chain
//! validates against Muninn's CA *only* (anchor set replaced, host name checked by
//! the trust object WebKit built), it answers `useCredential(forTrust)`; every
//! other challenge gets the default handling, so real certificate errors still fail.
//! Whether WebKit routes sub-resource and proxied loads here cannot be checked on
//! Linux: if it does not, the proxy sees the handshake rejections and trust.rs falls
//! back to the keychain.

use crate::shell::Shell;
use objc2::msg_send;
use objc2::runtime::AnyObject;
use security_framework::certificate::SecCertificate;
use security_framework::trust::SecTrust;
use std::ffi::{c_char, c_void, CStr};
use std::sync::{Arc, OnceLock};
use tauri::Webview;

static CA_DER: OnceLock<Vec<u8>> = OnceLock::new();

extern "C" {
    fn object_getClass(obj: *const c_void) -> *const c_void;
    fn sel_registerName(name: *const c_char) -> *const c_void;
    fn class_addMethod(
        cls: *const c_void,
        name: *const c_void,
        imp: *const c_void,
        types: *const c_char,
    ) -> bool;
    fn objc_getClass(name: *const c_char) -> *const c_void;
}

type Completion = block2::Block<dyn Fn(isize, *mut AnyObject)>;

fn host_is_youtube(host: &str) -> bool {
    let h = host.to_ascii_lowercase();
    h == "youtube.com" || h.ends_with(".youtube.com")
}

unsafe fn nsstring(obj: *mut AnyObject) -> Option<String> {
    if obj.is_null() {
        return None;
    }
    let utf8: *const c_char = msg_send![obj, UTF8String];
    (!utf8.is_null()).then(|| CStr::from_ptr(utf8).to_string_lossy().into_owned())
}

/// True when the trust object's chain validates with Muninn's CA as the only anchor.
unsafe fn chain_ends_in_muninn_ca(trust_ptr: *mut c_void) -> bool {
    use core_foundation::base::TCFType;
    let Some(der) = CA_DER.get() else {
        return false;
    };
    let Some(ca) = SecCertificate::from_der(der).ok() else {
        return false;
    };
    let mut trust = SecTrust::wrap_under_get_rule(trust_ptr as _);
    if trust.set_anchor_certificates(&[ca]).is_err() {
        return false;
    }
    let _ = trust.set_trust_anchor_certificates_only(true);
    trust.evaluate_with_error().is_ok()
}

extern "C" fn did_receive_challenge(
    _this: *mut AnyObject,
    _cmd: *const c_void,
    _webview: *mut AnyObject,
    challenge: *mut AnyObject,
    handler: *mut c_void,
) {
    // 0 = use credential, 1 = perform default handling.
    let (mut disposition, mut credential): (isize, *mut AnyObject) = (1, std::ptr::null_mut());
    unsafe {
        let space: *mut AnyObject = msg_send![challenge, protectionSpace];
        if !space.is_null() {
            let method = nsstring(msg_send![space, authenticationMethod]);
            let host = nsstring(msg_send![space, host]);
            let trust: *mut c_void = msg_send![space, serverTrust];
            if method.as_deref() == Some("NSURLAuthenticationMethodServerTrust")
                && host.as_deref().is_some_and(host_is_youtube)
                && !trust.is_null()
                && chain_ends_in_muninn_ca(trust)
            {
                let class = objc_getClass(c"NSURLCredential".as_ptr()) as *const AnyObject;
                if !class.is_null() {
                    let cred: *mut AnyObject = msg_send![class, credentialForTrust: trust];
                    if !cred.is_null() {
                        disposition = 0;
                        credential = cred;
                    }
                }
            }
        }
        if !handler.is_null() {
            let block = &*(handler as *const Completion);
            block.call((disposition, credential));
        }
    }
}

/// Adds the challenge method to the web view's navigation delegate class.
pub fn prepare(shell: &Arc<Shell>, wv: &Webview) {
    if let Some(ca) = shell.core.ca() {
        let _ = CA_DER.set(ca.cert_der().to_vec());
    }
    let _ = wv.with_webview(|w| unsafe {
        let webview = w.inner() as *mut AnyObject;
        let delegate: *mut AnyObject = msg_send![webview, navigationDelegate];
        if delegate.is_null() {
            return;
        }
        let class = object_getClass(delegate as *const c_void);
        let sel = sel_registerName(
            c"webView:didReceiveAuthenticationChallenge:completionHandler:".as_ptr(),
        );
        // v@:@@@? = void, self, _cmd, webview, challenge, block
        let _ = class_addMethod(
            class,
            sel,
            did_receive_challenge as *const c_void,
            c"v@:@@@?".as_ptr(),
        );
    });
}
