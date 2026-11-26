//! The PR updates feed, newest first: what other people said and voted on the
//! PRs the user wrote or reviews, and which PRs others published, merged or
//! abandoned.

use serde::Serialize;

use super::{AzurePr, PrStatusChange};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum PrUpdateKind {
    Comment {
        text: String,
        file_path: Option<String>,
        line: Option<u64>,
    },
    /// 10 approved, 5 approved with suggestions, -5 waiting for author, -10 rejected.
    Vote {
        vote: i64,
    },
    /// Left draft, or was opened as a ready PR.
    Published,
    Merged,
    Abandoned,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrUpdate {
    /// Stays the same from poll to poll, so the user can close the update.
    pub id: String,
    pub pr_id: u64,
    pub pr_title: String,
    pub pr_url: String,
    /// 0 when Azure DevOps sent none.
    pub thread_id: u64,
    /// Who did it; empty when Azure DevOps did not say.
    pub author: String,
    /// Unix milliseconds.
    pub at: u64,
    pub kind: PrUpdateKind,
}

impl PrUpdate {
    /// The PR page opened at the update's thread, when it has one.
    #[must_use]
    pub fn link(&self) -> String {
        let base = self.pr_url.split('?').next().unwrap_or(&self.pr_url);
        match self.kind {
            PrUpdateKind::Comment { .. } | PrUpdateKind::Vote { .. } if self.thread_id > 0 => {
                format!("{base}?_a=overview&discussionId={}", self.thread_id)
            }
            _ => format!("{base}?_a=overview"),
        }
    }
}

/// Newest first: comments and votes by other people on the PRs `me` wrote, is
/// a reviewer on, or commented or voted on, and any PR that someone else
/// published, merged or abandoned. Give both active and recently closed PRs.
/// With `me` unknown every PR counts and nothing is left out as the user's own.
#[must_use]
pub fn pr_updates<'a>(
    pull_requests: impl IntoIterator<Item = &'a AzurePr>,
    me: Option<&str>,
) -> Vec<PrUpdate> {
    let is_me = |name: Option<&str>| {
        me.zip(name)
            .is_some_and(|(me, name)| me.eq_ignore_ascii_case(name))
    };
    let mut updates = Vec::new();
    for pull_request in pull_requests {
        let threads = pull_request.threads.as_deref().unwrap_or_default();
        let involved = me.is_none()
            || pull_request.is_own_author
            || is_me(pull_request.author_unique_name.as_deref())
            || pull_request
                .reviewers
                .iter()
                .any(|reviewer| is_me(Some(reviewer)))
            || threads
                .iter()
                .flat_map(|thread| &thread.comments)
                .any(|comment| is_me(comment.author_unique_name.as_deref()))
            || pull_request
                .votes
                .iter()
                .any(|vote| is_me(vote.voter_unique_name.as_deref()));
        let update = |id: String, thread_id, author: &str, at, kind| PrUpdate {
            id: format!("{}:{id}", pull_request.id),
            pr_id: pull_request.id,
            pr_title: pull_request.title.clone(),
            pr_url: pull_request.url.clone(),
            thread_id,
            author: author.to_owned(),
            at,
            kind,
        };
        let mut published = false;
        for event in &pull_request.status_events {
            published |= event.change == PrStatusChange::Published;
            if is_me(event.actor_unique_name.as_deref()) {
                continue;
            }
            let kind = match event.change {
                PrStatusChange::Published => PrUpdateKind::Published,
                PrStatusChange::Completed => PrUpdateKind::Merged,
                PrStatusChange::Abandoned => PrUpdateKind::Abandoned,
            };
            updates.push(update(
                format!("status:{}", event.thread_id),
                event.thread_id,
                &event.actor,
                event.at,
                kind,
            ));
        }
        let own = pull_request.is_own_author || is_me(pull_request.author_unique_name.as_deref());
        // A PR opened ready for review has no draft thread: it was published when made.
        if !published && !pull_request.is_draft && !own && pull_request.threads.is_some() {
            updates.push(update(
                "published".to_owned(),
                0,
                &pull_request.author,
                pull_request.created_at,
                PrUpdateKind::Published,
            ));
        }
        // Threads that failed to load leave the closing time without who did it.
        let closed_kind = match pull_request.status.as_str() {
            "completed" => Some(PrUpdateKind::Merged),
            "abandoned" => Some(PrUpdateKind::Abandoned),
            _ => None,
        };
        if let (Some(kind), Some(closed_at)) = (closed_kind, pull_request.closed_at)
            && pull_request.threads.is_none()
        {
            updates.push(update("closed".to_owned(), 0, "", closed_at, kind));
        }
        if !involved {
            continue;
        }
        for thread in threads {
            for comment in &thread.comments {
                if is_me(comment.author_unique_name.as_deref()) {
                    continue;
                }
                updates.push(update(
                    format!("comment:{}:{}", thread.id, comment.id),
                    thread.id,
                    &comment.author,
                    comment.updated_at,
                    PrUpdateKind::Comment {
                        text: comment.content.clone(),
                        file_path: thread.file_path.clone(),
                        line: thread.line,
                    },
                ));
            }
        }
        for vote in &pull_request.votes {
            // A reset vote takes a vote back; it is not news.
            if vote.vote == 0 || is_me(vote.voter_unique_name.as_deref()) {
                continue;
            }
            updates.push(update(
                format!("vote:{}", vote.thread_id),
                vote.thread_id,
                &vote.voter,
                vote.at,
                PrUpdateKind::Vote { vote: vote.vote },
            ));
        }
    }
    updates.sort_by(|left, right| {
        right
            .at
            .cmp(&left.at)
            .then_with(|| right.pr_id.cmp(&left.pr_id))
            .then_with(|| right.thread_id.cmp(&left.thread_id))
    });
    updates
}

