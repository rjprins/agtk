use std::env;
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

use agtk::appearance::ThemeKey;
use agtk::azure::PrAttention;
use agtk::claude_presets::ClaudeModelPreset;
use agtk::control::{
    AgentListParams, AgentPreviewParams, AgentRestoreParams, AgentSignalState, AppearanceSetParams,
    ClaudePresetApplyParams, ClaudePresetsSetParams, ClientError, CloseSessionParams,
    ControlClient, ControlCommand, ControlRequest, CreateSessionParams, ErrorCode, GetTextParams,
    PROTOCOL_VERSION, PrAcknowledgeParams, PrLaunchReviewParams, PrListParams,
    PrSetAutoReviewParams, ProjectSetParams, RenameSessionParams, ResponseBody, SendInputParams,
    SessionIdParams, SessionKind, SessionSetStateParams, SessionState, SetSessionWorktreeParams,
    ShortcutSetParams, UiDiffScope, UiOpenDiffParams, UiOpenFileParams, UiShowParams, UiSurface,
    WaitCondition, WorktreeCreateParams, WorktreeListParams, WorktreeReapParams,
};
use agtk::instance::{InstanceName, InstancePaths};
use agtk::providers::AgentProvider;
use agtk::shortcuts::ShortcutAction;
use agtk::worktrees::DeleteBranch;

const EXIT_USAGE_OR_PROTOCOL: u8 = 2;
const EXIT_CONNECTION: u8 = 3;
const EXIT_TIMEOUT: u8 = 4;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Usage(message)) => {
            eprintln!("{message}");
            ExitCode::from(EXIT_USAGE_OR_PROTOCOL)
        }
        Err(Failure::Client(error)) => {
            eprintln!("{error}");
            ExitCode::from(client_exit_code(&error))
        }
        Err(Failure::Server(error)) => {
            match serde_json::to_string(&error) {
                Ok(json) => println!("{json}"),
                Err(encoding) => eprintln!("could not encode server error: {encoding}"),
            }
            ExitCode::from(EXIT_USAGE_OR_PROTOCOL)
        }
        Err(Failure::Timeout(message)) => {
            eprintln!("{message}");
            ExitCode::from(EXIT_TIMEOUT)
        }
    }
}

fn run() -> Result<(), Failure> {
    let mut arguments = env::args().skip(1).collect::<Vec<_>>();
    let instance = if arguments.first().is_some_and(|value| value == "--instance") {
        if arguments.len() < 2 {
            return Err(Failure::Usage("--instance requires a value".to_owned()));
        }
        let value = arguments.remove(1);
        arguments.remove(0);
        InstanceName::parse(&value).map_err(|error| Failure::Usage(error.to_string()))?
    } else {
        InstanceName::from_environment().map_err(|error| Failure::Usage(error.to_string()))?
    };

    let paths = InstancePaths::from_environment(instance);
    let client = ControlClient::new(paths.control_socket());
    if arguments.first().is_some_and(|argument| argument == "wait") {
        return run_wait(&client, parse_wait(&arguments[1..])?);
    }

    let command = match arguments.as_slice() {
        [command] if command == "state" => ControlCommand::AppGetState,
        [group, command] if group == "ui" && command == "inspect" => ControlCommand::UiInspect,
        [group, command] if group == "ui" && command == "capture" => ControlCommand::UiCapture,
        [group, command, surface] if group == "ui" && command == "show" => {
            ControlCommand::UiShow(UiShowParams {
                surface: parse_ui_surface(surface)?,
            })
        }
        [group, command, session_id, rest @ ..] if group == "ui" && command == "diff" => {
            ControlCommand::UiOpenDiff(parse_ui_diff(session_id, rest)?)
        }
        [group, command, session_id, rest @ ..] if group == "ui" && command == "file" => {
            ControlCommand::UiOpenFile(parse_ui_file(session_id, rest)?)
        }
        [group, action, rest @ ..] if group == "appearance" && action == "set" => {
            parse_appearance_set(rest)?
        }
        [group, action, rest @ ..] if group == "shortcut" => parse_shortcut_command(action, rest)?,
        [group, action, rest @ ..] if group == "project" && action == "set" => {
            parse_project_set(rest)?
        }
        [group, action, rest @ ..] if group == "worktree" => parse_worktree_command(action, rest)?,
        [group, action, rest @ ..] if group == "pr" => parse_pr_command(action, rest)?,
        [group, action, rest @ ..] if group == "claude" => parse_claude_command(action, rest)?,
        [group, action, rest @ ..] if group == "agent" => parse_agent_command(action, rest)?,
        [group, action, rest @ ..] if group == "session" => parse_session_command(action, rest)?,
        _ => {
            return Err(Failure::Usage(usage().to_owned()));
        }
    };
    let response = client
        .send(&ControlRequest {
            version: PROTOCOL_VERSION,
            id: format!("ctl-{}", std::process::id()),
            command,
        })
        .map_err(Failure::Client)?;

    match response.body {
        ResponseBody::Success(result) => {
            println!(
                "{}",
                serde_json::to_string(&result)
                    .map_err(|error| Failure::Usage(error.to_string()))?
            );
            Ok(())
        }
        ResponseBody::Failure(error) => Err(Failure::Server(error)),
    }
}

