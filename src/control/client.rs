use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

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
        let encoded = encode_request(request).map_err(ClientError::Encoding)?;
        let mut stream = UnixStream::connect(&self.socket_path).map_err(ClientError::Connection)?;
        stream
            .set_read_timeout(Some(self.timeout))
            .map_err(ClientError::Connection)?;
        stream
            .set_write_timeout(Some(self.timeout))
            .map_err(ClientError::Connection)?;
        stream
            .write_all(encoded.as_bytes())
            .map_err(ClientError::Connection)?;
        stream
            .shutdown(Shutdown::Write)
            .map_err(ClientError::Connection)?;

        let mut response = Vec::new();
        let mut reader = BufReader::new(stream).take((MAX_RESPONSE_BYTES + 2) as u64);
        reader
            .read_until(b'\n', &mut response)
            .map_err(ClientError::Connection)?;
        if response.len() > MAX_RESPONSE_BYTES {
            return Err(ClientError::ResponseTooLarge);
        }
        if !response.ends_with(b"\n") {
            return Err(ClientError::Connection(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "control response is not newline terminated",
            )));
        }
        response.pop();
        decode_response(&response).map_err(ClientError::Protocol)
    }
}

#[derive(Debug)]
pub enum ClientError {
    Connection(io::Error),
    Encoding(serde_json::Error),
    Protocol(ControlError),
    ResponseTooLarge,
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
        }
    }
}

impl std::error::Error for ClientError {}
