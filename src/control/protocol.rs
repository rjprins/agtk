use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;

use crate::appearance::ThemeKey;

pub const PROTOCOL_VERSION: u16 = 1;
pub(crate) const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_REQUEST_ID_CHARS: usize = 128;
const MAX_SESSION_ID_CHARS: usize = 128;
const MAX_INPUT_BYTES: usize = 64 * 1024;
const MAX_TEXT_LINES: u32 = 20_000;

#[derive(Debug, Clone, PartialEq)]
pub struct ControlRequest {
    pub version: u16,
    pub id: String,
    pub command: ControlCommand,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ControlCommand {
    AppGetState,
    UiInspect,
    UiCapture,
    AppearanceSet(AppearanceSetParams),
    SessionCreate(CreateSessionParams),
    SessionSelect(SessionIdParams),
    SessionSendInput(SendInputParams),
    SessionGetText(GetTextParams),
    SessionRename(RenameSessionParams),
    SessionClose(CloseSessionParams),
    HistoryList(SessionIdParams),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionKind {
    Shell,
    Codex,
    Claude,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateSessionParams {
    pub kind: SessionKind,
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub name: Option<String>,
    pub project_root: Option<PathBuf>,
    pub worktree_path: Option<PathBuf>,
    pub initial_input: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionIdParams {
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SendInputParams {
    pub session_id: String,
    pub text: String,
    #[serde(default = "default_true")]
    pub append_enter: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GetTextParams {
    pub session_id: String,
    #[serde(default = "default_text_lines")]
    pub lines: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RenameSessionParams {
    pub session_id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CloseSessionParams {
    pub session_id: String,
    #[serde(default)]
    pub allow_missing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppearanceSetParams {
    pub theme: Option<ThemeKey>,
    pub follow_system: Option<bool>,
    pub font: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    InvalidRequest,
    UnsupportedVersion,
    MethodNotFound,
    NotImplemented,
    InvalidParams,
    RequestTooLarge,
    SessionNotFound,
    InternalError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ControlResponse {
    pub version: u16,
    pub id: String,
    pub body: ResponseBody,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResponseBody {
    Success(Value),
    Failure(ControlError),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireRequest {
    version: u16,
    id: String,
    method: String,
    #[serde(default = "empty_params")]
    params: Value,
}

#[derive(Serialize)]
struct EncodedRequest<'a> {
    version: u16,
    id: &'a str,
    method: &'a str,
    params: Value,
}

#[derive(Serialize)]
struct EncodedSuccessResponse<'a> {
    version: u16,
    id: &'a str,
    result: &'a Value,
}

#[derive(Serialize)]
struct EncodedFailureResponse<'a> {
    version: u16,
    id: &'a str,
    error: &'a ControlError,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DecodedSuccessResponse {
    version: u16,
    id: String,
    result: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DecodedFailureResponse {
    version: u16,
    id: String,
    error: ControlError,
}

pub fn decode_request(bytes: &[u8]) -> Result<ControlRequest, ControlError> {
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err(ControlError::new(
            ErrorCode::RequestTooLarge,
            "Request exceeds the 1 MiB limit",
        ));
    }
    let wire: WireRequest = serde_json::from_slice(bytes).map_err(|error| {
        ControlError::with_details(
            ErrorCode::InvalidRequest,
            "Request is not a valid protocol envelope",
            json!({ "reason": error.to_string() }),
        )
    })?;
    if wire.version != PROTOCOL_VERSION {
        return Err(ControlError::with_details(
            ErrorCode::UnsupportedVersion,
            "Protocol version is not supported",
            json!({ "supported": [PROTOCOL_VERSION], "received": wire.version }),
        ));
    }
    validate_id("requestId", &wire.id, MAX_REQUEST_ID_CHARS)?;

    let command = match wire.method.as_str() {
        "app.get_state" => {
            decode_empty_params(wire.params)?;
            ControlCommand::AppGetState
        }
        "ui.inspect" => {
            decode_empty_params(wire.params)?;
            ControlCommand::UiInspect
        }
        "ui.capture" => {
            decode_empty_params(wire.params)?;
            ControlCommand::UiCapture
        }
        "appearance.set" => {
            let params: AppearanceSetParams = decode_params(wire.params)?;
            validate_appearance(&params)?;
            ControlCommand::AppearanceSet(params)
        }
        "session.create" => {
            let params: CreateSessionParams = decode_params(wire.params)?;
            validate_create_session(&params)?;
            ControlCommand::SessionCreate(params)
        }
        "session.select" => ControlCommand::SessionSelect(decode_session_params(wire.params)?),
        "session.send_input" => {
            let params: SendInputParams = decode_params(wire.params)?;
            validate_session_id(&params.session_id)?;
            if params.text.len() > MAX_INPUT_BYTES {
                return Err(invalid_params("text exceeds the 64 KiB limit"));
            }
            ControlCommand::SessionSendInput(params)
        }
        "session.get_text" => {
            let params: GetTextParams = decode_params(wire.params)?;
            validate_session_id(&params.session_id)?;
            if !(1..=MAX_TEXT_LINES).contains(&params.lines) {
                return Err(invalid_params("lines must be between 1 and 20000"));
            }
            ControlCommand::SessionGetText(params)
        }
        "session.rename" => {
            let params: RenameSessionParams = decode_params(wire.params)?;
            validate_session_id(&params.session_id)?;
            validate_name(&params.name)?;
            ControlCommand::SessionRename(params)
        }
        "session.close" => {
            let params: CloseSessionParams = decode_params(wire.params)?;
            validate_session_id(&params.session_id)?;
            ControlCommand::SessionClose(params)
        }
        "history.list" => ControlCommand::HistoryList(decode_session_params(wire.params)?),
        _ => {
            return Err(ControlError::with_details(
                ErrorCode::MethodNotFound,
                "Control method is not supported",
                json!({ "method": wire.method }),
            ));
        }
    };

    Ok(ControlRequest {
        version: wire.version,
        id: wire.id,
        command,
    })
}

pub fn encode_request(request: &ControlRequest) -> serde_json::Result<String> {
    let (method, params) = request.command.wire_parts()?;
    let mut encoded = serde_json::to_string(&EncodedRequest {
        version: request.version,
        id: &request.id,
        method,
        params,
    })?;
    encoded.push('\n');
    Ok(encoded)
}

pub fn encode_response(response: &ControlResponse) -> serde_json::Result<String> {
    let mut encoded = match &response.body {
        ResponseBody::Success(result) => serde_json::to_string(&EncodedSuccessResponse {
            version: response.version,
            id: &response.id,
            result,
        })?,
        ResponseBody::Failure(error) => serde_json::to_string(&EncodedFailureResponse {
            version: response.version,
            id: &response.id,
            error,
        })?,
    };
    encoded.push('\n');
    Ok(encoded)
}

pub fn decode_response(bytes: &[u8]) -> Result<ControlResponse, ControlError> {
    let value: Value = serde_json::from_slice(bytes).map_err(invalid_response)?;
    let object = value
        .as_object()
        .ok_or_else(|| invalid_response("response is not an object"))?;
    let has_result = object.contains_key("result");
    let has_error = object.contains_key("error");
    if has_result == has_error {
        return Err(invalid_response(
            "response must contain exactly one of result or error",
        ));
    }

    let response = if has_result {
        let decoded: DecodedSuccessResponse =
            serde_json::from_value(value).map_err(invalid_response)?;
        ControlResponse {
            version: decoded.version,
            id: decoded.id,
            body: ResponseBody::Success(decoded.result),
        }
    } else {
        let decoded: DecodedFailureResponse =
            serde_json::from_value(value).map_err(invalid_response)?;
        ControlResponse {
            version: decoded.version,
            id: decoded.id,
            body: ResponseBody::Failure(decoded.error),
        }
    };
    if response.version != PROTOCOL_VERSION {
        return Err(ControlError::new(
            ErrorCode::UnsupportedVersion,
            "Response protocol version is not supported",
        ));
    }
    validate_id("responseId", &response.id, MAX_REQUEST_ID_CHARS)?;
    Ok(response)
}

impl ControlCommand {
    pub const fn method(&self) -> &'static str {
        match self {
            Self::AppGetState => "app.get_state",
            Self::UiInspect => "ui.inspect",
            Self::UiCapture => "ui.capture",
            Self::AppearanceSet(_) => "appearance.set",
            Self::SessionCreate(_) => "session.create",
            Self::SessionSelect(_) => "session.select",
            Self::SessionSendInput(_) => "session.send_input",
            Self::SessionGetText(_) => "session.get_text",
            Self::SessionRename(_) => "session.rename",
            Self::SessionClose(_) => "session.close",
            Self::HistoryList(_) => "history.list",
        }
    }

    fn wire_parts(&self) -> serde_json::Result<(&'static str, Value)> {
        match self {
            Self::AppGetState => Ok(("app.get_state", empty_params())),
            Self::UiInspect => Ok(("ui.inspect", empty_params())),
            Self::UiCapture => Ok(("ui.capture", empty_params())),
            Self::AppearanceSet(params) => Ok(("appearance.set", serde_json::to_value(params)?)),
            Self::SessionCreate(params) => Ok(("session.create", serde_json::to_value(params)?)),
            Self::SessionSelect(params) => Ok(("session.select", serde_json::to_value(params)?)),
            Self::SessionSendInput(params) => {
                Ok(("session.send_input", serde_json::to_value(params)?))
            }
            Self::SessionGetText(params) => Ok(("session.get_text", serde_json::to_value(params)?)),
            Self::SessionRename(params) => Ok(("session.rename", serde_json::to_value(params)?)),
            Self::SessionClose(params) => Ok(("session.close", serde_json::to_value(params)?)),
            Self::HistoryList(params) => Ok(("history.list", serde_json::to_value(params)?)),
        }
    }
}

impl ControlResponse {
    pub fn success(id: impl Into<String>, result: Value) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            id: id.into(),
            body: ResponseBody::Success(result),
        }
    }

    pub fn failure(
        id: impl Into<String>,
        code: ErrorCode,
        message: impl Into<String>,
        details: Option<Value>,
    ) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            id: id.into(),
            body: ResponseBody::Failure(ControlError {
                code,
                message: message.into(),
                details,
            }),
        }
    }

    pub(crate) fn from_error(id: impl Into<String>, error: ControlError) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            id: id.into(),
            body: ResponseBody::Failure(error),
        }
    }
}

impl ControlError {
    pub(crate) fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: None,
        }
    }

