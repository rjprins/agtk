//! Lists pull requests and their details through the az CLI.

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::Duration;

use serde_json::Value;

use super::normalize::{
    ThreadSummary, normalize_active_prs, normalize_linked_pbis, normalize_thread_summary,
};
use super::remote::parse_azure_remote;
use super::{AzurePr, AzurePrList, AzureRepoRef, AzureResult, LinkedPbi};
use crate::command_runner::run_bounded;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(20);
const OUTPUT_LIMIT: usize = 8 * 1024 * 1024;
const PR_LIMIT: &str = "100";
const DETAIL_CONCURRENCY: usize = 4;

#[derive(Debug, Clone)]
pub struct AzureClient {
    command: OsString,
}

impl AzureClient {
    pub fn from_environment() -> Self {
        Self::new(
            std::env::var_os("AGTK_AZURE_BIN")
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
        // The configured URL, not `remote get-url`: an insteadOf rewrite to a
        // mirror or SSH alias must not hide the Azure origin.
        let Some(remote) = crate::git::optional(&root, ["config", "--get", "remote.origin.url"])
        else {
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
                    pull_request.resolved_threads = summary.resolved_threads;
                    pull_request.total_threads = Some(summary.total_threads);
                    pull_request.latest_review_at = summary.latest_review_at;
                    pull_request.comments = Some(summary.comments);
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
        if !raw.is_array() && !raw.get("value").is_some_and(Value::is_array) {
            return Err("Azure PR thread response is not an array".into());
        }
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
