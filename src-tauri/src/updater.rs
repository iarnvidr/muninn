//! Signature-verified, chrome-only updater lifecycle.
use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use tauri::{AppHandle, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

pub const ENDPOINT: &str =
    "https://github.com/iarnvidr/muninn/releases/latest/download/latest.json";

/// Isolated updater fixture overrides are intentionally unavailable in release builds.
pub fn plugin_builder() -> tauri::plugin::TauriPlugin<tauri::Wry, tauri_plugin_updater::Config> {
    // Endpoint and key are pinned in the application configuration. Overrides and
    // comparator policy are configured on each updater operation below.
    tauri_plugin_updater::Builder::new().build()
}

// GitHub's latest endpoint excludes prereleases. Preview discovery is explicit;
// only this repository's fixed-tag manifest is handed to the signature verifier.
fn release_endpoint(releases: &serde_json::Value, previews: bool) -> Result<String, String> {
    let release = releases.as_array().ok_or("Invalid release listing")?
        .iter()
        .filter(|r| r["draft"] == false && (previews || r["prerelease"] == false))
        .filter_map(|r| {
            let tag = r["tag_name"].as_str()?;
            let version = semver::Version::parse(tag.strip_prefix('v')?).ok()?;
            if !version.pre.is_empty() || tag != format!("v{version}") {
                return None;
            }
            let manifest = format!("https://github.com/iarnvidr/muninn/releases/download/{tag}/latest.json");
            let has_manifest = r["assets"].as_array()?.iter().any(|a|
                a["name"] == "latest.json" && a["browser_download_url"] == manifest);
            has_manifest.then_some((version, manifest))
        })
        .max_by(|a, b| a.0.cmp(&b.0));
    release.map(|(_, endpoint)| if previews { endpoint } else { ENDPOINT.into() })
        .ok_or_else(|| if previews {
            "No published update is available yet.".into()
        } else {
            "No stable release has been published yet. Select ‘Stable and previews’ to receive preview updates.".into()
        })
}

async fn configured_updater(app: &AppHandle, previews: bool) -> Result<tauri_plugin_updater::Updater, String> {
    let mut builder = app.updater_builder();
    let smoke_mode = debug_smoke_mode();
    let smoke_current = smoke_mode.then(current_version);
    if !smoke_mode {
        let endpoint = tauri::async_runtime::spawn_blocking(move || {
            let response = ureq::get("https://api.github.com/repos/iarnvidr/muninn/releases?per_page=100")
                .set("User-Agent", "Muninn updater")
                .set("Accept", "application/vnd.github+json")
                .timeout(std::time::Duration::from_secs(20))
                .call().map_err(|_| "Could not check releases. Check your connection and try again.".to_string())?;
            use std::io::Read;
            let mut bytes = Vec::new();
            response.into_reader().take(2 * 1024 * 1024 + 1).read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            if bytes.len() > 2 * 1024 * 1024 { return Err("Release listing is too large".into()); }
            let releases = serde_json::from_slice(&bytes).map_err(|_| "Invalid release listing".to_string())?;
            release_endpoint(&releases, previews)
        }).await.map_err(|e| e.to_string())??;
        builder = builder.endpoints(vec![url::Url::parse(&endpoint).map_err(|e| e.to_string())?])
            .map_err(|e| e.to_string())?;
    }
    #[cfg(debug_assertions)]
    if debug_smoke_mode() {
        if let (Ok(endpoint), Ok(pubkey)) = (
            std::env::var("MUNINN_UPDATER_SMOKE_ENDPOINT"),
            std::env::var("MUNINN_UPDATER_SMOKE_PUBKEY"),
        ) {
            if let Ok(url) = url::Url::parse(&endpoint) {
                builder = builder.endpoints(vec![url]).map_err(|e| e.to_string())?;
                builder = builder.pubkey(pubkey);
            }
        }
    }
    Ok(builder
        .version_comparator(move |installed, release| {
            let installed_version = installed.to_string();
            let installed = smoke_current.as_deref().unwrap_or(&installed_version);
            compare_versions_policy(installed, &release.version.to_string(), smoke_mode)
                .is_ok_and(|order| order == std::cmp::Ordering::Greater)
        })
        .build()
        .map_err(|e| e.to_string())?)
}
#[cfg(debug_assertions)]
fn debug_smoke_mode() -> bool {
    std::env::args().any(|a| a == "--updater-smoke")
        && std::env::var_os("MUNINN_NATIVE_UI_SMOKE").is_some()
}
#[cfg(not(debug_assertions))]
fn debug_smoke_mode() -> bool {
    false
}
fn error_cause_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    // Keep diagnostics useful but bounded; updater endpoints are the pinned public
    // feed or the isolated loopback smoke feed, never user-provided URLs in release.
    for _ in 0..7 {
        let Some(cause) = source else { break };
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

fn signature_error_reason(error: &tauri_plugin_updater::Error) -> Option<&'static str> {
    use tauri_plugin_updater::Error;
    match error {
        Error::Minisign(_)
        | Error::Base64(_)
        | Error::SignatureUtf8(_)
        | Error::SignedVersionMismatch { .. }
        | Error::MissingSignedVersion => Some("signature_verification_failed"),
        _ => None,
    }
}

fn current_version() -> String {
    if debug_smoke_mode() {
        if let Ok(version) = std::env::var("MUNINN_UPDATER_SMOKE_CURRENT_VERSION") {
            return version;
        }
    }
    env!("CARGO_PKG_VERSION").to_string()
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum UpdateStatus {
    Idle,
    Checking,
    Current {
        version: String,
    },
    Available {
        version: String,
        notes: Option<String>,
        date: Option<String>,
    },
    Downloading {
        version: String,
        downloaded: u64,
        content_length: Option<u64>,
    },
    ReadyToInstall {
        version: String,
    },
    Installing {
        version: String,
    },
    Unavailable {
        reason: String,
    },
    Failed {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}
impl Default for UpdateStatus {
    fn default() -> Self {
        Self::Idle
    }
}
#[derive(Default)]
pub struct UpdaterState {
    busy: AtomicBool,
    generation: AtomicU64,
    pending: Mutex<Option<(u64, Update, Vec<u8>)>>,
    status: Mutex<UpdateStatus>,
}
impl UpdaterState {
    pub fn status(&self) -> UpdateStatus {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    fn set(&self, s: UpdateStatus) {
        *self.status.lock().unwrap_or_else(|e| e.into_inner()) = s;
    }
    fn begin(&self) -> Result<u64, String> {
        self.busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "an update operation is already in progress".to_string())?;
        Ok(self.generation.fetch_add(1, Ordering::AcqRel) + 1)
    }
    fn finish(&self) {
        self.busy.store(false, Ordering::Release);
    }
    fn active(&self, g: u64) -> bool {
        self.generation.load(Ordering::Acquire) == g && self.busy.load(Ordering::Acquire)
    }
}
async fn wait_cancelled(state: Arc<UpdaterState>, generation: u64) {
    while state.active(generation) {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}
fn compare_versions_policy(
    current: &str,
    candidate: &str,
    allow_smoke_prerelease: bool,
) -> Result<std::cmp::Ordering, String> {
    let a = semver::Version::parse(current.trim_start_matches('v'))
        .map_err(|e| format!("invalid installed version: {e}"))?;
    let b = semver::Version::parse(candidate.trim_start_matches('v'))
        .map_err(|e| format!("invalid release version: {e}"))?;
    // Preview releases are never production updates. The explicit smoke policy
    // permits only smoke-to-smoke comparisons; semver ordering still determines
    // whether the smoke build is newer (and prevents cross-family upgrades).
    if !b.pre.is_empty() && (!allow_smoke_prerelease || !b.pre.as_str().starts_with("smoke.")) {
        return Ok(std::cmp::Ordering::Less);
    }
    if allow_smoke_prerelease && !a.pre.is_empty() && !a.pre.as_str().starts_with("smoke.") {
        return Ok(std::cmp::Ordering::Less);
    }
    Ok(b.cmp(&a))
}
fn compare_versions(current: &str, candidate: &str) -> Result<std::cmp::Ordering, String> {
    compare_versions_policy(current, candidate, debug_smoke_mode())
}
fn trusted(w: &tauri::Webview) -> Result<(), String> {
    if w.label() == "chrome" {
        Ok(())
    } else {
        Err("updater commands are restricted to the Muninn UI".into())
    }
}
#[tauri::command]
pub async fn updater_status(
    webview: tauri::Webview,
    state: tauri::State<'_, Arc<UpdaterState>>,
) -> Result<UpdateStatus, String> {
    trusted(&webview)?;
    Ok(state.status())
}
#[tauri::command]
pub async fn updater_check(
    app: AppHandle,
    webview: tauri::Webview,
    state: tauri::State<'_, Arc<UpdaterState>>,
    include_previews: Option<bool>,
) -> Result<UpdateStatus, String> {
    trusted(&webview)?;
    if let Some(shell) = app.try_state::<Arc<crate::shell::Shell>>() {
        if shell.core.is_offline() {
            state.set(UpdateStatus::Unavailable {
                reason: "updates are unavailable in offline mode".into(),
            });
            return Ok(state.status());
        }
    }
    let g = state.begin()?;
    state
        .pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    state.set(UpdateStatus::Checking);
    let state = state.inner().clone();
    tauri::async_runtime::spawn(async move {
        let result = async {
            let update = configured_updater(&app, include_previews.unwrap_or(false)).await?
                .check()
                .await
                .map_err(|e| format!("updater check failed: {}", error_cause_chain(&e)))?;
            let Some(update) = update else {
                return Ok(UpdateStatus::Current { version: current_version() });
            };
            match compare_versions(&current_version(), &update.version)? {
                std::cmp::Ordering::Greater => Ok(UpdateStatus::Available {
                    version: update.version,
                    notes: update.body,
                    date: update.date.map(|d| d.to_string()),
                }),
                _ => Ok(UpdateStatus::Current {
                    version: current_version(),
                }),
            }
        };
        let result =
            tokio::select! { r=result => Some(r), _=wait_cancelled(state.clone(),g) => None };
        if state.active(g) {
            if let Some(result) = result {
                state.set(result.unwrap_or_else(|e| UpdateStatus::Unavailable { reason: e }));
                state.finish();
            }
        }
    });
    Ok(UpdateStatus::Checking)
}
#[tauri::command]
pub async fn updater_download(
    app: AppHandle,
    webview: tauri::Webview,
    state: tauri::State<'_, Arc<UpdaterState>>,
    include_previews: Option<bool>,
) -> Result<UpdateStatus, String> {
    trusted(&webview)?;
    if let Some(shell) = app.try_state::<Arc<crate::shell::Shell>>() {
        if shell.core.is_offline() {
            state.set(UpdateStatus::Unavailable {
                reason: "updates are unavailable in offline mode".into(),
            });
            return Ok(state.status());
        }
    }
    let g = state.begin()?;
    let state = state.inner().clone();
    state
        .pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    state.set(UpdateStatus::Checking);
    tauri::async_runtime::spawn(async move {
        let result = async {
            let update = configured_updater(&app, include_previews.unwrap_or(false)).await
                .map_err(|e| (e, None))?
                .check()
                .await
                .map_err(|e| {
                    let reason = signature_error_reason(&e).map(str::to_string);
                    (e.to_string(), reason)
                })?
                .ok_or(("no valid signed update available".to_string(), None))?;
            if compare_versions(&current_version(), &update.version).map_err(|e| (e, None))?
                != std::cmp::Ordering::Greater
            {
                return Err(("release is not newer than this version".into(), None));
            }
            let version = update.version.clone();
            let total = Arc::new(AtomicU64::new(0));
            let progress = total.clone();
            state.set(UpdateStatus::Downloading {
                version: version.clone(),
                downloaded: 0,
                content_length: None,
            });
            let bytes = update
                .download(
                    |n, len| {
                        let downloaded = progress.fetch_add(n as u64, Ordering::AcqRel) + n as u64;
                        if state.active(g) {
                            state.set(UpdateStatus::Downloading {
                                version: version.clone(),
                                downloaded,
                                content_length: len,
                            });
                        }
                    },
                    || {},
                )
                .await
                .map_err(|e| {
                    let reason = signature_error_reason(&e).map(str::to_string);
                    (e.to_string(), reason)
                })?;
            Ok::<_, (String, Option<String>)>((update, bytes, version))
        };
        let result =
            tokio::select! { r=result => Some(r), _=wait_cancelled(state.clone(),g) => None };
        if !state.active(g) {
            return;
        }
        match result {
            Some(Ok((update, bytes, version))) => {
                *state.pending.lock().unwrap_or_else(|e| e.into_inner()) = Some((g, update, bytes));
                state.set(UpdateStatus::ReadyToInstall { version })
            }
            Some(Err((message, reason))) => state.set(UpdateStatus::Failed { message, reason }),
            None => return,
        }
        state.finish();
    });
    Ok(UpdateStatus::Checking)
}
#[tauri::command]
pub async fn updater_install(
    app: AppHandle,
    webview: tauri::Webview,
    state: tauri::State<'_, Arc<UpdaterState>>,
) -> Result<UpdateStatus, String> {
    trusted(&webview)?;
    if let Some(shell) = app.try_state::<Arc<crate::shell::Shell>>() {
        if shell.core.is_offline() {
            state.set(UpdateStatus::Unavailable {
                reason: "updates are unavailable in offline mode".into(),
            });
            return Ok(state.status());
        }
    }
    let g = state.begin()?;
    let pending = state
        .pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .ok_or_else(|| {
            state.finish();
            "download a verified update before installing".to_string()
        })?;
    if pending.0 != g.wrapping_sub(1) && pending.0 != state.generation.load(Ordering::Acquire) {
        state.finish();
        return Err("download expired".into());
    }
    let (_, update, bytes) = pending;
    let comparison = match compare_versions(&current_version(), &update.version) {
        Ok(order) => order,
        Err(error) => {
            state.finish();
            return Err(error);
        }
    };
    if comparison != std::cmp::Ordering::Greater {
        state.finish();
        return Err("release is not newer than this version".into());
    }
    let version = update.version.clone();
    state.set(UpdateStatus::Installing { version });
    update.install(&bytes).map_err(|e| {
        state.finish();
        e.to_string()
    })?;
    if let Some(shell) = app.try_state::<Arc<crate::shell::Shell>>() {
        shell.core.shutdown();
    }
    app.restart();
}
#[tauri::command]
pub async fn updater_cancel(
    webview: tauri::Webview,
    state: tauri::State<'_, Arc<UpdaterState>>,
) -> Result<(), String> {
    trusted(&webview)?;
    state.generation.fetch_add(1, Ordering::AcqRel);
    state
        .pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    state.set(UpdateStatus::Idle);
    state.finish();
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn published_preview_requires_opt_in_and_uses_repository_manifest() {
        let release = |tag: &str, preview: bool, draft: bool| serde_json::json!({
            "tag_name": tag, "prerelease": preview, "draft": draft,
            "assets": [{"name": "latest.json", "browser_download_url":
                format!("https://github.com/iarnvidr/muninn/releases/download/{tag}/latest.json")}]
        });
        let previews = serde_json::json!([release("v0.2.16", true, false)]);
        assert!(release_endpoint(&previews, false).unwrap_err().contains("No stable release"));
        assert!(release_endpoint(&previews, true).unwrap().ends_with("/v0.2.16/latest.json"));
        let mixed = serde_json::json!([
            release("v0.2.17", true, false), release("v9.0.0", true, true),
            release("v0.2.16", false, false), release("v0.2.15", true, false)
        ]);
        assert_eq!(release_endpoint(&mixed, false).unwrap(), ENDPOINT);
        assert!(release_endpoint(&mixed, true).unwrap().ends_with("/v0.2.17/latest.json"));
        let mut foreign = release("v0.2.18", true, false);
        foreign["assets"][0]["browser_download_url"] = "https://example.com/latest.json".into();
        assert!(release_endpoint(&serde_json::json!([foreign]), true).is_err());
        assert!(release_endpoint(&serde_json::json!([release("v../escape", true, false)]), true).is_err());
        assert!(release_endpoint(&serde_json::json!([]), true).is_err());
    }
    #[test]
    fn only_greater_is_eligible() {
        assert_eq!(
            compare_versions("1.2.0", "1.2.1").unwrap(),
            std::cmp::Ordering::Greater
        );
        assert_ne!(
            compare_versions("1.2.0", "1.2.0").unwrap(),
            std::cmp::Ordering::Greater
        );
        assert_ne!(
            compare_versions("1.2.0", "1.1.9").unwrap(),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_versions("1.2.0-preview.3", "1.2.0").unwrap(),
            std::cmp::Ordering::Greater
        );
    }
    #[test]
    fn prereleases_are_gated_by_smoke_policy() {
        assert_ne!(
            compare_versions_policy("0.2.14", "0.2.14-smoke.2", false).unwrap(),
            std::cmp::Ordering::Greater
        );
        assert_ne!(
            compare_versions_policy("0.2.14", "0.2.14-smoke.2", true).unwrap(),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_versions_policy("0.2.14-smoke.1", "0.2.14-smoke.2", true).unwrap(),
            std::cmp::Ordering::Greater
        );
        assert_ne!(
            compare_versions_policy("0.2.14-smoke.3", "0.2.14-smoke.2", true).unwrap(),
            std::cmp::Ordering::Greater
        );
        assert_ne!(
            compare_versions_policy("0.2.15", "0.2.14-smoke.2", true).unwrap(),
            std::cmp::Ordering::Greater
        );
        assert_ne!(
            compare_versions_policy("0.2.14-smoke.2", "0.2.14-smoke.2", true).unwrap(),
            std::cmp::Ordering::Greater
        );
    }
    #[test]
    fn malformed_rejected() {
        assert!(compare_versions("garbage", "1.0.0").is_err());
        assert!(compare_versions("1.0.0", "garbage").is_err());
    }
    #[test]
    fn comparator_uses_supplied_current_version() {
        assert_eq!(
            compare_versions_policy("0.2.14-smoke.2", "0.2.14-smoke.3", true).unwrap(),
            std::cmp::Ordering::Greater
        );
        assert_ne!(
            compare_versions("0.2.14-preview.4", "0.2.14-smoke.2").unwrap(),
            std::cmp::Ordering::Greater
        );
    }
}
