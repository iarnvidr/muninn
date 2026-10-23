//! Shared tab-group and library-collection state for the app and CLI.
use crate::{Core, Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TabGroup {
    pub tab_ids: Vec<String>,
    pub collapsed: bool,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Organization {
    pub tab_order: Vec<String>,
    pub tab_groups: BTreeMap<String, TabGroup>,
    pub collections: BTreeMap<String, Vec<String>>,
    pub tags: BTreeMap<String, Vec<String>>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub kind: String,
    pub action: String,
    pub name: String,
    #[serde(default)]
    pub ids: Vec<String>,
    pub new_name: Option<String>,
    pub collapsed: Option<bool>,
    pub before_id: Option<String>,
}
fn name_ok(s: &str) -> bool {
    !s.trim().is_empty()
        && s == s.trim()
        && s.chars().count() <= 80
        && !s.chars().any(char::is_control)
}
impl Organization {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.tab_order.len() > 10000
            || self
                .tab_order
                .iter()
                .any(|id| id.is_empty() || id.len() > 128)
            || self.tab_order.iter().collect::<HashSet<_>>().len() != self.tab_order.len()
        {
            return Err(Error::bad("invalid tab order"));
        }
        if self.tab_groups.len() > 200 || self.collections.len() > 500 || self.tags.len() > 500 {
            return Err(Error::bad("too many groups, collections or tags"));
        }
        let mut seen = HashSet::new();
        for (name, group) in &self.tab_groups {
            if !name_ok(name) || group.tab_ids.len() > 10000 {
                return Err(Error::bad("invalid tab group"));
            }
            for id in &group.tab_ids {
                if id.is_empty() || id.len() > 128 || !seen.insert(id) {
                    return Err(Error::bad(
                        "a tab belongs to at most one group; tab IDs must be unique",
                    ));
                }
            }
        }
        for (name, ids) in self.collections.iter().chain(self.tags.iter()) {
            if !name_ok(name)
                || ids.len() > 10000
                || ids.iter().any(|id| !crate::util::valid_video_id(id))
            {
                return Err(Error::bad("invalid collection name or video ID"));
            }
            if ids.iter().collect::<HashSet<_>>().len() != ids.len() {
                return Err(Error::bad("duplicate video in collection"));
            }
        }
        Ok(())
    }
}
impl Core {
    pub fn organization(&self) -> Organization {
        self.video
            .kv_get("organization")
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    pub fn put_organization(&self, org: &Organization) -> Result<()> {
        let _guard = self
            .organization_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        org.validate()?;
        self.video
            .kv_put("organization", &serde_json::to_string(org)?)
    }
    pub fn change_organization(&self, change: Change) -> Result<Organization> {
        let _guard = self
            .organization_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut org = self.organization();
        if !(change.kind == "tab" && change.name.is_empty()) && !name_ok(&change.name) {
            return Err(Error::bad("name must contain 1..80 characters"));
        }
        match change.kind.as_str() {
            "tab" if change.action == "move" => {
                if change.ids.is_empty()
                    || change.ids.iter().any(|id| id.is_empty() || id.len() > 128)
                {
                    return Err(Error::bad("tab IDs required"));
                }
                if !change.name.is_empty() && !org.tab_groups.contains_key(&change.name) {
                    return Err(Error::not_found("no such group"));
                }
                for group in org.tab_groups.values_mut() {
                    group.tab_ids.retain(|id| !change.ids.contains(id));
                }
                if let Some(group) = org.tab_groups.get_mut(&change.name) {
                    let index = change
                        .before_id
                        .as_ref()
                        .and_then(|id| group.tab_ids.iter().position(|x| x == id))
                        .unwrap_or(group.tab_ids.len());
                    group
                        .tab_ids
                        .splice(index..index, change.ids.iter().cloned());
                }
                org.tab_order.retain(|id| !change.ids.contains(id));
                if let Some(before) = change
                    .before_id
                    .as_ref()
                    .filter(|id| !change.ids.contains(id))
                {
                    if !org.tab_order.contains(before) {
                        org.tab_order.push(before.clone());
                    }
                }
                let index = change
                    .before_id
                    .as_ref()
                    .and_then(|id| org.tab_order.iter().position(|x| x == id))
                    .unwrap_or(org.tab_order.len());
                org.tab_order.splice(index..index, change.ids);
            }
            "tab_group" => match change.action.as_str() {
                "create" => {
                    if org.tab_groups.contains_key(&change.name) {
                        return Err(Error::Conflict("group already exists".into()));
                    }
                    org.tab_groups
                        .insert(change.name.clone(), TabGroup::default());
                }
                "assign" => {
                    if !org.tab_groups.contains_key(&change.name) {
                        return Err(Error::not_found("no such group"));
                    }
                    for group in org.tab_groups.values_mut() {
                        group.tab_ids.retain(|id| !change.ids.contains(id));
                    }
                    org.tab_groups
                        .get_mut(&change.name)
                        .unwrap()
                        .tab_ids
                        .extend(change.ids);
                }
                "remove" => {
                    org.tab_groups
                        .get_mut(&change.name)
                        .ok_or_else(|| Error::not_found("no such group"))?
                        .tab_ids
                        .retain(|id| !change.ids.contains(id));
                }
                "collapse" => {
                    org.tab_groups
                        .get_mut(&change.name)
                        .ok_or_else(|| Error::not_found("no such group"))?
                        .collapsed = change
                        .collapsed
                        .ok_or_else(|| Error::bad("collapsed is required"))?;
                }
                "rename" => {
                    let new = change
                        .new_name
                        .as_ref()
                        .filter(|s| name_ok(s))
                        .ok_or_else(|| Error::bad("valid new_name is required"))?;
                    if org.tab_groups.contains_key(new) {
                        return Err(Error::Conflict("group already exists".into()));
                    }
                    let value = org
                        .tab_groups
                        .remove(&change.name)
                        .ok_or_else(|| Error::not_found("no such group"))?;
                    org.tab_groups.insert(new.clone(), value);
                }
                "delete" => {
                    org.tab_groups
                        .remove(&change.name)
                        .ok_or_else(|| Error::not_found("no such group"))?;
                }
                _ => return Err(Error::bad("unknown group action")),
            },
            "collection" | "tag" => {
                let sets = if change.kind == "tag" {
                    &mut org.tags
                } else {
                    &mut org.collections
                };
                match change.action.as_str() {
                    "create" => {
                        if sets.contains_key(&change.name) {
                            return Err(Error::Conflict("name already exists".into()));
                        }
                        sets.insert(change.name.clone(), Vec::new());
                    }
                    "add" => {
                        let ids = sets
                            .get_mut(&change.name)
                            .ok_or_else(|| Error::not_found("no such collection or tag"))?;
                        for id in change.ids {
                            if !ids.contains(&id) {
                                ids.push(id);
                            }
                        }
                    }
                    "remove" => {
                        sets.get_mut(&change.name)
                            .ok_or_else(|| Error::not_found("no such collection or tag"))?
                            .retain(|id| !change.ids.contains(id));
                    }
                    "rename" => {
                        let new = change
                            .new_name
                            .as_ref()
                            .filter(|s| name_ok(s))
                            .ok_or_else(|| Error::bad("valid new_name is required"))?;
                        if sets.contains_key(new) {
                            return Err(Error::Conflict("name already exists".into()));
                        }
                        let value = sets
                            .remove(&change.name)
                            .ok_or_else(|| Error::not_found("no such collection or tag"))?;
                        sets.insert(new.clone(), value);
                    }
                    "delete" => {
                        sets.remove(&change.name)
                            .ok_or_else(|| Error::not_found("no such collection or tag"))?;
                    }
                    _ => return Err(Error::bad("unknown collection action")),
                }
            }
            _ => return Err(Error::bad("kind must be tab_group, collection or tag")),
        }
        org.validate()?;
        self.video
            .kv_put("organization", &serde_json::to_string(&org)?)?;
        Ok(org)
    }
}
