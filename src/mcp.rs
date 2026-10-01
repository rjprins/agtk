use std::fs;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

use crate::control::{
    CloseSessionParams, ControlClient, ControlCommand, ControlRequest, CreateSessionParams,
    PROTOCOL_VERSION, ResponseBody, SessionIdParams, UiOpenDiffParams, UiOpenFileParams,
    WorktreeListParams,
};
use crate::providers::AgentProvider;

pub const MODERN_MCP_VERSION: &str = "2026-07-28";
pub const LEGACY_MCP_VERSION: &str = "2025-11-25";
const SERVER_NAME: &str = "agtk";
const MAX_MCP_MESSAGE_BYTES: usize = 1024 * 1024;
const MAX_CAPTURE_BYTES: u64 = 32 * 1024 * 1024;
const SUPPORTED_VERSIONS: [&str; 5] = [
    MODERN_MCP_VERSION,
    LEGACY_MCP_VERSION,
    "2025-06-18",
    "2025-03-26",
    "2024-11-05",
];

pub trait ControlBackend {
    fn call(&self, command: ControlCommand) -> Result<Value, String>;
}

#[derive(Debug)]
pub struct SocketControlBackend {
    client: ControlClient,
    sequence: AtomicU64,
}

impl SocketControlBackend {
    pub fn new(client: ControlClient) -> Self {
        Self {
            client,
            sequence: AtomicU64::new(0),
        }
    }
}

impl ControlBackend for SocketControlBackend {
    fn call(&self, command: ControlCommand) -> Result<Value, String> {
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let response = self
            .client
            .send(&ControlRequest {
                version: PROTOCOL_VERSION,
                id: format!("mcp-{}-{sequence}", std::process::id()),
                command,
            })
            .map_err(|error| error.to_string())?;
        match response.body {
            ResponseBody::Success(value) => Ok(value),
            ResponseBody::Failure(error) => {
                let mut message = format!("{}: {}", error_code_name(error.code), error.message);
                if let Some(details) = error.details {
                    message.push_str(": ");
                    message.push_str(&details.to_string());
                }
                Err(message)
            }
        }
    }
}

pub struct McpServer<B> {
    backend: B,
}

