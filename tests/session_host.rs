use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use agmux_native::session::receive_attachment;
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;

#[test]
fn session_host_survives_disconnect_and_accepts_reattachment() {
    let directory = tempfile::tempdir().expect("create runtime directory");
    let socket_path = directory.path().join("session.sock");
    let script = concat!(
        "printf 'env:%s:%s\\n' \"$TERM\" \"$COLORTERM\"; ",
        "printf 'color-vars:%s:%s:%s\\n' \"${NO_COLOR-unset}\" \"$TERM_PROGRAM\" \"${TMUX-unset}\"; ",
        "printf 'ready\\n'; ",
        "IFS= read -r first; printf 'got:%s\\n' \"$first\"; ",
        "sleep 0.2; printf 'detached\\n'; ",
        "IFS= read -r second; printf 'again:%s\\n' \"$second\"; sleep 30"
    );

    let mut host = Command::new(env!("CARGO_BIN_EXE_agmux-session"))
        .args(["--socket", socket_path.to_str().expect("UTF-8 socket path")])
        .args(["--", "/bin/sh", "-c", script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .env("NO_COLOR", "1")
        .env("TERM_PROGRAM", "tmux")
        .env("TMUX", "/tmp/user-tmux,1,0")
        .spawn()
        .expect("start session host");

    wait_for_socket(&socket_path, &mut host);
    assert_eq!(
        std::fs::metadata(&socket_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );

    let first_control = UnixStream::connect(&socket_path).expect("connect first client");
    let first_attachment = receive_attachment(&first_control).expect("attach first client");
    let mut first_pty = File::from(first_attachment.pty);
    let mut initial_output = String::from_utf8_lossy(&first_attachment.replay).into_owned();
    if !initial_output.contains("ready") {
        initial_output.push_str(&read_until(&mut first_pty, "ready", Duration::from_secs(2)));
    }
    assert!(initial_output.contains("env:xterm-256color:truecolor"));
    assert!(initial_output.contains("color-vars:unset:agmux-native:unset"));
    first_pty.write_all(b"one\n").expect("write first input");
    assert!(read_until(&mut first_pty, "got:one", Duration::from_secs(2)).contains("got:one"));

    drop(first_pty);
    drop(first_control);
    thread::sleep(Duration::from_millis(350));

    let mut second_control = UnixStream::connect(&socket_path).expect("connect second client");
    let second_attachment = receive_attachment(&second_control).expect("attach second client");
    assert!(String::from_utf8_lossy(&second_attachment.replay).contains("detached"));

    let mut second_pty = File::from(second_attachment.pty);
    second_pty.write_all(b"two\n").expect("write second input");
    assert!(read_until(&mut second_pty, "again:two", Duration::from_secs(2)).contains("again:two"));

    second_control.write_all(b"K").expect("request shutdown");
    wait_for_exit(&mut host, Duration::from_secs(2));
}

#[test]
fn session_host_escalates_shutdown_for_a_signal_resistant_child() {
    let directory = tempfile::tempdir().expect("create runtime directory");
    let socket_path = directory.path().join("stubborn.sock");
    let script = "trap '' HUP TERM; printf 'child:%s\\n' \"$$\"; while :; do sleep 10; done";
    let mut host = Command::new(env!("CARGO_BIN_EXE_agmux-session"))
        .args([
            "--socket",
            socket_path.to_str().unwrap(),
            "--",
            "/bin/sh",
            "-c",
            script,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_for_socket(&socket_path, &mut host);
    let mut control = UnixStream::connect(&socket_path).unwrap();
    let attachment = receive_attachment(&control).unwrap();
    let mut pty = File::from(attachment.pty);
    let mut output = String::from_utf8_lossy(&attachment.replay).into_owned();
    if !output.contains("child:") {
        output.push_str(&read_until(&mut pty, "child:", Duration::from_secs(2)));
    }
    let child = output
        .lines()
        .find_map(|line| line.strip_prefix("child:"))
        .and_then(|pid| pid.trim().parse::<i32>().ok())
        .expect("child PID");
    control.write_all(b"K").unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if host.try_wait().unwrap().is_some() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let _ = host.kill();
    let _ = killpg(Pid::from_raw(child), Signal::SIGKILL);
    let _ = host.wait();
    panic!("session host did not escalate shutdown");
}

fn wait_for_socket(path: &Path, host: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if path.exists() {
            return;
        }
        assert!(
            host.try_wait().expect("check host status").is_none(),
            "host exited before creating its socket"
        );
        thread::sleep(Duration::from_millis(10));
    }
    panic!("session socket was not created");
}

fn read_until(file: &mut File, needle: &str, timeout: Duration) -> String {
    let deadline = Instant::now() + timeout;
    let mut output = Vec::new();
    while Instant::now() < deadline {
        let mut descriptors = [PollFd::new(file.as_fd(), PollFlags::POLLIN)];
        if poll(&mut descriptors, PollTimeout::from(50_u16)).expect("poll PTY") == 0 {
            continue;
        }
        let mut chunk = [0_u8; 4096];
        match file.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => output.extend_from_slice(&chunk[..count]),
            Err(error) if error.raw_os_error() == Some(libc::EIO) => break,
            Err(error) => panic!("read PTY: {error}"),
        }
        if String::from_utf8_lossy(&output).contains(needle) {
            return String::from_utf8_lossy(&output).into_owned();
        }
    }
    panic!(
        "did not read {needle:?}; output was {:?}",
        String::from_utf8_lossy(&output)
    );
}

fn wait_for_exit(child: &mut Child, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if child.try_wait().expect("check host status").is_some() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    child.kill().expect("kill stuck host");
    panic!("session host did not exit");
}
