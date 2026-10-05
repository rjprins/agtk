//! Azure DevOps pull requests for the repositories agtk works in.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

mod attention;
mod client;
mod normalize;
mod remote;

pub use attention::{
    AttentionReconciliation, KnownPr, PrAttention, PrPreferences, PrProjectState, PrReviewSettings,
    acknowledge_attention, reconcile_attention,
};
pub use client::AzureClient;
pub use normalize::normalize_active_prs;
pub use remote::parse_azure_remote;

pub use crate::AppResult as AzureResult;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AzureRepoRef {
    pub org_url: String,
    pub project: String,
    pub repository: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AzurePr {
    pub id: u64,
    pub title: String,
    pub author: String,
    pub author_unique_name: Option<String>,
    pub is_own_author: bool,
    pub is_draft: bool,
    pub source_branch: String,
    pub target_branch: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub latest_review_at: u64,
    pub head_sha: Option<String>,
    pub merge_status: String,
    pub reviewer_votes: Vec<i64>,
    pub unresolved_threads: u32,
    #[serde(default)]
    pub linked_pbis: Vec<LinkedPbi>,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkedPbi {
    pub id: u64,
    pub title: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AzurePrList {
    pub repository: AzureRepoRef,
    pub current_user: Option<String>,
    pub pull_requests: Vec<AzurePr>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrItem {
    pub pull_request: AzurePr,
    pub attention: Option<PrAttention>,
    pub worktree_path: Option<PathBuf>,
    /// Where a review of this PR runs, when the worktree layout was readable.
    pub review_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrContext {
    pub project_root: PathBuf,
    pub repository: AzureRepoRef,
    pub current_user: Option<String>,
    pub auto_review: bool,
    pub pull_requests: Vec<PrItem>,
}

#[cfg(test)]
mod tests {
    use super::normalize::{normalize_linked_pbis, normalize_thread_summary};
    use super::parse_azure_remote;
    use serde_json::json;

    #[test]
    fn ssh_remotes_decode_spaces_in_project_and_repository() {
        let reference = parse_azure_remote(
            "git@ssh.dev.azure.com:v3/greenchoice/Flex%20Optimization/Flex%20Optimization",
        )
        .unwrap();
        assert_eq!(reference.org_url, "https://dev.azure.com/greenchoice");
        assert_eq!(reference.project, "Flex Optimization");
        assert_eq!(reference.repository, "Flex Optimization");
    }

    #[test]
    fn linked_pbis_validate_ids_deduplicate_and_build_browser_links() {
        let reference =
            parse_azure_remote("https://dev.azure.com/demo/Project%20One/_git/app").unwrap();
        let pbis = normalize_linked_pbis(
            &reference,
            &json!([
                {"id":42, "url":"javascript:ignored", "fields":{"System.WorkItemType":"Product Backlog Item", "System.Title":"Title <&>", "System.TeamProject":"Other Project"}},
                {"id":42, "fields":{"System.WorkItemType":"Product Backlog Item"}},
                {"id":0, "fields":{"System.WorkItemType":"Product Backlog Item"}},
                {"id":-1, "fields":{"System.WorkItemType":"Product Backlog Item"}},
                {"id":"invalid", "fields":{"System.WorkItemType":"Product Backlog Item"}},
                {"id":43, "fields":{"System.WorkItemType":"Task"}},
                null
            ]),
        );
        assert_eq!(pbis.len(), 1);
        assert_eq!(pbis[0].id, 42);
        assert_eq!(pbis[0].title, "Title <&>");
        assert_eq!(
            pbis[0].url,
            "https://dev.azure.com/demo/Other%20Project/_workitems/edit/42"
        );
        assert!(normalize_linked_pbis(&reference, &json!([])).is_empty());
    }

    #[test]
    fn system_events_do_not_count_as_unresolved_or_update_review_time() {
        let summary = normalize_thread_summary(&json!({"value": [
            {"status": null, "comments": [{"commentType": "system", "content": "Branch updated", "publishedDate": "2026-09-30T10:00:00Z"}]},
            {"comments": [{"commentType": "system", "content": "Reviewer added", "publishedDate": "2026-09-30T11:00:00Z"}]},
            {"status": "active", "comments": [{"commentType": "system", "content": "Policy updated", "publishedDate": "2026-09-30T12:00:00Z"}]}
        ]}));

        assert_eq!(summary.unresolved_threads, 0);
        assert_eq!(summary.latest_review_at, 0);
    }

    #[test]
    fn only_active_and_pending_review_threads_are_unresolved() {
        for status in [
            json!("active"),
            json!("pending"),
            json!("fixed"),
            json!("closed"),
            json!("wontFix"),
            json!("byDesign"),
            json!("unknown"),
            json!(null),
        ] {
            let summary = normalize_thread_summary(&json!([
                {"status": status, "comments": [{"commentType": "text", "content": "Please fix this"}]}
            ]));
            let expected = u32::from(status == "active" || status == "pending");
            assert_eq!(summary.unresolved_threads, expected, "status: {status}");
        }
    }

    #[test]
    fn review_summary_ignores_deleted_comments_and_system_replies() {
        let summary = normalize_thread_summary(&json!({"value": [
            {"status": "active", "comments": [
                {"commentType": "text", "content": "Please fix this", "publishedDate": "2026-09-15T09:00:00Z"},
                {"commentType": "text", "content": "More detail", "lastUpdatedDate": "2026-09-15T10:00:00Z"},
                {"commentType": "system", "content": "Branch updated", "publishedDate": "2026-09-30T10:00:00Z"},
                {"commentType": "text", "content": "Deleted", "isDeleted": true, "publishedDate": "2026-09-30T11:00:00Z"}
            ]},
            {"status": "active", "isDeleted": true, "comments": [{"commentType": "text", "content": "Deleted thread"}]},
            {"status": "active", "comments": [{"commentType": "text", "content": "Deleted comment", "isDeleted": true}]},
            {"status": "active", "comments": [{"commentType": "text", "content": "   "}]}
        ]}));

        assert_eq!(summary.unresolved_threads, 1);
        assert_eq!(summary.latest_review_at, 1_789_466_400_000);
    }
}