impl<B: ControlBackend> McpServer<B> {
    pub fn new(backend: B) -> Self {
        Self { backend }
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn handle(&mut self, message: Value) -> Option<Value> {
        let Some(object) = message.as_object() else {
            return Some(rpc_error(Value::Null, -32600, "Invalid Request", None));
        };
        let id = object.get("id").cloned();
        let is_notification = id.is_none();
        if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
            || object.get("method").and_then(Value::as_str).is_none()
            || id
                .as_ref()
                .is_some_and(|id| !id.is_string() && !id.is_i64() && !id.is_u64())
        {
            return (!is_notification)
                .then(|| rpc_error(id.unwrap_or(Value::Null), -32600, "Invalid Request", None));
        }
        let method = object["method"].as_str().expect("checked above");
        let params = object.get("params").cloned().unwrap_or_else(|| json!({}));
        if is_notification {
            return None;
        }
        let id = id.expect("request ID checked above");
        Some(match method {
            "initialize" => self.initialize(id, &params),
            "server/discover" => self.discover(id),
            "ping" => rpc_success(id, json!({})),
            "tools/list" => {
                let modern = request_is_modern(&params);
                if modern && let Err(error) = validate_modern_meta(&params) {
                    rpc_error(id, -32602, "Invalid params", Some(json!({"reason":error})))
                } else {
                    let mut result = json!({"tools":tool_definitions()});
                    add_modern_result_fields(&mut result, modern);
                    // Modern clients reject a tool list without cache hints.
                    if modern && let Some(object) = result.as_object_mut() {
                        object.insert("ttlMs".to_owned(), json!(300_000));
                        object.insert("cacheScope".to_owned(), json!("public"));
                    }
                    rpc_success(id, result)
                }
            }
            "tools/call" => self.call_tool(id, params),
            _ => rpc_error(id, -32601, "Method not found", None),
        })
    }

    fn initialize(&self, id: Value, params: &Value) -> Value {
        let requested = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or(LEGACY_MCP_VERSION);
        let selected = SUPPORTED_VERSIONS
            .iter()
            .copied()
            .find(|version| *version == requested && *version != MODERN_MCP_VERSION)
            .unwrap_or(LEGACY_MCP_VERSION);
        rpc_success(
            id,
            json!({
                "protocolVersion": selected,
                "capabilities":{"tools":{}},
                "serverInfo":server_info(),
                "instructions":server_instructions()
            }),
        )
    }

    fn discover(&self, id: Value) -> Value {
        rpc_success(
            id,
            json!({
                "resultType":"complete",
                "supportedVersions":SUPPORTED_VERSIONS,
                "capabilities":{"tools":{}},
                "instructions":server_instructions(),
                "ttlMs":300_000,
                "cacheScope":"public",
                "_meta":{"io.modelcontextprotocol/serverInfo":server_info()}
            }),
        )
    }

    fn call_tool(&self, id: Value, params: Value) -> Value {
        let modern = request_is_modern(&params);
        if modern && let Err(error) = validate_modern_meta(&params) {
            return rpc_error(id, -32602, "Invalid params", Some(json!({"reason":error})));
        }
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return rpc_error(
                id,
                -32602,
                "Invalid params",
                Some(json!({"reason":"tool name is required"})),
            );
        };
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if !arguments.is_object() {
            return rpc_error(
                id,
                -32602,
                "Invalid params",
                Some(json!({"reason":"tool arguments must be an object"})),
            );
        }

        let result = self.execute_tool(name, arguments);
        let result = match result {
            Ok(ToolOutput::Json(value)) => tool_result(value, false, modern, None),
            Ok(ToolOutput::Capture { value, data }) => {
                tool_result(value, false, modern, Some(data))
            }
            Err(message) => tool_result(json!({"error":message}), true, modern, None),
        };
        rpc_success(id, result)
    }

