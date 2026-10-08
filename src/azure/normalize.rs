//! Turns az CLI JSON into agtk's pull request types.

use serde_json::{Map, Value};

use super::remote::percent_encode;
use super::{AzurePr, AzureRepoRef, LinkedPbi, PrComment};

pub fn normalize_active_prs(reference: &AzureRepoRef, value: Value) -> Vec<AzurePr> {
    let Some(candidates) = value.as_array() else {
        return Vec::new();
    };
    candidates
        .iter()
        .filter_map(|candidate| normalize_active_pr(reference, candidate.as_object()?))
        .collect()
}

fn normalize_active_pr(reference: &AzureRepoRef, raw: &Map<String, Value>) -> Option<AzurePr> {
    let id = raw.get("pullRequestId")?.as_u64().filter(|id| *id > 0)?;
    let title = nonempty(raw.get("title")?)?;
    let source_branch = branch(raw.get("sourceRefName")?)?;
    let target_branch = branch(raw.get("targetRefName")?)?;
    let created_at = parse_timestamp(nonempty(raw.get("creationDate")?)?)?;
    let is_draft = raw.get("isDraft")?.as_bool()?;
    let created_by = raw.get("createdBy").and_then(Value::as_object);
    let author_unique_name = created_by
        .and_then(|created| created.get("uniqueName"))
        .and_then(nonempty)
        .map(str::to_owned);
    let author = created_by
        .and_then(|created| created.get("displayName"))
        .and_then(nonempty)
        .map(str::to_owned)
        .or_else(|| author_unique_name.clone())
        .unwrap_or_else(|| "Unknown".to_owned());
    let head_sha = raw
        .get("lastMergeSourceCommit")
        .and_then(Value::as_object)
        .and_then(|commit| commit.get("commitId"))
        .and_then(nonempty)
        .map(str::to_owned);
    let merge_status = raw
        .get("mergeStatus")
        .and_then(nonempty)
        .unwrap_or("unknown")
        .to_owned();
    let reviewer_votes = raw
        .get("reviewers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter(|reviewer| reviewer.get("isContainer").and_then(Value::as_bool) != Some(true))
        .filter_map(|reviewer| reviewer.get("vote").and_then(Value::as_i64))
        .filter(|vote| matches!(vote, -10 | -5 | 0 | 5 | 10))
        .collect();
    Some(AzurePr {
        id,
        title: title.to_owned(),
        author,
        author_unique_name,
        is_own_author: false,
        is_draft,
        source_branch,
        target_branch,
        created_at,
        updated_at: created_at,
        latest_review_at: 0,
        head_sha,
        merge_status,
        reviewer_votes,
        unresolved_threads: 0,
        resolved_threads: 0,
        total_threads: None,
        comments: None,
        linked_pbis: Vec::new(),
        url: format!(
            "{}/{}/_git/{}/pullrequest/{id}?_a=files",
            reference.org_url,
            percent_encode(&reference.project),
            percent_encode(&reference.repository)
        ),
    })
}

pub(super) fn normalize_linked_pbis(reference: &AzureRepoRef, value: &Value) -> Vec<LinkedPbi> {
    let mut pbis = value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let fields = item.get("fields")?.as_object()?;
            let kind = fields.get("System.WorkItemType").and_then(nonempty)?;
            if !kind.eq_ignore_ascii_case("Product Backlog Item") {
                return None;
            }
            let id = item.get("id")?.as_u64().filter(|id| *id > 0)?;
            let title = fields
                .get("System.Title")
                .and_then(nonempty)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("PBI #{id}"));
            let project = fields
                .get("System.TeamProject")
                .and_then(nonempty)
                .unwrap_or(&reference.project);
            Some(LinkedPbi {
                id,
                title,
                url: format!(
                    "{}/{}/_workitems/edit/{id}",
                    reference.org_url,
                    percent_encode(project)
                ),
            })
        })
        .collect::<Vec<_>>();
    pbis.sort_by_key(|pbi| pbi.id);
    pbis.dedup_by_key(|pbi| pbi.id);
    pbis
}

pub(super) fn normalize_thread_summary(value: &Value) -> ThreadSummary {
    let threads = value
        .get("value")
        .and_then(Value::as_array)
        .or_else(|| value.as_array());
    let mut summary = ThreadSummary::default();
    for thread in threads.into_iter().flatten().filter_map(Value::as_object) {
        if thread.get("isDeleted").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let status = thread
            .get("status")
            .and_then(nonempty)
            .unwrap_or("unknown")
            .to_ascii_lowercase();
        let unresolved = matches!(status.as_str(), "active" | "pending");
        let comments = thread
            .get("comments")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_object)
            .filter(|comment| comment.get("isDeleted").and_then(Value::as_bool) != Some(true))
            .filter(|comment| {
                comment
                    .get("commentType")
                    .and_then(nonempty)
                    .is_some_and(|kind| kind.eq_ignore_ascii_case("text"))
            })
            .filter(|comment| comment.get("content").and_then(nonempty).is_some())
            .collect::<Vec<_>>();
        if !comments.is_empty() {
            summary.total_threads += 1;
            if unresolved {
                summary.unresolved_threads += 1;
            } else if matches!(status.as_str(), "fixed" | "closed" | "wontfix" | "bydesign") {
                summary.resolved_threads += 1;
            }
        }
        for comment in comments {
            let mut updated_at = 0;
            for key in ["lastContentUpdatedDate", "lastUpdatedDate", "publishedDate"] {
                if let Some(timestamp) = comment
                    .get(key)
                    .and_then(nonempty)
                    .and_then(parse_timestamp)
                {
                    summary.latest_review_at = summary.latest_review_at.max(timestamp);
                    updated_at = timestamp;
                    break;
                }
            }
            if let (Some(thread_id), Some(id)) = (
                thread
                    .get("id")
                    .and_then(Value::as_u64)
                    .filter(|id| *id > 0),
                comment
                    .get("id")
                    .and_then(Value::as_u64)
                    .filter(|id| *id > 0),
            ) {
                summary.comments.push(PrComment {
                    thread_id,
                    id,
                    updated_at,
                    author_unique_name: comment
                        .get("author")
                        .and_then(|author| author.get("uniqueName"))
                        .and_then(nonempty)
                        .map(str::to_owned),
                });
            }
        }
    }
    summary
}

#[derive(Debug, Default)]
pub(super) struct ThreadSummary {
    pub(super) unresolved_threads: u32,
    pub(super) resolved_threads: u32,
    pub(super) total_threads: u32,
    pub(super) latest_review_at: u64,
    pub(super) comments: Vec<PrComment>,
}

fn nonempty(value: &Value) -> Option<&str> {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn branch(value: &Value) -> Option<String> {
    nonempty(value)?
        .strip_prefix("refs/heads/")
        .filter(|branch| !branch.is_empty())
        .map(str::to_owned)
}

pub(super) fn strip_git(mut value: String) -> String {
    if value.ends_with(".git") {
        value.truncate(value.len() - 4);
    }
    value
}

fn parse_timestamp(value: &str) -> Option<u64> {
    crate::timestamps::parse_utc_millis(value.as_bytes())
}
