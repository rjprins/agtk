mod client;
mod protocol;
mod server;
mod state;

pub use client::{ClientError, ControlClient};
pub use protocol::{
    CloseSessionParams, ControlCommand, ControlError, ControlRequest, ControlResponse,
    CreateSessionParams, ErrorCode, GetTextParams, PROTOCOL_VERSION, RenameSessionParams,
    ResponseBody, SendInputParams, SessionIdParams, SessionKind, decode_request, decode_response,
    encode_request, encode_response,
};
pub use server::{ControlServer, PendingRequest};
pub use state::{
    AppState, AttentionSummary, ProjectSummary, SessionState, SessionSummary, WindowState,
    WorktreeGroupSummary,
};
