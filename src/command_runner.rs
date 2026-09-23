use std::fmt;
use std::io::{self, Read};
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedOutput {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedByteOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Debug)]
pub enum CommandError {
    Io(io::Error),
    TimedOut(Duration),
    OutputExceeded(usize),
    ReaderPanicked,
}

impl fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "command I/O failed: {error}"),
            Self::TimedOut(timeout) => {
                write!(
                    formatter,
                    "command timed out after {} ms",
                    timeout.as_millis()
                )
            }
            Self::OutputExceeded(limit) => {
                write!(formatter, "command output exceeded {limit} bytes")
            }
            Self::ReaderPanicked => formatter.write_str("command output reader stopped"),
        }
    }
}

impl std::error::Error for CommandError {}

impl From<io::Error> for CommandError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub fn run_bounded(
    command: Command,
    timeout: Duration,
    output_limit: usize,
) -> Result<BoundedOutput, CommandError> {
    let output = run_bounded_bytes(command, timeout, output_limit)?;
    Ok(BoundedOutput {
        status: output.status,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

pub fn run_bounded_bytes(
    mut command: Command,
    timeout: Duration,
    output_limit: usize,
) -> Result<BoundedByteOutput, CommandError> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = command.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| CommandError::Io(io::Error::other("command stdout pipe is unavailable")))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| CommandError::Io(io::Error::other("command stderr pipe is unavailable")))?;
    let stdout_reader = thread::spawn(move || read_bounded(stdout, output_limit));
    let stderr_reader = thread::spawn(move || read_bounded(stderr, output_limit));

    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Ok(status);
        }
        if Instant::now() >= deadline {
            terminate_process_group(&mut child);
            break Err(CommandError::TimedOut(timeout));
        }
        thread::sleep(Duration::from_millis(10));
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| CommandError::ReaderPanicked)??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| CommandError::ReaderPanicked)??;
    let status = status?;
    if stdout.exceeded || stderr.exceeded {
        return Err(CommandError::OutputExceeded(output_limit));
    }
    Ok(BoundedByteOutput {
        status,
        stdout: stdout.bytes,
        stderr: stderr.bytes,
    })
}

fn terminate_process_group(child: &mut std::process::Child) {
    let group = Pid::from_raw(child.id().cast_signed());
    let _ = killpg(group, Signal::SIGTERM);
    let deadline = Instant::now() + Duration::from_millis(200);
    while Instant::now() < deadline {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let _ = killpg(group, Signal::SIGKILL);
    let _ = child.wait();
}

struct ReadResult {
    bytes: Vec<u8>,
    exceeded: bool,
}

fn read_bounded(mut reader: impl Read, limit: usize) -> io::Result<ReadResult> {
    let mut bytes = Vec::with_capacity(limit.min(8192));
    let mut exceeded = false;
    let mut chunk = [0_u8; 8192];
    loop {
        let count = reader.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        let remaining = limit.saturating_sub(bytes.len());
        bytes.extend_from_slice(&chunk[..count.min(remaining)]);
        if count > remaining {
            exceeded = true;
        }
    }
    Ok(ReadResult { bytes, exceeded })
}
