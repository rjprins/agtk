use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::command_runner::run_bounded;

pub type AzureResult<T> = Result<T, Box<dyn Error + Send + Sync>>;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(20);
const OUTPUT_LIMIT: usize = 8 * 1024 * 1024;
const PR_LIMIT: &str = "100";
const DETAIL_CONCURRENCY: usize = 4;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PrAttention {
    New,
    Published,
    Review,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnownPr {
    pub is_draft: bool,
    pub unresolved_threads: u32,
    pub latest_review_at: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrPreferences {
    #[serde(default)]
    pub projects: BTreeMap<String, PrProjectState>,
}

impl PrPreferences {
    pub fn project(&self, root: &str) -> PrProjectState {
        self.projects.get(root).cloned().unwrap_or_default()
    }

    pub fn set_project(&mut self, root: String, state: PrProjectState) {
        self.projects.insert(root, state);
    }
}

#[derive(Debug, Clone)]
pub struct AzureClient {
    command: OsString,
}

impl AzureClient {
    pub fn from_environment() -> Self {
        Self::new(
            std::env::var_os("AGMUX_AZURE_BIN")
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| OsString::from("az")),
        )
    }

    pub fn new(command: impl Into<OsString>) -> Self {
        Self {
            command: command.into(),
        }
    }

    pub fn repository(&self, project_root: &Path) -> AzureResult<Option<AzureRepoRef>> {
        let root = project_root.canonicalize()?;
        let remote = run_command(
            "git",
            [
                OsString::from("-C"),
                root.as_os_str().to_owned(),
                OsString::from("remote"),
                OsString::from("get-url"),
                OsString::from("origin"),
            ],
        );
        let Ok(remote) = remote else {
            return Ok(None);
        };
        Ok(parse_azure_remote(remote.trim()))
    }

    pub fn list_active(&self, project_root: &Path) -> AzureResult<Option<AzurePrList>> {
        let Some(reference) = self.repository(project_root)? else {
            return Ok(None);
        };
        let current_user = self
            .az_text([
                "account",
                "show",
                "--query",
                "user.name",
                "--only-show-errors",
                "-o",
                "tsv",
            ])
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        let raw = self.az_json([
            "repos",
            "pr",
            "list",
            "--org",
            &reference.org_url,
            "--project",
            &reference.project,
            "--repository",
            &reference.repository,
            "--status",
            "active",
            "--top",
            PR_LIMIT,
            "--only-show-errors",
            "-o",
            "json",
        ])?;
        let mut pull_requests = normalize_active_prs(&reference, raw);
        if let Some(user) = current_user.as_deref() {
            for pull_request in &mut pull_requests {
                pull_request.is_own_author = pull_request
                    .author_unique_name
                    .as_deref()
                    .is_some_and(|author| author.eq_ignore_ascii_case(user));
            }
        }
        self.add_pr_details(&reference, &mut pull_requests);
        pull_requests.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| right.id.cmp(&left.id))
        });
        Ok(Some(AzurePrList {
            repository: reference,
            current_user,
            pull_requests,
        }))
    }

    fn add_pr_details(&self, reference: &AzureRepoRef, pull_requests: &mut [AzurePr]) {
        for chunk in pull_requests.chunks_mut(DETAIL_CONCURRENCY) {
            let results = thread::scope(|scope| {
                chunk
                    .iter()
                    .map(|pull_request| {
                        let client = self.clone();
                        let reference = reference.clone();
                        let id = pull_request.id;
                        scope.spawn(move || {
                            (
                                client.thread_summary(&reference, id).ok(),
                                client.linked_pbis(&reference, id).ok(),
                            )
                        })
                    })
                    .collect::<Vec<_>>()
                    .into_iter()
                    .map(|handle| handle.join().ok())
                    .collect::<Vec<_>>()
            });
            for (pull_request, details) in chunk.iter_mut().zip(results) {
                let Some((summary, linked_pbis)) = details else {
                    continue;
                };
                if let Some(summary) = summary {
                    pull_request.unresolved_threads = summary.unresolved_threads;
                    pull_request.latest_review_at = summary.latest_review_at;
                    pull_request.updated_at = pull_request.updated_at.max(summary.latest_review_at);
                }
                if let Some(linked_pbis) = linked_pbis {
                    pull_request.linked_pbis = linked_pbis;
                }
            }
        }
    }

    fn linked_pbis(&self, reference: &AzureRepoRef, id: u64) -> AzureResult<Vec<LinkedPbi>> {
        let id = id.to_string();
        let raw = self.az_json([
            "repos",
            "pr",
            "work-item",
            "list",
            "--id",
            &id,
            "--org",
            &reference.org_url,
            "--detect",
            "false",
            "--only-show-errors",
            "-o",
            "json",
        ])?;
        Ok(normalize_linked_pbis(reference, &raw))
    }

    fn thread_summary(&self, reference: &AzureRepoRef, id: u64) -> AzureResult<ThreadSummary> {
        let project = format!("project={}", reference.project);
        let repository = format!("repositoryId={}", reference.repository);
        let pull_request = format!("pullRequestId={id}");
        let raw = self.az_json([
            "devops",
            "invoke",
            "--org",
            &reference.org_url,
            "--area",
            "git",
            "--resource",
            "pullRequestThreads",
            "--route-parameters",
            &project,
            &repository,
            &pull_request,
            "--api-version",
            "7.1",
            "--only-show-errors",
            "-o",
            "json",
        ])?;
        Ok(normalize_thread_summary(&raw))
    }

    fn az_json<'a>(&self, args: impl IntoIterator<Item = &'a str>) -> AzureResult<Value> {
        let text = self.az_text(args)?;
        Ok(serde_json::from_str(&text)?)
    }

    fn az_text<'a>(&self, args: impl IntoIterator<Item = &'a str>) -> AzureResult<String> {
        run_command(&self.command, args.into_iter().map(OsString::from))
    }
}

