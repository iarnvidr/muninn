//! The rules engine: decides what to cache for a video, when, and for how long.
//!
//! A rule matches on channel, duration and title keywords, and fires on one
//! encounter (how the video came to our attention). The first enabled rule, in
//! list order, whose conditions hold *and* whose trigger equals the encounter wins.

use crate::quality::Quality;
use crate::util::random_id;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    OpenTab,
    FirstPlay,
    NewUpload,
    Manual,
}

/// How a video was encountered. Same vocabulary as [`Trigger`].
pub type Encounter = Trigger;

impl Trigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Trigger::OpenTab => "open_tab",
            Trigger::FirstPlay => "first_play",
            Trigger::NewUpload => "new_upload",
            Trigger::Manual => "manual",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Match {
    /// Channel names or ids, compared case-insensitively.
    pub channels: Vec<String>,
    /// When true the video must come from one of `channels` (an empty list then
    /// matches nothing). When false `channels` is ignored.
    pub channels_only: bool,
    /// Any one of these (case-insensitive substrings) must appear in the title.
    pub title_keywords: Vec<String>,
    pub min_duration_seconds: Option<u32>,
    pub max_duration_seconds: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default, rename = "match")]
    pub matcher: Match,
    pub trigger: Trigger,
    pub quality: Quality,
    pub retention_days: u32,
}

fn yes() -> bool {
    true
}

/// What the rules engine knows about a video.
#[derive(Debug, Clone, Default)]
pub struct VideoFacts {
    pub video_id: String,
    pub title: Option<String>,
    pub channel: Option<String>,
    pub channel_id: Option<String>,
    pub duration_seconds: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Decision {
    pub rule_id: Option<String>,
    pub rule: Option<String>,
    pub quality: Quality,
    pub retention_days: u32,
}

/// Fallback when no rule matches at all.
/// Zero means retain until watched; completion cleanup supplies the grace period.
pub const DEFAULT_RETENTION_DAYS: u32 = 0;

impl Match {
    pub fn matches(&self, facts: &VideoFacts) -> bool {
        if self.channels_only {
            let hit = self.channels.iter().any(|wanted| {
                let wanted = wanted.trim();
                !wanted.is_empty()
                    && (facts.channel.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(wanted))
                        || facts.channel_id.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(wanted)))
            });
            if !hit {
                return false;
            }
        }
        if !self.title_keywords.is_empty() {
            let title = facts.title.as_deref().unwrap_or("").to_lowercase();
            let hit = self
                .title_keywords
                .iter()
                .map(|k| k.trim().to_lowercase())
                .any(|k| !k.is_empty() && title.contains(&k));
            if !hit {
                return false;
            }
        }
        if let Some(min) = self.min_duration_seconds {
            if facts.duration_seconds.map_or(true, |d| d < min) {
                return false;
            }
        }
        if let Some(max) = self.max_duration_seconds {
            if facts.duration_seconds.map_or(true, |d| d > max) {
                return false;
            }
        }
        true
    }
}

/// First enabled rule whose trigger equals `encounter` and whose conditions hold.
pub fn evaluate(rules: &[Rule], facts: &VideoFacts, encounter: Encounter) -> Option<Decision> {
    rules
        .iter()
        .find(|r| r.enabled && r.trigger == encounter && r.matcher.matches(facts))
        .map(decision_of)
}

/// Retention for an explicit user request: the first enabled rule that matches
/// the video's conditions, whatever its trigger, else the default.
pub fn retention_for_request(rules: &[Rule], facts: &VideoFacts) -> Decision {
    rules
        .iter()
        .find(|r| r.enabled && r.matcher.matches(facts))
        .map(decision_of)
        .unwrap_or(Decision {
            rule_id: None,
            rule: None,
            quality: Quality::None,
            retention_days: DEFAULT_RETENTION_DAYS,
        })
}

fn decision_of(rule: &Rule) -> Decision {
    Decision {
        rule_id: Some(rule.id.clone()),
        rule: Some(rule.name.clone()),
        quality: rule.quality,
        retention_days: rule.retention_days,
    }
}

