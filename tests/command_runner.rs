use std::process::Command;
use std::time::{Duration, Instant};

use agtk::command_runner::run_bounded;

#[test]
fn runner_returns_bounded_text_output() {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "printf stdout; printf stderr >&2"]);
    let output = run_bounded(command, Duration::from_secs(2), 64).unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, "stdout");
    assert_eq!(output.stderr, "stderr");
}

#[test]
fn runner_rejects_excess_output() {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "printf 123456"]);
    let error = run_bounded(command, Duration::from_secs(2), 4)
        .unwrap_err()
        .to_string();
    assert!(error.contains("output exceeded"));
}

#[test]
fn runner_enforces_a_process_group_timeout() {
    let started = Instant::now();
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "sleep 30 & wait"]);
    let error = run_bounded(command, Duration::from_millis(100), 64)
        .unwrap_err()
        .to_string();
    assert!(error.contains("timed out"));
    assert!(started.elapsed() < Duration::from_secs(3));
}
