use std::env;
use std::process::ExitCode;

use agmux_native::control::{
    ClientError, ControlClient, ControlCommand, ControlRequest, PROTOCOL_VERSION, ResponseBody,
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
        _ => {
            return Err(Failure::Usage(
                "usage: agmuxctl [--instance NAME] state".to_owned(),
            ));
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