/// Gives rules without an id one and trims names. Rejects nonsense.
pub fn normalize(mut rules: Vec<Rule>) -> Result<Vec<Rule>, String> {
    if rules.len() > 200 {
        return Err("too many rules (max 200)".into());
    }
    let mut seen = std::collections::HashSet::new();
    for rule in &mut rules {
        rule.name = rule.name.trim().to_string();
        if rule.name.is_empty() {
            return Err("every rule needs a name".into());
        }
        if rule.retention_days > 3650 {
            return Err(format!("rule '{}': retention above 3650 days", rule.name));
        }
        if let (Some(min), Some(max)) = (rule.matcher.min_duration_seconds, rule.matcher.max_duration_seconds) {
            if min > max {
                return Err(format!("rule '{}': minimum duration is above the maximum", rule.name));
            }
        }
        if rule.id.trim().is_empty() || !seen.insert(rule.id.clone()) {
            rule.id = random_id();
            seen.insert(rule.id.clone());
        }
    }
    Ok(rules)
}

/// The example rules Esben asked for. Keyword lists and channel lists are
/// starting points, meant to be edited in the Rules tab.
pub fn seed_rules() -> Vec<Rule> {
    let kw = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
    vec![
        Rule {
            id: "seed-talking-head".into(),
            name: "Talking-head".into(),
            enabled: true,
            matcher: Match {
                title_keywords: kw(&["explained", "commentary", "vlog", "review", "reaction", "opinion", "news", "essay", "rant"]),
                max_duration_seconds: Some(40 * 60),
                ..Match::default()
            },
            trigger: Trigger::FirstPlay,
            quality: Quality::P360,
            retention_days: 0,
        },
        Rule {
            id: "seed-podcasts".into(),
            name: "Podcasts / interviews".into(),
            enabled: true,
            matcher: Match {
                title_keywords: kw(&["podcast", "interview", "conversation", "episode", "ep.", "fireside", "q&a", "panel"]),
                min_duration_seconds: Some(20 * 60),
                ..Match::default()
            },
            trigger: Trigger::OpenTab,
            quality: Quality::Audio,
            retention_days: 0,
        },
        Rule {
            id: "seed-music".into(),
            name: "Music".into(),
            enabled: true,
            matcher: Match {
                title_keywords: kw(&["official video", "music video", "official audio", "lyrics", "lyric video", "remix", "live at"]),
                max_duration_seconds: Some(15 * 60),
                ..Match::default()
            },
            trigger: Trigger::Manual,
            quality: Quality::P720,
            retention_days: 0,
        },
        Rule {
            id: "seed-favorite-channels".into(),
            name: "Favorite subscribed channels".into(),
            enabled: true,
            // Add channel names or ids here. Until you do, this rule matches nothing.
            matcher: Match { channels_only: true, ..Match::default() },
            trigger: Trigger::NewUpload,
            quality: Quality::P480,
            retention_days: 0,
        },
        Rule {
            id: "seed-everything-else".into(),
            name: "Everything else".into(),
            enabled: true,
            matcher: Match::default(),
            trigger: Trigger::Manual,
            quality: Quality::None,
            retention_days: 0,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(title: &str, channel: &str, duration: Option<u32>) -> VideoFacts {
        VideoFacts {
            video_id: "dQw4w9WgXcQ".into(),
            title: Some(title.into()),
            channel: Some(channel.into()),
            channel_id: Some("UC123".into()),
            duration_seconds: duration,
        }
    }

    #[test]
    fn keyword_duration_and_channel_conditions() {
        let rules = seed_rules();
        // Talking head: keyword + short enough, on first play.
        let d = evaluate(&rules, &facts("Why X is wrong - commentary", "Someone", Some(600)), Trigger::FirstPlay).unwrap();
        assert_eq!(d.quality, Quality::P360);
        assert_eq!(d.retention_days, 0);
        // Too long for the talking-head rule.
        assert!(evaluate(&rules, &facts("Long commentary", "Someone", Some(3 * 3600)), Trigger::FirstPlay).is_none());
        // Podcast on open tab is audio-only, retained until watched.
        let d = evaluate(&rules, &facts("Podcast #5: Rust", "Pod", Some(3 * 3600)), Trigger::OpenTab).unwrap();
        assert_eq!((d.quality, d.retention_days), (Quality::Audio, 0));
        // A podcast-looking title that is too short does not match.
        assert!(evaluate(&rules, &facts("Podcast teaser", "Pod", Some(60)), Trigger::OpenTab).is_none());
        // Music is manual only.
        assert!(evaluate(&rules, &facts("Band - Song (Official Video)", "Band", Some(240)), Trigger::OpenTab).is_none());
        let d = evaluate(&rules, &facts("Band - Song (Official Video)", "Band", Some(240)), Trigger::Manual).unwrap();
        assert_eq!((d.quality, d.retention_days), (Quality::P720, 0));
    }

    #[test]
    fn favorite_channels_rule_matches_only_listed_channels() {
        let mut rules = seed_rules();
        // Nothing listed yet: matches nothing.
        assert!(evaluate(&rules, &facts("New video", "Fav Channel", Some(100)), Trigger::NewUpload).is_none());
        let fav = rules.iter_mut().find(|r| r.id == "seed-favorite-channels").unwrap();
        fav.matcher.channels = vec!["fav channel".into()];
        let d = evaluate(&rules, &facts("New video", "Fav Channel", Some(100)), Trigger::NewUpload).unwrap();
        assert_eq!((d.quality, d.retention_days), (Quality::P480, 0));
        assert!(evaluate(&rules, &facts("New video", "Other", Some(100)), Trigger::NewUpload).is_none());
        // Channel ids work too.
        let fav = rules.iter_mut().find(|r| r.id == "seed-favorite-channels").unwrap();
        fav.matcher.channels = vec!["uc123".into()];
        assert!(evaluate(&rules, &facts("New video", "Other", Some(100)), Trigger::NewUpload).is_some());
    }

    #[test]
    fn everything_else_caches_nothing_on_request() {
        let rules = seed_rules();
        let d = evaluate(&rules, &facts("Random clip", "X", Some(100)), Trigger::Manual).unwrap();
        assert_eq!(d.quality, Quality::None);
        assert_eq!(d.retention_days, 0);
    }

    #[test]
    fn order_decides_and_disabled_rules_are_skipped() {
        let mut rules = seed_rules();
        rules.iter_mut().find(|r| r.id == "seed-music").unwrap().enabled = false;
        let d = evaluate(&rules, &facts("Song (Official Video)", "B", Some(200)), Trigger::Manual).unwrap();
        assert_eq!(d.rule.as_deref(), Some("Everything else"));
        // Reordering: catch-all first wins.
        let last = rules.pop().unwrap();
        rules.insert(0, last);
        rules.iter_mut().find(|r| r.id == "seed-music").unwrap().enabled = true;
        let d = evaluate(&rules, &facts("Song (Official Video)", "B", Some(200)), Trigger::Manual).unwrap();
        assert_eq!(d.rule.as_deref(), Some("Everything else"));
    }

    #[test]
    fn unknown_duration_fails_duration_conditions() {
        let rules = seed_rules();
        assert!(evaluate(&rules, &facts("Great podcast", "P", None), Trigger::OpenTab).is_none());
    }

    #[test]
    fn explicit_requests_take_retention_from_matching_rule() {
        let mut rules = seed_rules();
        rules.iter_mut().find(|r|r.id=="seed-music").unwrap().retention_days=30;
        let d = retention_for_request(&rules, &facts("Band (Official Video)", "B", Some(200)));
        assert_eq!(d.retention_days, 30);
        let d = retention_for_request(&rules, &facts("Random clip", "X", Some(100)));
        assert_eq!(d.retention_days, 0);
        let d = retention_for_request(&[], &facts("x", "y", None));
        assert_eq!(d.retention_days, DEFAULT_RETENTION_DAYS);
    }

    #[test]
    fn normalize_assigns_ids_and_rejects_bad_rules() {
        let mut rules = seed_rules();
        rules[0].id = String::new();
        rules[1].id = rules[2].id.clone();
        let fixed = normalize(rules).unwrap();
        let ids: std::collections::HashSet<_> = fixed.iter().map(|r| r.id.clone()).collect();
        assert_eq!(ids.len(), fixed.len());
        let mut bad = seed_rules();
        bad[0].name = "  ".into();
        assert!(normalize(bad).is_err());
        let mut bad = seed_rules();
        bad[0].matcher.min_duration_seconds = Some(100);
        bad[0].matcher.max_duration_seconds = Some(10);
        assert!(normalize(bad).is_err());
    }

    #[test]
    fn rules_roundtrip_json_in_api_shape() {
        let json = serde_json::to_value(&seed_rules()[0]).unwrap();
        assert_eq!(json["trigger"], "first_play");
        assert_eq!(json["quality"], "360");
        assert!(json["match"]["title_keywords"].is_array());
        let back: Rule = serde_json::from_value(json).unwrap();
        assert_eq!(back, seed_rules()[0]);
    }
}
