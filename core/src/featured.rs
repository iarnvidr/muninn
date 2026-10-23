//! Featured uploads are discovery records, independent from cache protection.
use crate::{Core, Result};
use crate::model::VideoIngest;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;

#[derive(Serialize, Deserialize)]
struct Featured {
    video_id: String,
    channel_id: String,
    title: String,
    channel: Option<String>,
    featured_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::testutil::test_core;
    use crate::channel_groups::ChannelGroup;
    use crate::model::PlaybackIngest;
    fn group() -> ChannelGroup {serde_json::from_value(json!({"name":"Favourites","favourite":true,"channel_ids":["UCabcdefghijklmnopqrstuv"],"quality":"1080","retention_days":0,"keep":false,"enabled":true,"poll_minutes":60,"max_per_poll":3})).unwrap()}
    fn item() -> VideoIngest {VideoIngest{video_id:"aaaaaaaaaaa".into(),title:"New upload".into(),channel_id:Some("UCabcdefghijklmnopqrstuv".into()),..Default::default()}}
    #[test]
    fn featured_lifetime_is_first_admission_and_actual_playing_hides_without_completion() {
        let (_dir,core)=test_core(None,None);core.set_now_for_tests(Some(1_000_000));
        core.put_channel_groups(&[group()]).unwrap();core.feature_upload(&item()).unwrap();
        core.ingest_playback(PlaybackIngest{video_id:"aaaaaaaaaaa".into(),position_seconds:30.,paused:Some(true),..Default::default()}).unwrap();
        assert_eq!(core.featured_uploads()["items"].as_array().unwrap().len(),1,"opening or seeking must not hide featured videos");
        core.ingest_playback(PlaybackIngest{video_id:"aaaaaaaaaaa".into(),position_seconds:1.,started:true,..Default::default()}).unwrap();
        assert!(core.featured_uploads()["items"].as_array().unwrap().is_empty());
        assert!(core.video.list_entries().unwrap().is_empty(),"featured state must not keep or download media");
        let mut s=core.video.settings();s.favourite_hide_started=false;core.apply_settings(&s).unwrap();
        assert_eq!(core.featured_uploads()["items"].as_array().unwrap().len(),1);
        core.set_now_for_tests(Some(1_000_000+4*86_400_000));core.feature_upload(&item()).unwrap();
        core.set_now_for_tests(Some(1_000_000+5*86_400_000));assert!(core.featured_uploads()["items"].as_array().unwrap().is_empty());
    }
    #[test]
    fn favourites_settings_and_explicit_channel_override_control_downloads() {
        let (_dir,core)=test_core(None,None);core.put_channel_groups(&[group()]).unwrap();
        assert!(core.effective_subscriptions()[0].enabled);
        let mut s=core.video.settings();s.favourite_downloads=false;core.apply_settings(&s).unwrap();assert!(!core.effective_subscriptions()[0].enabled);
        let mut channel=core.effective_subscriptions()[0].clone();channel.enabled=true;
        core.put_subscriptions(&[channel]).unwrap();assert!(core.effective_subscriptions()[0].enabled);
        core.feature_upload(&item()).unwrap();s.favourite_carousel=false;core.apply_settings(&s).unwrap();assert!(core.featured_uploads()["items"].as_array().unwrap().is_empty());
        let account="a".repeat(64);core.observe_youtube_account(true,Some(&account),core.now()).unwrap();
        core.confirm_channel_unsubscribed("UCabcdefghijklmnopqrstuv",&account).unwrap();assert!(core.effective_subscriptions().is_empty());assert!(core.channel_groups()[0].channel_ids.is_empty());
    }
}
impl Core {
    pub(crate) fn favourite_channels(&self) -> HashSet<String> {
        self.channel_groups().into_iter().filter(|g|g.favourite).flat_map(|g|g.channel_ids).collect()
    }
    pub fn record_actual_playback(&self, id: &str) -> Result<()> {
        let key=format!("started:{id}");
        if self.video.kv_get(&key)?.is_none() { self.video.kv_put(&key, &self.now().to_string())?; }
        Ok(())
    }
    fn featured_records(&self) -> Vec<Featured> {
        self.video.kv_get("featured-uploads").ok().flatten().and_then(|s|serde_json::from_str(&s).ok()).unwrap_or_default()
    }
    pub(crate) fn feature_upload(&self, item: &VideoIngest) -> Result<()> {
        let _lock=self.organization_lock.lock().unwrap_or_else(|e|e.into_inner());
        let mut records=self.featured_records();
        if records.iter().any(|r|r.video_id==item.video_id) { return Ok(()); }
        records.push(Featured {video_id:item.video_id.clone(),channel_id:item.channel_id.clone().unwrap_or_default(),
            title:item.title.clone(),channel:item.channel.clone(),featured_at:self.now()});
        records.retain(|r|self.now()-r.featured_at < 30*86_400_000);
        if records.len()>500 {records.drain(..records.len()-500);}
        self.video.kv_put("featured-uploads", &serde_json::to_string(&records)?)
    }
    pub fn featured_uploads(&self) -> Value {
        let settings=self.video.settings();
        if !settings.favourite_carousel {return json!({"items":[]});}
        let favourites=self.favourite_channels();
        let mut records=self.featured_records();
        records.retain(|r|favourites.contains(&r.channel_id)
            && self.now()-r.featured_at < settings.favourite_carousel_days as i64*86_400_000
            && (!settings.favourite_hide_started || self.video.kv_get(&format!("started:{}",r.video_id)).ok().flatten().is_none()));
        records.sort_by(|a,b|b.featured_at.cmp(&a.featured_at));
        let items:Vec<Value>=records.into_iter().take(32).map(|r| {
            let cached=self.best_complete_entry(&r.video_id).map(|entry|self.entry_view(&entry));
            json!({"video_id":r.video_id,"title":r.title,"channel":r.channel,"channel_id":r.channel_id,
                "featured_at":r.featured_at,"thumbnail_path":format!("/thumb/{}",r.video_id),"cached":cached})
        }).collect();
        json!({"items":items})
    }
}
