use std::env;
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

use agmux_native::appearance::ThemeKey;
use agmux_native::control::{
    AppearanceSetParams, ClientError, CloseSessionParams, ControlClient, ControlCommand,
    ControlRequest, CreateSessionParams, ErrorCode, GetTextParams, PROTOCOL_VERSION,
    RenameSessionParams, ResponseBody, SendInputParams, SessionIdParams, SessionKind, SessionState,
    WaitCondition,
};
use agmux_native::instance::{InstanceName, InstancePaths};

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
        [group, action, rest @ ..] if group == "appearance" && action == "set" => {
            parse_appearance_set(rest)?
        }
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
            .map_err(Failure::Client)?;
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
        "exited" => Ok(SessionState::Exited),
        "reconnecting" => Ok(SessionState::Reconnecting),
        _ => Err(Failure::Usage(format!("unknown session state: {value}"))),
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
        "custom" => Ok(SessionKind::Custom),
        _ => Err(Failure::Usage(format!("unknown session kind: {value}"))),
    }
}

fn parse_appearance_set(arguments: &[String]) -> Result<ControlCommand, Failure> {
    let mut theme = None;
    let mut follow_system = None;
    let mut font = None;
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
            _ => {
                return Err(Failure::Usage(format!(
                    "unknown appearance set option: {option}"
                )));
            }
        }
        index += 2;
    }
    if theme.is_none() && follow_system.is_none() && font.is_none() {
        return Err(Failure::Usage(
            "appearance set requires at least one option".to_owned(),
        ));
    }
    Ok(ControlCommand::AppearanceSet(AppearanceSetParams {
        theme,
        follow_system,
        font,
    }))
}

fn parse_theme(value: &str) -> Result<ThemeKey, Failure> {
    ThemeKey::ALL
        .into_iter()
        .find(|key| key.as_str() == value)
        .ok_or_else(|| Failure::Usage(format!("unknown theme: {value}")))
}

fn usage_failure() -> Failure {
    Failure::Usage(usage().to_owned())
}

fn usage() -> &'static str {
    "usage: agmuxctl [--instance NAME] <state|ui inspect|ui capture|appearance set OPTIONS|session create OPTIONS|session select ID|session input ID --text TEXT [--no-enter]|session text ID [--lines N]|session rename ID --name NAME|session close ID [--allow-missing]|wait OPTIONS>"
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
    Server(agmux_native::control::ControlError),
    Timeout(String),
}
