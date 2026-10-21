//! The daily brief an assistant posts over MCP: items grouped in sections,
//! each with read and removed marks that survive re-posts of the same id.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use crate::AppResult as BriefResult;

pub const PREFERENCE_KEY: &str = "brief";
const MAX_ITEMS: usize = 500;
const MAX_POSTED_ITEMS: usize = 200;
const MAX_REMEMBERED: usize = 2000;
const MAX_SECTIONS: usize = 100;
/// How long agtk remembers the marks of an item the assistant took out.
pub const REMEMBER_MILLIS: u64 = 30 * 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BriefItemInput {
    pub id: String,
    pub section: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BriefPostParams {
    #[serde(default)]
    pub items: Vec<BriefItemInput>,
    /// Sections whose current items are replaced by the posted ones.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replace_sections: Vec<String>,
    /// Items the assistant takes out of the brief.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retire_ids: Vec<String>,
}

impl BriefPostParams {
    pub fn validate(&self) -> BriefResult<()> {
        if self.items.len() > MAX_POSTED_ITEMS {
            return Err("no more than 200 items can be posted at once".into());
        }
        let mut ids = HashSet::new();
        for item in &self.items {
            validate_id(&item.id)?;
            validate_section(&item.section)?;
            validate_text(&item.text)?;
            if let Some(link) = &item.link {
                validate_link(link)?;
            }
            if !ids.insert(item.id.as_str()) {
                return Err(format!("item {} is posted twice", item.id).into());
            }
        }
        for section in &self.replace_sections {
            validate_section(section)?;
        }
        for id in &self.retire_ids {
            validate_id(id)?;
            if ids.contains(id.as_str()) {
                return Err(format!("item {id} cannot be both posted and retired").into());
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefItem {
    pub id: String,
    pub section: String,
    pub text: String,
    pub link: Option<String>,
    pub posted_at: u64,
    pub updated_at: u64,
    pub read: bool,
    /// Hidden in the panel; the underlying task or mail is untouched.
    pub removed: bool,
    /// Marked done in the panel. The assistant completes the task itself.
    #[serde(default)]
    pub done: bool,
    /// When it was marked done, in Unix milliseconds.
    #[serde(default)]
    pub done_at: Option<u64>,
}

/// The marks of an item that left the brief, kept so a re-post restores them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetiredItem {
    pub id: String,
    pub read: bool,
    pub removed: bool,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub done_at: Option<u64>,
    pub retired_at: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefChanges {
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub retired: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Brief {
    /// In the order the panel shows them within each section.
    #[serde(default)]
    pub items: Vec<BriefItem>,
    /// Section names in the order they were first posted.
    #[serde(default)]
    pub sections: Vec<String>,
    #[serde(default)]
    pub retired: Vec<RetiredItem>,
}

impl Brief {
    pub fn from_value_lossy(value: Value) -> Self {
        serde_json::from_value(value).unwrap_or_else(|error| {
            eprintln!("agtk: ignoring saved brief: {error}");
            Self::default()
        })
    }

    /// Applies one post: retires replaced and named items, then adds or
    /// updates the posted ones. Nothing changes when the post is invalid.
    pub fn post(&mut self, params: &BriefPostParams, now: u64) -> BriefResult<BriefChanges> {
        params.validate()?;
        let replaced = params
            .replace_sections
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let retire_ids = params
            .retire_ids
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        // Posts into a replaced section are re-appended in posted order.
        let reordered = params
            .items
            .iter()
            .filter(|item| replaced.contains(item.section.as_str()))
            .map(|item| item.id.as_str())
            .collect::<HashSet<_>>();
        let mut next = self.clone();
        let mut taken = Vec::new();
        let mut kept = Vec::with_capacity(next.items.len());
        for item in next.items.drain(..) {
            if replaced.contains(item.section.as_str())
                || retire_ids.contains(item.id.as_str())
                || reordered.contains(item.id.as_str())
            {
                taken.push(item);
            } else {
                kept.push(item);
            }
        }
        next.items = kept;

        let mut changes = BriefChanges::default();
        for input in &params.items {
            if let Some(existing) = next.items.iter_mut().find(|item| item.id == input.id) {
                if existing.apply(input, now) {
                    changes.updated.push(input.id.clone());
                }
            } else if let Some(position) = taken.iter().position(|item| item.id == input.id) {
                let mut item = taken.remove(position);
                if item.apply(input, now) {
                    changes.updated.push(input.id.clone());
                }
                next.items.push(item);
            } else {
                let marks = next
                    .retired
                    .iter()
                    .position(|retired| retired.id == input.id)
                    .map(|position| next.retired.remove(position));
                next.items.push(BriefItem {
                    id: input.id.clone(),
                    section: input.section.clone(),
                    text: input.text.clone(),
                    link: input.link.clone(),
                    posted_at: now,
                    updated_at: now,
                    read: marks.as_ref().is_some_and(|marks| marks.read),
                    removed: marks.as_ref().is_some_and(|marks| marks.removed),
                    done: marks.as_ref().is_some_and(|marks| marks.done),
                    done_at: marks.and_then(|marks| marks.done_at),
                });
                changes.added.push(input.id.clone());
            }
            if !next.sections.contains(&input.section) {
                next.sections.push(input.section.clone());
            }
        }
        if next.items.len() > MAX_ITEMS {
            return Err("the brief holds no more than 500 items".into());
        }
        for item in taken {
            changes.retired.push(item.id.clone());
            if item.read || item.removed || item.done {
                next.retired.retain(|retired| retired.id != item.id);
                next.retired.push(RetiredItem {
                    id: item.id,
                    read: item.read,
                    removed: item.removed,
                    done: item.done,
                    done_at: item.done_at,
                    retired_at: now,
                });
            }
        }
        next.forget_old(now);
        *self = next;
        Ok(changes)
    }

    pub fn mark_read(&mut self, id: &str) -> bool {
        match self.items.iter_mut().find(|item| item.id == id) {
            Some(item) if !item.read => {
                item.read = true;
                true
            }
            _ => false,
        }
    }

    pub fn mark_all_read(&mut self) -> bool {
        let mut changed = false;
        for item in self.items.iter_mut().filter(|item| !item.removed) {
            changed |= !item.read;
            item.read = true;
        }
        changed
    }

    /// Marks one item done, or not done again. Done also means read.
    pub fn toggle_done(&mut self, id: &str, now: u64) -> bool {
        let Some(item) = self.items.iter_mut().find(|item| item.id == id) else {
            return false;
        };
        item.done = !item.done;
        item.done_at = item.done.then_some(now);
        item.read |= item.done;
        true
    }

    /// Hides one item. It stays in the brief, so a re-post keeps it hidden.
    pub fn remove(&mut self, id: &str) -> bool {
        match self.items.iter_mut().find(|item| item.id == id) {
            Some(item) if !item.removed => {
                item.removed = true;
                true
            }
            _ => false,
        }
    }

    /// The items the panel shows, grouped by section in section order.
    pub fn visible_sections(&self) -> Vec<(&str, Vec<&BriefItem>)> {
        let mut sections = self
            .sections
            .iter()
            .map(|section| (section.as_str(), Vec::new()))
            .collect::<Vec<_>>();
        for item in self.items.iter().filter(|item| !item.removed) {
            match sections.iter_mut().find(|(name, _)| *name == item.section) {
                Some((_, items)) => items.push(item),
                None => sections.push((item.section.as_str(), vec![item])),
            }
        }
        sections.retain(|(_, items)| !items.is_empty());
        sections
    }

    /// What get_brief returns: the stored brief plus the unread count.
    pub fn summary(&self) -> Value {
        let mut value = serde_json::to_value(self).unwrap_or_default();
        value["unreadCount"] = serde_json::json!(self.unread_count());
        value
    }

    pub fn unread_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| !item.read && !item.removed)
            .count()
    }

    fn forget_old(&mut self, now: u64) {
        self.retired
            .retain(|retired| now.saturating_sub(retired.retired_at) < REMEMBER_MILLIS);
        let excess = self.retired.len().saturating_sub(MAX_REMEMBERED);
        self.retired.drain(..excess);
        if self.sections.len() > MAX_SECTIONS {
            let items = &self.items;
            let mut excess = self.sections.len() - MAX_SECTIONS;
            self.sections.retain(|section| {
                let unused = !items.iter().any(|item| &item.section == section);
                let drop = excess > 0 && unused;
                excess -= usize::from(drop);
                !drop
            });
        }
    }
}

impl BriefItem {
    /// Takes the posted fields; true when any of them changed.
    fn apply(&mut self, input: &BriefItemInput, now: u64) -> bool {
        let changed =
            self.section != input.section || self.text != input.text || self.link != input.link;
        if changed {
            self.section = input.section.clone();
            self.text = input.text.clone();
            self.link = input.link.clone();
            self.updated_at = now;
        }
        changed
    }
}

fn validate_id(id: &str) -> BriefResult<()> {
    if id.trim() != id
        || !(1..=200).contains(&id.chars().count())
        || id.chars().any(char::is_control)
    {
        return Err("item id must be 1 to 200 printable characters".into());
    }
    Ok(())
}

fn validate_section(section: &str) -> BriefResult<()> {
    if section.trim() != section
        || !(1..=60).contains(&section.chars().count())
        || section.chars().any(char::is_control)
    {
        return Err("section must be 1 to 60 printable characters".into());
    }
    Ok(())
}

fn validate_text(text: &str) -> BriefResult<()> {
    if text.trim().is_empty()
        || text.chars().count() > 2000
        || text
            .chars()
            .any(|character| character.is_control() && character != '\n')
    {
        return Err("item text must be 1 to 2000 printable characters".into());
    }
    Ok(())
}

fn validate_link(link: &str) -> BriefResult<()> {
    let web = link.starts_with("https://") || link.starts_with("http://");
    if !web
        || link.len() > 2048
        || link
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err("item link must be an http or https URL".into());
    }
    Ok(())
}
