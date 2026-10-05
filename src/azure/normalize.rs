//! Turns az CLI JSON into agtk's pull request types.

use serde_json::{Map, Value};

use super::remote::percent_encode;
use super::{AzurePr, AzureRepoRef, LinkedPbi};

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
        if unresolved && !comments.is_empty() {
            summary.unresolved_threads += 1;
        }
        for comment in comments {
            for key in ["lastUpdatedDate", "publishedDate"] {
                if let Some(timestamp) = comment
                    .get(key)
                    .and_then(nonempty)
                    .and_then(parse_timestamp)
                {
                    summary.latest_review_at = summary.latest_review_at.max(timestamp);
                    break;
                }
            }
        }
    }
    summary
}

#[derive(Debug, Default)]
pub(super) struct ThreadSummary {
    pub(super) unresolved_threads: u32,
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
    let bytes = value.as_bytes();
    if bytes.len() < 20
        || bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || bytes.get(10) != Some(&b'T')
        || bytes.get(13) != Some(&b':')
        || bytes.get(16) != Some(&b':')
        || !(value.ends_with('Z') || value.ends_with("+00:00"))
    {
        return None;
    }
    let year = parse_digits(bytes, 0, 4)? as i64;
    let month = parse_digits(bytes, 5, 7)? as i64;
    let day = parse_digits(bytes, 8, 10)? as i64;
    let hour = parse_digits(bytes, 11, 13)? as i64;
    let minute = parse_digits(bytes, 14, 16)? as i64;
    let second = parse_digits(bytes, 17, 19)? as i64;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let fraction_end = value.find('Z').or_else(|| value.find('+')).unwrap_or(19);
    let millis = value
        .get(19..fraction_end)
        .and_then(|fraction| fraction.strip_prefix('.'))
        .and_then(|fraction| {
            let digits = fraction.chars().take(3).collect::<String>();
            let padded = format!("{digits:0<3}");
            padded.parse::<i64>().ok()
        })
        .unwrap_or(0);
    let days = days_from_civil(year, month, day);
    let seconds = days
        .checked_mul(86_400)?
        .checked_add(hour * 3_600 + minute * 60 + second)?;
    u64::try_from(seconds.checked_mul(1_000)?.checked_add(millis)?).ok()
}

fn parse_digits(bytes: &[u8], start: usize, end: usize) -> Option<u32> {
    bytes
        .get(start..end)?
        .iter()
        .try_fold(0_u32, |value, byte| {
            byte.is_ascii_digit()
                .then(|| value * 10 + u32::from(byte - b'0'))
        })
}

fn days_from_civil(mut year: i64, month: i64, day: i64) -> i64 {
    year -= i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let adjusted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * adjusted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}