    fn execute_tool(&self, name: &str, arguments: Value) -> Result<ToolOutput, String> {
        let command = match name {
            "list_sessions" => ControlCommand::AppGetState,
            "send_input" => ControlCommand::SessionSendInput(parse_args(arguments)?),
            "snapshot" => ControlCommand::SessionGetText(parse_args(arguments)?),
            "spawn_shell" => {
                let params: SpawnShellArgs = parse_args(arguments)?;
                ControlCommand::SessionCreate(CreateSessionParams {
                    kind: crate::control::SessionKind::Shell,
                    command: None,
                    args: Vec::new(),
                    cwd: params.cwd,
                    name: params.name,
                    project_root: params.project_root,
                    worktree_path: params.worktree_path,
                    initial_input: None,
                })
            }
            "launch_agent" => {
                let params: LaunchAgentArgs = parse_args(arguments)?;
                ControlCommand::SessionCreate(CreateSessionParams {
                    kind: params.provider.kind(),
                    command: None,
                    args: params.args,
                    cwd: params.cwd,
                    name: params.name,
                    project_root: params.project_root,
                    worktree_path: params.worktree_path,
                    initial_input: params.initial_input,
                })
            }
            "select_session" => ControlCommand::SessionSelect(parse_args(arguments)?),
            "open_diff" => ControlCommand::UiOpenDiff(parse_args::<UiOpenDiffParams>(arguments)?),
            "open_file" => ControlCommand::UiOpenFile(parse_args::<UiOpenFileParams>(arguments)?),
            "kill_session" => {
                let params: SessionIdParams = parse_args(arguments)?;
                ControlCommand::SessionClose(CloseSessionParams {
                    session_id: params.session_id,
                    allow_missing: false,
                })
            }
            "rename_session" => ControlCommand::SessionRename(parse_args(arguments)?),
            "set_session_worktree" => ControlCommand::SessionSetWorktree(parse_args(arguments)?),
            "open_magit" => ControlCommand::SessionOpenMagit(parse_args(arguments)?),
            "open_branch_review" => ControlCommand::SessionOpenBranchReview(parse_args(arguments)?),
            "list_agent_sessions" => ControlCommand::AgentList(parse_args(arguments)?),
            "preview_agent_session" => ControlCommand::AgentPreview(parse_args(arguments)?),
            "restore_agent_session" => ControlCommand::AgentRestore(parse_args(arguments)?),
            "worktree_list" => ControlCommand::WorktreeList(parse_args(arguments)?),
            "worktree_create" => ControlCommand::WorktreeCreate(parse_args(arguments)?),
            "worktree_reap" => ControlCommand::WorktreeReap(parse_args(arguments)?),
            "worktree_context" => return self.worktree_context(arguments),
            "list_pull_requests" => ControlCommand::PrList(parse_args(arguments)?),
            "acknowledge_pull_request" => ControlCommand::PrAcknowledge(parse_args(arguments)?),
            "set_auto_review" => ControlCommand::PrSetAutoReview(parse_args(arguments)?),
            "launch_pr_review" => ControlCommand::PrLaunchReview(parse_args(arguments)?),
            "set_claude_presets" => ControlCommand::ClaudePresetsSet(parse_args(arguments)?),
            "list_claude_presets" => {
                require_empty(arguments)?;
                ControlCommand::ClaudePresetsGet
            }
            "apply_claude_preset" => ControlCommand::ClaudePresetApply(parse_args(arguments)?),
            "inspect_ui" => ControlCommand::UiInspect,
            "capture_ui" => {
                require_empty(arguments)?;
                let value = self.backend.call(ControlCommand::UiCapture)?;
                let data = capture_data(&value)?;
                return Ok(ToolOutput::Capture { value, data });
            }
            "set_session_state" => ControlCommand::SessionSetState(parse_args(arguments)?),
            _ => return Err(format!("unknown agtk tool: {name}")),
        };
        let value = self.backend.call(command)?;
        let value = match name {
            "list_sessions" => json!({
                "sessions":value.get("sessions").cloned().unwrap_or(Value::Array(Vec::new())),
                "selectedSessionId":value.get("selectedSessionId").cloned().unwrap_or(Value::Null),
                "attention":value.get("attention").cloned().unwrap_or_else(|| json!({"count":0}))
            }),
            _ => value,
        };
        Ok(ToolOutput::Json(value))
    }

    fn worktree_context(&self, arguments: Value) -> Result<ToolOutput, String> {
        let params: WorktreeContextArgs = parse_args(arguments)?;
        let state = self.backend.call(ControlCommand::AppGetState)?;
        let project_root_filter = params
            .project_root
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned());
        let matches_root = |value: &Value| {
            project_root_filter.as_deref().is_none_or(|root| {
                value.get("root").and_then(Value::as_str) == Some(root)
                    || value.get("projectRoot").and_then(Value::as_str) == Some(root)
            })
        };
        let filter = |key: &str| {
            state
                .get(key)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|value| matches_root(value))
                .cloned()
                .collect::<Vec<_>>()
        };
        let inventory = if let Some(project_root) = params.project_root {
            self.backend
                .call(ControlCommand::WorktreeList(WorktreeListParams {
                    project_root,
                }))?
        } else {
            Value::Null
        };
        Ok(ToolOutput::Json(json!({
            "projects":filter("projects"),
            "worktreeGroups":filter("worktreeGroups"),
            "sessions":filter("sessions"),
            "inventory":inventory
        })))
    }
}