pub fn parse_azure_remote(remote: &str) -> Option<AzureRepoRef> {
    let remote = remote.trim();
    if let Some(path) = remote.strip_prefix("git@ssh.dev.azure.com:v3/") {
        let mut parts = path.trim_end_matches(".git").split('/');
        let org = parts.next()?;
        let project = parts.next()?;
        let repository = parts.next()?;
        if parts.next().is_none()
            && [org, project, repository]
                .iter()
                .all(|part| !part.is_empty())
        {
            return Some(AzureRepoRef {
                org_url: format!("https://dev.azure.com/{org}"),
                project: project.to_owned(),
                repository: repository.to_owned(),
            });
        }
        return None;
    }
    let without_scheme = remote
        .strip_prefix("https://")
        .or_else(|| remote.strip_prefix("http://"))?;
    let (authority, path) = without_scheme.split_once('/')?;
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let parts = path.trim_end_matches('/').split('/').collect::<Vec<_>>();
    if host.eq_ignore_ascii_case("dev.azure.com") && parts.len() == 4 && parts[2] == "_git" {
        return Some(AzureRepoRef {
            org_url: format!("https://dev.azure.com/{}", percent_decode(parts[0])?),
            project: percent_decode(parts[1])?,
            repository: strip_git(percent_decode(parts[3])?),
        });
    }
    let org = host.strip_suffix(".visualstudio.com")?;
    if parts.len() == 3 && parts[1] == "_git" && !org.is_empty() {
        return Some(AzureRepoRef {
            org_url: format!("https://dev.azure.com/{org}"),
            project: percent_decode(parts[0])?,
            repository: strip_git(percent_decode(parts[2])?),
        });
    }
    None
}

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

fn normalize_linked_pbis(reference: &AzureRepoRef, value: &Value) -> Vec<LinkedPbi> {
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

fn normalize_thread_summary(value: &Value) -> ThreadSummary {
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
struct ThreadSummary {
    unresolved_threads: u32,
    latest_review_at: u64,
}

fn run_command(
    program: impl AsRef<OsStr>,
    args: impl IntoIterator<Item = OsString>,
) -> AzureResult<String> {
    let mut command = Command::new(program);
    command.args(args);
    let output = run_bounded(command, COMMAND_TIMEOUT, OUTPUT_LIMIT)?;
    if !output.status.success() {
        return Err(format!(
            "command exited with {}: {}",
            output.status,
            output.stderr.trim()
        )
        .into());
    }
    Ok(output.stdout)
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

fn strip_git(mut value: String) -> String {
    if value.ends_with(".git") {
        value.truncate(value.len() - 4);
    }
    value
}

fn percent_decode(value: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(value.len());
    let mut input = value.as_bytes().iter().copied();
    while let Some(byte) = input.next() {
        if byte == b'%' {
            let high = hex(input.next()?)?;
            let low = hex(input.next()?)?;
            bytes.push(high * 16 + low);
        } else {
            bytes.push(byte);
        }
    }
    String::from_utf8(bytes).ok()
}

fn percent_encode(value: &str) -> String {
    let mut output = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            output.push(char::from(byte));
        } else {
            use std::fmt::Write;
            let _ = write!(output, "%{byte:02X}");
        }
    }
    output
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
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

#[cfg(test)]
mod tests {
    use super::{normalize_linked_pbis, normalize_thread_summary, parse_azure_remote};
    use serde_json::json;

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
