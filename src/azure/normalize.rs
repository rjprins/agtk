//! Turns az CLI JSON into agtk's pull request types.

use serde_json::{Map, Value};

use super::remote::percent_encode;
use super::{AzurePr, AzureRepoRef, LinkedPbi, PrThread, PrThreadComment};

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
        .filter(|reviewer| {
            let name = reviewer.get("uniqueName").and_then(nonempty);
            !name
                .zip(author_unique_name.as_deref())
                .is_some_and(|(name, author)| name.eq_ignore_ascii_case(author))
        })
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
        threads: None,
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

/// Threads with at least one comment a person wrote, in the order Azure
/// DevOps sent them. Deleted threads and comments and system events are left out.
pub fn normalize_threads(value: &Value) -> Vec<PrThread> {
    let threads = value
        .get("value")
        .and_then(Value::as_array)
        .or_else(|| value.as_array());
    threads
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter(|thread| thread.get("isDeleted").and_then(Value::as_bool) != Some(true))
        .filter_map(|thread| {
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
                .filter_map(|comment| {
                    let content = comment.get("content").and_then(nonempty)?.to_owned();
                    let updated_at = ["lastContentUpdatedDate", "lastUpdatedDate", "publishedDate"]
                        .into_iter()
                        .find_map(|key| {
                            comment
                                .get(key)
                                .and_then(nonempty)
                                .and_then(parse_timestamp)
                        })
                        .unwrap_or(0);
                    let author = comment.get("author").and_then(Value::as_object);
                    let author = ["displayName", "uniqueName"]
                        .into_iter()
                        .find_map(|key| {
                            author.and_then(|author| author.get(key)).and_then(nonempty)
                        })
                        .unwrap_or("Unknown")
                        .to_owned();
                    Some(PrThreadComment {
                        id: comment.get("id").and_then(Value::as_u64).unwrap_or(0),
                        author,
                        content,
                        updated_at,
                    })
                })
                .collect::<Vec<_>>();
            if comments.is_empty() {
                return None;
            }
            let context = thread.get("threadContext").and_then(Value::as_object);
            let file_path = context
                .and_then(|context| context.get("filePath"))
                .and_then(nonempty)
                .map(str::to_owned);
            let line = ["rightFileStart", "leftFileStart"]
                .into_iter()
                .find_map(|key| {
                    context?
                        .get(key)?
                        .get("line")?
                        .as_u64()
                        .filter(|line| *line > 0)
                });
            Some(PrThread {
                id: thread.get("id").and_then(Value::as_u64).unwrap_or(0),
                status: thread
                    .get("status")
                    .and_then(nonempty)
                    .unwrap_or("unknown")
                    .to_ascii_lowercase(),
                file_path,
                line,
                comments,
            })
        })
        .collect()
}

#[cfg(test)]
pub(super) fn normalize_thread_summary(value: &Value) -> ThreadSummary {
    summarize_threads(&normalize_threads(value))
}

pub(super) fn summarize_threads(threads: &[PrThread]) -> ThreadSummary {
    let mut summary = ThreadSummary::default();
    for thread in threads {
        summary.total_threads += 1;
        if thread.is_unresolved() {
            summary.unresolved_threads += 1;
        } else if thread.is_resolved() {
            summary.resolved_threads += 1;
        }
        for comment in &thread.comments {
            summary.latest_review_at = summary.latest_review_at.max(comment.updated_at);
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
