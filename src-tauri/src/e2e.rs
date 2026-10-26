//! Test automation hook, active only when `MUNINN_E2E_DIR` is set in the
//! environment of the launching process (never in a normal launch).
//!
//! A test drops `<id>.cmd.json` into that directory; the app executes it and
//! writes `<id>.res.json`. Used by the Linux/xvfb end-to-end checks to drive the
//! real window (open tabs, navigate, hibernate, read page values) while a script
//! takes screenshots.

use crate::shell::{label, Shell};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tauri::Manager;

fn dir() -> Option<PathBuf> {
    // Native UI automation is restricted to explicit smoke launch and debug builds.
    if !cfg!(debug_assertions)
        || std::env::var_os("MUNINN_NATIVE_UI_SMOKE").is_none()
        || !std::env::args().any(|a| a == "--native-ui-smoke")
    {
        return None;
    }
    std::env::var_os("MUNINN_E2E_DIR").map(PathBuf::from)
}

pub fn store_result(id: &str, value: String) -> Result<(), String> {
    let d = dir().ok_or("automation is off")?;
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("bad id".into());
    }
    std::fs::write(d.join(format!("{id}.res.json")), value).map_err(|e| e.to_string())
}

fn respond(d: &std::path::Path, id: &str, v: &Value) {
    let _ = std::fs::write(d.join(format!("{id}.res.json.tmp")), v.to_string());
    let _ = std::fs::rename(
        d.join(format!("{id}.res.json.tmp")),
        d.join(format!("{id}.res.json")),
    );
}

pub fn start(shell: &Arc<Shell>) {
    let Some(d) = dir() else { return };
    let _ = std::fs::create_dir_all(&d);
    let shell = shell.clone();
    std::thread::Builder::new()
        .name("muninn-e2e".into())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_millis(200));
            let Ok(read) = std::fs::read_dir(&d) else {
                continue;
            };
            let mut cmds: Vec<PathBuf> = read
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.to_string_lossy().ends_with(".cmd.json"))
                .collect();
            cmds.sort();
            for path in cmds {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                let _ = std::fs::remove_file(&path);
                let Ok(cmd) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                let id = cmd["id"].as_str().unwrap_or("x").to_string();
                run(&shell, &d, &id, &cmd);
            }
        })
        .ok();
}

