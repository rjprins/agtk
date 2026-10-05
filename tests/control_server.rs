use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::thread;
use std::time::Duration;

use agtk::control::{ControlCommand, ControlResponse, ControlServer};
use serde_json::json;

#[test]
fn server_delivers_a_typed_request_and_returns_its_response() {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let socket = directory.path().join("control.sock");
    let (server, requests) = ControlServer::bind(&socket).expect("bind control server");
    let worker = thread::spawn(move || {
        let pending = requests.recv().expect("receive request");
        assert_eq!(pending.request.command, ControlCommand::AppGetState);
        let request_id = pending.request.id.clone();
        pending
            .respond(ControlResponse::success(
                request_id,
                json!({ "instance": "test-1" }),
            ))
            .expect("send response");
    });

    let response = exchange(
        &socket,
        r#"{"version":1,"id":"state-1","method":"app.get_state","params":{}}
"#,
    );

    assert_eq!(
        response,
        r#"{"version":1,"id":"state-1","result":{"instance":"test-1"}}
"#
    );
    worker.join().expect("request worker");
    drop(server);
    assert!(!socket.exists());
}

#[test]
fn server_answers_a_request_while_an_earlier_one_is_still_pending() {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let socket = directory.path().join("control.sock");
    let (_server, requests) = ControlServer::bind(&socket).expect("bind control server");
    let slow_socket = socket.clone();
    let slow = thread::spawn(move || {
        exchange(
            &slow_socket,
            r#"{"version":1,"id":"slow-1","method":"app.get_state","params":{}}
"#,
        )
    });
    let held = requests.recv().expect("receive slow request");
    assert_eq!(held.request.id, "slow-1");

    let worker = thread::spawn(move || {
        // Well inside the 5 second reply deadline that the held request waits on.
        let pending = requests
            .recv_timeout(Duration::from_secs(1))
            .expect("second request arrives while the first is pending");
        let request_id = pending.request.id.clone();
        pending
            .respond(ControlResponse::success(request_id, json!({})))
            .expect("send response");
    });
    let response = exchange(
        &socket,
        r#"{"version":1,"id":"quick-1","method":"app.get_state","params":{}}
"#,
    );
    assert!(response.contains(r#""id":"quick-1""#), "{response}");
    worker.join().expect("request worker");

    let request_id = held.request.id.clone();
    held.respond(ControlResponse::success(request_id, json!({})))
        .expect("answer the slow request");
    assert!(
        slow.join()
            .expect("slow client")
            .contains(r#""id":"slow-1""#)
    );
}

#[test]
fn server_rejects_bad_requests_without_dispatching_them() {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let socket = directory.path().join("control.sock");
    let (_server, requests) = ControlServer::bind(&socket).expect("bind control server");

    let response = exchange(
        &socket,
        r#"{"version":1,"id":"bad-1","method":"unknown","params":{}}
"#,
    );
    let decoded: serde_json::Value = serde_json::from_str(&response).expect("JSON response");

    assert_eq!(decoded["id"], "bad-1");
    assert_eq!(decoded["error"]["code"], "METHOD_NOT_FOUND");
    assert!(requests.try_recv().is_err());
}

#[test]
fn server_returns_json_for_a_request_without_a_newline() {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let socket = directory.path().join("control.sock");
    let (_server, _requests) = ControlServer::bind(&socket).expect("bind control server");
    let mut client = UnixStream::connect(&socket).expect("connect to control server");
    client
        .write_all(br#"{"version":1,"id":"partial-1","method":"app.get_state","params":{}}"#)
        .expect("write unterminated request");
    client
        .shutdown(std::net::Shutdown::Write)
        .expect("finish request bytes");
    let mut response = String::new();
    client.read_to_string(&mut response).expect("read response");
    let decoded: serde_json::Value = serde_json::from_str(&response).expect("JSON response");

    assert_eq!(decoded["id"], "partial-1");
    assert_eq!(decoded["error"]["code"], "INVALID_REQUEST");
}

#[test]
fn server_socket_is_private_to_the_current_user() {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let socket = directory.path().join("nested/control.sock");
    let (_server, _requests) = ControlServer::bind(&socket).expect("bind control server");

    let socket_mode = socket
        .metadata()
        .expect("socket metadata")
        .permissions()
        .mode()
        & 0o777;
    let directory_mode = socket
        .parent()
        .expect("socket parent")
        .metadata()
        .expect("directory metadata")
        .permissions()
        .mode()
        & 0o777;

    assert_eq!(socket_mode, 0o600);
    assert_eq!(directory_mode, 0o700);
}

#[test]
fn server_replaces_a_stale_socket_left_by_an_interrupted_process() {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let socket = directory.path().join("control.sock");
    let stale = UnixListener::bind(&socket).expect("create stale socket");
    drop(stale);

    let (_server, _requests) = ControlServer::bind(&socket).expect("replace stale socket");

    UnixStream::connect(&socket).expect("connect to replacement server");
}

#[test]
fn server_does_not_replace_a_socket_that_is_still_live() {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let socket = directory.path().join("control.sock");
    let (_server, _requests) = ControlServer::bind(&socket).expect("bind first server");

    let error = ControlServer::bind(&socket)
        .err()
        .expect("second server must not replace a live socket");

    assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse);
}

fn exchange(socket: &std::path::Path, request: &str) -> String {
    let mut client = UnixStream::connect(socket).expect("connect to control server");
    client
        .write_all(request.as_bytes())
        .expect("write control request");
    let mut response = String::new();
    client
        .read_to_string(&mut response)
        .expect("read control response");
    response
}

#[test]
fn a_client_that_trickles_bytes_is_dropped_at_the_request_deadline() {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let socket = directory.path().join("control.sock");
    let (_server, _requests) = ControlServer::bind(&socket).expect("bind control server");
    let mut stream = UnixStream::connect(&socket).expect("connect");
    let started = std::time::Instant::now();
    // Each byte arrives well within the per-read timeout, but the request never ends.
    while stream.write_all(b" ").is_ok() && started.elapsed() < Duration::from_secs(12) {
        thread::sleep(Duration::from_millis(500));
        let mut probe = [0u8; 1];
        stream.set_nonblocking(true).unwrap();
        let closed = stream.read(&mut probe).is_ok();
        stream.set_nonblocking(false).unwrap();
        if closed {
            break;
        }
    }
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(8),
        "server kept a trickling client for {elapsed:?}"
    );
}
