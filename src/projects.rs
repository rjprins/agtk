use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// The first remote's browser URL, without credentials or Git transport rewrites.
pub fn repository_url(root: &Path) -> Option<String> {
    let remotes = crate::git::optional(root, ["remote"])?;
    let remote = remotes.lines().next()?;
    // `remote get-url` applies insteadOf rules, which may introduce SSH aliases.
    let urls = crate::git::optional(
        root,
        ["config", "--get-all", &format!("remote.{remote}.url")],
    )?;
    remote_web_url(urls.lines().next()?)
}

fn remote_web_url(remote: &str) -> Option<String> {
    let remote = remote.trim();
    let normalized;
    let remote = if remote.contains("://") {
        remote
    } else {
        // Git's scp-like SSH syntax: git@host:owner/repository.git.
        let (authority, path) = remote.split_once(':')?;
        if !authority.contains('@') || authority.contains('/') {
            return None;
        }
        normalized = format!("ssh://{authority}/{path}");
        &normalized
    };
    let uri = glib::Uri::parse(remote, glib::UriFlags::ENCODED).ok()?;
    let scheme = uri.scheme();
    if !matches!(scheme.as_str(), "http" | "https" | "ssh") {
        return None;
    }
    let mut host = uri.host()?.to_ascii_lowercase();
    let path = uri.path();
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if host.is_empty() || path.trim_matches('/').is_empty() {
        return None;
    }
    let mut path = path.to_owned();
    let is_ssh = scheme == "ssh";
    if is_ssh {
        match host.as_str() {
            "ssh.dev.azure.com" | "vs-ssh.visualstudio.com" => {
                let parts = path.strip_prefix("/v3/")?.split('/').collect::<Vec<_>>();
                if parts.len() != 3 || parts.iter().any(|part| part.is_empty()) {
                    return None;
                }
                path = format!("/{}/{}/_git/{}", parts[0], parts[1], parts[2]);
                host = "dev.azure.com".to_owned();
            }
            "ssh.github.com" => host = "github.com".to_owned(),
            _ => {}
        }
    }
    Some(
        glib::Uri::build(
            glib::UriFlags::ENCODED,
            if is_ssh { "https" } else { &scheme },
            None,
            Some(&host),
            if is_ssh { -1 } else { uri.port() },
            &path,
            None,
            None,
        )
        .to_string(),
    )
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSettings {
    #[serde(default)]
    pub is_pinned: bool,
    #[serde(default)]
    pub is_collapsed: bool,
}

/// Every project a session ran in, with its settings. A project stays listed
/// after its sessions close, until it is removed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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
