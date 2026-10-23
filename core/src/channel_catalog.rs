//! Inline channel browsing: local copies and paged public upload listings.
use crate::content::KIND_META;
use crate::{Core, Error, Result};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::engine::testutil::test_core;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn persistent_pages_refresh_without_duplicate_fetches_or_losing_stale_data() {
        let (dir,core)=test_core(None,None);core.set_now_for_tests(Some(1_000_000_000));
        let tool=dir.path().join("catalogue-fixture");let calls=dir.path().join("calls");
        let script=|title:&str,delay:bool|format!("#!/bin/sh\necho call >> '{}'\n{}\nprintf '%s' '{{\"entries\":[{{\"id\":\"aaaaaaaaaaa\",\"title\":\"{title}\"}}]}}'\n",calls.display(),if delay{"sleep 1"}else{":"});
        std::fs::write(&tool,script("Original",false)).unwrap();std::fs::set_permissions(&tool,std::fs::Permissions::from_mode(0o755)).unwrap();
        core.tools.write().unwrap().ytdlp.path=Some(tool.display().to_string());let id="UCabcdefghijklmnopqrstuv";
        assert_eq!(core.channel_uploads(id,0,false).unwrap()["items"][0]["title"],"Original");
        assert_eq!(core.channel_uploads(id,0,false).unwrap()["cached"],true);assert_eq!(std::fs::read_to_string(&calls).unwrap().lines().count(),1);
        std::fs::write(&tool,script("Updated",true)).unwrap();core.set_now_for_tests(Some(1_000_000_000+25*3_600_000));
        let start=Instant::now();let stale=core.channel_uploads(id,0,false).unwrap();
        assert!(start.elapsed()<Duration::from_millis(500));assert_eq!(stale["items"][0]["title"],"Original");assert_eq!(stale["refreshing"],true);
        for _ in 0..3 {assert_eq!(core.channel_uploads(id,0,false).unwrap()["items"][0]["title"],"Original");}
        while !core.catalog_running.lock().unwrap().is_empty()&&start.elapsed()<Duration::from_secs(5){std::thread::sleep(Duration::from_millis(50));}
        assert_eq!(core.channel_uploads(id,0,false).unwrap()["items"][0]["title"],"Updated");assert_eq!(std::fs::read_to_string(&calls).unwrap().lines().count(),2);
        std::fs::write(&tool,"#!/bin/sh\nexit 1\n").unwrap();let fallback=core.channel_uploads(id,0,true).unwrap();
        assert_eq!(fallback["items"][0]["title"],"Updated");assert!(fallback["refresh_error"].is_string());
        let mut settings=core.video.settings();settings.offline_mode=true;core.apply_settings(&settings).unwrap();drop(core);
        let reopened=Core::open(crate::CoreConfig{data_dir:dir.path().into(),tool_dirs:Some(vec![]),..Default::default()}).unwrap();
        assert!(reopened.is_offline());assert_eq!(reopened.channel_uploads(id,0,true).unwrap()["items"][0]["title"],"Updated");
    }
}