impl<B: ControlBackend> McpServer<B> {
    pub fn serve<R: BufRead, W: Write>(&mut self, mut input: R, mut output: W) -> io::Result<()> {
        let mut bytes = Vec::new();
        loop {
            bytes.clear();
            let read = input.read_until(b'\n', &mut bytes)?;
            if read == 0 {
                return Ok(());
            }
            if bytes.len() > MAX_MCP_MESSAGE_BYTES {
                write_message(
                    &mut output,
                    &rpc_error(
                        Value::Null,
                        -32600,
                        "Invalid Request",
                        Some(json!({"reason":"message exceeds 1 MiB"})),
                    ),
                )?;
                continue;
            }
            let message = match serde_json::from_slice(&bytes) {
                Ok(message) => message,
                Err(error) => {
                    write_message(
                        &mut output,
                        &rpc_error(
                            Value::Null,
                            -32700,
                            "Parse error",
                            Some(json!({"reason":error.to_string()})),
                        ),
                    )?;
                    continue;
                }
            };
            if let Some(response) = self.handle(message) {
                write_message(&mut output, &response)?;
            }
        }
    }
}

fn write_message(output: &mut impl Write, value: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut *output, value).map_err(io::Error::other)?;
    output.write_all(b"\n")?;
    output.flush()
}

fn rpc_success(id: Value, result: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":result})
}

fn rpc_error(id: Value, code: i32, message: &str, data: Option<Value>) -> Value {
    let mut error = Map::new();
    error.insert("code".to_owned(), json!(code));
    error.insert("message".to_owned(), json!(message));
    if let Some(data) = data {
        error.insert("data".to_owned(), data);
    }
    json!({"jsonrpc":"2.0","id":id,"error":error})
}

fn tool_result(value: Value, is_error: bool, modern: bool, image: Option<String>) -> Value {
    let structured = if value.is_object() {
        value.clone()
    } else {
        json!({"result":value})
    };
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    let mut content = vec![json!({"type":"text","text":text})];
    if let Some(data) = image {
        content.push(json!({"type":"image","data":data,"mimeType":"image/png"}));
    }
    let mut result = json!({
        "content":content,
        "structuredContent":structured,
        "isError":is_error
    });
    add_modern_result_fields(&mut result, modern);
    result
}

fn add_modern_result_fields(result: &mut Value, modern: bool) {
    if modern && let Some(object) = result.as_object_mut() {
        object.insert("resultType".to_owned(), json!("complete"));
        object.insert(
            "_meta".to_owned(),
            json!({"io.modelcontextprotocol/serverInfo":server_info()}),
        );
    }
}

fn request_is_modern(params: &Value) -> bool {
    params
        .get("_meta")
        .and_then(|meta| meta.get("io.modelcontextprotocol/protocolVersion"))
        .and_then(Value::as_str)
        == Some(MODERN_MCP_VERSION)
}

fn validate_modern_meta(params: &Value) -> Result<(), String> {
    let meta = params
        .get("_meta")
        .and_then(Value::as_object)
        .ok_or("modern request requires _meta")?;
    if meta
        .get("io.modelcontextprotocol/protocolVersion")
        .and_then(Value::as_str)
        != Some(MODERN_MCP_VERSION)
    {
        return Err("unsupported or missing MCP protocol version".to_owned());
    }
    if !meta
        .get("io.modelcontextprotocol/clientCapabilities")
        .is_some_and(Value::is_object)
    {
        return Err("modern request requires client capabilities".to_owned());
    }
    Ok(())
}

fn parse_args<T: DeserializeOwned>(arguments: Value) -> Result<T, String> {
    serde_json::from_value(arguments).map_err(|error| format!("invalid tool arguments: {error}"))
}

fn require_empty(arguments: Value) -> Result<(), String> {
    if arguments.as_object().is_some_and(Map::is_empty) {
        Ok(())
    } else {
        Err("this tool accepts no arguments".to_owned())
    }
}