#[cfg(test)]
mod tests {
    use super::{PrUpdateKind, pr_updates};
    use crate::azure::{AzurePr, PrThread, PrThreadComment, PrVote, normalize_votes};
    use serde_json::json;

    const ME: &str = "me@example.com";

    fn pr(id: u64, author: &str) -> AzurePr {
        serde_json::from_value(json!({
            "id": id, "title": format!("PR {id}"), "author": author,
            "authorUniqueName": author, "isOwnAuthor": false, "isDraft": false,
            "sourceBranch": "feature", "targetBranch": "main", "createdAt": 0,
            "updatedAt": 0, "latestReviewAt": 0, "mergeStatus": "succeeded",
            "reviewerVotes": [], "unresolvedThreads": 0,
            "url": format!("https://dev.azure.com/org/p/_git/r/pullrequest/{id}?_a=files")
        }))
        .unwrap()
    }

    fn comment(id: u64, author: &str, at: u64) -> PrThreadComment {
        PrThreadComment {
            id,
            author: author.to_uppercase(),
            author_unique_name: Some(author.to_owned()),
            content: format!("comment {id}"),
            updated_at: at,
        }
    }

    fn thread(id: u64, comments: Vec<PrThreadComment>) -> PrThread {
        PrThread {
            id,
            status: "fixed".into(),
            file_path: None,
            line: None,
            comments,
        }
    }

    fn vote(thread_id: u64, voter: &str, vote: i64, at: u64) -> PrVote {
        PrVote {
            thread_id,
            voter: voter.to_uppercase(),
            voter_unique_name: Some(voter.to_owned()),
            vote,
            at,
        }
    }

