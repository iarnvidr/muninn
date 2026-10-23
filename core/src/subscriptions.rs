//! Public channel upload polling, independent of Google sign-in. Explicit channel IDs only.
use crate::model::VideoIngest;
use crate::quality::Quality;
use crate::rules::Decision;
use crate::{Core, Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::sync::{atomic::Ordering, Arc};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Channel {
    pub channel_id: String,
    pub quality: Quality,
    #[serde(default)]
    pub retention_days: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub keep: bool,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "interval")]
    pub poll_minutes: u32,
    #[serde(default = "limit")]
    pub max_per_poll: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredChannel {
    pub channel_id: String,
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signed_in_channel_renderers_become_named_candidates_without_enabling_downloads() {
        let (_d, core) = crate::engine::testutil::test_core(None, None);
        let payload = json!({"contents":[{"channelRenderer":{"channelId":"UCabcdefghijklmnopqrstuv","title":{"simpleText":"Example channel"}}},{"guideEntryRenderer":{"title":{"simpleText":"Second channel"},"navigationEndpoint":{"browseEndpoint":{"browseId":"UCzyxwvutsrqponmlkjihgfe"}}}}]});
        core.discover_channels(&payload, true).unwrap();
        let known = core.known_channels();
        let known = known.as_array().unwrap();
        assert_eq!(known.len(), 2);
        assert_eq!(known[0]["name"], "Example channel");
        assert_eq!(known[0]["source"], "subscriptions");
        assert!(
            core.subscriptions().is_empty(),
            "Discovery must not opt every subscription into downloads"
        );
    }
}
fn yes() -> bool {
    true
}
fn interval() -> u32 {
    60
}
fn limit() -> usize {
    3
}