#[derive(Debug)]
struct WaitOptions {
    condition: WaitCondition,
    timeout: Duration,
    poll_interval: Duration,
}

fn parse_ui_diff(session_id: &str, arguments: &[String]) -> Result<UiOpenDiffParams, Failure> {
    let mut scope = None;
    let mut path = None;
    let mut commit_id = None;
    let mut index = 0;
    while index < arguments.len() {
        let option = arguments[index].as_str();
        let value = arguments
            .get(index + 1)
            .ok_or_else(|| Failure::Usage(format!("{option} requires a value")))?
            .clone();
        match option {
            "--scope" => {
                scope = Some(match value.as_str() {
                    "all" => UiDiffScope::All,
                    "staged" => UiDiffScope::Staged,
                    "unstaged" => UiDiffScope::Unstaged,
                    "untracked" => UiDiffScope::Untracked,
                    "committed" => UiDiffScope::Committed,
                    "commit" => UiDiffScope::Commit,
                    _ => return Err(Failure::Usage("unknown diff scope".to_owned())),
                });
            }
            "--path" => path = Some(value),
            "--commit" => commit_id = Some(value),
            _ => return Err(Failure::Usage(format!("unknown ui diff option: {option}"))),
        }
        index += 2;
    }
    let scope = scope.ok_or_else(|| Failure::Usage("ui diff requires --scope".to_owned()))?;
    let path = path.ok_or_else(|| Failure::Usage("ui diff requires --path".to_owned()))?;
    Ok(UiOpenDiffParams {
        session_id: session_id.to_owned(),
        scope,
        path,
        commit_id,
    })
}

fn parse_ui_file(session_id: &str, arguments: &[String]) -> Result<UiOpenFileParams, Failure> {
    let mut path = None;
    let mut line = None;
    let mut column = None;
    let mut index = 0;
    while index < arguments.len() {
        let option = arguments[index].as_str();
        let value = arguments
            .get(index + 1)
            .ok_or_else(|| Failure::Usage(format!("{option} requires a value")))?
            .clone();
        let number = || {
            value
                .parse::<u32>()
                .ok()
                .filter(|number| *number > 0)
                .ok_or_else(|| Failure::Usage(format!("{option} requires a positive number")))
        };
        match option {
            "--path" => path = Some(value),
            "--line" => line = Some(number()?),
            "--column" => column = Some(number()?),
            _ => return Err(Failure::Usage(format!("unknown ui file option: {option}"))),
        }
        index += 2;
    }
    let path = path.ok_or_else(|| Failure::Usage("ui file requires --path".to_owned()))?;
    Ok(UiOpenFileParams {
        session_id: session_id.to_owned(),
        path,
        line,
        column,
    })
}

fn parse_wait(arguments: &[String]) -> Result<WaitOptions, Failure> {
    let mut session_id = None;
    let mut selected_id = None;
    let mut exists = false;
    let mut text = None;
    let mut state = None;
    let mut lines = 200;
    let mut timeout_ms = 5_000;
    let mut poll_ms = 50;
    let mut index = 0;
    while index < arguments.len() {
        let option = arguments[index].as_str();
        if option == "--exists" {
            exists = true;
            index += 1;
            continue;
        }
        let value = arguments
            .get(index + 1)
            .ok_or_else(|| Failure::Usage(format!("{option} requires a value")))?
            .clone();
        match option {
            "--session" => session_id = Some(value),
            "--selected" => selected_id = Some(value),
            "--text" => text = Some(value),
            "--state" => state = Some(parse_session_state(&value)?),
            "--lines" => lines = parse_bounded_u32("--lines", &value, 1, 20_000)?,
            "--timeout-ms" => {
                timeout_ms = parse_bounded_u64("--timeout-ms", &value, 1, 3_600_000)?;
            }
            "--poll-ms" => poll_ms = parse_bounded_u64("--poll-ms", &value, 1, 5_000)?,
            _ => return Err(Failure::Usage(format!("unknown wait option: {option}"))),
        }
        index += 2;
    }

    let condition_count = usize::from(exists)
        + usize::from(text.is_some())
        + usize::from(state.is_some())
        + usize::from(selected_id.is_some());
    if condition_count != 1 {
        return Err(Failure::Usage(
            "wait requires exactly one of --exists, --text, --state, or --selected".to_owned(),
        ));
    }
    let condition = if let Some(selected_id) = selected_id {
        if session_id.is_some() {
            return Err(Failure::Usage(
                "--selected cannot be combined with --session".to_owned(),
            ));
        }
        WaitCondition::SelectedSession(selected_id)
    } else {
        let session_id = session_id
            .ok_or_else(|| Failure::Usage("wait condition requires --session".to_owned()))?;
        if exists {
            WaitCondition::SessionExists(session_id)
        } else if let Some(literal) = text {
            if literal.is_empty() {
                return Err(Failure::Usage("--text cannot be empty".to_owned()));
            }
            WaitCondition::TerminalText {
                session_id,
                literal,
                lines,
            }
        } else {
            WaitCondition::SessionState {
                session_id,
                state: state.expect("condition count guarantees state"),
            }
        }
    };

    Ok(WaitOptions {
        condition,
        timeout: Duration::from_millis(timeout_ms),
        poll_interval: Duration::from_millis(poll_ms),
    })
}

