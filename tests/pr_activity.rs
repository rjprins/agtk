use agtk::azure::{AzurePr, PrActivityTracker, PrComment};
use serde_json::json;

fn pr(comments: Option<Vec<PrComment>>) -> AzurePr {
    let mut pr: AzurePr = serde_json::from_value(json!({
        "id":42, "title":"Example", "author":"Me", "authorUniqueName":"me@example.com",
        "isOwnAuthor":true, "isDraft":false, "sourceBranch":"feature", "targetBranch":"main",
        "createdAt":1, "updatedAt":1, "latestReviewAt":0, "headSha":"a",
        "mergeStatus":"succeeded", "reviewerVotes":[], "unresolvedThreads":0,
        "url":"https://dev.azure.com/org/project/_git/repo/pullrequest/42"
    }))
    .unwrap();
    pr.comments = comments;
    pr
}

fn comment(thread_id: u64, id: u64, updated_at: u64, author: &str) -> PrComment {
    PrComment {
        thread_id,
        id,
        updated_at,
        author_unique_name: Some(author.into()),
    }
}

#[test]
fn first_snapshot_is_a_baseline_and_each_session_gets_later_comments_once() {
    let mut tracker = PrActivityTracker::default();
    let initial = pr(Some(vec![comment(1, 1, 10, "other@example.com")]));
    for session in ["author", "review"] {
        tracker.observe(session, &initial, Some("me@example.com"));
        assert!(tracker.take_notification(session).is_none());
    }
    let update = pr(Some(vec![
        comment(1, 1, 10, "other@example.com"),
        comment(1, 2, 10, "other@example.com"),
    ]));
    for session in ["author", "review"] {
        tracker.observe(session, &update, Some("me@example.com"));
        let notification = tracker.take_notification(session).unwrap();
        assert!(notification.contains("PR #42"));
        assert!(notification.contains("1 new or edited comment"));
        assert!(notification.contains(&update.url));
        tracker.observe(session, &update, Some("me@example.com"));
        assert!(tracker.take_notification(session).is_none());
    }
}

#[test]
fn only_comments_notify_not_pushes_or_votes() {
    let mut tracker = PrActivityTracker::default();
    let initial = pr(Some(vec![]));
    tracker.observe("agent", &initial, Some("me@example.com"));
    let mut changed = initial.clone();
    changed.head_sha = Some("b".into());
    changed.reviewer_votes = vec![10];
    tracker.observe("agent", &changed, Some("me@example.com"));
    assert!(!tracker.has_pending("agent"));
    assert!(tracker.take_notification("agent").is_none());

    changed.comments = Some(vec![comment(1, 1, 10, "other")]);
    tracker.observe("agent", &changed, Some("me@example.com"));
    let notification = tracker.take_notification("agent").unwrap();
    assert!(notification.contains("1 new or edited comment"));
    assert!(!notification.contains("branch"), "{notification}");
}

#[test]
fn a_push_saved_as_pending_by_an_older_version_is_dropped_on_load() {
    let saved = json!({"sessions":{
        "push":{
            "url":"u", "id":42, "headSha":"b", "comments":{}, "pendingComments":[],
            "pendingHead":["a", "b"]
        },
        "both":{
            "url":"u", "id":43, "headSha":"b", "comments":{"1:1":10}, "pendingComments":["1:1"],
            "pendingHead":["a", "b"]
        }
    }});
    let mut tracker: PrActivityTracker = serde_json::from_value(saved).unwrap();
    assert!(!tracker.has_pending("push"));
    assert!(tracker.take_notification("push").is_none());
    let notification = tracker.take_notification("both").unwrap();
    assert!(notification.contains("1 new or edited comment"));
    assert!(!notification.contains("branch"), "{notification}");
}

#[test]
fn own_comments_and_missing_identity_do_not_create_reply_loops() {
    let mut tracker = PrActivityTracker::default();
    tracker.observe("agent", &pr(Some(vec![])), Some("me@example.com"));
    let own = pr(Some(vec![comment(1, 1, 10, "ME@example.com")]));
    tracker.observe("agent", &own, Some("me@example.com"));
    assert!(tracker.take_notification("agent").is_none());
    let others = pr(Some(vec![comment(1, 2, 20, "other@example.com")]));
    tracker.observe("agent", &others, None);
    assert!(tracker.take_notification("agent").is_none());
    tracker.observe("agent", &others, Some("me@example.com"));
    assert!(tracker.take_notification("agent").is_some());
}

#[test]
fn failed_details_keep_the_baseline_and_do_not_swallow_changes() {
    let mut tracker = PrActivityTracker::default();
    tracker.observe("agent", &pr(None), Some("me@example.com"));
    tracker.observe(
        "agent",
        &pr(Some(vec![comment(1, 1, 10, "other")])),
        Some("me@example.com"),
    );
    assert!(tracker.take_notification("agent").is_none());
    tracker.observe("agent", &pr(None), Some("me@example.com"));
    let update = pr(Some(vec![
        comment(1, 1, 20, "other"),
        comment(2, 1, 10, "other"),
    ]));
    tracker.observe("agent", &update, Some("me@example.com"));
    let notification = tracker.take_notification("agent").unwrap();
    assert!(notification.contains("2 new or edited comments"));
}

#[test]
fn pending_comments_coalesce_and_survive_restart() {
    let mut tracker = PrActivityTracker::default();
    tracker.observe("agent", &pr(Some(vec![])), Some("me@example.com"));
    tracker.observe(
        "agent",
        &pr(Some(vec![comment(1, 1, 10, "other")])),
        Some("me@example.com"),
    );
    tracker.observe(
        "agent",
        &pr(Some(vec![comment(1, 1, 20, "other")])),
        Some("me@example.com"),
    );
    let mut restored: PrActivityTracker =
        serde_json::from_value(serde_json::to_value(&tracker).unwrap()).unwrap();
    let notification = restored.take_notification("agent").unwrap();
    assert!(notification.contains("1 new or edited comment"));
    assert!(restored.take_notification("agent").is_none());
    restored.observe(
        "agent",
        &pr(Some(vec![comment(1, 1, 20, "other")])),
        Some("me@example.com"),
    );
    assert!(restored.take_notification("agent").is_none());
}

#[test]
fn changing_pr_or_repository_discards_old_pending_activity() {
    let mut tracker = PrActivityTracker::default();
    tracker.observe("agent", &pr(Some(vec![])), Some("me@example.com"));
    tracker.observe(
        "agent",
        &pr(Some(vec![comment(1, 1, 10, "other")])),
        Some("me@example.com"),
    );
    assert!(tracker.has_pending("agent"));
    let mut other = pr(Some(vec![comment(1, 1, 10, "other")]));
    other.url = other.url.replace("/_git/repo/", "/_git/other/");
    tracker.observe("agent", &other, Some("me@example.com"));
    assert!(tracker.take_notification("agent").is_none());
    tracker.forget("agent");
    tracker.observe(
        "agent",
        &pr(Some(vec![comment(1, 2, 10, "other")])),
        Some("me@example.com"),
    );
    assert!(tracker.take_notification("agent").is_none());
}
