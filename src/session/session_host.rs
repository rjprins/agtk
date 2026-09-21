use std::ffi::CString;
use std::io::{self, Read};
use std::os::fd::AsFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use nix::errno::Errno;
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use nix::pty::{ForkptyResult, Winsize, forkpty};
use nix::sys::signal::{Signal, killpg};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::{Pid, dup, execvpe};

use super::{ReplayBuffer, send_attachment};

const REPLAY_CAPACITY: usize = 1024 * 1024;
// A client that stops reading must not stall the host, which also drains the PTY.
const ATTACH_WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const SOCKET_CHECK_INTERVAL: Duration = Duration::from_secs(1);

pub fn run_session_host(socket_path: &Path, command: &[String]) -> io::Result<()> {
    let command = prepare_command(command)?;
    let environment = prepare_environment()?;
    let mut listener = bind_listener(socket_path)?;
    let _socket_guard = SocketGuard(socket_path.to_path_buf());
    let winsize = Winsize {
        ws_row: 30,
        ws_col: 120,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    // SAFETY: the child branch calls only execvpe and _exit with data prepared before fork.
    let fork = unsafe { forkpty(&winsize, None) }.map_err(io::Error::from)?;
    let ForkptyResult::Parent { child, master } = fork else {
        // SAFETY: restoring a default disposition is async-signal-safe. The host ignores
        // SIGHUP, and ignored signals would otherwise survive exec.
        unsafe { libc::signal(libc::SIGHUP, libc::SIG_DFL) };
        let _ = execvpe(&command[0], &command, &environment);
        // SAFETY: exec failed and _exit is async-signal-safe.
        unsafe { libc::_exit(127) }
    };
    let mut child_guard = ChildGuard::new(child);

    let flags = OFlag::from_bits_truncate(fcntl(&master, FcntlArg::F_GETFL)?);
    fcntl(&master, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))?;

    let mut replay = ReplayBuffer::new(REPLAY_CAPACITY);
    let mut next_socket_check = Instant::now() + SOCKET_CHECK_INTERVAL;

    // Like a tmux server, only the child exiting or an explicit kill ends the host.
    // Every other failure drops the client or retries, because exiting kills the agent.
    loop {
        if child_has_exited(child) {
            child_guard.disarm();
            return Ok(());
        }
        if Instant::now() >= next_socket_check {
            next_socket_check = Instant::now() + SOCKET_CHECK_INTERVAL;
            if !socket_path.exists()
                && let Ok(rebound) = bind_listener(socket_path)
            {
                listener = rebound;
            }
        }

        match listener.accept() {
            Ok((client, _)) => {
                drain_output(&master, &mut replay);
                let Ok(client_pty) = dup(&master) else {
                    continue;
                };
                let detached_output = replay.take();
                let sent = client
                    .set_write_timeout(Some(ATTACH_WRITE_TIMEOUT))
                    .and_then(|()| send_attachment(&client, &detached_output, client_pty.as_fd()));
                match sent {
                    Ok(()) if monitor_client(&client, child) => {
                        terminate_child(child);
                        child_guard.disarm();
                        return Ok(());
                    }
                    Ok(()) => {}
                    Err(_) => replay.push(&detached_output),
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                drain_output(&master, &mut replay);
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => {
                drain_output(&master, &mut replay);
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

fn bind_listener(socket_path: &Path) -> io::Result<UnixListener> {
    let listener = UnixListener::bind(socket_path)?;
    std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

fn prepare_environment() -> io::Result<Vec<CString>> {
    let wayland_display = std::env::var("WAYLAND_DISPLAY").ok();
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    let prefer_wayland_clipboard =
        wayland_clipboard_socket(wayland_display.as_deref(), runtime_dir.as_deref()).is_some();
    let mut environment = std::env::vars_os()
        .filter(|(key, _)| {
            let is_terminal_override = matches!(
                key.as_bytes(),
                b"TERM"
                    | b"COLORTERM"
                    | b"NO_COLOR"
                    | b"TERM_PROGRAM"
                    | b"TERM_PROGRAM_VERSION"
                    | b"TMUX"
                    | b"TMUX_PANE"
            );
            let is_x11_clipboard_variable =
                key.as_bytes() == b"DISPLAY" || key.as_bytes() == b"XAUTHORITY";
            !is_terminal_override && !(prefer_wayland_clipboard && is_x11_clipboard_variable)
        })
        .map(|(key, value)| {
            let mut entry = key.as_bytes().to_vec();
            entry.push(b'=');
            entry.extend_from_slice(value.as_bytes());
            CString::new(entry).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "environment contains a NUL byte",
                )
            })
        })
        .collect::<io::Result<Vec<_>>>()?;
    environment.push(CString::new("TERM=xterm-256color").expect("static environment variable"));
    environment.push(CString::new("COLORTERM=truecolor").expect("static environment variable"));
    environment
        .push(CString::new("TERM_PROGRAM=agmux-native").expect("static environment variable"));
    environment.push(
        CString::new(format!(
            "TERM_PROGRAM_VERSION={}",
            env!("CARGO_PKG_VERSION")
        ))
        .expect("package version cannot contain a NUL byte"),
    );
    Ok(environment)
}

fn wayland_clipboard_socket(display: Option<&str>, runtime_dir: Option<&Path>) -> Option<PathBuf> {
    let display = Path::new(display?);
    let socket = if display.is_absolute() {
        display.to_owned()
    } else {
        runtime_dir?.join(display)
    };
    socket.exists().then_some(socket)
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

// Best effort: output is kept for replay, and a failed read is retried next tick.
fn drain_output(master: &impl AsFd, replay: &mut ReplayBuffer) {
    let mut chunk = [0_u8; 16 * 1024];
    while let Ok(count @ 1..) = nix::unistd::read(master, &mut chunk) {
        replay.push(&chunk[..count]);
    }
}

/// Returns true when the client asked to kill the session; any client failure is a detach.
fn monitor_client(mut client: &UnixStream, child: Pid) -> bool {
    if client
        .set_read_timeout(Some(Duration::from_millis(100)))
        .is_err()
    {
        return false;
    }
    let mut command = [0_u8; 1];
    loop {
        match client.read(&mut command) {
            Ok(0) => return false,
            Ok(_) if command[0] == b'K' => return true,
            Ok(_) => {}
            // Thawing after suspend interrupts this timed read.
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) =>
            {
                if child_has_exited(child) {
                    return false;
                }
            }
            Err(_) => return false,
        }
    }
}

fn child_has_exited(child: Pid) -> bool {
    !matches!(
        waitpid(child, Some(WaitPidFlag::WNOHANG)),
        Ok(WaitStatus::StillAlive) | Err(Errno::EINTR)
    )
}

fn terminate_child(child: Pid) {
    for signal in [Signal::SIGHUP, Signal::SIGTERM] {
        let _ = killpg(child, signal);
        if wait_for_child(child, Duration::from_millis(300)) {
            return;
        }
    }
    let _ = killpg(child, Signal::SIGKILL);
    while waitpid(child, None) == Err(Errno::EINTR) {}
}

fn wait_for_child(child: Pid, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if child_has_exited(child) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

struct ChildGuard(Option<Pid>);

impl ChildGuard {
    fn new(child: Pid) -> Self {
        Self(Some(child))
    }
    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(child) = self.0 {
            terminate_child(child);
        }
    }
}

struct SocketGuard(PathBuf);

impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::wayland_clipboard_socket;

    #[test]
    fn resolves_a_named_wayland_socket_from_the_runtime_directory() {
        let runtime = tempfile::tempdir().expect("create runtime directory");
        let socket = runtime.path().join("wayland-0");
        fs::write(&socket, []).expect("create socket marker");

        assert_eq!(
            wayland_clipboard_socket(Some("wayland-0"), Some(runtime.path())),
            Some(socket)
        );
    }

    #[test]
    fn resolves_an_absolute_wayland_socket_without_a_runtime_directory() {
        let runtime = tempfile::tempdir().expect("create runtime directory");
        let socket = runtime.path().join("custom-wayland");
        fs::write(&socket, []).expect("create socket marker");

        assert_eq!(
            wayland_clipboard_socket(Some(socket.to_str().expect("UTF-8 socket path")), None),
            Some(socket)
        );
    }
}