fn run_wait(client: &ControlClient, options: WaitOptions) -> Result<(), Failure> {
    let deadline = Instant::now() + options.timeout;
    let mut sequence = 0_u64;
    loop {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return Err(Failure::Timeout(format!(
                "wait timed out after {} ms",
                options.timeout.as_millis()
            )));
        };
        let response = client
            .send_with_timeout(
                &ControlRequest {
                    version: PROTOCOL_VERSION,
                    id: format!("ctl-{}-wait-{sequence}", std::process::id()),
                    command: options.condition.observation(),
                },
                remaining,
            )
            .map_err(|error| {
                // The request only gets the time left, so a slow reply is the wait deadline.
                if error.is_timeout() {
                    Failure::Timeout(format!(
                        "wait timed out after {} ms",
                        options.timeout.as_millis()
                    ))
                } else {
                    Failure::Client(error)
                }
            })?;
        match response.body {
            ResponseBody::Success(result) => {
                if options.condition.is_satisfied(&result) {
                    println!(
                        "{}",
                        serde_json::to_string(&result)
                            .map_err(|error| Failure::Usage(error.to_string()))?
                    );
                    return Ok(());
                }
            }
            ResponseBody::Failure(error)
                if error.code == ErrorCode::SessionNotFound
                    && matches!(options.condition, WaitCondition::TerminalText { .. }) => {}
            ResponseBody::Failure(error) => return Err(Failure::Server(error)),
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(Failure::Timeout(format!(
                "wait timed out after {} ms",
                options.timeout.as_millis()
            )));
        }
        thread::sleep(options.poll_interval.min(deadline - now));
        sequence += 1;
    }
}

fn parse_session_state(value: &str) -> Result<SessionState, Failure> {
    match value {
        "running" => Ok(SessionState::Running),
        "busy" => Ok(SessionState::Busy),
        "ready" => Ok(SessionState::Ready),
        "waiting" => Ok(SessionState::Waiting),
        "idle" => Ok(SessionState::Idle),
        "exited" => Ok(SessionState::Exited),
        "reconnecting" => Ok(SessionState::Reconnecting),
        _ => Err(Failure::Usage(format!("unknown session state: {value}"))),
    }
}

fn parse_ui_surface(value: &str) -> Result<UiSurface, Failure> {
    match value {
        "launch" => Ok(UiSurface::Launch),
        "appearance" => Ok(UiSurface::Appearance),
        "shortcuts" => Ok(UiSurface::Shortcuts),
        "history" => Ok(UiSurface::History),
        "search" => Ok(UiSurface::Search),
        "changes" => Ok(UiSurface::Changes),
        "files" => Ok(UiSurface::Files),
        "worktrees" => Ok(UiSurface::Worktrees),
        "agents" => Ok(UiSurface::Agents),
        "pull-requests" | "prs" => Ok(UiSurface::PullRequests),
        "claude-models" => Ok(UiSurface::ClaudeModels),
        "close-session" => Ok(UiSurface::CloseSession),
        _ => Err(Failure::Usage(format!("unknown UI surface: {value}"))),
    }
}

fn parse_bounded_u32(
    option: &str,
    value: &str,
    minimum: u32,
    maximum: u32,
) -> Result<u32, Failure> {
    let parsed = value
        .parse::<u32>()
        .map_err(|_| Failure::Usage(format!("{option} requires an integer")))?;
    if !(minimum..=maximum).contains(&parsed) {
        return Err(Failure::Usage(format!(
            "{option} must be between {minimum} and {maximum}"
        )));
    }
    Ok(parsed)
}

