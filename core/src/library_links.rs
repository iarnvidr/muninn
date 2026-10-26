//! Durable library references, independent of media files and browser tabs.
use crate::{Core, Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum LinkInput {
    Url(String),
    Named { url: String, title: Option<String> },
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinksImport {
    pub items: Vec<LinkInput>,
    pub collection: String,
    #[serde(default)]
    pub dry_run: bool,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct LibraryLink {
    pub video_id: String,
    pub title: Option<String>,
    pub added_at: i64,
}
impl Core {
    fn library_links(&self) -> Result<BTreeMap<String, LibraryLink>> {
        self.video
            .kv_get("library-links")?
            .map(|s| serde_json::from_str(&s).map_err(Error::from))
            .unwrap_or_else(|| Ok(BTreeMap::new()))
    }

    pub fn import_links(&self, req: LinksImport) -> Result<Value> {
        if req.items.len() > 10000 {
            return Err(Error::bad("at most 10000 links per import"));
        }
        let _guard = self
            .organization_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut org = self.organization();
        let members = org.collections.entry(req.collection.clone()).or_default();
        let mut links = self.library_links()?;
        let mut seen = HashSet::new();
        let mut added = 0;
        let mut memberships = 0;
        for item in req.items {
            let (url, title) = match item {
                LinkInput::Url(url) => (url, None),
                LinkInput::Named { url, title } => (url, title),
            };
            let id = crate::tabs::video_id_of_url(url.trim())
                .ok_or_else(|| Error::bad("link imports accept YouTube video URLs only"))?;
            if title
                .as_ref()
                .is_some_and(|s| s.chars().count() > 1000 || s.chars().any(char::is_control))
            {
                return Err(Error::bad("invalid link title"));
            }
            if !seen.insert(id.clone()) {
                continue;
            }
            if !members.contains(&id) {
                members.push(id.clone());
                memberships += 1;
            }
            if !links.contains_key(&id) {
                added += 1;
                links.insert(
                    id.clone(),
                    LibraryLink {
                        video_id: id,
                        title,
                        added_at: self.now(),
                    },
                );
            }
        }
        if links.len() > 10000 {
            return Err(Error::bad("at most 10000 saved library links"));
        }
        org.validate()?;
        if !req.dry_run {
            self.video.kv_put_many(&[
                ("library-links", serde_json::to_string(&links)?),
                ("organization", serde_json::to_string(&org)?),
            ])?;
            self.log_event(
                "info",
                "import.links",
                json!({"added":added,"collection_memberships_added":memberships}),
            );
        }
        Ok(
            json!({"ok":true,"dry_run":req.dry_run,"added":added,"collection_memberships_added":memberships,"unique":seen.len(),"collection":req.collection,"tabs_opened":0,"downloads_queued":0}),
        )
    }

    pub fn remove_library_link(&self, id: &str) -> Result<Value> {
        let _guard = self
            .organization_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut links = self.library_links()?;
        if links.remove(id).is_none() {
            return Err(Error::not_found("no saved library link"));
        }
        let mut org = self.organization();
        for ids in org.collections.values_mut().chain(org.tags.values_mut()) {
            ids.retain(|v| v != id);
        }
        self.video.kv_put_many(&[
            ("library-links", serde_json::to_string(&links)?),
            ("organization", serde_json::to_string(&org)?),
        ])?;
        Ok(json!({"ok":true,"video_id":id}))
    }

    pub fn library_view(&self) -> Result<Vec<Value>> {
        let mut entries = self.entries_view(None)?;
        let links = self.library_links()?;
        // A saved reference remains saved when a disposable media copy appears.
        for entry in &mut entries {
            let saved = entry["video_id"].as_str().is_some_and(|id| links.contains_key(id));
            entry["saved_link"] = json!(saved);
        }
        let cached: HashSet<String> = entries
            .iter()
            .filter_map(|v| v["video_id"].as_str().map(str::to_owned))
            .collect();
        for (id, link) in links {
            if cached.contains(&id) {
                continue;
            }
            let facts = self.facts_for(&id);
            entries.push(json!({"id":format!("link:{id}"),"video_id":id,"title":facts.title.or(link.title),"channel":facts.channel,"duration_seconds":facts.duration_seconds,
                "state":"link","status":"link","origin":"link","protected":false,"saved_link":true,"size_bytes":0,"progress":0,"variant":null,
                "created_at":link.added_at,"last_access_at":link.added_at,"thumb_url":format!("/thumb/{id}")}));
        }
        Ok(entries)
    }
}
