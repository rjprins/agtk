//! Read account quota with the agent's existing sign-in. No token refresh or inference.

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use crate::providers::{AgentProvider, DiscoveryRoots};
use serde_json::Value;

const MAX_RESPONSE: u64 = 256 * 1024;

#[derive(Debug, Clone)]
pub struct QuotaWindow {
    pub label: String,
    pub remaining: f64,
    pub resets_at: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct AccountQuota {
    pub provider: AgentProvider,
    pub windows: Vec<QuotaWindow>,
    pub status: String,
}

impl AccountQuota {
    pub fn unavailable(provider: AgentProvider, status: &str) -> Self {
        Self {
            provider,
            windows: Vec::new(),
            status: status.to_owned(),
        }
    }
}

/// Auth is read afresh each time so switching accounts doesn't leave an old token cached.
pub fn fetch(provider: AgentProvider, roots: &DiscoveryRoots) -> AccountQuota {
    let (path, pointer, url) = match provider {
        AgentProvider::Codex => (
            roots.codex_home_dir.join("auth.json"),
            "/tokens/access_token",
            "https://chatgpt.com/backend-api/wham/usage",
        ),
        AgentProvider::Claude => (
            roots.claude_config_dir.join(".credentials.json"),
            "/claudeAiOauth/accessToken",
            "https://api.anthropic.com/api/oauth/usage",
        ),
    };
    let Some(auth) = read_json(&path) else {
        return AccountQuota::unavailable(provider, "Not signed in");
    };
    let Some(token) = auth
        .pointer(pointer)
        .and_then(Value::as_str)
        .filter(|value| valid_header(value))
    else {
        return AccountQuota::unavailable(provider, "No subscription sign-in");
    };
    let mut headers = format!("Authorization: Bearer {token}\n");
    match provider {
        AgentProvider::Claude => {
            headers.push_str("anthropic-beta: oauth-2025-04-20\nUser-Agent: claude-code/2.1.0\n")
        }
        AgentProvider::Codex => {
            headers.push_str(
                "User-Agent: codex-cli\nOpenAI-Beta: codex-1\noriginator: Codex Desktop\n",
            );
            if let Some(id) = auth
                .pointer("/tokens/account_id")
                .and_then(Value::as_str)
                .filter(|value| valid_header(value))
            {
                headers.push_str(&format!("ChatGPT-Account-Id: {id}\n"));
            }
        }
    }
    match request(url, &headers) {
        Ok(value) => parse(provider, &value),
        Err(status) => AccountQuota::unavailable(provider, status),
    }
}

fn read_json(path: &Path) -> Option<Value> {
    let mut bytes = Vec::new();
    File::open(path)
        .ok()?
        .take(MAX_RESPONSE + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_RESPONSE {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

fn valid_header(value: &str) -> bool {
    !value.is_empty() && value.len() < 16_384 && value.bytes().all(|byte| byte.is_ascii_graphic())
}

fn request(url: &str, headers: &str) -> Result<Value, &'static str> {
    // Disable curlrc and redirects. Auth travels through stdin, never argv or a file.
    // Curl bounds both transfer time and size; take() also bounds older curl versions.
    let mut child = Command::new("curl")
        .args([
            "--disable",
            "--silent",
            "--fail",
            "--proto",
            "=https",
            "--max-time",
            "10",
            "--max-filesize",
            "262144",
            "--header",
            "@-",
            url,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "curl unavailable")?;
    let sent = child
        .stdin
        .take()
        .is_some_and(|mut stdin| stdin.write_all(headers.as_bytes()).is_ok());
    if !sent {
        let _ = child.kill();
        let _ = child.wait();
        return Err("Usage unavailable");
    }
    let mut bytes = Vec::new();
    let read = child
        .stdout
        .take()
        .unwrap()
        .take(MAX_RESPONSE + 1)
        .read_to_end(&mut bytes);
    if read.is_err() || bytes.len() as u64 > MAX_RESPONSE {
        let _ = child.kill();
        let _ = child.wait();
        return Err("Usage unavailable");
    }
    let status = child.wait().map_err(|_| "Usage unavailable")?;
    if !status.success() {
        return Err("Usage unavailable; check provider sign-in");
    }
    serde_json::from_slice(&bytes).map_err(|_| "Usage unavailable")
}

pub fn parse(provider: AgentProvider, value: &Value) -> AccountQuota {
    let mut windows = Vec::new();
    let mut add = |label: &str, used: &Value, reset: &Value| {
        if let Some(used) = used
            .as_f64()
            .filter(|used| used.is_finite() && *used >= 0.0)
        {
            let resets_at = reset.as_u64().or_else(|| {
                reset
                    .as_str()
                    .and_then(|text| glib::DateTime::from_iso8601(text, None).ok())
                    .and_then(|time| u64::try_from(time.to_unix()).ok())
            });
            windows.push(QuotaWindow {
                label: label.to_owned(),
                remaining: (100.0 - used).clamp(0.0, 100.0),
                resets_at,
            });
        }
    };
    match provider {
        AgentProvider::Claude => {
            for (key, label) in [("five_hour", "5h"), ("seven_day", "7d")] {
                let used = value[key]
                    .get("utilization")
                    .or_else(|| value[key].get("used_percentage"))
                    .unwrap_or(&Value::Null);
                add(label, used, &value[key]["resets_at"]);
            }
        }
        AgentProvider::Codex => {
            for key in ["primary_window", "secondary_window"] {
                let window = &value["rate_limit"][key];
                let label = match window["limit_window_seconds"].as_u64() {
                    Some(18_000) => "5h".to_owned(),
                    Some(604_800) => "7d".to_owned(),
                    Some(seconds) if seconds > 0 => format!("{}m", seconds.div_ceil(60)),
                    _ => "Limit".to_owned(),
                };
                add(&label, &window["used_percent"], &window["reset_at"]);
            }
        }
    }
    let status = if windows.is_empty() {
        "No quota data"
    } else {
        "Live account quota"
    };
    AccountQuota {
        provider,
        windows,
        status: status.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn quota_keeps_windows_distinct_and_inverts_usage() {
        let quota = parse(
            AgentProvider::Codex,
            &json!({"rate_limit":{
            "primary_window":{"used_percent":28,"limit_window_seconds":18000,"reset_at":1800000000},
            "secondary_window":{"used_percent":54,"limit_window_seconds":604800}}}),
        );
        assert_eq!(quota.windows[0].label, "5h");
        assert_eq!(quota.windows[0].remaining, 72.0);
        assert_eq!(quota.windows[1].label, "7d");
        assert_eq!(quota.windows[1].remaining, 46.0);
        assert_eq!(quota.windows[1].resets_at, None);
    }

    #[test]
    fn missing_or_invalid_data_is_never_zero_usage() {
        let quota = parse(
            AgentProvider::Claude,
            &json!({"five_hour":{"utilization":-1},"seven_day":null}),
        );
        assert!(quota.windows.is_empty());
        let quota = parse(
            AgentProvider::Claude,
            &json!({"five_hour":{"utilization":110,"resets_at":"2026-09-29T14:46:47.853Z"}}),
        );
        assert_eq!(quota.windows[0].remaining, 0.0);
        assert_eq!(quota.windows[0].resets_at, Some(1790693207));
        assert!(!valid_header("token\nX-Injected: yes"));
        assert!(!valid_header(""));
    }
}