fn parse_bounded_u64(
    option: &str,
    value: &str,
    minimum: u64,
    maximum: u64,
) -> Result<u64, Failure> {
    let parsed = value
        .parse::<u64>()
        .map_err(|_| Failure::Usage(format!("{option} requires an integer")))?;
    if !(minimum..=maximum).contains(&parsed) {
        return Err(Failure::Usage(format!(
            "{option} must be between {minimum} and {maximum}"
        )));
    }
    Ok(parsed)
}

fn parse_session_command(action: &str, arguments: &[String]) -> Result<ControlCommand, Failure> {
    match action {
        "create" => parse_session_create(arguments),
        "select" => match arguments {
            [session_id] => Ok(ControlCommand::SessionSelect(SessionIdParams {
                session_id: session_id.clone(),
            })),
            _ => Err(usage_failure()),
        },
        "input" => parse_session_input(arguments),
        "text" => match arguments {
            [session_id] => Ok(ControlCommand::SessionGetText(GetTextParams {
                session_id: session_id.clone(),
                lines: 200,
            })),
            [session_id, flag, lines] if flag == "--lines" => {
                let lines = lines
                    .parse::<u32>()
                    .map_err(|_| Failure::Usage("--lines requires an integer".to_owned()))?;
                Ok(ControlCommand::SessionGetText(GetTextParams {
                    session_id: session_id.clone(),
                    lines,
                }))
            }
            _ => Err(usage_failure()),
        },
        "rename" => match arguments {
            [session_id, flag, name] if flag == "--name" => {
                Ok(ControlCommand::SessionRename(RenameSessionParams {
                    session_id: session_id.clone(),
                    name: name.clone(),
                }))
            }
            _ => Err(usage_failure()),
        },
        "worktree" => match arguments {
            [session_id, flag, path] if flag == "--path" => Ok(ControlCommand::SessionSetWorktree(
                SetSessionWorktreeParams {
                    session_id: session_id.clone(),
                    worktree_path: path.into(),
                },
            )),
            _ => Err(usage_failure()),
        },
        "close" => match arguments {
            [session_id] => Ok(ControlCommand::SessionClose(CloseSessionParams {
                session_id: session_id.clone(),
                allow_missing: false,
            })),
            [session_id, flag] if flag == "--allow-missing" => {
                Ok(ControlCommand::SessionClose(CloseSessionParams {
                    session_id: session_id.clone(),
                    allow_missing: true,
                }))
            }
            _ => Err(usage_failure()),
        },
        "restart" => match arguments {
            [session_id] => Ok(ControlCommand::SessionRestart(SessionIdParams {
                session_id: session_id.clone(),
            })),
            _ => Err(usage_failure()),
        },
        "state" => match arguments {
            [session_id, state] => Ok(ControlCommand::SessionSetState(SessionSetStateParams {
                session_id: session_id.clone(),
                state: parse_agent_signal_state(state)?,
                conversation_id: None,
            })),
            [session_id, state, flag] if flag == "--hook-input" => {
                Ok(ControlCommand::SessionSetState(SessionSetStateParams {
                    session_id: session_id.clone(),
                    state: parse_agent_signal_state(state)?,
                    conversation_id: hook_conversation_id(std::io::stdin().lock()),
                }))
            }
            _ => Err(usage_failure()),
        },
        "magit" => match arguments {
            [session_id] => Ok(ControlCommand::SessionOpenMagit(SessionIdParams {
                session_id: session_id.clone(),
            })),
            _ => Err(usage_failure()),
        },
        "review" => match arguments {
            [session_id] => Ok(ControlCommand::SessionOpenBranchReview(SessionIdParams {
                session_id: session_id.clone(),
            })),
            _ => Err(usage_failure()),
        },
        _ => Err(usage_failure()),
    }
}

fn parse_session_create(arguments: &[String]) -> Result<ControlCommand, Failure> {
    let mut kind = None;
    let mut command = None;
    let mut args = Vec::new();
    let mut cwd = None;
    let mut name = None;
    let mut project_root = None;
    let mut worktree_path = None;
    let mut initial_input = None;
    let mut index = 0;
    while index < arguments.len() {
        let option = arguments[index].as_str();
        let value = arguments
            .get(index + 1)
            .ok_or_else(|| Failure::Usage(format!("{option} requires a value")))?
            .clone();
        match option {
            "--kind" => kind = Some(parse_session_kind(&value)?),
            "--command" => command = Some(value),
            "--arg" => args.push(value),
            "--cwd" => cwd = Some(value.into()),
            "--name" => name = Some(value),
            "--project-root" => project_root = Some(value.into()),
            "--worktree-path" => worktree_path = Some(value.into()),
            "--initial-input" => initial_input = Some(value),
            _ => {
                return Err(Failure::Usage(format!(
                    "unknown session create option: {option}"
                )));
            }
        }
        index += 2;
    }
    let kind = kind.ok_or_else(|| Failure::Usage("session create requires --kind".to_owned()))?;
    Ok(ControlCommand::SessionCreate(CreateSessionParams {
        kind,
        command,
        args,
        cwd,
        name,
        project_root,
        worktree_path,
        initial_input,
    }))
}

