//! A live browser observation, never inferred from cached subscription data.
//! Account identity is separate from the native, memory-only downloader session.
use crate::{Core, Error, Result};
use serde_json::{json, Value};

#[derive(Default)]
pub(crate) struct Account {
    pub signed_in: Option<bool>,
    pub key: Option<String>,
    pub changed_at: i64,
    pub cookies: Vec<crate::youtube_session::YoutubeCookie>,
}
impl Core {
    pub fn invalidate_youtube_account(&self) {
        *self.account.write().unwrap_or_else(|e|e.into_inner())=Account{changed_at:self.now(),..Default::default()};
    }
    pub fn observe_youtube_account(&self, signed_in: bool, key: Option<&str>, changed_at: i64) -> Result<bool> {
        if key.is_some_and(|s|s.len()!=64||!s.bytes().all(|b|b.is_ascii_hexdigit())) || changed_at<0 || changed_at>self.now()+60_000 {
            return Err(Error::bad("invalid account observation"));
        }
        let mut account=self.account.write().unwrap_or_else(|e|e.into_inner());
        // Older background documents must not replace a newer login observation.
        if changed_at<account.changed_at {return Ok(false);}
        if !signed_in || account.key.as_deref() != key {
            account.cookies.clear();
        }
        account.signed_in=Some(signed_in);
        account.key=if signed_in {key.map(str::to_owned)} else {None};
        account.changed_at=changed_at;
        Ok(true)
    }
    pub fn youtube_account(&self) -> Value {
        let a=self.account.read().unwrap_or_else(|e|e.into_inner());
        json!({"state":match a.signed_in{Some(true)=>"signed_in",Some(false)=>"signed_out",None=>"unknown"},
            "verified":a.signed_in==Some(true)&&a.key.is_some()&&!self.is_offline(),
            "observed_at":if a.changed_at>0{Some(a.changed_at)}else{None}})
    }
    pub(crate) fn youtube_account_key(&self) -> Option<String> {
        let a=self.account.read().unwrap_or_else(|e|e.into_inner());
        (a.signed_in==Some(true)).then(||a.key.clone()).flatten()
    }
    /// Pin an action to the account that supplied this channel's subscription.
    pub fn unsubscribe_account(&self, id: &str) -> Result<String> {
        self.require_online()?;
        let account=self.account.read().unwrap_or_else(|e|e.into_inner());
        let key=account.key.as_ref().filter(|_|account.signed_in==Some(true)).ok_or_else(||Error::Conflict("Verify YouTube sign-in in Browse before unsubscribing.".into()))?;
        let known:Value=self.video.kv_get("known-channels")?.and_then(|s|serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
        let unsubscribed=self.video.kv_get(&format!("unsubscribed:{key}:{id}"))?.and_then(|v|v.parse::<i64>().ok()).is_some();
        if known[id]["subscription_account"].as_str()!=Some(key.as_str()) || unsubscribed {
            return Err(Error::Conflict("Load subscriptions for the current YouTube account before unsubscribing.".into()));
        }
        Ok(key.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::testutil::test_core;
    use crate::subscriptions::{Channel, DiscoveredChannel};
    use crate::quality::Quality;
    const ID:&str="UCabcdefghijklmnopqrstuv";
    fn channels()->Vec<DiscoveredChannel>{vec![DiscoveredChannel{channel_id:ID.into(),name:"Creator".into()}]}
    #[test]
    fn cached_discovery_is_not_sign_in_and_sign_out_or_account_switch_disables_account_actions() {
        let (_dir,c)=test_core(None,None);c.set_now_for_tests(Some(1000));
        c.ingest_channel_candidates(&channels(),"ready",None).unwrap();
        assert_eq!(c.youtube_account()["state"],"unknown");assert_eq!(c.known_channels()[0]["youtube_subscription"],"cached");assert!(c.unsubscribe_account(ID).is_err());
        let a="a".repeat(64);c.observe_youtube_account(true,Some(&a),100).unwrap();
        assert!(c.unsubscribe_account(ID).is_err(),"old unbound cache is not proof of membership");
        c.ingest_channel_candidates(&channels(),"ready",Some(&a)).unwrap();assert_eq!(c.unsubscribe_account(ID).unwrap(),a);
        assert!(!c.known_channels().to_string().contains(&a));assert!(!c.youtube_account().to_string().contains(&a));
        assert_eq!(c.channel_discovery()["subscriptions_current"],true);
        c.observe_youtube_account(false,None,200).unwrap();
        assert_eq!(c.youtube_account()["state"],"signed_out");assert_eq!(c.channel_discovery()["phase"],"needs_sign_in");assert_eq!(c.known_channels()[0]["youtube_subscription"],"cached");
        assert!(c.unsubscribe_account(ID).is_err());assert_eq!(c.known_channels()[0]["name"],"Creator");
        let b="b".repeat(64);c.observe_youtube_account(true,Some(&b),300).unwrap();
        assert!(c.unsubscribe_account(ID).is_err());assert!(c.ingest_channel_candidates(&channels(),"ready",Some(&a)).is_err());
        c.observe_youtube_account(true,Some(&a),100).unwrap();assert_eq!(c.youtube_account_key().unwrap(),b,"old background documents cannot replace the new session");
        c.ingest_channel_candidates(&channels(),"ready",Some(&b)).unwrap();assert_eq!(c.unsubscribe_account(ID).unwrap(),b);
        let mut settings=c.video.settings();settings.offline_mode=true;c.apply_settings(&settings).unwrap();
        assert_eq!(c.youtube_account()["verified"],false);assert!(c.unsubscribe_account(ID).is_err());
    }
    #[test]
    fn unsubscribe_acknowledgement_is_bound_to_account_and_startup_requires_a_live_observation() {
        let (dir,c)=test_core(None,None);c.set_now_for_tests(Some(1000));let a="a".repeat(64);let b="b".repeat(64);
        c.observe_youtube_account(true,Some(&a),100).unwrap();c.ingest_channel_candidates(&channels(),"ready",Some(&a)).unwrap();
        c.put_subscriptions(&[Channel{channel_id:ID.into(),name:"Creator".into(),quality:Quality::P1080,retention_days:0,keep:false,enabled:true,poll_minutes:60,max_per_poll:3}]).unwrap();
        c.observe_youtube_account(true,Some(&b),200).unwrap();assert!(c.confirm_channel_unsubscribed(ID,&a).is_err());assert!(c.subscriptions()[0].enabled);
        c.observe_youtube_account(true,Some(&a),300).unwrap();c.confirm_channel_unsubscribed(ID,&a).unwrap();assert_eq!(c.known_channels()[0]["unsubscribed"],true);
        c.ingest_channel_candidates(&channels(),"ready",Some(&a)).unwrap();assert_eq!(c.known_channels()[0]["unsubscribed"],true,"old page cannot undo a confirmed unsubscribe");
        c.observe_youtube_account(true,Some(&b),1100).unwrap();assert_eq!(c.known_channels()[0]["unsubscribed"],false,"another account does not inherit an unsubscribe marker");
        c.set_now_for_tests(Some(2000));c.observe_youtube_account(true,Some(&a),1500).unwrap();c.ingest_channel_candidates(&channels(),"ready",Some(&a)).unwrap();assert_eq!(c.known_channels()[0]["unsubscribed"],false,"a newly loaded subscription page can confirm resubscription");
        drop(c);let c=Core::open(crate::CoreConfig{data_dir:dir.path().into(),tool_dirs:Some(vec![]),..Default::default()}).unwrap();
        assert_eq!(c.youtube_account()["state"],"unknown");assert!(c.unsubscribe_account(ID).is_err());assert_eq!(c.known_channels()[0]["youtube_subscription"],"cached");
    }
    #[test]
    fn signed_out_public_polling_and_favourites_continue_without_credentials() {
        struct Feed;impl crate::ytdlp::Fetcher for Feed{fn get(&self,_:&str,_:u64)->Result<Vec<u8>>{Ok(b"<feed><entry><yt:videoId>aaaaaaaaaaa</yt:videoId><title>Public upload</title></entry></feed>".to_vec())}}
        let (_d,c)=test_core(None,Some(std::sync::Arc::new(Feed)));c.set_now_for_tests(Some(10_000_000));c.observe_youtube_account(false,None,c.now()).unwrap();
        let group=serde_json::from_value(json!({"name":"Favourites","favourite":true,"channel_ids":[ID],"quality":"1080","retention_days":0,"keep":false,"enabled":true,"poll_minutes":60,"max_per_poll":3})).unwrap();
        c.put_channel_groups(&[group]).unwrap();c.poll_channels().unwrap();assert_eq!(c.video.list_entries().unwrap().len(),1);assert_eq!(c.featured_uploads()["items"].as_array().unwrap().len(),1);
        c.ingest_channel_candidates(&channels(),"needs_sign_in",None).unwrap();assert!(!c.known_channels().as_array().unwrap().iter().any(|v|v["youtube_subscription"]=="current"));
        assert!(c.effective_subscriptions()[0].enabled,"signing out does not erase local policies");
    }
    #[test]
    fn empty_current_subscription_list_invalidates_membership_without_deleting_local_policies() {
        let (_d,c)=test_core(None,None);c.set_now_for_tests(Some(1000));let a="a".repeat(64);
        c.observe_youtube_account(true,Some(&a),100).unwrap();c.ingest_channel_candidates(&channels(),"ready",Some(&a)).unwrap();
        c.ingest_channel_candidates(&[],"empty",Some(&a)).unwrap();assert_eq!(c.known_channels()[0]["youtube_subscription"],"cached");assert!(c.unsubscribe_account(ID).is_err());
        c.invalidate_youtube_account();c.observe_youtube_account(true,Some(&a),100).unwrap();assert_eq!(c.youtube_account()["state"],"unknown");
        c.observe_youtube_account(true,Some(&a),1001).unwrap();assert_eq!(c.youtube_account()["verified"],true);
    }
}