    #[test]
    fn lists_others_comments_and_votes_on_my_prs_newest_first() {
        let mut mine = pr(1, ME);
        mine.threads = Some(vec![thread(
            10,
            vec![comment(1, "bob@example.com", 100), comment(2, ME, 300)],
        )]);
        mine.votes = vec![
            vote(11, "bob@example.com", 10, 200),
            vote(12, "carol@example.com", 0, 400),
        ];
        let updates = pr_updates([&mine], Some("ME@example.com"));
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0].kind, PrUpdateKind::Vote { vote: 10 });
        assert_eq!(updates[0].author, "BOB@EXAMPLE.COM");
        assert_eq!(updates[1].at, 100);
        assert_eq!(updates[0].id, "1:vote:11");
        assert_eq!(updates[1].id, "1:comment:10:1");
        assert_eq!(
            updates[0].link(),
            "https://dev.azure.com/org/p/_git/r/pullrequest/1?_a=overview&discussionId=11"
        );
    }

    #[test]
    fn counts_prs_i_review_or_took_part_in_and_skips_the_rest() {
        let mut reviewing = pr(1, "bob@example.com");
        reviewing.reviewers = vec![ME.to_owned()];
        reviewing.threads = Some(vec![thread(1, vec![comment(1, "bob@example.com", 1)])]);
        let mut commented = pr(2, "bob@example.com");
        commented.threads = Some(vec![thread(
            2,
            vec![comment(2, ME, 1), comment(3, "bob@example.com", 2)],
        )]);
        let mut other = pr(3, "bob@example.com");
        other.threads = Some(vec![thread(3, vec![comment(4, "carol@example.com", 3)])]);
        let comments_on = |updates: Vec<super::PrUpdate>| {
            updates
                .into_iter()
                .filter(|update| matches!(update.kind, PrUpdateKind::Comment { .. }))
                .map(|update| update.pr_id)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            comments_on(pr_updates([&reviewing, &commented, &other], Some(ME))),
            vec![2, 1]
        );
        assert_eq!(comments_on(pr_updates([&other], None)), vec![3]);
    }

    #[test]
    fn lists_publishing_merging_and_abandoning_by_others_on_any_pr() {
        use crate::azure::{PrStatusChange, PrStatusEvent};
        let event = |thread_id, change, actor: &str, at| PrStatusEvent {
            thread_id,
            change,
            actor: actor.to_uppercase(),
            actor_unique_name: Some(actor.to_owned()),
            at,
        };
        // Opened ready for review: published when made.
        let mut opened = pr(1, "bob@example.com");
        opened.created_at = 10;
        opened.threads = Some(Vec::new());
        // Published from draft, then merged by its author.
        let mut merged = pr(2, "bob@example.com");
        merged.status = "completed".into();
        merged.threads = Some(Vec::new());
        merged.status_events = vec![
            event(5, PrStatusChange::Published, "bob@example.com", 20),
            event(6, PrStatusChange::Completed, "bob@example.com", 30),
        ];
        // Mine, which I abandoned: nothing to tell me.
        let mut mine = pr(3, ME);
        mine.is_own_author = true;
        mine.status = "abandoned".into();
        mine.threads = Some(Vec::new());
        mine.status_events = vec![event(7, PrStatusChange::Abandoned, ME, 40)];
        // Abandoned, but its threads did not load: the closing time still counts.
        let mut unknown = pr(4, "carol@example.com");
        unknown.is_draft = true;
        unknown.status = "abandoned".into();
        unknown.closed_at = Some(50);
        let updates = pr_updates([&opened, &merged, &mine, &unknown], Some(ME));
        let summary = updates
            .iter()
            .map(|update| (update.id.as_str(), update.kind.clone(), update.at))
            .collect::<Vec<_>>();
        assert_eq!(
            summary,
            vec![
                ("4:closed", PrUpdateKind::Abandoned, 50),
                ("2:status:6", PrUpdateKind::Merged, 30),
                ("2:status:5", PrUpdateKind::Published, 20),
                ("1:published", PrUpdateKind::Published, 10),
            ]
        );
        assert_eq!(updates[0].author, "");
        assert!(updates[1].link().ends_with("/pullrequest/2?_a=overview"));
    }

    #[test]
    fn status_events_come_from_draft_and_status_threads() {
        let events = crate::azure::normalize_status_events(&json!({"value": [
            {"id": 1, "properties": {"CodeReviewThreadType": {"$value": "IsDraftUpdate"}, "CodeReviewIsDraftNowSet": {"$value": "0"}},
             "comments": [{"publishedDate": "2026-11-20T15:46:40.637Z", "author": {"displayName": "Bob", "uniqueName": "bob@example.com"}}]},
            {"id": 2, "properties": {"CodeReviewThreadType": {"$value": "IsDraftUpdate"}, "CodeReviewIsDraftNowSet": {"$value": "1"}},
             "comments": [{"publishedDate": "2026-11-21T15:46:40.637Z"}]},
            {"id": 3, "properties": {"CodeReviewThreadType": {"$value": "StatusUpdate"}, "CodeReviewStatus": {"$value": "Completed"}},
             "comments": [{"publishedDate": "2026-11-26T13:37:27.823Z", "author": {"displayName": "Dino"}}]},
            {"id": 4, "properties": {"CodeReviewThreadType": {"$value": "StatusUpdate"}, "CodeReviewStatus": {"$value": "Abandoned"}},
             "comments": [{"publishedDate": "2026-11-26T09:19:56.44Z"}]},
            {"id": 5, "properties": {"CodeReviewThreadType": {"$value": "StatusUpdate"}, "CodeReviewStatus": {"$value": "Active"}},
             "comments": [{"publishedDate": "2026-11-26T09:19:56.44Z"}]}
        ]}));
        let changes = events
            .iter()
            .map(|event| (event.thread_id, event.change))
            .collect::<Vec<_>>();
        use crate::azure::PrStatusChange::{Abandoned, Completed, Published};
        assert_eq!(
            changes,
            vec![(1, Published), (3, Completed), (4, Abandoned)]
        );
        assert_eq!(events[0].actor, "Bob");
        assert_eq!(events[1].actor, "Dino");
        assert_eq!(events[2].actor, "Unknown");
    }

    #[test]
    fn votes_come_from_vote_update_threads() {
        let votes = normalize_votes(&json!({"value": [
            {"id": 5, "properties": {"CodeReviewThreadType": {"$value": "VoteUpdate"}, "CodeReviewVoteResult": {"$value": "10"}},
             "comments": [{"commentType": "system", "content": "Bob voted 10", "publishedDate": "2026-11-13T09:16:47.5Z",
                           "author": {"displayName": "Bob", "uniqueName": "bob@example.com"}}]},
            {"id": 6, "properties": {"CodeReviewThreadType": {"$value": "ReviewersUpdate"}},
             "comments": [{"commentType": "system", "content": "Bob joined", "publishedDate": "2026-11-13T09:16:47.5Z"}]},
            {"id": 7, "isDeleted": true, "properties": {"CodeReviewThreadType": {"$value": "VoteUpdate"}, "CodeReviewVoteResult": {"$value": "-5"}},
             "comments": [{"publishedDate": "2026-11-13T09:16:47.5Z"}]}
        ]}));
        assert_eq!(votes.len(), 1);
        assert_eq!(votes[0].thread_id, 5);
        assert_eq!(votes[0].vote, 10);
        assert_eq!(votes[0].voter, "Bob");
        assert_eq!(
            votes[0].voter_unique_name.as_deref(),
            Some("bob@example.com")
        );
        assert!(votes[0].at > 0);
    }
}