fn capture_data(value: &Value) -> Result<String, String> {
    let path = value
        .get("path")
        .and_then(Value::as_str)
        .ok_or("capture response has no path")?;
    let metadata =
        fs::metadata(path).map_err(|error| format!("could not inspect capture: {error}"))?;
    if metadata.len() > MAX_CAPTURE_BYTES {
        return Err("capture exceeds 32 MiB".to_owned());
    }
    let bytes = fs::read(path).map_err(|error| format!("could not read capture: {error}"))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

fn server_info() -> Value {
    json!({"name":SERVER_NAME,"title":"agtk","version":env!("CARGO_PKG_VERSION")})
}

fn server_instructions() -> &'static str {
    "Controls one named local agtk instance. Use exact session IDs, inspect before mutation, and only close disposable or explicitly targeted sessions."
}

fn error_code_name(code: crate::control::ErrorCode) -> &'static str {
    use crate::control::ErrorCode;
    match code {
        ErrorCode::InvalidRequest => "INVALID_REQUEST",
        ErrorCode::UnsupportedVersion => "UNSUPPORTED_VERSION",
        ErrorCode::MethodNotFound => "METHOD_NOT_FOUND",
        ErrorCode::NotImplemented => "NOT_IMPLEMENTED",
        ErrorCode::InvalidParams => "INVALID_PARAMS",
        ErrorCode::RequestTooLarge => "REQUEST_TOO_LARGE",
        ErrorCode::SessionNotFound => "SESSION_NOT_FOUND",
        ErrorCode::OperationRefused => "OPERATION_REFUSED",
        ErrorCode::InternalError => "INTERNAL_ERROR",
    }
}

