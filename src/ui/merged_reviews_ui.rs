use std::collections::{BTreeMap, BTreeSet};

use super::*;
use crate::azure::{AzureClient, PrContext};
use crate::worktrees::{DeleteBranch, ReapRequest, WorktreeManager};

use super::azure_ui::{in_project, review_pr_id};

/// A PR whose review is done for good because the PR was merged.
struct MergedReview {
    id: u64,
    checkout: Option<PathBuf>,
}

impl Workspace {
    /// Closes the review sessions of PRs that left the active list because they
    /// were merged, and removes their `pr-<id>` checkouts. An abandoned PR, or one
    /// Azure could not describe, keeps its review.
    pub(super) fn remove_merged_reviews(&self, context: &PrContext) {
        let root_key = context.project_root.to_string_lossy().into_owned();
        let active = context
            .pull_requests
            .iter()
            .map(|item| item.pull_request.id)
            .collect::<BTreeSet<_>>();
        let named = self
            .sessions
            .borrow()
            .values()
            .filter(|session| in_project(&session.record, context))
            .filter_map(|session| review_pr_id(&session.record.name))
            .collect::<BTreeSet<_>>();
        let settled = self
            .settled_reviews
            .borrow()
            .iter()
            .filter(|(root, _)| *root == root_key)
            .map(|(_, id)| *id)
            .collect::<BTreeSet<_>>();
        let project_root = context.project_root.clone();
        let attic = self.paths.attic_dir();
        self.run_on(
            &self.pr_poll_io,
            move || {
                let manager = WorktreeManager::new(attic);
                let layout = manager.layout(&project_root)?;
                let mut checkouts = BTreeMap::new();
                for path in manager.linked_paths(&project_root)? {
                    let Some(id) = path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .and_then(|name| name.strip_prefix("pr-"))
                        .and_then(|id| id.parse::<u64>().ok())
                    else {
                        continue;
                    };
                    if layout
                        .review_path(id)
                        .is_ok_and(|expected| same_path(&expected, &path))
                    {
                        checkouts.insert(id, path);
                    }
                }
                let client = AzureClient::from_environment();
                let mut checked = Vec::new();
                for id in checkouts
                    .keys()
                    .copied()
                    .chain(named)
                    .collect::<BTreeSet<_>>()
                {
                    if active.contains(&id) || settled.contains(&id) {
                        continue;
                    }
                    // A failed lookup is retried on the next poll.
                    if let Ok(merged) = client.is_merged(&project_root, id) {
                        checked.push((
                            MergedReview {
                                id,
                                checkout: checkouts.get(&id).cloned(),
                            },
                            merged,
                        ));
                    }
                }
                Ok(checked)
            },
            move |workspace, result| {
                let Ok(checked) = result else {
                    return;
                };
                for (review, merged) in checked {
                    // Each PR is looked up once per run; a merged one is cleaned up once.
                    if !workspace
                        .settled_reviews
                        .borrow_mut()
                        .insert((root_key.clone(), review.id))
                    {
                        continue;
                    }
                    if merged {
                        workspace.close_merged_review(&root_key, review);
                    }
                }
            },
        );
    }

    fn close_merged_review(&self, root_key: &str, review: MergedReview) {
        let root = Path::new(root_key);
        let sessions = self
            .sessions
            .borrow()
            .values()
            .map(|session| &session.record)
            .filter(|record| {
                record
                    .project_root
                    .as_deref()
                    .is_some_and(|project| same_path(project, root))
            })
            .filter(|record| {
                review_pr_id(&record.name) == Some(review.id)
                    || review.checkout.as_deref().is_some_and(|checkout| {
                        record
                            .worktree_path
                            .as_deref()
                            .is_some_and(|path| same_path(path, checkout))
                    })
            })
            .map(|record| record.id.clone())
            .collect::<Vec<_>>();
        let remaining = Rc::new(Cell::new(sessions.len()));
        let review = Rc::new(review);
        if sessions.is_empty() {
            self.remove_merged_checkout(&review);
            return;
        }
        for id in sessions {
            let remaining = remaining.clone();
            let review = review.clone();
            // A failed stop keeps its session, and the checkout with it.
            self.stop_session_then(&id, None, move |workspace, _| {
                remaining.set(remaining.get() - 1);
                if remaining.get() == 0 {
                    workspace.remove_merged_checkout(&review);
                }
            });
        }
    }

    fn remove_merged_checkout(&self, review: &MergedReview) {
        let id = review.id;
        let Some(checkout) = review.checkout.clone() else {
            self.show_error(&format!("PR #{id} merged: its review session was closed"));
            return;
        };
        let manager = WorktreeManager::new(self.paths.attic_dir());
        let live_paths = self.live_worktree_paths();
        self.run_slow(
            move || {
                let repo = manager.repository_root(&checkout)?;
                let preview = manager
                    .inspect(&repo, &checkout, &live_paths)?
                    .ok_or("the checkout is not listed by its repository")?;
                // The merge is the confirmation; edits still go to the attic first.
                manager.reap(
                    ReapRequest {
                        path: preview.path,
                        expected_head: preview.head.unwrap_or_default(),
                        expected_status_hash: preview.status_hash,
                        delete_branch: DeleteBranch::Never,
                        confirmed: true,
                    },
                    &live_paths,
                )
            },
            move |workspace, result| {
                let message = match result {
                    Ok(result) if result.ok => match result.salvage_path {
                        Some(salvage) => format!(
                            "PR #{id} merged: its review was removed, edits saved to {}",
                            salvage.display()
                        ),
                        None => format!("PR #{id} merged: its review was removed"),
                    },
                    Ok(result) => format!(
                        "PR #{id} merged, review checkout kept: {}",
                        result.reason.as_deref().unwrap_or("removal aborted")
                    ),
                    Err(error) => format!("PR #{id} merged, review checkout kept: {error}"),
                };
                workspace.show_error(&message);
                if workspace.worktrees.modal.is_visible() {
                    workspace.refresh_worktree_panel();
                }
            },
        );
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    left.canonicalize().unwrap_or_else(|_| left.to_path_buf())
        == right.canonicalize().unwrap_or_else(|_| right.to_path_buf())
}