fn parse_session_input(arguments: &[String]) -> Result<ControlCommand, Failure> {
    let Some(session_id) = arguments.first() else {
        return Err(usage_failure());
    };
    let mut text = None;
    let mut append_enter = true;
    let mut index = 1;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--text" => {
                text = Some(
                    arguments
                        .get(index + 1)
                        .ok_or_else(|| Failure::Usage("--text requires a value".to_owned()))?
                        .clone(),
                );
                index += 2;
            }
            "--no-enter" => {
                append_enter = false;
                index += 1;
            }
            option => {
                return Err(Failure::Usage(format!(
                    "unknown session input option: {option}"
                )));
            }
        }
    }
    let text = text.ok_or_else(|| Failure::Usage("session input requires --text".to_owned()))?;
    Ok(ControlCommand::SessionSendInput(SendInputParams {
        session_id: session_id.clone(),
        text,
        append_enter,
    }))
}

fn parse_session_kind(value: &str) -> Result<SessionKind, Failure> {
    match value {
        "shell" => Ok(SessionKind::Shell),
        "codex" => Ok(SessionKind::Codex),
        "claude" => Ok(SessionKind::Claude),
        "gemini" => Ok(SessionKind::Gemini),
        "custom" => Ok(SessionKind::Custom),
        _ => Err(Failure::Usage(format!("unknown session kind: {value}"))),
    }
}

fn parse_appearance_set(arguments: &[String]) -> Result<ControlCommand, Failure> {
    let mut theme = None;
    let mut follow_system = None;
    let mut font = None;
    let mut ui_font_size = None;
    let mut index = 0;
    while index < arguments.len() {
        let option = arguments[index].as_str();
        let value = arguments
            .get(index + 1)
            .ok_or_else(|| Failure::Usage(format!("{option} requires a value")))?
            .clone();
        match option {
            "--theme" => theme = Some(parse_theme(&value)?),
            "--follow-system" => {
                follow_system = Some(match value.as_str() {
                    "true" => true,
                    "false" => false,
                    _ => {
                        return Err(Failure::Usage(
                            "--follow-system requires true or false".to_owned(),
                        ));
                    }
                });
            }
            "--font" => font = Some(value),
            "--ui-font-size" => {
                ui_font_size = Some(value.parse::<u8>().map_err(|_| {
                    Failure::Usage("--ui-font-size must be an integer between 9 and 24".to_owned())
                })?);
            }
            _ => {
                return Err(Failure::Usage(format!(
                    "unknown appearance set option: {option}"
                )));
            }
        }
        index += 2;
    }
    if theme.is_none() && follow_system.is_none() && font.is_none() && ui_font_size.is_none() {
        return Err(Failure::Usage(
            "appearance set requires at least one option".to_owned(),
        ));
    }
    Ok(ControlCommand::AppearanceSet(AppearanceSetParams {
        theme,
        follow_system,
        font,
        ui_font_size,
    }))
}

fn parse_theme(value: &str) -> Result<ThemeKey, Failure> {
    ThemeKey::ALL
        .into_iter()
        .find(|key| key.as_str() == value)
        .ok_or_else(|| Failure::Usage(format!("unknown theme: {value}")))
}

fn parse_shortcut_command(action: &str, arguments: &[String]) -> Result<ControlCommand, Failure> {
    let mut shortcut_action = None;
    let mut accelerator = None;
    let mut index = 0;
    while index < arguments.len() {
        let option = arguments[index].as_str();
        let value = arguments
            .get(index + 1)
            .ok_or_else(|| Failure::Usage(format!("{option} requires a value")))?
            .clone();
        match option {
            "--action" => shortcut_action = Some(parse_shortcut_action(&value)?),
            "--accelerator" if action == "set" => accelerator = Some(value),
            _ => return Err(Failure::Usage(format!("unknown shortcut option: {option}"))),
        }
        index += 2;
    }
    let shortcut_action =
        shortcut_action.ok_or_else(|| Failure::Usage("shortcut requires --action".to_owned()))?;
    match action {
        "set" => {
            Ok(ControlCommand::ShortcutSet(ShortcutSetParams {
                action: shortcut_action,
                accelerator: Some(accelerator.ok_or_else(|| {
                    Failure::Usage("shortcut set requires --accelerator".to_owned())
                })?),
                reset: false,
            }))
        }
        "reset" if accelerator.is_none() => Ok(ControlCommand::ShortcutSet(ShortcutSetParams {
            action: shortcut_action,
            accelerator: None,
            reset: true,
        })),
        _ => Err(usage_failure()),
    }
}