enum ToolOutput {
    Json(Value),
    Capture { value: Value, data: String },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SpawnShellArgs {
    cwd: Option<PathBuf>,
    name: Option<String>,
    project_root: Option<PathBuf>,
    worktree_path: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LaunchAgentArgs {
    provider: AgentProvider,
    #[serde(default)]
    args: Vec<String>,
    cwd: Option<PathBuf>,
    name: Option<String>,
    project_root: Option<PathBuf>,
    worktree_path: Option<PathBuf>,
    initial_input: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorktreeContextArgs {
    project_root: Option<PathBuf>,
}

fn tool_definitions() -> Vec<Value> {
    vec![
        tool(
            "list_sessions",
            "List sessions",
            "List live native terminal sessions and attention state.",
            empty_schema(),
        ),
        tool(
            "send_input",
            "Send input",
            "Write bounded UTF-8 input to one exact session ID.",
            schema(
                &[
                    ("sessionId", string()),
                    ("text", string()),
                    ("appendEnter", boolean()),
                ],
                &["sessionId", "text"],
            ),
        ),
        tool(
            "snapshot",
            "Read terminal text",
            "Read a bounded observational terminal-text snapshot.",
            schema(
                &[("sessionId", string()), ("lines", integer(1, 20_000))],
                &["sessionId"],
            ),
        ),
        tool(
            "spawn_shell",
            "Spawn shell",
            "Launch a native shell session in an optional project or worktree.",
            launch_schema(false),
        ),
        tool(
            "launch_agent",
            "Launch agent",
            "Launch Codex or Claude directly with optional arguments and initial input. The session opens in the background and does not take the selection.",
            launch_schema(true),
        ),
        tool(
            "select_session",
            "Select session",
            "Show and focus one exact session.",
            schema(&[("sessionId", string())], &["sessionId"]),
        ),
        tool(
            "open_diff",
            "Open file diff",
            "Open a read-only diff from the selected agent worktree in agtk.",
            schema(
                &[
                    ("sessionId", string()),
                    (
                        "scope",
                        enum_values(&[
                            "all",
                            "staged",
                            "unstaged",
                            "untracked",
                            "committed",
                            "commit",
                        ]),
                    ),
                    ("path", string()),
                    ("commitId", string()),
                ],
                &["sessionId", "scope", "path"],
            ),
        ),
        tool(
            "open_file",
            "Open file",
            "Show a worktree file read-only in agtk, at an optional line and column.",
            schema(
                &[
                    ("sessionId", string()),
                    ("path", string()),
                    ("line", integer(1, 10_000_000)),
                    ("column", integer(1, 100_000)),
                ],
                &["sessionId", "path"],
            ),
        ),
        tool(
            "kill_session",
            "Close session",
            "Explicitly terminate one exact session and its child process group.",
            schema(&[("sessionId", string())], &["sessionId"]),
        ),
        tool(
            "rename_session",
            "Rename session",
            "Change one session's display name.",
            schema(
                &[("sessionId", string()), ("name", string())],
                &["sessionId", "name"],
            ),
        ),
        tool(
            "set_session_worktree",
            "Set session worktree",
            "Associate one session with a Git worktree: sidebar grouping, changes and Emacs follow it. The process keeps its own working directory. Call it with $AGTK_SESSION_ID after moving into a new worktree.",
            schema(
                &[("sessionId", string()), ("worktreePath", string())],
                &["sessionId", "worktreePath"],
            ),
        ),
        tool(
            "open_magit",
            "Open Magit",
            "Open one session's worktree in an existing or new graphical Emacs frame.",
            schema(&[("sessionId", string())], &["sessionId"]),
        ),
        tool(
            "open_branch_review",
            "Open branch review",
            "Open Emacs branch-review for one session's worktree and resolved base branch.",
            schema(&[("sessionId", string())], &["sessionId"]),
        ),
        tool(
            "list_agent_sessions",
            "List recent agent sessions",
            "Discover bounded recent Codex and Claude conversations that are not live.",
            schema(
                &[("limit", integer(1, 500)), ("maxAgeDays", integer(1, 3650))],
                &[],
            ),
        ),
        tool(
            "preview_agent_session",
            "Preview agent session",
            "Read bounded conversation context before restoring it.",
            provider_session_schema(true, false),
        ),
        tool(
            "restore_agent_session",
            "Restore agent session",
            "Resume one exact provider conversation in a chosen directory or worktree.",
            provider_session_schema(false, true),
        ),
        tool(
            "worktree_list",
            "List worktrees",
            "Inspect deterministic worktree lifecycle and exact reap guards.",
            schema(&[("projectRoot", string())], &["projectRoot"]),
        ),
        tool(
            "worktree_create",
            "Create worktree",
            "Create a purpose-labelled worktree using the repository path convention.",
            schema(
                &[
                    ("projectRoot", string()),
                    ("branch", string()),
                    ("baseBranch", string()),
                    ("purpose", string()),
                ],
                &["projectRoot", "branch", "purpose"],
            ),
        ),
        tool(
            "worktree_reap",
            "Safely reap worktree",
            "Reap using exact HEAD and content-aware status tokens returned by worktree_list.",
            schema(
                &[
                    ("path", string()),
                    ("expectedHead", string()),
                    ("expectedStatusHash", string()),
                    ("deleteBranch", enum_values(&["auto", "never", "force"])),
                ],
                &["path", "expectedHead", "expectedStatusHash", "deleteBranch"],
            ),
        ),
        tool(
            "worktree_context",
            "Inspect worktree context",
            "Return project, grouped session, and optional lifecycle inventory context.",
            schema(&[("projectRoot", string())], &[]),
        ),
        tool(
            "list_pull_requests",
            "List pull requests",
            "List active Azure DevOps pull requests, local worktree matches, and persisted attention.",
            schema(&[("projectRoot", string())], &["projectRoot"]),
        ),
        tool(
            "acknowledge_pull_request",
            "Acknowledge PR attention",
            "Clear one exact current PR attention marker without changing Azure DevOps state.",
            schema(
                &[
                    ("projectRoot", string()),
                    ("pullRequestId", integer(1, u32::MAX)),
                    ("marker", enum_values(&["new", "published", "review"])),
                ],
                &["projectRoot", "pullRequestId", "marker"],
            ),
        ),
        tool(
            "set_auto_review",
            "Set automatic PR review",
            "Opt one project in or out of Codex launches for later PR attention changes.",
            schema(
                &[("projectRoot", string()), ("enabled", boolean())],
                &["projectRoot", "enabled"],
            ),
        ),
        tool(
            "launch_pr_review",
            "Launch PR review",
            "Launch Codex with /review-pr for one active PR in its own detached pr-<id> checkout next to the project.",
            schema(
                &[
                    ("projectRoot", string()),
                    ("pullRequestId", integer(1, u32::MAX)),
                ],
                &["projectRoot", "pullRequestId"],
            ),
        ),
        tool(
            "list_claude_presets",
            "List Claude presets",
            "List the validated named Claude model and effort presets.",
            empty_schema(),
        ),
        tool(
            "set_claude_presets",
            "Set Claude presets",
            "Replace the bounded persisted list of named Claude model and effort presets.",
            schema(
                &[(
                    "presets",
                    json!({
                        "type":"array",
                        "maxItems":50,
                        "items":{
                            "type":"object",
                            "properties":{
                                "id":string(),
                                "name":string(),
                                "model":string(),
                                "effort":enum_values(&["auto","low","medium","high","xhigh","max","ultracode"])
                            },
                            "required":["id","name","model","effort"],
                            "additionalProperties":false
                        }
                    }),
                )],
                &["presets"],
            ),
        ),
        tool(
            "apply_claude_preset",
            "Apply Claude preset",
            "Send exact /model and /effort commands to one live Claude session.",
            schema(
                &[("sessionId", string()), ("presetId", string())],
                &["sessionId", "presetId"],
            ),
        ),
        tool(
            "inspect_ui",
            "Inspect UI",
            "Return the bounded logical agtk widget tree without terminal or clipboard contents.",
            empty_schema(),
        ),
        tool(
            "capture_ui",
            "Capture UI",
            "Return an app-only PNG of the workspace or open transient surface.",
            empty_schema(),
        ),
        tool(
            "set_session_state",
            "Set readiness",
            "Mark an exact agent session busy, ready, waiting, or idle through the trusted local callback.",
            schema(
                &[
                    ("sessionId", string()),
                    ("state", enum_values(&["busy", "ready", "waiting", "idle"])),
                ],
                &["sessionId", "state"],
            ),
        ),
    ]
}

fn tool(name: &str, title: &str, description: &str, input_schema: Value) -> Value {
    json!({"name":name,"title":title,"description":description,"inputSchema":input_schema})
}

fn empty_schema() -> Value {
    json!({"type":"object","properties":{},"additionalProperties":false})
}

fn schema(properties: &[(&str, Value)], required: &[&str]) -> Value {
    let properties = properties
        .iter()
        .map(|(name, value)| ((*name).to_owned(), value.clone()))
        .collect::<Map<_, _>>();
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}

fn launch_schema(agent: bool) -> Value {
    let mut properties = vec![
        ("cwd", string()),
        ("name", string()),
        ("projectRoot", string()),
        ("worktreePath", string()),
    ];
    let mut required = Vec::new();
    if agent {
        properties.insert(0, ("provider", enum_values(&["codex", "claude"])));
        properties.push(("args", json!({"type":"array","items":{"type":"string"}})));
        properties.push(("initialInput", string()));
        required.push("provider");
    }
    schema(&properties, &required)
}

fn provider_session_schema(preview: bool, restore: bool) -> Value {
    let mut properties = vec![
        ("provider", enum_values(&["codex", "claude"])),
        ("providerSessionId", string()),
    ];
    if preview {
        properties.push(("maxMessages", integer(1, 100)));
    }
    if restore {
        properties.extend([
            ("cwd", string()),
            ("projectRoot", string()),
            ("worktreePath", string()),
            ("name", string()),
        ]);
    }
    schema(&properties, &["provider", "providerSessionId"])
}

fn string() -> Value {
    json!({"type":"string"})
}

fn boolean() -> Value {
    json!({"type":"boolean"})
}

fn integer(minimum: u32, maximum: u32) -> Value {
    json!({"type":"integer","minimum":minimum,"maximum":maximum})
}

fn enum_values(values: &[&str]) -> Value {
    json!({"type":"string","enum":values})
}
