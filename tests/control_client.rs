use std::thread;

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
