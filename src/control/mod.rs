mod client;
mod protocol;
mod server;
mod state;
mod wait;

pub use client::{ClientError, ControlClient};
pub use protocol::{
    CloseSessionParams, ControlCommand, ControlError, ControlRequest, ControlResponse,
    CreateSessionParams, ErrorCode, GetTextParams, PROTOCOL_VERSION, RenameSessionParams,
    ResponseBody, SendInputParams, SessionIdParams, SessionKind, decode_request, decode_response,
    encode_request, encode_response,
};
pub use server::{ControlServer, PendingRequest};
pub use state::{
    AppState, AttentionSummary, Bounds, ProjectSummary, SessionState, SessionSummary, TextSnapshot,
    UiInspection, UiNode, WindowState, WorktreeGroupSummary,
};
pub use wait::WaitCondition;
