use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use agtk::azure::{
    AzureClient, AzurePr, AzureRepoRef, PrAttention, acknowledge_attention, normalize_active_prs,
    parse_azure_remote, reconcile_attention,
};
use serde_json::json;

#[test]
fn azure_remotes_and_active_pr_payloads_are_strictly_normalized() {
    assert_eq!(
        parse_azure_remote("https://dev.azure.com/demo-org/Project%20One/_git/app.git"),
        Some(AzureRepoRef {
            org_url: "https://dev.azure.com/demo-org".to_owned(),
            project: "Project One".to_owned(),
            repository: "app".to_owned(),
        })
    );
    assert_eq!(
        parse_azure_remote("git@ssh.dev.azure.com:v3/demo-org/Project/app"),
        Some(AzureRepoRef {
            org_url: "https://dev.azure.com/demo-org".to_owned(),
            project: "Project".to_owned(),
            repository: "app".to_owned(),
        })
    );
    assert_eq!(parse_azure_remote("git@github.com:demo/app.git"), None);

    let reference = parse_azure_remote("https://dev.azure.com/demo/Project/_git/app").unwrap();
    let prs = normalize_active_prs(
        &reference,
        json!([
            {
                "pullRequestId":42,
                "title":"Native workspace",
                "sourceRefName":"refs/heads/native-workspace",
                "targetRefName":"refs/heads/main",
                "creationDate":"2026-09-15T08:00:00.000Z",
                "isDraft":false,
                "createdBy":{"displayName":"Ada","uniqueName":"ada@example.com"},
                "lastMergeSourceCommit":{"commitId":"abcdef"},
                "mergeStatus":"succeeded",
                "reviewers":[{"vote":10,"isContainer":false},{"vote":-10,"isContainer":true}]
            },
            {"pullRequestId":"bad","title":"ignored"}
        ]),
    );
    assert_eq!(prs.len(), 1);
    assert_eq!(prs[0].id, 42);
    assert_eq!(prs[0].source_branch, "native-workspace");
    assert_eq!(prs[0].reviewer_votes, [10]);
    assert!(prs[0].url.contains("pullrequest/42"));

    // Snapshots saved before PBI links were added still restore successfully.
    let mut legacy = serde_json::to_value(&prs[0]).unwrap();
    legacy.as_object_mut().unwrap().remove("linkedPbis");
    let restored: AzurePr = serde_json::from_value(legacy).unwrap();
    assert!(restored.linked_pbis.is_empty());
}

#[test]
fn attention_baselines_then_tracks_new_published_and_review_events_with_cas_acknowledgement() {
    let draft = pr(1, true, 0, 100);
    let initial = reconcile_attention(None, std::slice::from_ref(&draft));
    assert!(initial.state.attention.is_empty());

    let published = pr(1, false, 0, 110);
    let new_pr = pr(2, false, 0, 110);
    let next = reconcile_attention(Some(&initial.state), &[published, new_pr]);
    assert_eq!(next.state.attention[&1], PrAttention::Published);
    assert_eq!(next.state.attention[&2], PrAttention::New);
    assert_eq!(next.changed, [1, 2].into_iter().collect());

    let reviewed = reconcile_attention(
        Some(&next.state),
        &[pr(1, false, 2, 120), pr(2, false, 0, 110)],
    );
    assert_eq!(reviewed.state.attention[&1], PrAttention::Review);
    assert_eq!(reviewed.changed, [1].into_iter().collect());
    // A review thread (even one we posted ourselves) must not start another review.
    assert!(!PrAttention::Review.launches_auto_review());
    assert!(PrAttention::New.launches_auto_review());
    assert!(PrAttention::Published.launches_auto_review());
    let acknowledged = acknowledge_attention(
        &reviewed.state,
        &[(1, PrAttention::Published), (2, PrAttention::New)],
    );
    assert_eq!(acknowledged.attention[&1], PrAttention::Review);
    assert!(!acknowledged.attention.contains_key(&2));
}