fn parse_shortcut_action(value: &str) -> Result<ShortcutAction, Failure> {
    ShortcutAction::ALL
        .into_iter()
        .find(|action| action.as_str() == value)
        .ok_or_else(|| Failure::Usage(format!("unknown shortcut action: {value}")))
}

fn parse_project_set(arguments: &[String]) -> Result<ControlCommand, Failure> {
    let Some(root) = arguments.first() else {
        return Err(Failure::Usage("project set requires a root".to_owned()));
    };
    let mut is_pinned = None;
    let mut is_collapsed = None;
    let mut index = 1;
    while index < arguments.len() {
        let option = arguments[index].as_str();
        let value = arguments
            .get(index + 1)
            .ok_or_else(|| Failure::Usage(format!("{option} requires a value")))?;
        match option {
            "--pinned" => is_pinned = Some(parse_bool(option, value)?),
            "--collapsed" => is_collapsed = Some(parse_bool(option, value)?),
            _ => return Err(Failure::Usage(format!("unknown project option: {option}"))),
        }
        index += 2;
    }
    if is_pinned.is_none() && is_collapsed.is_none() {
        return Err(Failure::Usage(
            "project set requires --pinned or --collapsed".to_owned(),
        ));
    }
    Ok(ControlCommand::ProjectSet(ProjectSetParams {
        root: root.into(),
        is_pinned,
        is_collapsed,
    }))
}

fn parse_worktree_command(action: &str, arguments: &[String]) -> Result<ControlCommand, Failure> {
    match action {
        "list" => match arguments {
            [project_root] => Ok(ControlCommand::WorktreeList(WorktreeListParams {
                project_root: project_root.into(),
            })),
            _ => Err(usage_failure()),
        },
        "create" => parse_worktree_create(arguments),
        "reap" => parse_worktree_reap(arguments),
        _ => Err(usage_failure()),
    }
}

fn parse_worktree_create(arguments: &[String]) -> Result<ControlCommand, Failure> {
    let Some(project_root) = arguments.first() else {
        return Err(Failure::Usage(
            "worktree create requires a project root".to_owned(),
        ));
    };
    let mut branch = None;
    let mut base_branch = None;
    let mut purpose = None;
    let mut index = 1;
    while index < arguments.len() {
        let option = arguments[index].as_str();
        let value = arguments
            .get(index + 1)
            .ok_or_else(|| Failure::Usage(format!("{option} requires a value")))?
            .clone();
        match option {
            "--branch" => branch = Some(value),
            "--base" => base_branch = Some(value),
            "--purpose" => purpose = Some(value),
            _ => {
                return Err(Failure::Usage(format!(
                    "unknown worktree create option: {option}"
                )));
            }
        }
        index += 2;
    }
    Ok(ControlCommand::WorktreeCreate(WorktreeCreateParams {
        project_root: project_root.into(),
        branch: branch
            .ok_or_else(|| Failure::Usage("worktree create requires --branch".to_owned()))?,
        base_branch,
        purpose: purpose
            .ok_or_else(|| Failure::Usage("worktree create requires --purpose".to_owned()))?,
    }))
}

fn parse_worktree_reap(arguments: &[String]) -> Result<ControlCommand, Failure> {
    let Some(path) = arguments.first() else {
        return Err(Failure::Usage("worktree reap requires a path".to_owned()));
    };
    let mut expected_head = None;
    let mut expected_status_hash = None;
    let mut delete_branch = DeleteBranch::Auto;
    let mut index = 1;
    while index < arguments.len() {
        let option = arguments[index].as_str();
        let value = arguments
            .get(index + 1)
            .ok_or_else(|| Failure::Usage(format!("{option} requires a value")))?
            .clone();
        match option {
            "--expected-head" => expected_head = Some(value),
            "--expected-status-hash" => expected_status_hash = Some(value),
            "--delete-branch" => {
                delete_branch = match value.as_str() {
                    "auto" => DeleteBranch::Auto,
                    "never" => DeleteBranch::Never,
                    "force" => DeleteBranch::Force,
                    _ => {
                        return Err(Failure::Usage(
                            "--delete-branch requires auto, never, or force".to_owned(),
                        ));
                    }
                };
            }
            _ => {
                return Err(Failure::Usage(format!(
                    "unknown worktree reap option: {option}"
                )));
            }
        }
        index += 2;
    }
    Ok(ControlCommand::WorktreeReap(WorktreeReapParams {
        path: path.into(),
        expected_head: expected_head
            .ok_or_else(|| Failure::Usage("worktree reap requires --expected-head".to_owned()))?,
        expected_status_hash: expected_status_hash.ok_or_else(|| {
            Failure::Usage("worktree reap requires --expected-status-hash".to_owned())
        })?,
        delete_branch,
    }))
}

