use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use agtk::session::receive_attachment;
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;

#[test]
fn session_host_preserves_x11_clipboard_access_on_wayland() {
    let directory = tempfile::tempdir().expect("create runtime directory");
    let wayland_socket = directory.path().join("wayland-test");
    let _wayland = UnixListener::bind(&wayland_socket).expect("bind Wayland socket");
    let authority = directory.path().join("xauthority");

    for display in [Path::new("wayland-test"), wayland_socket.as_path()] {
        let socket_path = directory.path().join("session.sock");
        let script = concat!(
            "printf 'x11:%s:%s\\n' \"${DISPLAY-unset}\" \"${XAUTHORITY-unset}\"; ",
            "printf 'wayland:%s:%s\\n' \"$WAYLAND_DISPLAY\" \"$XDG_RUNTIME_DIR\"; ",
            "printf 'ready\\n'; sleep 30"
        );
        let mut host = Command::new(env!("CARGO_BIN_EXE_agtk-session"))
            .args(["--socket", socket_path.to_str().unwrap()])
            .args(["--", "/bin/sh", "-c", script])
            .env("DISPLAY", ":99")
            .env("XAUTHORITY", &authority)
            .env("WAYLAND_DISPLAY", display)
            .env("XDG_RUNTIME_DIR", directory.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start session host");

        wait_for_socket(&socket_path, &mut host);
        let mut control = UnixStream::connect(&socket_path).expect("connect client");
        let attachment = receive_attachment(&control).expect("attach client");
        let mut output = String::from_utf8_lossy(&attachment.replay).into_owned();
        let mut pty = File::from(attachment.pty);
        if !output.contains("ready") {
            output.push_str(&read_until(&mut pty, "ready", Duration::from_secs(2)));
        }
        control.write_all(b"K").expect("request shutdown");
        wait_for_exit(&mut host, Duration::from_secs(2));

        assert!(
            output.contains(&format!("x11::99:{}", authority.display())),
            "X11 clipboard environment was lost: {output:?}"
        );
        assert!(
            output.contains(&format!(
                "wayland:{}:{}",
                display.display(),
                directory.path().display()
            )),
            "Wayland clipboard environment was lost: {output:?}"
        );
    }
}

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

    let mut host = Command::new(env!("CARGO_BIN_EXE_agtk-session"))
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
    assert!(initial_output.contains("color-vars:unset:agtk:unset"));
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
    let mut host = Command::new(env!("CARGO_BIN_EXE_agtk-session"))
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

#[test]
fn session_host_ignores_sighup_but_its_child_does_not() {
    let directory = tempfile::tempdir().expect("create runtime directory");
    let socket_path = directory.path().join("hup.sock");
    let script = "printf 'sigign:%s\\n' \"$(grep SigIgn /proc/$$/status)\"; sleep 30";
    let mut host = spawn_host(&socket_path, script);
    wait_for_socket(&socket_path, &mut host);

    nix::sys::signal::kill(Pid::from_raw(host.id() as i32), Signal::SIGHUP).unwrap();
    thread::sleep(Duration::from_millis(200));
    assert!(host.try_wait().unwrap().is_none(), "host died on SIGHUP");

    let mut control = UnixStream::connect(&socket_path).unwrap();
    let attachment = receive_attachment(&control).unwrap();
    let mut output = String::from_utf8_lossy(&attachment.replay).into_owned();
    let mut pty = File::from(attachment.pty);
    if !output.contains("sigign:") {
        output.push_str(&read_until(&mut pty, "sigign:", Duration::from_secs(2)));
    }
    let mask = output
        .lines()
        .find_map(|line| line.split("SigIgn:").nth(1))
        .map(str::trim)
        .and_then(|mask| u64::from_str_radix(mask, 16).ok())
        .expect("child SigIgn mask");
    assert_eq!(mask & 1, 0, "child inherited an ignored SIGHUP");

    control.write_all(b"K").unwrap();
    wait_for_exit(&mut host, Duration::from_secs(2));
}

#[test]
fn session_host_recreates_a_deleted_socket() {
    let directory = tempfile::tempdir().expect("create runtime directory");
    let socket_path = directory.path().join("gone.sock");
    let mut host = spawn_host(&socket_path, "printf 'alive\\n'; sleep 30");
    wait_for_socket(&socket_path, &mut host);

    std::fs::remove_file(&socket_path).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !socket_path.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    assert!(socket_path.exists(), "socket was not recreated");

    let mut control = UnixStream::connect(&socket_path).unwrap();
    let attachment = receive_attachment(&control).unwrap();
    let mut output = String::from_utf8_lossy(&attachment.replay).into_owned();
    if !output.contains("alive") {
        let mut pty = File::from(attachment.pty);
        output.push_str(&read_until(&mut pty, "alive", Duration::from_secs(2)));
    }
    assert!(output.contains("alive"));
    control.write_all(b"K").unwrap();
    wait_for_exit(&mut host, Duration::from_secs(2));
}

#[test]
fn session_host_drops_a_client_that_stops_reading() {
    let directory = tempfile::tempdir().expect("create runtime directory");
    let socket_path = directory.path().join("stuck.sock");
    // More replay than a socket buffer holds, so the stuck client blocks the send.
    let script = "head -c 900000 /dev/zero | tr '\\0' x; printf '\\nend\\n'; sleep 30";
    let mut host = spawn_host(&socket_path, script);
    wait_for_socket(&socket_path, &mut host);
    thread::sleep(Duration::from_millis(500));

    let stuck = UnixStream::connect(&socket_path).unwrap();
    thread::sleep(Duration::from_millis(100));

    let mut control = UnixStream::connect(&socket_path).unwrap();
    control
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let attachment = receive_attachment(&control).expect("attach after stuck client");
    assert!(String::from_utf8_lossy(&attachment.replay).contains("end"));
    drop(stuck);
    control.write_all(b"K").unwrap();
    wait_for_exit(&mut host, Duration::from_secs(2));
}

fn spawn_host(socket_path: &Path, script: &str) -> Child {
    Command::new(env!("CARGO_BIN_EXE_agtk-session"))
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
        .unwrap()
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