#[test]
fn azure_client_uses_bounded_cli_calls_and_collects_unresolved_threads() {
    let directory = tempfile::tempdir().unwrap();
    let repository = directory.path().join("repository");
    fs::create_dir(&repository).unwrap();
    run_git(&repository, &["init", "-q"]);
    run_git(
        &repository,
        &[
            "remote",
            "add",
            "origin",
            "https://dev.azure.com/demo/Project/_git/app",
        ],
    );
    let az = directory.path().join("fake-az");
    fs::write(
        &az,
        r##"#!/bin/sh
case "$*" in
  "account show"*) printf 'rutger@example.com\n' ;;
  "repos pr list"*) printf '%s\n' '[{"pullRequestId":7,"title":"Review me","sourceRefName":"refs/heads/review-me","targetRefName":"refs/heads/main","creationDate":"2026-09-15T08:00:00Z","isDraft":false,"createdBy":{"displayName":"Other","uniqueName":"other@example.com"},"reviewers":[]}]' ;;
  "repos pr work-item list"*) [ -e "$0.fail-pbis" ] && exit 7; printf '%s\n' '[{"id":123,"fields":{"System.WorkItemType":"Product Backlog Item","System.Title":"Linked backlog item","System.TeamProject":"Project One"}},{"id":124,"fields":{"System.WorkItemType":"Bug","System.Title":"Linked bug"}},{"id":125,"fields":{"System.WorkItemType":"Product Backlog Item","System.Title":"Another backlog item"}}]' ;;
  "devops invoke"*) [ -e "$0.fail-threads" ] && exit 7; printf '%s\n' '{"value":[{"id":3,"status":"active","comments":[{"id":1,"commentType":"text","content":"Please fix this","isDeleted":false,"publishedDate":"2026-09-15T09:00:00Z"}]}]}' ;;
  *) printf 'unexpected arguments: %s\n' "$*" >&2; exit 7 ;;
esac
"##,
    )
    .unwrap();
    fs::set_permissions(&az, fs::Permissions::from_mode(0o700)).unwrap();

    let result = AzureClient::new(&az)
        .list_active(&repository)
        .unwrap()
        .unwrap();
    assert_eq!(result.current_user.as_deref(), Some("rutger@example.com"));
    assert_eq!(result.pull_requests.len(), 1);
    assert_eq!(result.pull_requests[0].unresolved_threads, 1);
    assert_eq!(result.pull_requests[0].latest_review_at, 1_789_462_800_000);
    let pbis = &result.pull_requests[0].linked_pbis;
    assert_eq!(pbis.len(), 2);
    assert_eq!(pbis[0].id, 123);
    assert_eq!(pbis[0].title, "Linked backlog item");
    assert_eq!(
        pbis[0].url,
        "https://dev.azure.com/demo/Project%20One/_workitems/edit/123"
    );
    assert_eq!(
        pbis[1].url,
        "https://dev.azure.com/demo/Project/_workitems/edit/125"
    );

    let failed_pbis = directory.path().join("fake-az.fail-pbis");
    fs::write(&failed_pbis, "").unwrap();
    let result = AzureClient::new(&az)
        .list_active(&repository)
        .unwrap()
        .unwrap();
    assert_eq!(result.pull_requests[0].unresolved_threads, 1);
    assert!(result.pull_requests[0].linked_pbis.is_empty());

    fs::remove_file(failed_pbis).unwrap();
    fs::write(directory.path().join("fake-az.fail-threads"), "").unwrap();
    let result = AzureClient::new(&az)
        .list_active(&repository)
        .unwrap()
        .unwrap();
    assert_eq!(result.pull_requests[0].linked_pbis.len(), 2);
}

fn pr(id: u64, is_draft: bool, unresolved_threads: u32, updated_at: u64) -> AzurePr {
    AzurePr {
        id,
        title: format!("PR {id}"),
        author: "Author".to_owned(),
        author_unique_name: Some("author@example.com".to_owned()),
        is_own_author: false,
        is_draft,
        source_branch: format!("branch-{id}"),
        target_branch: "main".to_owned(),
        created_at: 100,
        updated_at,
        latest_review_at: updated_at,
        head_sha: None,
        merge_status: "unknown".to_owned(),
        reviewer_votes: Vec::new(),
        unresolved_threads,
        linked_pbis: Vec::new(),
        url: format!("https://example.test/{id}"),
    }
}

fn run_git(cwd: &std::path::Path, args: &[&str]) {
    assert!(
        Command::new("git")
            .args(args)
            .current_dir(cwd)
            .status()
            .unwrap()
            .success()
    );
}