    fn with_details(code: ErrorCode, message: impl Into<String>, details: Value) -> Self {
        Self {
            code,
            message: message.into(),
            details: Some(details),
        }
    }
}

fn decode_empty_params(params: Value) -> Result<(), ControlError> {
    if params.as_object().is_some_and(serde_json::Map::is_empty) {
        Ok(())
    } else {
        Err(invalid_params("params must be an empty object"))
    }
}

fn decode_session_params(params: Value) -> Result<SessionIdParams, ControlError> {
    let params: SessionIdParams = decode_params(params)?;
    validate_session_id(&params.session_id)?;
    Ok(params)
}

fn validate_create_session(params: &CreateSessionParams) -> Result<(), ControlError> {
    if params.kind == SessionKind::Custom
        && params
            .command
            .as_deref()
            .is_none_or(|command| command.trim().is_empty())
    {
        return Err(invalid_params(
            "custom sessions require a non-empty command",
        ));
    }
    if params
        .command
        .as_deref()
        .is_some_and(|command| command.trim().is_empty())
    {
        return Err(invalid_params("command cannot be blank"));
    }
    if let Some(name) = &params.name {
        validate_name(name)?;
    }
    if params
        .initial_input
        .as_ref()
        .is_some_and(|input| input.len() > MAX_INPUT_BYTES)
    {
        return Err(invalid_params("initialInput exceeds the 64 KiB limit"));
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), ControlError> {
    let trimmed = name.trim();
    if trimmed != name || !(1..=80).contains(&name.chars().count()) {
        return Err(invalid_params(
            "name must be trimmed and contain between 1 and 80 characters",
        ));
    }
    Ok(())
}

fn validate_appearance(params: &AppearanceSetParams) -> Result<(), ControlError> {
    if params.theme.is_none() && params.follow_system.is_none() && params.font.is_none() {
        return Err(invalid_params(
            "at least one appearance setting is required",
        ));
    }
    if let Some(font) = &params.font {
        let character_count = font.chars().count();
        if font.trim() != font || !(1..=120).contains(&character_count) || font.contains('\0') {
            return Err(invalid_params(
                "font must be trimmed and contain between 1 and 120 characters",
            ));
        }
    }
    Ok(())
}

fn decode_params<T: for<'de> Deserialize<'de>>(params: Value) -> Result<T, ControlError> {
    serde_json::from_value(params).map_err(|error| {
        ControlError::with_details(
            ErrorCode::InvalidParams,
            "Method parameters are invalid",
            json!({ "reason": error.to_string() }),
        )
    })
}

fn validate_session_id(session_id: &str) -> Result<(), ControlError> {
    validate_id("sessionId", session_id, MAX_SESSION_ID_CHARS)
}

fn validate_id(field: &str, value: &str, max_chars: usize) -> Result<(), ControlError> {
    if value.is_empty()
        || value.chars().count() > max_chars
        || value.chars().any(char::is_whitespace)
    {
        return Err(ControlError::with_details(
            ErrorCode::InvalidParams,
            "Identifier is empty, too long, or contains whitespace",
            json!({ "field": field }),
        ));
    }
    Ok(())
}

fn invalid_params(message: &str) -> ControlError {
    ControlError::new(ErrorCode::InvalidParams, message)
}

fn invalid_response(reason: impl ToString) -> ControlError {
    ControlError::with_details(
        ErrorCode::InvalidRequest,
        "Response is not a valid protocol envelope",
        json!({ "reason": reason.to_string() }),
    )
}

fn empty_params() -> Value {
    json!({})
}

const fn default_true() -> bool {
    true
}

const fn default_text_lines() -> u32 {
    200
}
