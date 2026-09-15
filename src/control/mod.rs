mod client;
mod protocol;
mod server;
mod state;
mod wait;

pub use client::{ClientError, ControlClient};
pub use protocol::{
    AppearanceSetParams, CloseSessionParams, ControlCommand, ControlError, ControlRequest,
    ControlResponse, CreateSessionParams, ErrorCode, GetTextParams, PROTOCOL_VERSION,
    RenameSessionParams, ResponseBody, SendInputParams, SessionIdParams, SessionKind,
    ShortcutSetParams, decode_request, decode_response, encode_request, encode_response,
};
pub use server::{ControlServer, PendingRequest};
pub use state::{
    AppState, AppearanceSummary, AttentionSummary, Bounds, CaptureResult, ProjectSummary,
    SessionState, SessionSummary, ShortcutSummary, TextSnapshot, UiInspection, UiNode, WindowState,
    WorktreeGroupSummary,
};
pub use wait::WaitCondition;
