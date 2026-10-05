mod client;
mod protocol;
mod server;
mod state;
mod wait;

pub use client::{ClientError, ControlClient};
pub use protocol::{
    AgentListParams, AgentPreviewParams, AgentRestoreParams, AgentSignalState, AppearanceSetParams,
    ClaudePresetApplyParams, ClaudePresetsSetParams, CloseSessionParams, ControlCommand,
    ControlError, ControlRequest, ControlResponse, CreateSessionParams, ErrorCode, GetTextParams,
    PROTOCOL_VERSION, PrAcknowledgeParams, PrLaunchReviewParams, PrListParams,
    PrSetAutoReviewParams, ProjectRemoveParams, ProjectSetParams, RenameSessionParams,
    ResponseBody, SendInputParams, SessionIdParams, SessionSetStateParams,
    SetSessionWorktreeParams, ShortcutSetParams, UiDiffScope, UiOpenDiffParams, UiOpenFileParams,
    UiShowParams, UiSurface, WorktreeCreateParams, WorktreeListParams, WorktreeReapParams,
    control_timeout, decode_request, decode_response, encode_request, encode_response,
};
pub use server::{ControlServer, PendingRequest};
pub use state::{
    AppState, AppearanceSummary, AttentionSummary, Bounds, CaptureResult, ProjectSummary,
    SessionSummary, ShortcutSummary, TextSnapshot, UiInspection, UiNode, WindowState,
    WorktreeGroupSummary,
};
pub use wait::WaitCondition;

pub use crate::session::{SessionKind, SessionState};
