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

pub fn run_session_host(socket_path: &Path, command: &[String]) -> io::Result<()> {
    let command = prepare_command(command)?;
    let environment = prepare_environment()?;
    let listener = UnixListener::bind(socket_path)?;
    std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
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
        let _ = execvpe(&command[0], &command, &environment);
        // SAFETY: exec failed and _exit is async-signal-safe.
        unsafe { libc::_exit(127) }
    };
    let mut child_guard = ChildGuard::new(child);

    let flags = OFlag::from_bits_truncate(fcntl(&master, FcntlArg::F_GETFL)?);
    fcntl(&master, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))?;

    let mut replay = ReplayBuffer::new(REPLAY_CAPACITY);

    loop {
        if child_has_exited(child)? {
            child_guard.disarm();
            return Ok(());
        }

        match listener.accept() {
            Ok((client, _)) => {
                drain_output(&master, &mut replay)?;
                let client_pty = dup(&master).map_err(io::Error::from)?;
                let detached_output = replay.take();
                match send_attachment(&client, &detached_output, client_pty.as_fd()) {
                    Ok(()) if monitor_client(&client, child)? => {
                        terminate_child(child)?;
                        child_guard.disarm();
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

fn terminate_child(child: Pid) -> io::Result<()> {
    for signal in [Signal::SIGHUP, Signal::SIGTERM] {
        let _ = killpg(child, signal);
        if wait_for_child(child, Duration::from_millis(300))? {
            return Ok(());
        }
    }
    let _ = killpg(child, Signal::SIGKILL);
    match waitpid(child, None) {
        Ok(_) | Err(Errno::ECHILD) => Ok(()),
        Err(error) => Err(io::Error::from(error)),
    }
}

fn wait_for_child(child: Pid, timeout: Duration) -> io::Result<bool> {
    let deadline = Instant::now() + timeout;
    loop {
        if child_has_exited(child)? {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
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
            let _ = terminate_child(child);
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
