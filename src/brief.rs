//! The daily brief an assistant posts over MCP: items grouped in sections,
//! each with read, hidden and done marks and the user's comment, which all
//! survive re-posts of the same id.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::providers::AgentProvider;

pub use crate::AppResult as BriefResult;

pub const PREFERENCE_KEY: &str = "brief";
const MAX_ITEMS: usize = 500;
const MAX_POSTED_ITEMS: usize = 200;
const MAX_REMEMBERED: usize = 2000;
const MAX_LINKS: usize = 8;
/// How long agtk remembers the marks of an item the assistant took out. A
/// pending comment stays until an assistant clears it.
pub const REMEMBER_MILLIS: u64 = 30 * 24 * 60 * 60 * 1000;
pub const MAX_COMMENT_CHARS: usize = 4000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BriefItemInput {
    pub id: String,
    pub section: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<BriefSession>,
}

/// The agent session an item is about, as list_sessions reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BriefSession {
    /// The agtk session id, while the session lives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Needed with conversationId, to find or restore the conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<AgentProvider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
}

/// Where opening an item's session leads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionTarget {
    /// A row agtk shows, live or exited.
    Select(String),
    /// No row has the conversation any more; it can be restored.
    Restore {
        provider: AgentProvider,
        conversation_id: String,
    },
    Gone,
}

impl BriefSession {
    /// Picks the row by id, else a row on the same conversation, else a restore.
    /// `sessions` holds each row's id, provider and conversation.
    pub fn target<'a>(
        &self,
        sessions: impl IntoIterator<Item = (&'a str, Option<AgentProvider>, Option<&'a str>)>,
    ) -> SessionTarget {
        let sessions = sessions.into_iter().collect::<Vec<_>>();
        if let Some(id) = &self.id
            && sessions.iter().any(|(row, _, _)| row == id)
        {
            return SessionTarget::Select(id.clone());
        }
        let (Some(provider), Some(conversation_id)) = (self.provider, &self.conversation_id) else {
            return SessionTarget::Gone;
        };
        match sessions.iter().find(|(_, row_provider, conversation)| {
            *row_provider == Some(provider) && *conversation == Some(conversation_id.as_str())
        }) {
            Some((row, _, _)) => SessionTarget::Select((*row).to_owned()),
            None => SessionTarget::Restore {
                provider,
                conversation_id: conversation_id.clone(),
            },
        }
    }

    fn validate(&self) -> BriefResult<()> {
        if self.id.is_none() && self.conversation_id.is_none() {
            return Err("session needs an id or a conversationId".into());
        }
        if self.conversation_id.is_some() && self.provider.is_none() {
            return Err("session needs a provider with its conversationId".into());
        }
        for value in [&self.id, &self.conversation_id].into_iter().flatten() {
            if !(1..=200).contains(&value.chars().count())
                || value
                    .chars()
                    .any(|character| character.is_whitespace() || character.is_control())
            {
                return Err("session ids must be 1 to 200 characters without spaces".into());
            }
        }
        Ok(())
    }
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
    /// Hidden items the assistant shows again because they need attention.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unhide_ids: Vec<String>,
    /// Comments the assistant has handled, also those on retired items.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clear_comments: Vec<BriefCommentClear>,
    /// Replaces the links at the top of the page; left out, they stay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<Vec<BriefLink>>,
}

/// Clears an item's comment, only if it is still the one the assistant read.
/// So an edit made while the assistant worked is not lost.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BriefCommentClear {
    pub id: String,
    pub commented_at: u64,
}

/// A fixed link at the top of the Brief page, such as the calendar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BriefLink {
    pub label: String,
    pub url: String,
}

impl BriefLink {
    /// The host the link goes to, for its tooltip.
    pub fn domain(&self) -> Option<String> {
        link_domain(&self.url)
    }
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
            if let Some(session) = &item.session {
                session.validate()?;
            }
            if !ids.insert(item.id.as_str()) {
                return Err(format!("item {} is posted twice", item.id).into());
            }
        }
        for section in &self.replace_sections {
            validate_section(section)?;
        }
        for id in &self.unhide_ids {
            validate_id(id)?;
        }
        for clear in &self.clear_comments {
            validate_id(&clear.id)?;
        }
        if let Some(links) = &self.links {
            if links.len() > MAX_LINKS {
                return Err("no more than 8 links are allowed".into());
            }
            for link in links {
                if link.label.trim() != link.label
                    || !(1..=30).contains(&link.label.chars().count())
                    || link.label.chars().any(char::is_control)
                {
                    return Err("link label must be 1 to 30 printable characters".into());
                }
                validate_link(&link.url)?;
            }
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
    #[serde(default)]
    pub session: Option<BriefSession>,
    pub posted_at: u64,
    pub updated_at: u64,
    pub read: bool,
    /// Hidden in the panel; the underlying task or mail is untouched.
    #[serde(alias = "removed")]
    pub hidden: bool,
    /// Marked done in the panel. The assistant completes the task itself.
    #[serde(default)]
    pub done: bool,
    /// When it was marked done, in Unix milliseconds.
    #[serde(default)]
    pub done_at: Option<u64>,
    /// The user's comment, set in the panel, until an assistant clears it.
    #[serde(default)]
    pub comment: Option<String>,
    /// When the comment was last written, in Unix milliseconds.
    #[serde(default)]
    pub commented_at: Option<u64>,
}

