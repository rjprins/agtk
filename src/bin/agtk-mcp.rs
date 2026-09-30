use std::env;
use std::io;
use std::process::ExitCode;

use agtk::control::ControlClient;
use agtk::instance::{InstanceName, InstancePaths};
use agtk::mcp::{McpServer, SocketControlBackend};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("agtk-mcp: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args().skip(1);
    let instance = match arguments.next() {
        None => InstanceName::from_environment()?,
        Some(option) if option == "--instance" => {
            let value = arguments.next().ok_or("--instance requires a value")?;
            if arguments.next().is_some() {
                return Err("unexpected argument after --instance".into());
            }
            InstanceName::parse(&value)?
        }
        Some(argument) => return Err(format!("unexpected argument: {argument}").into()),
    };
    let paths = InstancePaths::from_environment(instance);
    let backend = SocketControlBackend::new(ControlClient::new(paths.control_socket()));
    McpServer::new(backend).serve(io::stdin().lock(), io::stdout().lock())?;
    Ok(())
}
