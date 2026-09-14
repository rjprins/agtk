use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::thread;

use agmux_native::control::{ControlCommand, ControlResponse, ControlServer};
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
