//! Recognizes Azure DevOps git remotes.

use super::AzureRepoRef;
use super::normalize::strip_git;

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
            // SSH remotes percent-encode spaces just like HTTPS ones do.
            return Some(AzureRepoRef {
                org_url: format!("https://dev.azure.com/{}", percent_decode(org)?),
                project: percent_decode(project)?,
                repository: percent_decode(repository)?,
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

fn percent_decode(value: &str) -> Option<String> {
    String::from_utf8(crate::text::percent_decode(value)?).ok()
}

pub(super) fn percent_encode(value: &str) -> String {
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
