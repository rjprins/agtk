//! The PR updates feed: what other people said and voted on the PRs the user
//! wrote or reviews, newest first.

use serde::Serialize;

use super::AzurePr;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum PrUpdateKind {
    Comment {
        text: String,
        file_path: Option<String>,
        line: Option<u64>,
    },
    /// 10 approved, 5 approved with suggestions, -5 waiting for author, -10 rejected.
    Vote { vote: i64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrUpdate {
    pub pr_id: u64,
    pub pr_title: String,
    pub pr_url: String,
    /// 0 when Azure DevOps sent none.
    pub thread_id: u64,
    pub author: String,
    /// Unix milliseconds.
    pub at: u64,
    pub kind: PrUpdateKind,
}

impl PrUpdate {
    /// The PR page opened at the update's thread, when it has one.
    #[must_use]
    pub fn link(&self) -> String {
        if self.thread_id == 0 {
            return self.pr_url.clone();
        }
        let base = self.pr_url.split('?').next().unwrap_or(&self.pr_url);
        format!("{base}?_a=overview&discussionId={}", self.thread_id)
    }
}

/// Comments and votes by other people on the PRs `me` wrote, is a reviewer on,
/// or commented or voted on, newest first. With `me` unknown every PR counts
/// and nothing is left out as the user's own.
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
        if !involved {
            continue;
        }
        let update = |thread_id, author: &str, at, kind| PrUpdate {
            pr_id: pull_request.id,
            pr_title: pull_request.title.clone(),
            pr_url: pull_request.url.clone(),
            thread_id,
            author: author.to_owned(),
            at,
            kind,
        };
        for thread in threads {
            for comment in &thread.comments {
                if is_me(comment.author_unique_name.as_deref()) {
                    continue;
                }
                updates.push(update(
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
        let ids = pr_updates([&reviewing, &commented, &other], Some(ME))
            .into_iter()
            .map(|update| update.pr_id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec![2, 1]);
        assert_eq!(pr_updates([&other], None).len(), 1);
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
