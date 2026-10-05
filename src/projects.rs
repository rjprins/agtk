use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectSettings {
    #[serde(default)]
    pub is_pinned: bool,
    #[serde(default)]
    pub is_collapsed: bool,
}

/// Every project a session ran in, with its settings. A project stays listed
/// after its sessions close, until it is removed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectPreferences {
    #[serde(default)]
    pub projects: BTreeMap<String, ProjectSettings>,
}

impl ProjectPreferences {
    pub fn get(&self, root: &str) -> ProjectSettings {
        self.projects.get(root).copied().unwrap_or_default()
    }

    pub fn set(&mut self, root: &str, is_pinned: Option<bool>, is_collapsed: Option<bool>) {
        let settings = self.projects.entry(root.to_owned()).or_default();
        if let Some(is_pinned) = is_pinned {
            settings.is_pinned = is_pinned;
        }
        if let Some(is_collapsed) = is_collapsed {
            settings.is_collapsed = is_collapsed;
        }
    }

    /// Lists a project a session runs in. True when it was not listed yet.
    pub fn remember(&mut self, root: &str) -> bool {
        if self.projects.contains_key(root) {
            return false;
        }
        self.projects
            .insert(root.to_owned(), ProjectSettings::default());
        true
    }

    /// Forgets a project and its settings. False when it was not listed.
    pub fn remove(&mut self, root: &str) -> bool {
        self.projects.remove(root).is_some()
    }
}