/// The marks of an item that left the brief, kept so a re-post restores them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetiredItem {
    pub id: String,
    pub read: bool,
    #[serde(alias = "removed")]
    pub hidden: bool,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub done_at: Option<u64>,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub commented_at: Option<u64>,
    pub retired_at: u64,
}

impl RetiredItem {
    fn holds_anything(&self) -> bool {
        self.read || self.hidden || self.done || self.comment.is_some()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefChanges {
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub retired: Vec<String>,
    pub unhidden: Vec<String>,
    pub comments_cleared: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Brief {
    /// In the order the panel shows them within each section.
    #[serde(default)]
    pub items: Vec<BriefItem>,
    /// Names of the sections that hold items, in the order they were first posted.
    #[serde(default)]
    pub sections: Vec<String>,
    #[serde(default)]
    pub retired: Vec<RetiredItem>,
    /// Links at the top of the page, kept until a post replaces them.
    #[serde(default)]
    pub links: Vec<BriefLink>,
    /// When an assistant last posted, also a post that changed nothing, in
    /// Unix milliseconds.
    #[serde(default)]
    pub last_posted_at: Option<u64>,
}

impl Brief {
    pub fn from_value_lossy(value: Value) -> Self {
        let mut brief: Self = serde_json::from_value(value).unwrap_or_else(|error| {
            eprintln!("agtk: ignoring saved brief: {error}");
            Self::default()
        });
        // Briefs saved before empty sections were dropped can still list some.
        brief.drop_empty_sections();
        brief
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
                    session: input.session.clone(),
                    posted_at: now,
                    updated_at: now,
                    read: marks.as_ref().is_some_and(|marks| marks.read),
                    hidden: marks.as_ref().is_some_and(|marks| marks.hidden),
                    done: marks.as_ref().is_some_and(|marks| marks.done),
                    done_at: marks.as_ref().and_then(|marks| marks.done_at),
                    comment: marks.as_ref().and_then(|marks| marks.comment.clone()),
                    commented_at: marks.and_then(|marks| marks.commented_at),
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
            if item.read || item.hidden || item.done || item.comment.is_some() {
                next.retired.retain(|retired| retired.id != item.id);
                next.retired.push(RetiredItem {
                    id: item.id,
                    read: item.read,
                    hidden: item.hidden,
                    done: item.done,
                    done_at: item.done_at,
                    comment: item.comment,
                    commented_at: item.commented_at,
                    retired_at: now,
                });
            }
        }
        for id in &params.unhide_ids {
            if next.unhide(id) {
                changes.unhidden.push(id.clone());
            }
        }
        for clear in &params.clear_comments {
            if next.clear_comment(clear) {
                changes.comments_cleared.push(clear.id.clone());
            }
        }
        if let Some(links) = &params.links {
            next.links = links.clone();
        }
        next.last_posted_at = Some(now);
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

    /// Marks one item done, which also reads and hides it, or not done again,
    /// which brings it back.
    pub fn toggle_done(&mut self, id: &str, now: u64) -> bool {
        let Some(item) = self.items.iter_mut().find(|item| item.id == id) else {
            return false;
        };
        item.done = !item.done;
        item.done_at = item.done.then_some(now);
        item.read |= item.done;
        item.hidden = item.done;
        true
    }

    /// Writes the user's comment on an item; an empty one removes it. Returns
    /// whether anything changed, so saving the same words keeps `commented_at`.
    pub fn set_comment(&mut self, id: &str, text: &str, now: u64) -> BriefResult<bool> {
        let printable = text
            .chars()
            .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
            .collect::<String>();
        let text = printable.trim();
        if text.chars().count() > MAX_COMMENT_CHARS {
            return Err(
                format!("a comment holds no more than {MAX_COMMENT_CHARS} characters").into(),
            );
        }
        let Some(item) = self.items.iter_mut().find(|item| item.id == id) else {
            return Ok(false);
        };
        let comment = (!text.is_empty()).then(|| text.to_owned());
        if item.comment == comment {
            return Ok(false);
        }
        item.commented_at = comment.is_some().then_some(now);
        item.comment = comment;
        Ok(true)
    }

    /// Clears a comment an assistant handled, unless the user has changed it since.
    fn clear_comment(&mut self, clear: &BriefCommentClear) -> bool {
        let current = Some(clear.commented_at);
        if let Some(item) = self.items.iter_mut().find(|item| item.id == clear.id) {
            let matches = item.comment.is_some() && item.commented_at == current;
            if matches {
                item.comment = None;
                item.commented_at = None;
            }
            return matches;
        }
        let Some(position) = self
            .retired
            .iter()
            .position(|retired| retired.id == clear.id)
        else {
            return false;
        };
        let retired = &mut self.retired[position];
        if retired.comment.is_none() || retired.commented_at != current {
            return false;
        }
        retired.comment = None;
        retired.commented_at = None;
        if !retired.holds_anything() {
            self.retired.remove(position);
        }
        true
    }

    /// Hides one item. It stays in the brief, so a re-post keeps it hidden.
    pub fn hide(&mut self, id: &str) -> bool {
        match self.items.iter_mut().find(|item| item.id == id) {
            Some(item) if !item.hidden => {
                item.hidden = true;
                true
            }
            _ => false,
        }
    }

    /// Shows a hidden item again, also one that left the brief.
    pub fn unhide(&mut self, id: &str) -> bool {
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
            let changed = item.hidden;
            item.hidden = false;
            return changed;
        }
        let Some(position) = self.retired.iter().position(|retired| retired.id == id) else {
            return false;
        };
        let retired = &mut self.retired[position];
        let changed = retired.hidden;
        retired.hidden = false;
        if !retired.holds_anything() {
            self.retired.remove(position);
        }
        changed
    }

    /// Hidden items, in brief order, for the panel's Hidden section.
    pub fn hidden_items(&self) -> Vec<&BriefItem> {
        self.items.iter().filter(|item| item.hidden).collect()
    }

    /// The items the panel shows, grouped by section in section order.
    pub fn visible_sections(&self) -> Vec<(&str, Vec<&BriefItem>)> {
        let mut sections = self
            .sections
            .iter()
            .map(|section| (section.as_str(), Vec::new()))
            .collect::<Vec<_>>();
        for item in self.items.iter().filter(|item| !item.hidden) {
            match sections.iter_mut().find(|(name, _)| *name == item.section) {
                Some((_, items)) => items.push(item),
                None => sections.push((item.section.as_str(), vec![item])),
            }
        }
        sections.retain(|(_, items)| !items.is_empty());
        sections
    }

    /// What get_brief returns: the stored brief plus the unread count. The
    /// panel no longer shows read; the phone page and assistants still use it.
    pub fn summary(&self) -> Value {
        let mut value = serde_json::to_value(self).unwrap_or_default();
        value["unreadCount"] = serde_json::json!(self.unread_count());
        value
    }

    /// The tab's title, with the time since the last post: "Brief (2m)". A
    /// brief saved before posts were timed goes by its newest item.
    pub fn tab_title(&self, now: u64) -> String {
        let newest = self.items.iter().map(|item| item.updated_at).max();
        match self.last_posted_at.or(newest) {
            Some(posted) => format!("Brief ({})", short_age(now.saturating_sub(posted))),
            None => "Brief".to_owned(),
        }
    }

    pub fn unread_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| !item.read && !item.hidden)
            .count()
    }

