use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, mpsc::RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::Value;

use super::protocol::MAX_REQUEST_BYTES;
use super::{
    ControlError, ControlRequest, ControlResponse, ErrorCode, control_timeout, decode_request,
    encode_response,
};
use crate::instance::ensure_private_dir;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// Beyond this many open requests, new callers wait in the listen backlog.
const MAX_ACTIVE_CLIENTS: usize = 32;

pub struct PendingRequest {
    pub request: ControlRequest,
    response: mpsc::SyncSender<ControlResponse>,
}

impl PendingRequest {
    pub fn respond(self, response: ControlResponse) -> Result<(), ControlResponse> {
        self.response.send(response).map_err(|error| error.0)
    }
}

pub struct ControlServer {
    socket_path: PathBuf,
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ControlServer {
    pub fn bind(socket_path: &Path) -> io::Result<(Self, Receiver<PendingRequest>)> {
        let parent = socket_path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "control socket has no parent")
        })?;
        ensure_private_dir(parent)?;
        prepare_socket_path(socket_path)?;

        let listener = UnixListener::bind(socket_path)?;
        fs::set_permissions(socket_path, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;

        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_shutdown = shutdown.clone();
        let (requests, receiver) = mpsc::channel();
        let thread = thread::spawn(move || serve(listener, requests, thread_shutdown));

        Ok((
            Self {
                socket_path: socket_path.to_owned(),
                shutdown,
                thread: Some(thread),
            },
            receiver,
        ))
    }
}

fn prepare_socket_path(socket_path: &Path) -> io::Result<()> {
    match UnixStream::connect(socket_path) {
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            "another control server is already listening",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {
            match fs::symlink_metadata(socket_path) {
                Ok(metadata) if metadata.file_type().is_socket() => fs::remove_file(socket_path),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Ok(_) => Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "control socket path exists and is not a Unix socket",
                )),
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(error),
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        let _ = UnixStream::connect(&self.socket_path);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = fs::remove_file(&self.socket_path);
    }
}

fn serve(listener: UnixListener, requests: Sender<PendingRequest>, shutdown: Arc<AtomicBool>) {
    let active = Arc::new(AtomicUsize::new(0));
    while !shutdown.load(Ordering::Acquire) {
        if active.load(Ordering::Acquire) >= MAX_ACTIVE_CLIENTS {
            thread::sleep(Duration::from_millis(10));
            continue;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                let requests = requests.clone();
                let slot = ActiveClient::claim(&active);
                // A slow request, such as a PR lookup, must not hold up agent state hooks.
                let _ = thread::Builder::new()
                    .name("agtk-control".to_owned())
                    .spawn(move || {
                        let _slot = slot;
                        let _ = handle_client(stream, &requests);
                    });
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => break,
        }
    }
}

/// Counts an open request until dropped, also when its thread fails to start.
struct ActiveClient(Arc<AtomicUsize>);

impl ActiveClient {
    fn claim(active: &Arc<AtomicUsize>) -> Self {
        active.fetch_add(1, Ordering::AcqRel);
        Self(active.clone())
    }
}

impl Drop for ActiveClient {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn handle_client(mut stream: UnixStream, requests: &Sender<PendingRequest>) -> io::Result<()> {
    stream.set_read_timeout(Some(REQUEST_TIMEOUT))?;
    stream.set_write_timeout(Some(REQUEST_TIMEOUT))?;
    let (bytes, newline_terminated) = read_request(&stream)?;
    let request_id = request_id_hint(&bytes);
    if !newline_terminated && bytes.len() <= MAX_REQUEST_BYTES {
        let error = ControlError::new(
            ErrorCode::InvalidRequest,
            "Control request is not newline terminated",
        );
        return write_response(&mut stream, &ControlResponse::from_error(request_id, error));
    }
    let request = match decode_request(&bytes) {
        Ok(request) => request,
        Err(error) => {
            return write_response(&mut stream, &ControlResponse::from_error(request_id, error));
        }
    };
    let response_timeout = control_timeout(&request.command);

    let (response_sender, response_receiver) = mpsc::sync_channel(1);
    let response_id = request.id.clone();
    requests
        .send(PendingRequest {
            request,
            response: response_sender,
        })
        .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "request receiver closed"))?;

    let response = match response_receiver.recv_timeout(response_timeout) {
        Ok(response) => response,
        Err(RecvTimeoutError::Timeout) => ControlResponse::failure(
            response_id,
            ErrorCode::InternalError,
            "Application did not respond before the deadline",
            None,
        ),
        Err(RecvTimeoutError::Disconnected) => ControlResponse::failure(
            response_id,
            ErrorCode::InternalError,
            "Application request handler disconnected",
            None,
        ),
    };
    write_response(&mut stream, &response)
}

fn read_request(stream: &UnixStream) -> io::Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    let mut reader = BufReader::new(stream).take((MAX_REQUEST_BYTES + 2) as u64);
    reader.read_until(b'\n', &mut bytes)?;
    let newline_terminated = bytes.ends_with(b"\n");
    if newline_terminated {
        bytes.pop();
    }
    Ok((bytes, newline_terminated))
}

fn request_id_hint(bytes: &[u8]) -> String {
    serde_json::from_slice::<Value>(bytes)
        .ok()
        .and_then(|value| value.get("id")?.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned())
}

fn write_response(stream: &mut UnixStream, response: &ControlResponse) -> io::Result<()> {
    let encoded = encode_response(response).map_err(io::Error::other)?;
    stream.write_all(encoded.as_bytes())
}
