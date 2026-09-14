use std::ffi::CString;
use std::io::{self, Read};
use std::os::fd::AsFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use nix::errno::Errno;
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use nix::pty::{ForkptyResult, Winsize, forkpty};
use nix::sys::signal::{Signal, killpg};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::{Pid, dup, execvp};

use super::{ReplayBuffer, send_attachment};

const REPLAY_CAPACITY: usize = 1024 * 1024;

pub fn run_session_host(socket_path: &Path, command: &[String]) -> io::Result<()> {
    let command = prepare_command(command)?;
    let winsize = Winsize {
        ws_row: 30,
        ws_col: 120,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    // SAFETY: the child branch calls only execvp and _exit with data prepared before fork.
    let fork = unsafe { forkpty(&winsize, None) }.map_err(io::Error::from)?;
    let ForkptyResult::Parent { child, master } = fork else {
        let _ = execvp(&command[0], &command);
        // SAFETY: exec failed and _exit is async-signal-safe.
        unsafe { libc::_exit(127) }
    };

    let flags = OFlag::from_bits_truncate(fcntl(&master, FcntlArg::F_GETFL)?);
    fcntl(&master, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))?;

    let listener = UnixListener::bind(socket_path)?;
    listener.set_nonblocking(true)?;
    let _socket_guard = SocketGuard(socket_path.to_path_buf());
    let mut replay = ReplayBuffer::new(REPLAY_CAPACITY);

    loop {
        if child_has_exited(child)? {
            return Ok(());
        }

        match listener.accept() {
            Ok((client, _)) => {
                drain_output(&master, &mut replay)?;
                let client_pty = dup(&master).map_err(io::Error::from)?;
                let detached_output = replay.take();
                match send_attachment(&client, &detached_output, client_pty.as_fd()) {
                    Ok(()) if monitor_client(&client, child)? => {
                        let _ = killpg(child, Signal::SIGHUP);
                        let _ = waitpid(child, None);
                        return Ok(());
                    }
                    Ok(()) => {}
                    Err(_) => replay.push(&detached_output),
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                drain_output(&master, &mut replay)?;
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error),
        }
    }
}

fn prepare_command(command: &[String]) -> io::Result<Vec<CString>> {
    if command.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "missing command",
        ));
    }
    command
        .iter()
        .map(|part| {
            CString::new(part.as_bytes()).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "command contains a NUL byte")
            })
        })
        .collect()
}

fn drain_output(master: &impl AsFd, replay: &mut ReplayBuffer) -> io::Result<()> {
    let mut chunk = [0_u8; 16 * 1024];
    loop {
        match nix::unistd::read(master, &mut chunk) {
            Ok(0) => return Ok(()),
            Ok(count) => replay.push(&chunk[..count]),
            Err(Errno::EAGAIN) => return Ok(()),
            Err(Errno::EIO) => return Ok(()),
            Err(error) => return Err(io::Error::from(error)),
        }
    }
}

fn monitor_client(mut client: &UnixStream, child: Pid) -> io::Result<bool> {
    client.set_read_timeout(Some(Duration::from_millis(100)))?;
    let mut command = [0_u8; 1];
    loop {
        match client.read(&mut command) {
            Ok(0) => return Ok(false),
            Ok(_) if command[0] == b'K' => return Ok(true),
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                if child_has_exited(child)? {
                    return Ok(false);
                }
            }
            Err(error) => return Err(error),
        }
    }
}

fn child_has_exited(child: Pid) -> io::Result<bool> {
    match waitpid(child, Some(WaitPidFlag::WNOHANG)) {
        Ok(WaitStatus::StillAlive) => Ok(false),
        Ok(_) | Err(Errno::ECHILD) => Ok(true),
        Err(error) => Err(io::Error::from(error)),
    }
}

struct SocketGuard(PathBuf);

impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
