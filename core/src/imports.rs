//! Copy existing media into the library without downloading it again.
use crate::model::{TabIngest, VideoIngest};
use crate::videodb::{EntryRow, STATUS_COMPLETE};
use crate::{Core, Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MediaImport {
    pub path: PathBuf,
    /// Explicit id, otherwise taken from a yt-dlp .info.json sidecar.
    pub video_id: Option<String>,
    pub metadata: Option<PathBuf>,
    #[serde(default)]
    pub move_file: bool,
    #[serde(default)]
    pub enrich: bool,
    #[serde(default)]
    pub dry_run: bool,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TabsImport {
    pub urls: Vec<String>,
    #[serde(default)]
    pub dry_run: bool,
}
impl Core {
    pub fn import_media(self: &Arc<Self>, req: MediaImport) -> Result<Value> {
        let source = req.path.canonicalize()?;
        if !source.is_file() {
            return Err(Error::bad("media path must be a file"));
        }
        let sidecar = req
            .metadata
            .unwrap_or_else(|| source.with_extension("info.json"));
        let info: Value = if sidecar.exists() {
            serde_json::from_slice(&std::fs::read(sidecar)?)?
        } else {
            json!({})
        };
        let id = req
            .video_id
            .or_else(|| info["id"].as_str().map(str::to_owned))
            .ok_or_else(|| Error::bad("supply video_id or a yt-dlp .info.json with id"))?;
        if !crate::util::valid_video_id(&id) {
            return Err(Error::bad("invalid video id"));
        }
        let ext = source
            .extension()
            .and_then(|x| x.to_str())
            .unwrap_or("")
            .to_lowercase();
        if !matches!(
            ext.as_str(),
            "mp4" | "m4v" | "webm" | "mkv" | "mov" | "m4a" | "mp3" | "opus" | "ogg" | "aac" | "wav"
        ) {
            return Err(Error::bad("unsupported media extension"));
        }
        let height = info["height"].as_u64().map(|n| n as u32);
        let audio = matches!(ext.as_str(), "m4a" | "mp3" | "opus" | "ogg" | "aac" | "wav");
        let variant = if audio {
            crate::quality::Quality::Audio
        } else {
            crate::stream::variant_for_height(height.unwrap_or(720))
        };
        let mut row = EntryRow::new(
            &id,
            variant.as_str(),
            variant.as_str(),
            "import",
            self.now(),
        );
        if self.video.get_entry(&row.id)?.is_some() || self.is_running(&row.id) {
            return Err(Error::Conflict(format!(
                "{} already exists; existing media was left untouched",
                row.id
            )));
        }
        let target = self.paths.library_dir.join(format!("{}.{}", row.id, ext));
        let bytes = source.metadata()?.len();
        if bytes == 0 {
            return Err(Error::bad("media file is empty"));
        }
        if req.dry_run {
            return Ok(
                json!({"dry_run":true,"video_id":id,"target":target,"bytes":bytes,"protected":true,"move_file":req.move_file}),
            );
        }
        // create_new prevents overwriting either another import or an unmanaged file.
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)?;
        let copied = std::io::copy(&mut std::fs::File::open(&source)?, &mut output)
            .and_then(|_| output.sync_all());
        if let Err(e) = copied {
            let _ = std::fs::remove_file(&target);
            return Err(e.into());
        }
        row.status = STATUS_COMPLETE.into();
        row.protected = true;
        row.progress = 1.0;
        row.file_path = Some(target.to_string_lossy().into_owned());
        row.ext = Some(ext);
        row.size_bytes = bytes;
        row.actual_height = height;
        row.title = Some(
            info["title"]
                .as_str()
                .unwrap_or_else(|| source.file_stem().and_then(|s| s.to_str()).unwrap_or(&id))
                .into(),
        );
        row.channel = info["channel"]
            .as_str()
            .or(info["uploader"].as_str())
            .map(str::to_owned);
        row.duration_seconds = info["duration"]
            .as_f64()
            .filter(|d| d.is_finite() && *d > 0.0)
            .map(|d| d as u32);
        if let Err(e) = self.ingest_video(VideoIngest {
            video_id: id.clone(),
            title: row.title.clone().unwrap_or_default(),
            channel: row.channel.clone(),
            channel_id: info["channel_id"].as_str().map(str::to_owned),
            duration_seconds: row.duration_seconds,
            description: info["description"].as_str().map(str::to_owned),
            thumbnail: info["thumbnail"].as_str().map(str::to_owned),
            ..Default::default()
        }) {
            let _ = std::fs::remove_file(&target);
            return Err(e);
        }
        if let Err(e) = self.video.insert_entry(&row) {
            let _ = std::fs::remove_file(&target);
            return Err(e);
        }
        let source_removed = req.move_file && std::fs::remove_file(&source).is_ok();
        if req.enrich {
            self.ensure_meta(&id);
        }
        self.log_event(
            "info",
            "import.media",
            json!({"entry_id":row.id,"bytes":bytes,"container":row.ext,"video_codec":info["vcodec"],"audio_codec":info["acodec"],"height":row.actual_height,"source_removed":source_removed}),
        );
        Ok(json!({"ok":true,"entry":self.entry_view(&row),"source_removed":source_removed}))
    }

    /// Import sleeping tabs; do not create a renderer or start playback for each URL.
    pub fn import_tabs(&self, req: TabsImport) -> Result<Value> {
        if req.urls.len() > 10000 {
            return Err(Error::bad("at most 10000 URLs per import"));
        }
        let existing = self.browsing.tabs()?;
        let mut seen: std::collections::HashSet<String> = existing
            .iter()
            .filter(|t| t.state != "closed")
            .map(|t| t.url.clone())
            .collect();
        let mut tabs = Vec::new();
        for url in req.urls {
            let url = url.trim().to_string();
            if url.is_empty() {
                continue;
            }
            if !matches!(
                crate::tabs::classify_input(&url),
                crate::tabs::Input::Yt(_) | crate::tabs::Input::Video(_)
            ) {
                return Err(Error::bad("tab imports accept YouTube URLs only"));
            }
            if !seen.insert(url.clone()) {
                continue;
            }
            tabs.push(TabIngest {
                tab_id: format!("import-{}", crate::util::random_id()),
                video_id: crate::tabs::video_id_of_url(&url),
                url,
                state: "hidden".into(),
                ..Default::default()
            });
        }
        if !req.dry_run {
            for t in &tabs {
                self.browsing.upsert_tab(t, self.now())?;
            }
            // The shell checks this revision and adopts the tabs without reopening existing ones.
            self.video
                .kv_put("tabs_import_revision", &crate::util::random_id())?;
            self.log_event("info", "import.tabs", json!({"count":tabs.len()}));
        }
        Ok(json!({"ok":true,"dry_run":req.dry_run,"count":tabs.len(),"tabs":tabs}))
    }
}
