use std::fmt;
use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::{ControlError, ControlRequest, ControlResponse, decode_response, encode_request};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct ControlClient {
    socket_path: PathBuf,
    timeout: Duration,
}

impl ControlClient {
    pub fn new(socket_path: PathBuf) -> Self {
        Self {
            socket_path,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    pub fn send(&self, request: &ControlRequest) -> Result<ControlResponse, ClientError> {
        self.send_with_timeout(request, self.timeout)
    }

    pub fn send_with_timeout(
        &self,
        request: &ControlRequest,
        timeout: Duration,
    ) -> Result<ControlResponse, ClientError> {
        let deadline = Instant::now() + timeout;
        let encoded = encode_request(request).map_err(ClientError::Encoding)?;
        let mut stream = UnixStream::connect(&self.socket_path).map_err(ClientError::Connection)?;
        let mut remaining = encoded.as_bytes();
        while !remaining.is_empty() {
            stream
                .set_write_timeout(Some(time_left(deadline)?))
                .map_err(ClientError::Connection)?;
            let written = stream.write(remaining).map_err(ClientError::Connection)?;
            if written == 0 {
                return Err(ClientError::Connection(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "control socket accepted no bytes",
                )));
            }
            remaining = &remaining[written..];
        }
        stream
            .shutdown(Shutdown::Write)
            .map_err(ClientError::Connection)?;

        let mut response = Vec::new();
        let mut chunk = [0_u8; 8192];
        loop {
            stream
                .set_read_timeout(Some(time_left(deadline)?))
                .map_err(ClientError::Connection)?;
            let count = stream.read(&mut chunk).map_err(ClientError::Connection)?;
            if count == 0 {
                return Err(ClientError::Connection(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "control response is not newline terminated",
                )));
            }
            response.extend_from_slice(&chunk[..count]);
            if let Some(newline) = response.iter().position(|byte| *byte == b'\n') {
                response.truncate(newline);
                break;
            }
            if response.len() > MAX_RESPONSE_BYTES {
                return Err(ClientError::ResponseTooLarge);
            }
        }
        if response.len() > MAX_RESPONSE_BYTES {
            return Err(ClientError::ResponseTooLarge);
        }
        let decoded = decode_response(&response).map_err(ClientError::Protocol)?;
        if decoded.id != request.id {
            return Err(ClientError::UnexpectedResponseId {
                expected: request.id.clone(),
                received: decoded.id,
            });
        }
        Ok(decoded)
    }
}

fn time_left(deadline: Instant) -> Result<Duration, ClientError> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
        .ok_or_else(|| {
            ClientError::Connection(io::Error::new(
                io::ErrorKind::TimedOut,
                "control request deadline expired",
            ))
        })
}

#[derive(Debug)]
pub enum ClientError {
    Connection(io::Error),
    Encoding(serde_json::Error),
    Protocol(ControlError),
    ResponseTooLarge,
    UnexpectedResponseId { expected: String, received: String },
}

impl fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connection(error) => write!(formatter, "control connection failed: {error}"),
            Self::Encoding(error) => write!(formatter, "control request encoding failed: {error}"),
            Self::Protocol(error) => {
                write!(formatter, "invalid control response: {}", error.message)
            }
            Self::ResponseTooLarge => formatter.write_str("control response exceeds 16 MiB"),
            Self::UnexpectedResponseId { expected, received } => write!(
                formatter,
                "control response ID {received:?} does not match request {expected:?}"
            ),
        }
    }
}

impl ClientError {
    pub fn is_timeout(&self) -> bool {
        matches!(self, Self::Connection(error) if matches!(error.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock))
    }
}

impl std::error::Error for ClientError {}