fn run(shell: &Arc<Shell>, d: &std::path::Path, id: &str, cmd: &Value) {
    let u64_of = |k: &str| cmd["args"][k].as_u64().unwrap_or(0);
    let str_of = |k: &str| cmd["args"][k].as_str().unwrap_or("").to_string();
    match cmd["op"].as_str().unwrap_or("") {
        #[cfg(all(debug_assertions, target_os = "macos"))]
        "settings_menu" | "search_menu" | "address_menu" => {
            let search = cmd["op"] == "search_menu";
            let address = cmd["op"] == "address_menu";
            let tab = cmd["tab_id"].as_u64().unwrap_or(0);
            let app = shell.app.clone();
            let (sender, receiver) = std::sync::mpsc::sync_channel(1);
            let _ = shell.app.run_on_main_thread(move || {
                let result = (|| -> Result<Value, &'static str> {
                    let view = app.get_webview(&label(tab)).ok_or("native focus view missing")?;
                    view.set_focus().map_err(|_| "native focus failed")?;
                    // Inspect the actual Cocoa accelerator and invoke the real
                    // menu action with the YouTube webview holding focus.
                    use objc2::{class, msg_send, runtime::AnyObject};
                    unsafe {
                        let native: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
                        let menu: *mut AnyObject = msg_send![native, mainMenu];
                        let root_index = if search || address { 3isize } else { 0isize };
                        let root: *mut AnyObject = msg_send![menu, itemAtIndex: root_index];
                        let submenu: *mut AnyObject = msg_send![root, submenu];
                        let item_title = if address { c"Search or Open Address…" } else if search { c"Search Muninn…" } else { c"Settings…" };
                        let title: *mut AnyObject = msg_send![class!(NSString), stringWithUTF8String: item_title.as_ptr()];
                        let item: *mut AnyObject = msg_send![submenu, itemWithTitle: title];
                        if item.is_null() { return Err("Settings menu item missing"); }
                        let key: *mut AnyObject = msg_send![item, keyEquivalent];
                        let chars: *const std::ffi::c_char = msg_send![key, UTF8String];
                        if chars.is_null() { return Err("Settings accelerator missing"); }
                        let comma = std::ffi::CStr::from_ptr(chars).to_bytes() == b",";
                        let expected = std::ffi::CStr::from_ptr(chars).to_bytes() == if address { b"l" } else if search { b"k" } else { b"," };
                        let modifiers: usize = msg_send![item, keyEquivalentModifierMask];
                        let command = modifiers & (1 << 20) != 0;
                        let index: isize = msg_send![submenu, indexOfItem: item];
                        let _: () = msg_send![submenu, performActionForItemAtIndex: index];
                        Ok(json!({"ok":expected && command,"comma":comma,"command":command}))
                    }
                })();
                let _ = sender.send(result);
            });
            let result = receiver.recv_timeout(Duration::from_secs(10));
            respond(d, id, &match result {
                Ok(Ok(value)) => value,
                _ => json!({"ok":false,"error":"Settings menu fixture failed"}),
            });
        }
        "youtube_cookie_fixture" => {
            // The real persistent YouTube WKWebView store, with synthetic
            // credentials only. Read on this worker, never the UI thread.
            let tab = shell.new_tab(Some("https://www.youtube.com/"));
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let webview = loop {
                if let Some(view) = shell.app.get_webview(&label(tab)) { break Some(view); }
                if std::time::Instant::now() > deadline { break None; }
                std::thread::sleep(Duration::from_millis(50));
            };
            let result = (|| -> Result<Value, &'static str> {
                let view = webview.as_ref().ok_or("fixture webview missing")?;
                let cookie = tauri::webview::Cookie::build(("MUNINN_NATIVE_SESSION_TEST", "synthetic-only"))
                    .domain(".youtube.com").path("/").secure(true).http_only(true).build();
                view.set_cookie(cookie.clone()).map_err(|_| "fixture cookie setup failed")?;
                shell.core.invalidate_youtube_account();
                let reader = shell.core.youtube_cookie_reader.read().unwrap().clone().ok_or("native cookie reader missing")?;
                // No page_state, account ID or observed sign-in is supplied.
                let cookies = reader().map_err(|_| "native reader failed")?;
                let initial = cookies.iter().any(|cookie| cookie.name == "MUNINN_NATIVE_SESSION_TEST"
                    && cookie.value == "synthetic-only" && cookie.secure && cookie.http_only
                    && cookie.path == "/" && cookie.domain == ".youtube.com");
                let mut rotated = cookie.clone(); rotated.set_value("synthetic-rotated");
                view.set_cookie(rotated).map_err(|_| "fixture rotation failed")?;
                let rotated = reader().map_err(|_| "native reread failed")?.iter().any(|cookie|
                    cookie.name == "MUNINN_NATIVE_SESSION_TEST" && cookie.value == "synthetic-rotated");
                let _ = view.delete_cookie(cookie);
                let cleared = !reader().map_err(|_| "native sign-out reread failed")?.iter().any(|cookie|
                    cookie.name == "MUNINN_NATIVE_SESSION_TEST");
                Ok(json!({"ok":initial && rotated && cleared,"before_observation":initial,"after_rotation":rotated,"after_deletion":cleared}))
            })();
            shell.close_tab(tab);
            respond(d, id, &match result { Ok(result) => result, Err(error) => json!({"ok":false,"error":error}) });
        }
        "state" => {
            let mut state = shell.tabs_state();
            state["process_id"] = json!(std::process::id());
            state["executable_path"] = std::env::current_exe()
                .ok()
                .map(|path| json!(path.to_string_lossy().into_owned()))
                .unwrap_or(Value::Null);
            state["window_visible"] = json!(shell
                .app
                .get_window(crate::shell::MAIN)
                .is_some_and(|w| w.is_visible().unwrap_or(false)));
            respond(d, id, &state);
        }
        "call" => {
            match cmd["name"].as_str().unwrap_or("") {
                "tab_new" => {
                    let url = cmd["args"]["url"].as_str().map(str::to_string);
                    let n = shell.new_tab(url.as_deref());
                    return respond(d, id, &json!({"ok": true, "id": n}));
                }
                "tab_go" => shell.go(u64_of("id"), &str_of("input")),
                "tab_activate" => shell.activate(u64_of("id")),
                "tab_close" => shell.close_tab(u64_of("id")),
                "tab_hibernate" => shell.hibernate(u64_of("id")),
                "tab_back" => shell.back(u64_of("id")),
                "tab_forward" => shell.forward(u64_of("id")),
                "tab_open_video" => shell.open_video(u64_of("id"), &str_of("video_id"), false),
                "app_open_link" => crate::open_app_links(&shell.app, [str_of("url")]),
                "window_hide" => {
                    if let Some(window) = shell.app.get_window(crate::shell::MAIN) {
                        let _ = window.hide();
                    }
                }
                "hibernate_pass" => {
                    return respond(d, id, &json!({"ok": true, "slept": shell.hibernate_pass()}))
                }
                other => {
                    return respond(
                        d,
                        id,
                        &json!({"ok": false, "error": format!("unknown call {other}")}),
                    )
                }
            }
            respond(d, id, &json!({"ok": true}));
        }
        "eval" => {
            let target = cmd["target"].as_str().unwrap_or("chrome");
            let wlabel = match target {
                "chrome" => crate::shell::CHROME.to_string(),
                "active" => match shell.tabs_state()["active"].as_u64() {
                    Some(a) => label(a),
                    None => return respond(d, id, &json!({"ok": false, "error": "no active tab"})),
                },
                t => label(
                    t.strip_prefix("tab:")
                        .and_then(|n| n.parse().ok())
                        .unwrap_or(0),
                ),
            };
            let Some(wv) = shell.app.get_webview(&wlabel) else {
                return respond(
                    d,
                    id,
                    &json!({"ok": false, "error": format!("no web view {wlabel}")}),
                );
            };
            let js = cmd["js"].as_str().unwrap_or("null");
            let wrapped = format!(
                "(async()=>{{let r;try{{r=await (async()=>{{{js}\n}})()}}catch(e){{r={{error:String(e)}}}}\
                 try{{await window.__TAURI_INTERNALS__.invoke('e2e_result',{{id:'{id}',value:JSON.stringify(r===undefined?null:r)}})}}catch(e){{}}}})()"
            );
            if let Err(e) = wv.eval(wrapped) {
                respond(d, id, &json!({"ok": false, "error": e.to_string()}));
            }
        }
        // Synchronous eval whose value comes back through the runtime itself, so it also works on
        // pages without IPC access (Google sign-in).
        "evalsync" => {
            let target = cmd["target"].as_str().unwrap_or("active");
            let wlabel = match target {
                "chrome" => crate::shell::CHROME.to_string(),
                "active" => match shell.tabs_state()["active"].as_u64() {
                    Some(a) => label(a),
                    None => return respond(d, id, &json!({"ok": false, "error": "no active tab"})),
                },
                t => label(
                    t.strip_prefix("tab:")
                        .and_then(|n| n.parse().ok())
                        .unwrap_or(0),
                ),
            };
            let Some(wv) = shell.app.get_webview(&wlabel) else {
                return respond(
                    d,
                    id,
                    &json!({"ok": false, "error": format!("no web view {wlabel}")}),
                );
            };
            let js = cmd["js"].as_str().unwrap_or("null");
            let (dir, rid) = (d.to_path_buf(), id.to_string());
            let r = wv.eval_with_callback(format!("(function(){{{js}\n}})()"), move |value| {
                let parsed: Value = serde_json::from_str(&value).unwrap_or(Value::String(value));
                respond(&dir, &rid, &parsed);
            });
            if let Err(e) = r {
                respond(d, id, &json!({"ok": false, "error": e.to_string()}));
            }
        }
        other => respond(
            d,
            id,
            &json!({"ok": false, "error": format!("unknown op {other}")}),
        ),
    }
}
