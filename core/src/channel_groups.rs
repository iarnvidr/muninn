//! Shared channel policies. A per-channel subscription overrides its group's
//! policy; otherwise group members inherit policy edits on the next upload check.
use crate::quality::Quality;
use crate::subscriptions::Channel;
use crate::{Core, Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelGroup {
    #[serde(default)]
    pub favourite: bool,
    pub name: String,
    pub channel_ids: Vec<String>,
    pub quality: Quality,
    pub retention_days: u32,
    pub keep: bool,
    pub enabled: bool,
    pub poll_minutes: u32,
    pub max_per_poll: usize,
}

impl Core {
    pub fn channel_groups(&self) -> Vec<ChannelGroup> {
        self.video
            .kv_get("channel-groups")
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn put_channel_groups(&self, groups: &[ChannelGroup]) -> Result<()> {
        let _lock = self.channel_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut names = HashSet::new();
        let mut members = HashSet::new();
        if groups.len() > 100 {
            return Err(Error::bad("at most 100 channel groups"));
        }
        for group in groups {
            if group.name.trim().is_empty()
                || group.name.trim() != group.name
                || group.name.chars().count() > 80
                || group.name.chars().any(char::is_control)
                || !names.insert(&group.name)
                || group.quality == Quality::None
                || group.retention_days > 3650
                || !(5..=10080).contains(&group.poll_minutes)
                || !(1..=15).contains(&group.max_per_poll)
            {
                return Err(Error::bad("invalid channel group name or download policy"));
            }
            for id in &group.channel_ids {
                if id.len() != 24
                    || !id.starts_with("UC")
                    || !id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
                    || !members.insert(id)
                    || members.len() > 1000
                {
                    return Err(Error::bad(
                        "channel IDs must be valid and belong to at most one channel group",
                    ));
                }
            }
        }
        self.video
            .kv_put("channel-groups", &serde_json::to_string(groups)?)?;
        self.wake_scheduler();
        Ok(())
    }

    pub fn effective_subscriptions(&self) -> Vec<Channel> {
        let mut effective = BTreeMap::new();
        for group in self.channel_groups() {
            for id in group.channel_ids {
                effective.insert(
                    id.clone(),
                    Channel {
                        channel_id: id,
                        name: String::new(),
                        quality: group.quality,
                        retention_days: group.retention_days,
                        keep: group.keep,
                        enabled: group.enabled && (!group.favourite || self.video.settings().favourite_downloads),
                        poll_minutes: group.poll_minutes,
                        max_per_poll: group.max_per_poll,
                    },
                );
            }
        }
        for channel in self.subscriptions() {
            effective.insert(channel.channel_id.clone(), channel);
        }
        effective.into_values().collect()
    }
}