fn parse_agent_command(action: &str, arguments: &[String]) -> Result<ControlCommand, Failure> {
    match action {
        "list" => parse_agent_list(arguments),
        "preview" => parse_agent_preview(arguments),
        "restore" => parse_agent_restore(arguments),
        _ => Err(usage_failure()),
    }
}

fn parse_pr_command(action: &str, arguments: &[String]) -> Result<ControlCommand, Failure> {
    match (action, arguments) {
        ("list", [project_root]) => Ok(ControlCommand::PrList(PrListParams {
            project_root: project_root.into(),
        })),
        ("acknowledge", [project_root, pull_request_id, marker]) => {
            Ok(ControlCommand::PrAcknowledge(PrAcknowledgeParams {
                project_root: project_root.into(),
                pull_request_id: parse_pr_id(pull_request_id)?,
                marker: match marker.as_str() {
                    "new" => PrAttention::New,
                    "published" => PrAttention::Published,
                    "review" => PrAttention::Review,
                    _ => {
                        return Err(Failure::Usage(
                            "PR marker must be new, published, or review".to_owned(),
                        ));
                    }
                },
            }))
        }
        ("auto-review", [project_root, enabled]) => {
            Ok(ControlCommand::PrSetAutoReview(PrSetAutoReviewParams {
                project_root: project_root.into(),
                enabled: match enabled.as_str() {
                    "on" => true,
                    "off" => false,
                    _ => {
                        return Err(Failure::Usage(
                            "PR auto-review requires on or off".to_owned(),
                        ));
                    }
                },
            }))
        }
        ("review", [project_root, pull_request_id]) => {
            Ok(ControlCommand::PrLaunchReview(PrLaunchReviewParams {
                project_root: project_root.into(),
                pull_request_id: parse_pr_id(pull_request_id)?,
            }))
        }
        _ => Err(usage_failure()),
    }
}

fn parse_pr_id(value: &str) -> Result<u64, Failure> {
    value
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| Failure::Usage("pull request ID must be a positive integer".to_owned()))
}

fn parse_claude_command(action: &str, arguments: &[String]) -> Result<ControlCommand, Failure> {
    match (action, arguments) {
        ("presets", []) => Ok(ControlCommand::ClaudePresetsGet),
        ("presets", [flag, value]) if flag == "--json" => {
            let presets = serde_json::from_str::<Vec<ClaudeModelPreset>>(value)
                .map_err(|error| Failure::Usage(format!("invalid preset JSON: {error}")))?;
            Ok(ControlCommand::ClaudePresetsSet(ClaudePresetsSetParams {
                presets,
            }))
        }
        ("apply", [session_id, preset_id]) => {
            Ok(ControlCommand::ClaudePresetApply(ClaudePresetApplyParams {
                session_id: session_id.clone(),
                preset_id: preset_id.clone(),
            }))
        }
        _ => Err(usage_failure()),
    }
}

fn parse_agent_list(arguments: &[String]) -> Result<ControlCommand, Failure> {
    let mut limit = 100;
    let mut max_age_days = 90;
    let mut index = 0;
    while index < arguments.len() {
        let option = arguments[index].as_str();
        let value = arguments
            .get(index + 1)
            .ok_or_else(|| Failure::Usage(format!("{option} requires a value")))?;
        match option {
            "--limit" => limit = parse_bounded_u32(option, value, 1, 500)?,
            "--max-age-days" => max_age_days = parse_bounded_u32(option, value, 1, 3_650)?,
            _ => {
                return Err(Failure::Usage(format!(
                    "unknown agent list option: {option}"
                )));
            }
        }
        index += 2;
    }
    Ok(ControlCommand::AgentList(AgentListParams {
        limit,
        max_age_days,
    }))
}

fn parse_agent_preview(arguments: &[String]) -> Result<ControlCommand, Failure> {
    let [provider, provider_session_id, rest @ ..] = arguments else {
        return Err(usage_failure());
    };
    let mut max_messages = 40;
    let mut index = 0;
    while index < rest.len() {
        let option = rest[index].as_str();
        let value = rest
            .get(index + 1)
            .ok_or_else(|| Failure::Usage(format!("{option} requires a value")))?;
        match option {
            "--max-messages" => max_messages = parse_bounded_u32(option, value, 1, 100)?,
            _ => {
                return Err(Failure::Usage(format!(
                    "unknown agent preview option: {option}"
                )));
            }
        }
        index += 2;
    }
    Ok(ControlCommand::AgentPreview(AgentPreviewParams {
        provider: parse_agent_provider(provider)?,
        provider_session_id: provider_session_id.clone(),
        max_messages,
    }))
}

