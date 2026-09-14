use std::env;
use std::process::ExitCode;

use agmux_native::control::{
    ClientError, CloseSessionParams, ControlClient, ControlCommand, ControlRequest,
    CreateSessionParams, GetTextParams, PROTOCOL_VERSION, RenameSessionParams, ResponseBody,
    SendInputParams, SessionIdParams, SessionKind,
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

    let command = match arguments.as_slice() {
        [command] if command == "state" => ControlCommand::AppGetState,
        [group, command] if group == "ui" && command == "inspect" => ControlCommand::UiInspect,
        [group, action, rest @ ..] if group == "session" => parse_session_command(action, rest)?,
        _ => {
            return Err(Failure::Usage(usage().to_owned()));
        }
    };
    let paths = InstancePaths::from_environment(instance);
    let client = ControlClient::new(paths.control_socket());
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

fn usage_failure() -> Failure {
    Failure::Usage(usage().to_owned())
}

fn usage() -> &'static str {
    "usage: agmuxctl [--instance NAME] <state|ui inspect|session create OPTIONS|session select ID|session input ID --text TEXT [--no-enter]|session text ID [--lines N]|session rename ID --name NAME|session close ID [--allow-missing]>"
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
        ClientError::Encoding(_) | ClientError::Protocol(_) | ClientError::ResponseTooLarge => {
            EXIT_USAGE_OR_PROTOCOL
        }
    }
}

enum Failure {
    Usage(String),
    Client(ClientError),
    Server(agmux_native::control::ControlError),
}