fn tag<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    s.split_once(&format!("<{name}>"))?
        .1
        .split_once(&format!("</{name}>"))
        .map(|p| p.0)
}
fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}
pub fn parse_feed(s: &str, channel: &str) -> Vec<VideoIngest> {
    s.split("<entry>")
        .skip(1)
        .filter_map(|e| {
            let id = tag(e, "yt:videoId")?;
            crate::util::valid_video_id(id).then(|| VideoIngest {
                video_id: id.into(),
                title: unescape(tag(e, "title").unwrap_or(id)),
                channel_id: Some(channel.into()),
                channel: tag(e, "name").map(unescape),
                published_text: tag(e, "published").map(str::to_owned),
                ..Default::default()
            })
        })
        .collect()
}
impl Core {
    pub fn ingest_channel_candidates(
        &self,
        channels: &[DiscoveredChannel],
        phase: &str,
        expected_account: Option<&str>,
    ) -> Result<()> {
        if channels.len() > 1000
            || !matches!(
                phase,
                "loading" | "ready" | "empty" | "needs_sign_in" | "error"
            )
        {
            return Err(Error::bad("invalid discovery report"));
        }
        let _guard = self.channel_lock.lock().unwrap_or_else(|e| e.into_inner());
        let account=self.account.read().unwrap_or_else(|e|e.into_inner());
        if expected_account.is_some_and(|key|account.signed_in!=Some(true)||account.key.as_deref()!=Some(key)) {
            return Err(Error::Conflict("YouTube account changed during subscription discovery".into()));
        }
        let mut known: BTreeMap<String, Value> = self
            .video
            .kv_get("known-channels")?
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        if matches!(phase,"ready"|"empty") {
            if let Some(key)=expected_account {
                for (id,value) in &mut known {
                    if value["subscription_account"].as_str()==Some(key)&&!channels.iter().any(|c|&c.channel_id==id){value["subscription_account"]=Value::Null;}
                }
            }
        }
        for channel in channels {
            if channel.channel_id.len() != 24
                || !channel.channel_id.starts_with("UC")
                || !channel
                    .channel_id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
                || channel.name.chars().count() > 200
            {
                return Err(Error::bad("invalid discovered channel"));
            }
            // Signed-out pages can contain channel suggestions: never call those subscriptions.
            if phase!="needs_sign_in" {
                if let Some(key)=expected_account {
                    let marker=format!("unsubscribed:{key}:{}",channel.channel_id);
                    let unsubscribed_at=self.video.kv_get(&marker)?.and_then(|v|v.parse::<i64>().ok());
                    if unsubscribed_at.is_some_and(|at|account.changed_at>at){self.video.kv_put(&marker,"")?;}
                }
                known.insert(channel.channel_id.clone(),json!({"channel_id":channel.channel_id,"name":channel.name,"source":"subscriptions","last_seen_at":self.now(),"subscription_account":expected_account}));
            }
        }
        while known.len() > 1000 {
            known.pop_first();
        }
        self.video
            .kv_put("known-channels", &serde_json::to_string(&known)?)?;
        self.video.kv_put(
            "channel-discovery",
            &json!({"phase":phase,"count":channels.len(),"updated_at":self.now(),"subscription_account":expected_account}).to_string(),
        )?;
        self.log_event(
            "info",
            "channels.discovered",
            json!({"phase":phase,"count":channels.len()}),
        );
        Ok(())
    }
    pub fn channel_discovery(&self) -> Value {
        let mut result:Value=self.video
            .kv_get("channel-discovery")
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| json!({"phase":"idle"}));
        result["account"]=self.youtube_account();
        result["subscriptions_current"]=json!(self.youtube_account_key().as_deref().is_some_and(|key|result["subscription_account"].as_str()==Some(key))&&!self.is_offline());
        result.as_object_mut().map(|v|v.remove("subscription_account"));
        if result["account"]["state"]=="signed_out" {result["phase"]=json!("needs_sign_in");}
        result
    }
    pub fn known_channels(&self) -> Value {
        let mut channels: BTreeMap<String, Value> = self
            .video
            .kv_get("known-channels")
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        for snapshot in self.content.list_snapshots(None, 100).unwrap_or_default() {
            if let Ok(Some(feed)) = self.content.get_snapshot(snapshot.id) {
                for item in feed.items {
                    if let Some(id) = item
                        .channel_id
                        .filter(|s| s.starts_with("UC") && s.len() == 24)
                    {
                        channels.entry(id.clone()).or_insert(json!({"channel_id":id,"name":item.channel.unwrap_or_default(),"source":if snapshot.kind=="subscriptions" {"subscriptions"} else {"browsing"}}));
                    }
                }
            }
        }
        for config in self.effective_subscriptions() {
            channels.entry(config.channel_id.clone()).or_insert(
                json!({"channel_id":config.channel_id,"name":config.name,"source":"configured"}),
            );
        }
        let account=self.youtube_account_key();
        for (id, channel) in channels.iter_mut() {
            let same_account=account.as_deref().is_some_and(|key|channel["subscription_account"].as_str()==Some(key));
            let unsubscribed=account.as_ref().is_some_and(|key|self.video.kv_get(&format!("unsubscribed:{key}:{id}")).ok().flatten().and_then(|v|v.parse::<i64>().ok()).is_some());
            channel["unsubscribed"]=json!(unsubscribed);
            channel["youtube_subscription"]=json!(if unsubscribed{"unsubscribed"}else if same_account&&!self.is_offline(){"current"}else if channel["source"]=="subscriptions"{"cached"}else{"unknown"});
            channel.as_object_mut().map(|c|c.remove("subscription_account"));
            channel["last_checked_at"] = json!(self
                .video
                .kv_get(&format!("channel-check:{id}"))
                .ok()
                .flatten()
                .and_then(|s| s.parse::<i64>().ok()));
            channel["last_result"] = self
                .video
                .kv_get(&format!("channel-result:{id}"))
                .ok()
                .flatten()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or(Value::Null);
        }
        json!(channels.into_values().collect::<Vec<_>>())
    }

    /// Called only after the signed-in YouTube page confirms removal. Existing
    /// videos stay in the library; automatic channel following stops.
    pub fn confirm_channel_unsubscribed(&self, id: &str, expected_account: &str) -> Result<()> {
        if id.len()!=24||!id.starts_with("UC")||!id.bytes().all(|b|b.is_ascii_alphanumeric()||matches!(b,b'_'|b'-')) {return Err(Error::bad("Invalid channel ID"));}
        let _lock=self.channel_lock.lock().unwrap_or_else(|e|e.into_inner());
        let account=self.account.read().unwrap_or_else(|e|e.into_inner());
        if account.signed_in!=Some(true)||account.key.as_deref()!=Some(expected_account) {return Err(Error::Conflict("YouTube account changed. Reload subscriptions before retrying.".into()));}
        let configs:Vec<Channel>=self.subscriptions().into_iter().filter(|c|c.channel_id!=id).collect();
        let mut groups=self.channel_groups();
        for g in &mut groups {g.channel_ids.retain(|c|c!=id);}
        self.video.kv_put("subscriptions", &serde_json::to_string(&configs)?)?;
        self.video.kv_put("channel-groups", &serde_json::to_string(&groups)?)?;
        self.video.kv_put(&format!("unsubscribed:{expected_account}:{id}"), &self.now().to_string())?;
        self.log_event("info","channel.unsubscribed",json!({"channel_id":id}));
        Ok(())
    }

    pub(crate) fn discover_channels(&self, data: &Value, subscribed: bool) -> Result<()> {
        fn text(value: &Value) -> String {
            value
                .as_str()
                .map(str::to_owned)
                .or_else(|| value["simpleText"].as_str().map(str::to_owned))
                .unwrap_or_else(|| {
                    value["runs"]
                        .as_array()
                        .map(|runs| runs.iter().filter_map(|r| r["text"].as_str()).collect())
                        .unwrap_or_default()
                })
        }
        fn walk(data: &Value, depth: usize, out: &mut BTreeMap<String, Value>, subscribed: bool) {
            if depth > 50 || out.len() >= 1000 {
                return;
            }
            if let Some(map) = data.as_object() {
                for key in ["channelRenderer", "guideEntryRenderer"] {
                    if let Some(channel) = map.get(key) {
                        let id = channel["channelId"].as_str().or_else(|| {
                            channel
                                .pointer("/navigationEndpoint/browseEndpoint/browseId")
                                .and_then(Value::as_str)
                        });
                        if let Some(id) = id.filter(|s| s.starts_with("UC") && s.len() == 24) {
                            out.insert(id.into(),json!({"channel_id":id,"name":text(&channel["title"]),"source":if subscribed {"subscriptions"} else {"browsing"}}));
                        }
                    }
                }
                for value in map.values() {
                    walk(value, depth + 1, out, subscribed);
                }
            } else if let Some(a) = data.as_array() {
                for value in a {
                    walk(value, depth + 1, out, subscribed);
                }
            }
        }
        let mut found = BTreeMap::new();
        walk(data, 0, &mut found, subscribed);
        if found.is_empty() {
            return Ok(());
        }
        let _guard = self.channel_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut known: BTreeMap<String, Value> = self
            .video
            .kv_get("known-channels")?
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        for (id, mut value) in found {
            if subscribed || !known.contains_key(&id) {
                if let Some(key)=known.get(&id).and_then(|v|v.get("subscription_account")){value["subscription_account"]=key.clone();}
                known.insert(id, value);
            }
        }
        while known.len() > 1000 {
            known.pop_first();
        }
        self.video
            .kv_put("known-channels", &serde_json::to_string(&known)?)
    }

    pub fn check_channels_now(self: &Arc<Self>) -> Result<()> {
        self.require_online()?;
        if !self.video.settings().auto_prefetch {
            return Err(Error::bad("Enable automatic downloads first"));
        }
        for c in self.effective_subscriptions().iter().filter(|c| c.enabled) {
            self.video.kv_put(
                &format!("channel-check:{}", c.channel_id),
                &(self.now() - c.poll_minutes as i64 * 60000 - 1).to_string(),
            )?;
        }
        self.poll_subscriptions();
        Ok(())
    }
    pub fn subscriptions(&self) -> Vec<Channel> {
        self.video
            .kv_get("subscriptions")
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    pub fn put_subscriptions(&self, channels: &[Channel]) -> Result<()> {
        let mut seen = HashSet::new();
        if channels.len() > 1000 {
            return Err(Error::bad("at most 1000 channel overrides"));
        }
        for c in channels {
            if !c.channel_id.starts_with("UC")
                || c.channel_id.len() != 24
                || !c
                    .channel_id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                || !seen.insert(c.channel_id.clone())
                || !(5..=10080).contains(&c.poll_minutes)
                || !(1..=15).contains(&c.max_per_poll)
                || c.retention_days > 3650
                || c.quality == Quality::None
                || c.name.len() > 200
            {
                return Err(Error::bad("channels need unique UC IDs, quality, retention <=3650 days, poll interval 5..10080 minutes and max_per_poll 1..15"));
            }
        }
        self.video
            .kv_put("subscriptions", &serde_json::to_string(channels)?)?;
        self.wake_scheduler();
        Ok(())
    }
    pub fn poll_subscriptions(self: &Arc<Self>) {
        if self.is_offline() || (!self.video.settings().auto_prefetch && !self.video.settings().favourite_carousel)
            || self.effective_subscriptions().is_empty()
            || self.subscription_polling.swap(true, Ordering::SeqCst)
        {
            return;
        }
        let core = self.clone();
        if std::thread::Builder::new()
            .name("muninn-channels".into())
            .spawn(move || {
                if core.poll_channels().is_err() {
                    core.log_event("error", "channels.poll_failed", json!({}));
                }
                core.subscription_polling.store(false, Ordering::SeqCst);
            })
            .is_err()
        {
            self.subscription_polling.store(false, Ordering::SeqCst);
        }
    }
    pub fn poll_channels(&self) -> Result<()> {
        if self.is_offline() { return Ok(()); }
        let settings=self.video.settings();
        let favourites=self.favourite_channels();
        for c in self.effective_subscriptions().into_iter().filter(|c| (c.enabled&&settings.auto_prefetch) || (settings.favourite_carousel&&favourites.contains(&c.channel_id))) {
            if self.is_offline() {break;}
            let key = format!("channel-check:{}", c.channel_id);
            let last = self
                .video
                .kv_get(&key)?
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(0);
            if self.now() - last < c.poll_minutes as i64 * 60000 {
                continue;
            }
            // Record attempts too: a failed channel must not spin every scheduler tick.
            self.video.kv_put(&key, &self.now().to_string())?;
            let url = format!(
                "https://www.youtube.com/feeds/videos.xml?channel_id={}",
                c.channel_id
            );
            let data = match self.fetcher.get(&url, 1024 * 1024) {
                Ok(b) => b,
                Err(_) => {
                    self.video.kv_put(
                        &format!("channel-result:{}", c.channel_id),
                        &json!({"error":"Could not fetch channel uploads"}).to_string(),
                    )?;
                    self.log_event(
                        "error",
                        "channel.fetch_failed",
                        json!({"channel_id":c.channel_id}),
                    );
                    continue;
                }
            };
            let items = parse_feed(&String::from_utf8_lossy(&data), &c.channel_id);
            if items.is_empty() {
                self.video.kv_put(
                    &format!("channel-result:{}", c.channel_id),
                    &json!({"error":"No uploads found in channel feed"}).to_string(),
                )?;
                self.log_event(
                    "warn",
                    "channel.empty_feed",
                    json!({"channel_id":c.channel_id}),
                );
                continue;
            }
            let seen_key = format!("channel-seen:{}", c.channel_id);
            let mut seen: HashSet<String> = self
                .video
                .kv_get(&seen_key)?
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();
            let mut queued = 0;
            for item in &items {
                if seen.contains(&item.video_id) {
                    continue;
                }
                if queued < c.max_per_poll {
                    self.ingest_video(item.clone())?;
                    if favourites.contains(&c.channel_id) && settings.favourite_carousel { self.feature_upload(item)?; }
                    let facts = self.facts_for(&item.video_id);
                    let decision = Decision {
                        rule_id: None,
                        rule: Some(format!("channel:{}", c.channel_id)),
                        quality: c.quality,
                        retention_days: c.retention_days,
                    };
                    if c.enabled&&settings.auto_prefetch {self.create_entry(&facts, c.quality, &decision, "subscription", c.keep)?;}
                    queued += 1;
                }
                // Do not drain the entire historical feed over successive polls.
                seen.insert(item.video_id.clone());
            }
            // Bounded history covers the current feed; old uploads cannot reappear as new.
            seen.retain(|id| items.iter().any(|i| &i.video_id == id));
            self.video
                .kv_put(&seen_key, &serde_json::to_string(&seen)?)?;
            self.log_event(
                "info",
                "channel.polled",
                json!({"channel_id":c.channel_id,"queued":queued}),
            );
            self.video.kv_put(
                &format!("channel-result:{}", c.channel_id),
                &json!({"queued":queued}).to_string(),
            )?;
        }
        Ok(())
    }
}
