//! Tracks which pull requests have news the user has not acknowledged yet.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::AzurePr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PrAttention {
    New,
    Published,
    Review,
}

impl PrAttention {
    /// Only a PR that just appeared or left draft gets an automatic review. Review
    /// activity stays sidebar attention: it fires on every thread, including our own.
    pub const fn launches_auto_review(self) -> bool {
        matches!(self, Self::New | Self::Published)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownPr {
    pub is_draft: bool,
    pub unresolved_threads: u32,
    pub latest_review_at: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrProjectState {
    #[serde(default)]
    pub auto_review: bool,
    #[serde(default)]
    pub known: BTreeMap<u64, KnownPr>,
    #[serde(default)]
    pub attention: BTreeMap<u64, PrAttention>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttentionReconciliation {
    pub state: PrProjectState,
    pub changed: BTreeSet<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrPreferences {
    #[serde(default)]
    pub review: PrReviewSettings,
    #[serde(default)]
    pub projects: BTreeMap<String, PrProjectState>,
}

/// Settings shared by automatic and manual PR reviews. Empty fields inherit Codex config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PrReviewSettings {
    pub model: String,
    pub effort: String,
    pub approval: String,
    pub sandbox: String,
    pub prompt: String,
}

impl Default for PrReviewSettings {
    fn default() -> Self {
        Self {
            model: String::new(),
            effort: String::new(),
            approval: String::new(),
            sandbox: String::new(),
            prompt: "/review-pr {pr_id}".to_owned(),
        }
    }
}

impl PrReviewSettings {
    pub fn args(&self) -> Vec<String> {
        let mut args = crate::launch_model::codex_args_without_agtk_mcp();
        for (flag, value) in [
            ("--model", &self.model),
            ("--ask-for-approval", &self.approval),
            ("--sandbox", &self.sandbox),
        ] {
            if !value.trim().is_empty() {
                args.extend([flag.to_owned(), value.trim().to_owned()]);
            }
        }
        if !self.effort.is_empty() {
            args.extend([
                "-c".to_owned(),
                format!(
                    "model_reasoning_effort={}",
                    serde_json::to_string(&self.effort).unwrap()
                ),
            ]);
        }
        args
    }

    pub fn prompt(&self, pr_id: u64) -> String {
        self.prompt.replace("{pr_id}", &pr_id.to_string())
    }
}

impl PrPreferences {
    pub fn project(&self, root: &str) -> PrProjectState {
        self.projects.get(root).cloned().unwrap_or_default()
    }

    pub fn set_project(&mut self, root: String, state: PrProjectState) {
        self.projects.insert(root, state);
    }
}

pub fn reconcile_attention(
    previous: Option<&PrProjectState>,
    pull_requests: &[AzurePr],
) -> AttentionReconciliation {
    let first_load = previous.is_none();
    let mut state = PrProjectState {
        auto_review: previous.is_some_and(|state| state.auto_review),
        ..PrProjectState::default()
    };
    let mut changed = BTreeSet::new();
    for pull_request in pull_requests {
        let previous_known = previous.and_then(|state| state.known.get(&pull_request.id));
        let previous_attention = previous.and_then(|state| state.attention.get(&pull_request.id));
        let marker = if first_load {
            None
        } else if previous_known.is_none() {
            Some(PrAttention::New)
        } else if previous_known.is_some_and(|known| known.is_draft && !pull_request.is_draft) {
            Some(PrAttention::Published)
        } else if previous_known.is_some_and(|known| {
            pull_request.unresolved_threads > known.unresolved_threads
                || (pull_request.unresolved_threads > 0
                    && pull_request.latest_review_at > known.latest_review_at)
        }) {
            Some(PrAttention::Review)
        } else {
            None
        };
        if let Some(marker) = marker.or(previous_attention.copied()) {
            state.attention.insert(pull_request.id, marker);
        }
        if let Some(marker) = marker
            && previous_attention.copied() != Some(marker)
        {
            changed.insert(pull_request.id);
        }
        state.known.insert(
            pull_request.id,
            KnownPr {
                is_draft: pull_request.is_draft,
                unresolved_threads: pull_request.unresolved_threads,
                latest_review_at: pull_request.latest_review_at,
            },
        );
    }
    AttentionReconciliation { state, changed }
}

pub fn acknowledge_attention(
    state: &PrProjectState,
    markers: &[(u64, PrAttention)],
) -> PrProjectState {
    let mut state = state.clone();
    for (id, marker) in markers {
        if state.attention.get(id) == Some(marker) {
            state.attention.remove(id);
        }
    }
    state
}