fn parse_agent_restore(arguments: &[String]) -> Result<ControlCommand, Failure> {
    let [provider, provider_session_id, rest @ ..] = arguments else {
        return Err(usage_failure());
    };
    let mut cwd = None;
    let mut project_root = None;
    let mut worktree_path = None;
    let mut name = None;
    let mut index = 0;
    while index < rest.len() {
        let option = rest[index].as_str();
        let value = rest
            .get(index + 1)
            .ok_or_else(|| Failure::Usage(format!("{option} requires a value")))?
            .clone();
        match option {
            "--cwd" => cwd = Some(value.into()),
            "--project-root" => project_root = Some(value.into()),
            "--worktree-path" => worktree_path = Some(value.into()),
            "--name" => name = Some(value),
            _ => {
                return Err(Failure::Usage(format!(
                    "unknown agent restore option: {option}"
                )));
            }
        }
        index += 2;
    }
    Ok(ControlCommand::AgentRestore(AgentRestoreParams {
        provider: parse_agent_provider(provider)?,
        provider_session_id: provider_session_id.clone(),
        cwd,
        project_root,
        worktree_path,
        name,
    }))
}

fn parse_agent_provider(value: &str) -> Result<AgentProvider, Failure> {
    match value {
        "codex" => Ok(AgentProvider::Codex),
        "claude" => Ok(AgentProvider::Claude),
        _ => Err(Failure::Usage(format!("unknown agent provider: {value}"))),
    }
}

fn parse_agent_signal_state(value: &str) -> Result<AgentSignalState, Failure> {
    match value {
        "busy" => Ok(AgentSignalState::Busy),
        "ready" => Ok(AgentSignalState::Ready),
        "waiting" => Ok(AgentSignalState::Waiting),
        "idle" => Ok(AgentSignalState::Idle),
        _ => Err(Failure::Usage(format!(
            "unknown agent signal state: {value}"
        ))),
    }
}

fn parse_bool(option: &str, value: &str) -> Result<bool, Failure> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(Failure::Usage(format!("{option} requires true or false"))),
    }
}

/// The `session_id` from the JSON a Claude Code hook gets on stdin.
fn hook_conversation_id(input: impl std::io::Read) -> Option<String> {
    let mut text = String::new();
    std::io::Read::read_to_string(&mut input.take(1024 * 1024), &mut text).ok()?;
    let value = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    value
        .get("session_id")?
        .as_str()
        .map(str::trim)
        .filter(|id| !id.is_empty() && !id.chars().any(char::is_whitespace))
        .map(str::to_owned)
}

fn usage_failure() -> Failure {
    Failure::Usage(usage().to_owned())
}

fn usage() -> &'static str {
    "usage: agtkctl [--instance NAME] <state|ui inspect|ui capture|ui show SURFACE|ui diff SESSION_ID --scope SCOPE --path PATH [--commit OID]|ui file SESSION_ID --path PATH [--line N] [--column N]|appearance set OPTIONS|shortcut set OPTIONS|shortcut reset --action ACTION|project set ROOT OPTIONS|worktree list ROOT|worktree create ROOT OPTIONS|worktree reap PATH GUARDS|pr list ROOT|pr acknowledge ROOT ID MARKER|pr auto-review ROOT on|off|pr review ROOT ID|claude presets [--json JSON]|claude apply SESSION_ID PRESET_ID|agent list OPTIONS|agent preview PROVIDER ID OPTIONS|agent restore PROVIDER ID OPTIONS|session create OPTIONS|session select ID|session input ID --text TEXT [--no-enter]|session text ID [--lines N]|session rename ID --name NAME|session worktree ID --path PATH|session close ID [--allow-missing]|session restart ID|session state ID STATE [--hook-input]|session magit ID|session review ID|wait OPTIONS> (SURFACE includes changes and files, SCOPE is all|staged|unstaged|untracked|committed|commit)"
}

fn client_exit_code(error: &ClientError) -> u8 {
    match error {
        ClientError::Connection(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            ) =>
        {
            EXIT_TIMEOUT
        }
        ClientError::Connection(_) => EXIT_CONNECTION,
        ClientError::Encoding(_)
        | ClientError::Protocol(_)
        | ClientError::ResponseTooLarge
        | ClientError::UnexpectedResponseId { .. } => EXIT_USAGE_OR_PROTOCOL,
    }
}

enum Failure {
    Usage(String),
    Client(ClientError),
    Server(agtk::control::ControlError),
    Timeout(String),
}

#[cfg(test)]
mod tests {
    use super::hook_conversation_id;

    #[test]
    fn hook_input_yields_the_claude_session_id() {
        let input = br#"{"session_id":"a3030a89-3680","hook_event_name":"Stop"}"#;
        assert_eq!(
            hook_conversation_id(&input[..]).as_deref(),
            Some("a3030a89-3680")
        );
        assert_eq!(hook_conversation_id(&b"not json"[..]), None);
        assert_eq!(hook_conversation_id(&br#"{"session_id":""}"#[..]), None);
    }
}
