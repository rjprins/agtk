use std::path::PathBuf;
use std::process::ExitCode;

use agmux_native::session::run_session_host;
use nix::unistd::setsid;

fn main() -> ExitCode {
    match parse_args().and_then(|(socket_path, command)| {
        let _ = setsid();
        run_session_host(&socket_path, &command)
    }) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("agmux-session: {error}");
            ExitCode::FAILURE
        }
    }
}

fn parse_args() -> std::io::Result<(PathBuf, Vec<String>)> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("--socket") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "usage: agmux-session --socket PATH -- COMMAND [ARG...]",
        ));
    }
    let socket_path = args.next().map(PathBuf::from).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing socket path")
    })?;
    if args.next().as_deref() != Some("--") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "expected -- before command",
        ));
    }
    let command = args.collect::<Vec<_>>();
    if command.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "missing command",
        ));
    }
    Ok((socket_path, command))
}