fn valid_channel(id: &str) -> bool {
    id.len() == 24
        && id.starts_with("UC")
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

pub fn upload_items(data: &Value, id: &str) -> Vec<Value> {
    data["entries"].as_array().into_iter().flatten().filter_map(|v|{
        let video=v["id"].as_str().filter(|s|crate::util::valid_video_id(s))?;
        Some(json!({"video_id":video,"title":v["title"].as_str().unwrap_or(video),"channel_id":id,
            "channel":v["channel"].as_str().or_else(||data["channel"].as_str()),"duration_seconds":v["duration"].as_f64(),
            "thumbnail":format!("https://i.ytimg.com/vi/{video}/hqdefault.jpg")}))
    }).take(25).collect()
}

impl Core {
    pub fn channel_library(&self, id: &str) -> Result<Value> {
        if !valid_channel(id) {
            return Err(Error::bad("invalid channel ID"));
        }
        let known = self.known_channels();
        let name = known
            .as_array()
            .and_then(|a| a.iter().find(|c| c["channel_id"] == id))
            .and_then(|c| c["name"].as_str())
            .unwrap_or("");
        let mut seen = HashSet::new();
        let mut items = Vec::new();
        for entry in self.video.list_entries()? {
            let meta = self
                .content
                .get_json(KIND_META, &entry.video_id, 0)?
                .unwrap_or(Value::Null);
            if (meta["channel_id"] == id
                || (!name.is_empty()
                    && entry
                        .channel
                        .as_deref()
                        .is_some_and(|c| c.eq_ignore_ascii_case(name))))
                && seen.insert(entry.video_id.clone())
            {
                let mut item = self.entry_view(&entry);
                item["in_library"] = json!(true);
                items.push(item);
            }
        }
        Ok(json!({"items":items,"source":"library"}))
    }

    pub fn channel_uploads(self: &Arc<Self>, id: &str, offset: usize, refresh: bool) -> Result<Value> {
        if !valid_channel(id) || offset > 10000 {
            return Err(Error::bad("invalid channel or page"));
        }
        let key = format!("channel-videos:{id}:{offset}");
        let cached = self.video.kv_get(&key)?.and_then(|s| serde_json::from_str::<Value>(&s).ok());
        if let Some(mut page) = cached.clone() {
            let stale = self.now() - page["updated_at"].as_i64().unwrap_or(0)
                >= self.video.settings().channel_list_ttl_hours as i64 * 3_600_000;
            page["cached"] = json!(true);
            page["stale"] = json!(stale);
            page["offline"] = json!(self.is_offline());
            if self.is_offline() || (!refresh && !stale) { return Ok(page); }
            if !refresh {
                if self.claim_catalogue(&key, false) {
                    let core = self.clone(); let id = id.to_owned(); let key = key.clone();
                    std::thread::spawn(move || { let _ = core.update_channel_uploads(&id, offset, &key); });
                }
                page["refreshing"] = json!(self.catalog_running.lock().unwrap_or_else(|e|e.into_inner()).contains(&key));
                return Ok(page);
            }
        }
        self.require_online()?;
        if !self.claim_catalogue(&key, refresh) {
            if let Some(mut page) = cached { page["refreshing"] = json!(self.catalog_running.lock().unwrap_or_else(|e|e.into_inner()).contains(&key)); return Ok(page); }
            return Err(Error::Conflict("Channel listings are loading; try again shortly".into()));
        }
        self.update_channel_uploads(id, offset, &key).or_else(|error| {
            if let Some(mut page) = cached { page["refresh_error"] = json!(error.to_string()); Ok(page) }
            else { Err(error) }
        })
    }

    fn claim_catalogue(&self, key: &str, explicit: bool) -> bool {
        let mut running = self.catalog_running.lock().unwrap_or_else(|e|e.into_inner());
        let recent = self.video.kv_get(&format!("catalog-attempt:{key}")).ok().flatten()
            .and_then(|s|s.parse::<i64>().ok()).is_some_and(|at|self.now()-at < 60_000);
        if self.is_offline() || (!explicit&&recent) || running.len() >= 2 || !running.insert(key.to_owned()) { return false; }
        let _ = self.video.kv_put(&format!("catalog-attempt:{key}"), &self.now().to_string());
        true
    }

    fn update_channel_uploads(&self, id: &str, offset: usize, key: &str) -> Result<Value> {
        let result = (|| match self.fetch_channel_uploads(id, offset) {
            Ok(items) => {
                for item in &items {
                    if let Some(video) = item["video_id"].as_str() {
                        if !self.content.has(KIND_META, video, self.now()) {
                            let _ = self.ingest_video(crate::model::VideoIngest {
                                video_id: video.into(),
                                title: item["title"].as_str().unwrap_or(video).into(),
                                channel_id: Some(id.into()),
                                channel: item["channel"].as_str().map(str::to_owned),
                                thumbnail: item["thumbnail"].as_str().map(str::to_owned),
                                duration_seconds: item["duration_seconds"]
                                    .as_f64()
                                    .map(|n| n as u32),
                                ..Default::default()
                            });
                        }
                    }
                }
                let more = items.len() == 25;
                let value = json!({"items":items,"source":"uploads","offset":offset,"next_offset":if more{Some(offset+25)}else{None},"updated_at":self.now()});
                self.video.kv_put(key, &value.to_string())?;
                self.log_event(
                    "info",
                    "channels.videos",
                    json!({"channel_id":id,"offset":offset,"count":items.len()}),
                );
                Ok(value)
            }
            Err(error) => {
                self.log_event(
                    "warn",
                    "channels.videos_failed",
                    json!({"channel_id":id,"error":error.to_string()}),
                );
                Err(error)
            }
        })();
        // A finished refresh must expose its persisted page, not a window where
        // callers see "not running" together with the previous page.
        self.catalog_running.lock().unwrap_or_else(|e| e.into_inner()).remove(key);
        result
    }

    fn fetch_channel_uploads(&self, id: &str, offset: usize) -> Result<Vec<Value>> {
        self.require_online()?;
        let tool = self
            .tools()
            .ytdlp
            .path
            .ok_or_else(|| Error::other("yt-dlp is not ready"))?;
        let mut child = Command::new(tool)
            .args([
                "--ignore-config",
                "--flat-playlist",
                "--skip-download",
                "--dump-single-json",
                "--no-warnings",
                "--playlist-start",
            ])
            .arg((offset + 1).to_string())
            .arg("--playlist-end")
            .arg((offset + 25).to_string())
            .arg(format!("https://www.youtube.com/channel/{id}/videos"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let out = std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = stdout.by_ref().take(8 * 1024 * 1024).read_to_end(&mut b);
            let _ = std::io::copy(&mut stdout, &mut std::io::sink());
            b
        });
        let err = std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = stderr.by_ref().take(32768).read_to_end(&mut b);
            let _ = std::io::copy(&mut stderr, &mut std::io::sink());
            String::from_utf8_lossy(&b).into_owned()
        });
        let started = Instant::now();
        let mut timeout = false;
        let result = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(Some(status)),
                Ok(None)
                    if started.elapsed() < Duration::from_secs(45)
                        && !self.shutdown.load(Ordering::SeqCst) && !self.is_offline() =>
                {
                    std::thread::sleep(Duration::from_millis(100))
                }
                other => {
                    let _ = child.kill();
                    let _ = child.wait();
                    timeout = true;
                    break other.map(|_| None);
                }
            }
        };
        let bytes = out.join().unwrap_or_default();
        let error = err.join().unwrap_or_default();
        if timeout {
            return Err(Error::other("channel listing timed out or stopped"));
        }
        if !result.map_err(Error::from)?.is_some_and(|s| s.success()) {
            return Err(Error::other(
                error.lines().last().unwrap_or("channel listing failed"),
            ));
        }
        let data: Value = serde_json::from_slice(&bytes)?;
        Ok(upload_items(&data, id))
    }
}
