//! Tauri commands. Two audiences, two capability files:
//! the chrome page (Muninn's own UI) gets the tab and window commands; YouTube
//! pages get only `page_open_video` and `page_state` (src-tauri/capabilities).

use crate::shell::{tab_id_of, Rect, Shell};
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::{State, Webview};

type Sh<'a> = State<'a, Arc<Shell>>;

/// Opens an http(s) URL in the default browser.
pub fn open_in_browser(url: &str) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("only web links can be opened".into());
    }
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(not(target_os = "macos"))]
    let opener = "xdg-open";
    std::process::Command::new(opener)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn app_boot(shell: Sh<'_>) -> Result<Value, String> {
    Ok(json!({
        "token": shell.core.token(), "version": muninn_core::VERSION, "platform": std::env::consts::OS,
        "data_dir": shell.core.paths.data_dir,
    }))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tabs_state(shell: Sh<'_>) -> Result<Value, String> {
    Ok(shell.tabs_state())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tab_new(shell: Sh<'_>, url: Option<String>) -> Result<u64, String> {
    Ok(shell.new_tab(url.as_deref()))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tab_activate(shell: Sh<'_>, id: u64) -> Result<(), String> {
    shell.activate(id);
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tab_close(shell: Sh<'_>, id: u64) -> Result<(), String> {
    shell.close_tab(id);
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tab_go(shell: Sh<'_>, id: u64, input: String) -> Result<(), String> {
    shell.go(id, &input);
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tab_back(shell: Sh<'_>, id: u64) -> Result<(), String> {
    shell.back(id);
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tab_forward(shell: Sh<'_>, id: u64) -> Result<(), String> {
    shell.forward(id);
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tab_reload(shell: Sh<'_>, id: u64) -> Result<(), String> {
    shell.reload(id);
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tab_open_video(
    shell: Sh<'_>,
    id: u64,
    video_id: String,
    background: Option<bool>,
    playlist: Option<muninn_core::tabs::PlaylistContext>,
) -> Result<(), String> {
    if !muninn_core::util::valid_video_id(&video_id) {
        return Err("invalid video id".into());
    }
    let loc = match playlist {
        Some(playlist) if muninn_core::playlist::valid_list_id(&playlist.list_id) => {
            muninn_core::tabs::Loc::WatchInPlaylist { video_id, playlist }
        }
        Some(_) => return Err("invalid playlist context".into()),
        None => muninn_core::tabs::Loc::Watch(video_id),
    };
    shell.open_video_loc(id, loc, background.unwrap_or(false));
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn tab_hibernate(shell: Sh<'_>, id: u64) -> Result<(), String> {
    shell.hibernate(id);
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn layout(
    shell: Sh<'_>,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    viewport_width: Option<f64>,
    viewport_height: Option<f64>,
) -> Result<(), String> {
    shell.set_viewport_rect(Rect { x, y, w, h }, viewport_width, viewport_height);
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn native_visible(shell: Sh<'_>, visible: bool) -> Result<(), String> {
    shell.set_native_visible(visible);
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn open_external(url: String) -> Result<(), String> {
    open_in_browser(&url)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn trust_status(shell: Sh<'_>) -> Result<Value, String> {
    Ok(serde_json::to_value(crate::trust::status(&shell)).unwrap_or(Value::Null))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn trust_install(shell: Sh<'_>) -> Result<(), String> {
    let shell = shell.inner().clone();
    tauri::async_runtime::spawn_blocking(move || crate::trust::install(&shell))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn signin_popup(shell: Sh<'_>) -> Result<(), String> {
    shell.open_signin_window()
}

/// Asks where to save the support ZIP. `Ok(None)` = the user cancelled.
#[cfg(target_os = "macos")]
fn choose_export_path() -> Result<Option<std::path::PathBuf>, String> {
    let script = format!(
        "POSIX path of (choose file name with prompt \"Save Muninn diagnostics\" default name \"{}\")",
        muninn_core::diagnostics::default_export_name_today()
    );
    let out = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .map_err(|e| e.to_string())?;
    muninn_core::diagnostics::parse_save_dialog(
        out.status.success(),
        &String::from_utf8_lossy(&out.stdout),
        &String::from_utf8_lossy(&out.stderr),
    )
}

#[cfg(not(target_os = "macos"))]
fn choose_export_path() -> Result<Option<std::path::PathBuf>, String> {
    Err("Saving diagnostics from the app is only available on macOS. Use: muninn --cli diagnostics export FILE.zip".into())
}

/// Developer mode "Export diagnostics": save dialog, then the same redacted export the CLI uses.
/// Resolves to {status:"cancelled"} or {status:"saved", path}. Runs off the UI thread.
#[tauri::command(rename_all = "snake_case")]
pub async fn export_diagnostics(shell: Sh<'_>) -> Result<Value, String> {
    let data_dir = shell.core.paths.data_dir.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let destination = choose_export_path()?;
        muninn_core::diagnostics::export_to(&data_dir, destination.as_deref())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

// --- called by the YouTube pages (inject.js) ------------------------------------------------------
#[tauri::command(rename_all = "snake_case")]
pub async fn channel_unsubscribe(shell: Sh<'_>, channel_id: String) -> Result<(), String> {
    let shell = shell.inner().clone();
    tauri::async_runtime::spawn_blocking(move || shell.unsubscribe_channel(&channel_id))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn page_open_video(
    webview: Webview,
    shell: Sh<'_>,
    video_id: String,
    background: Option<bool>,
) -> Result<(), String> {
    let id = tab_id_of(webview.label()).ok_or("not a tab")?;
    if !muninn_core::util::valid_video_id(&video_id) {
        return Err("invalid video id".into());
    }
    shell.open_video(id, &video_id, background.unwrap_or(false));
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn page_state(
    webview: Webview,
    shell: Sh<'_>,
    url: String,
    title: String,
    scroll_y: f64,
    channels: Option<Vec<muninn_core::subscriptions::DiscoveredChannel>>,
    channel_status: Option<String>,
    signed_in: Option<bool>,
    account_key: Option<String>,
    account_changed_at: Option<i64>,
) -> Result<(), String> {
    let id = tab_id_of(webview.label()).ok_or("not a tab")?;
    shell.page_state(id, &url, &title, scroll_y);
    let actual = webview.url().map_err(|e| e.to_string())?;
    let youtube = actual.scheme() == "https"
        && actual
            .host_str()
            .is_some_and(|host| host == "www.youtube.com" || host == "youtube.com");
    let mut account_current = false;
    if youtube {
        if let (Some(signed_in), Some(changed_at)) = (signed_in, account_changed_at) {
            account_current = shell
                .core
                .observe_youtube_account(signed_in, account_key.as_deref(), changed_at)
                .map_err(|e| e.to_string())?;
        }
    }
    if let Some(channels) = channels {
        if youtube && account_current && actual.path() == "/feed/channels" {
            shell
                .core
                .ingest_channel_candidates(
                    &channels,
                    channel_status.as_deref().unwrap_or("loading"),
                    account_key.as_deref(),
                )
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

// --- test automation (only answers when MUNINN_E2E_DIR is set) -------------------------------------

#[tauri::command(rename_all = "snake_case")]
pub async fn e2e_result(id: String, value: String) -> Result<(), String> {
    crate::e2e::store_result(&id, value)
}