    fn forget_old(&mut self, now: u64) {
        // A pending comment is the user's words, so only an assistant ends it.
        self.retired.retain(|retired| {
            retired.comment.is_some() || now.saturating_sub(retired.retired_at) < REMEMBER_MILLIS
        });
        let mut excess = self.retired.len().saturating_sub(MAX_REMEMBERED);
        self.retired.retain(|retired| {
            let forget = excess > 0 && retired.comment.is_none();
            excess -= usize::from(forget);
            !forget
        });
        self.drop_empty_sections();
    }

    /// A section with no items left, not even hidden ones, leaves the order.
    fn drop_empty_sections(&mut self) {
        let items = &self.items;
        self.sections
            .retain(|section| items.iter().any(|item| &item.section == section));
    }
}

impl BriefItem {
    /// The host the link goes to, for the open button's tooltip.
    pub fn link_domain(&self) -> Option<String> {
        link_domain(self.link.as_deref()?)
    }

    /// Takes the posted fields; true when any of them changed.
    fn apply(&mut self, input: &BriefItemInput, now: u64) -> bool {
        let changed = self.section != input.section
            || self.text != input.text
            || self.link != input.link
            || self.session != input.session;
        if changed {
            self.section = input.section.clone();
            self.text = input.text.clone();
            self.link = input.link.clone();
            self.session = input.session.clone();
            self.updated_at = now;
        }
        changed
    }
}

fn short_age(millis: u64) -> String {
    let minutes = millis / 60_000;
    match minutes {
        0 => "now".to_owned(),
        1..60 => format!("{minutes}m"),
        60..1440 => format!("{}h", minutes / 60),
        _ => format!("{}d", minutes / 1440),
    }
}

fn link_domain(url: &str) -> Option<String> {
    let uri = glib::Uri::parse(url, glib::UriFlags::NONE).ok()?;
    uri.host().map(|host| host.to_string())
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
