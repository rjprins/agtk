use std::thread;
use std::time::{Duration, Instant};

use agmux_native::control::{
    ControlClient, ControlCommand, ControlRequest, ControlResponse, ControlServer,
    PROTOCOL_VERSION, ResponseBody,
};
use serde_json::json;

#[test]
fn client_exchanges_a_typed_request_with_the_control_server() {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let socket = directory.path().join("control.sock");
    let (_server, requests) = ControlServer::bind(&socket).expect("bind server");
    let worker = thread::spawn(move || {
        let pending = requests.recv().expect("receive request");
        let request_id = pending.request.id.clone();
        pending
            .respond(ControlResponse::success(
                request_id,
                json!({ "instance": "test-client" }),
            ))
            .expect("respond");
    });
    let client = ControlClient::new(socket);

    let response = client
        .send(&ControlRequest {
            version: PROTOCOL_VERSION,
            id: "client-1".to_owned(),
            command: ControlCommand::AppGetState,
        })
        .expect("exchange request");

    assert_eq!(
        response.body,
        ResponseBody::Success(json!({ "instance": "test-client" }))
    );
    worker.join().expect("request worker");
}

#[test]
fn client_timeout_bounds_a_slow_response() {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let socket = directory.path().join("control.sock");
    let (_server, requests) = ControlServer::bind(&socket).expect("bind server");
    let worker = thread::spawn(move || {
        let pending = requests.recv().expect("receive request");
        thread::sleep(Duration::from_millis(250));
        let request_id = pending.request.id.clone();
        let _ = pending.respond(ControlResponse::success(request_id, json!({})));
    });
    let client = ControlClient::new(socket);
    let started = Instant::now();
    let error = client
        .send_with_timeout(
            &ControlRequest {
                version: PROTOCOL_VERSION,
                id: "deadline".to_owned(),
                command: ControlCommand::AppGetState,
            },
            Duration::from_millis(40),
        )
        .expect_err("slow response must time out");

    assert!(error.is_timeout());
    assert!(started.elapsed() < Duration::from_millis(180));
    worker.join().expect("request worker");
}

#[test]
fn client_rejects_a_response_for_another_request() {
    let directory = tempfile::tempdir().expect("create temporary directory");
    let socket = directory.path().join("control.sock");
    let (_server, requests) = ControlServer::bind(&socket).expect("bind server");
    let worker = thread::spawn(move || {
        let pending = requests.recv().expect("receive request");
        pending
            .respond(ControlResponse::success("wrong-id", json!({})))
            .unwrap();
    });
    let client = ControlClient::new(socket);
    let error = client
        .send(&ControlRequest {
            version: PROTOCOL_VERSION,
            id: "expected-id".to_owned(),
            command: ControlCommand::AppGetState,
        })
        .expect_err("mismatched response ID must fail");

    assert!(error.to_string().contains("wrong-id"));
    worker.join().expect("request worker");
}
