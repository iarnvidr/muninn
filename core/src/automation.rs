//! Declarative completion policies. Same JSON contract for the UI, API and CLI.
use crate::rules::{Match, VideoFacts};
use crate::{Core, Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Automation {
    pub cleanup_completed: bool,
    /// Grace period after the player reaches the configured completion threshold.
    pub cleanup_delay_seconds: u32,
    pub completion_percent: u32,
    pub completion_rules: Vec<CompletionRule>,
}
impl Default for Automation {
    fn default() -> Self {
        Self {
            cleanup_completed: true,
            cleanup_delay_seconds: 86400,
            completion_percent: 95,
            completion_rules: Vec::new(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletionRule {
    pub name: String,
    #[serde(default, rename = "match")]
    pub matcher: Match,
    pub action: CompletionAction,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionAction {
    Delete,
    Keep,
}

impl Automation {
    pub fn action(&self, facts: &VideoFacts) -> Option<CompletionAction> {
        self.completion_rules
            .iter()
            .find(|r| r.matcher.matches(facts))
            .map(|r| r.action)
            .or(self.cleanup_completed.then_some(CompletionAction::Delete))
    }
}

impl Core {
    /// Upgrade only the old shipped defaults; leave user-authored rules intact.
    pub(crate) fn migrate_watch_retention(&self) -> Result<()> {
        if self.video.kv_get("watch-retention-v1")?.is_some() { return Ok(()); }
        let mut rules = self.video.rules();
        for seed in crate::rules::seed_rules() {
            let old_days = match seed.id.as_str() {
                "seed-talking-head" => 7, "seed-podcasts"|"seed-favorite-channels" => 14,
                "seed-music" => 30, "seed-everything-else" => 3, _ => continue,
            };
            let mut old = seed.clone(); old.retention_days = old_days;
            if let Some(rule) = rules.iter_mut().find(|r| **r == old) {
                *rule = seed.clone();
                for entry in self.video.list_entries()? {
                    if !entry.protected && entry.rule.as_deref() == Some(seed.name.as_str()) && entry.retention_days == Some(old_days) {
                        self.video.set_expiry(&entry.id, None, Some(0), None)?;
                    }
                }
            }
        }
        self.video.put_rules(&rules)?;
        if let Some(raw) = self.video.kv_get("automation")? {
            if let Ok(mut value) = serde_json::from_str::<Value>(&raw) {
                if value == json!({"cleanup_completed":true,"cleanup_delay_seconds":60,"completion_rules":[]}) {
                    value["cleanup_delay_seconds"] = json!(86400);
                    value["completion_percent"] = json!(95);
                    self.video.kv_put("automation", &value.to_string())?;
                }
            }
        }
        self.video.kv_put("watch-retention-v1","1")
    }
    pub fn playback_active(&self, video_id: &str) -> bool {
        self.playback_leases
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(video_id)
            .is_some_and(|until| *until > self.now())
    }
    pub fn automation(&self) -> Automation {
        self.video
            .kv_get("automation")
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    /// The effective watched/cleanup policy displayed by the player.
    pub(crate) fn completion_policy(&self, video_id: &str) -> Value {
        let config = self.automation();
        json!({
            "completion_percent": config.completion_percent,
            "cleanup_delay_seconds": config.cleanup_delay_seconds,
            "action": config.action(&self.facts_for(video_id)),
        })
    }
    pub fn put_automation(&self, config: &Automation) -> Result<()> {
        if config.cleanup_delay_seconds > 30 * 86400 || !(50..=100).contains(&config.completion_percent) || config.completion_rules.len() > 200 {
            return Err(Error::bad(
                "cleanup delay must be <= 30 days, completion 50..100 percent, and at most 200 completion rules are allowed",
            ));
        }
        for r in &config.completion_rules {
            if r.name.trim().is_empty()
                || matches!((r.matcher.min_duration_seconds,r.matcher.max_duration_seconds),(Some(a),Some(b)) if a>b)
            {
                return Err(Error::bad(
                    "completion rules need a name and valid duration bounds",
                ));
            }
        }
        self.video
            .kv_put("automation", &serde_json::to_string(config)?)?;
        self.log_event(
            "info",
            "automation.updated",
            json!({"rules":config.completion_rules.len()}),
        );
        self.wake_scheduler();
        Ok(())
    }
    pub(crate) fn completions(&self) -> BTreeMap<String, i64> {
        self.video
            .kv_get("completed")
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    pub fn mark_completed(&self, video_id: &str) -> Result<()> {
        let _guard = self
            .completion_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut pending = self.completions();
        if pending.contains_key(video_id) { return Ok(()); }
        pending.insert(video_id.into(), self.now());
        self.video
            .kv_put("completed", &serde_json::to_string(&pending)?)?;
        self.log_event("info", "playback.completed", json!({"video_id":video_id}));
        Ok(())
    }
    pub fn resume_cleanup(&self, video_id: &str) -> Result<()> {
        let _guard = self
            .completion_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut pending = self.completions();
        if pending.remove(video_id).is_some() {
            self.video
                .kv_put("completed", &serde_json::to_string(&pending)?)?;
        }
        Ok(())
    }
    pub fn cleanup_preview(&self) -> Result<Value> {
        let config = self.automation();
        let pending = self.completions();
        let mut result = Vec::new();
        for (id, completed_at) in pending {
            let action = config.action(&self.facts_for(&id));
            for e in self.video.entries_for_video(&id)? {
                result.push(json!({"entry_id":e.id,"video_id":id,"protected":e.protected,
                    "action":if e.protected {Some(CompletionAction::Keep)} else {action},
                    "due_at":completed_at + config.cleanup_delay_seconds as i64*1000,
                    "active":self.is_running(&e.id)||self.playback_active(&e.video_id),"size_bytes":e.size_bytes}));
            }
        }
        Ok(json!(result))
    }
    pub fn cleanup_completed(&self) -> Result<()> {
        let _guard = self
            .completion_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let config = self.automation();
        let mut pending = self.completions();
        let mut finished = Vec::new();
        for (id, completed_at) in &pending {
            if self.now() < completed_at + config.cleanup_delay_seconds as i64 * 1000 {
                continue;
            }
            let entries = self.video.entries_for_video(id)?;
            // A stream must finish publishing its output before it can be cleaned.
            if self.playback_active(id) || entries.iter().any(|e| self.is_running(&e.id)) {
                continue;
            }
            let action = config.action(&self.facts_for(id));
            for e in entries {
                if e.protected {
                    continue;
                }
                match action {
                    Some(CompletionAction::Keep) => {
                        self.promote(&e.id)?;
                    }
                    Some(CompletionAction::Delete) => {
                        self.evict_for(&e.id, false, "completed_cleanup")?;
                    }
                    None => {}
                }
            }
            if action == Some(CompletionAction::Delete) {
                // HLS is a second copy of the media and must go too.
                let mut jobs = self.streams.jobs.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(job) = jobs.remove(id) {
                    let _ = std::fs::remove_dir_all(&job.hls_dir);
                }
                self.streams
                    .sources
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .retain(|k, _| !k.starts_with(&format!("{id}/")));
            }
            finished.push(id.clone());
        }
        for id in finished {
            pending.remove(&id);
        }
        self.video
            .kv_put("completed", &serde_json::to_string(&pending)?)?;
        Ok(())
    }
}
