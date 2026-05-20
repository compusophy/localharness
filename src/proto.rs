//! Wire protocol with the local harness.
//!
//! Two transports run side-by-side:
//!
//! * **Stdin/stdout handshake** carries `InputConfig`/`OutputConfig` as
//!   length-prefixed protobufs (4-byte little-endian length, then bytes).
//!   The harness reports the WebSocket port and auth key it bound.
//! * **WebSocket** carries `InputEvent`/`OutputEvent` framed as JSON. The
//!   JSON shape is what the harness's `protobuf::json_format` emits, so
//!   serde tags mirror proto field names verbatim.

use serde::{Deserialize, Serialize};

use crate::types::UsageMetadata;

// =============================================================================
// Stdin/stdout handshake (protobuf)
// =============================================================================

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct InputConfig {
    #[prost(string, tag = "1")]
    pub storage_directory: ::prost::alloc::string::String,
    #[prost(uint32, tag = "2")]
    pub port: u32,
    #[prost(string, tag = "3")]
    pub bind_address: ::prost::alloc::string::String,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct OutputConfig {
    #[prost(int32, tag = "1")]
    pub port: i32,
    #[prost(string, tag = "2")]
    pub api_key: ::prost::alloc::string::String,
}

// =============================================================================
// WebSocket input (host → harness)
// =============================================================================

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InputEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_input: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub complex_user_input: Option<UserInput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_confirmation: Option<ToolConfirmation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_response: Option<ToolResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub question_response: Option<UserQuestionsResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub halt_request: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub automated_trigger: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInput {
    pub parts: Vec<UserInputPart>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserInputPart {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media: Option<UserInputMedia>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInputMedia {
    pub mime_type: String,
    pub description: String,
    /// Base64-encoded payload. The harness expects standard base64 (with
    /// padding); we encode locally to keep that single dependency in the
    /// connection layer instead of a public type.
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolConfirmation {
    pub trajectory_id: String,
    pub step_index: u32,
    pub accepted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResponse {
    pub id: String,
    pub response_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserQuestionsResponse {
    pub trajectory_id: String,
    pub step_index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancelled: Option<bool>,
    pub response: QuestionsResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionsResponse {
    pub answers: Vec<UserQuestionAnswer>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserQuestionAnswer {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unanswered: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multiple_choice_answer: Option<MultipleChoiceAnswer>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultipleChoiceAnswer {
    pub selected_choice_indices: Vec<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub freeform_response: Option<String>,
}

// =============================================================================
// WebSocket output (harness → host)
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputEvent {
    #[serde(default)]
    pub seq_num: i64,
    #[serde(default)]
    pub timestamp_micros: i64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub step_update: Option<StepUpdate>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub trajectory_state_update: Option<TrajectoryStateUpdate>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_call: Option<WireToolCall>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub usage_metadata: Option<UsageMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepUpdate {
    #[serde(default)]
    pub cascade_id: String,
    #[serde(default)]
    pub trajectory_id: String,
    #[serde(default)]
    pub step_index: u32,
    pub state: StepUpdateState,
    pub source: StepUpdateSource,
    pub target: StepUpdateTarget,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub thinking: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub text_delta: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub thinking_delta: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub request_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_confirmation_request: Option<ToolConfirmationRequest>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub questions_request: Option<UserQuestionsRequest>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub finish: Option<FinishPayload>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepUpdateState {
    #[serde(rename = "STATE_UNSPECIFIED")]
    Unspecified,
    #[serde(rename = "STATE_ACTIVE")]
    Active,
    #[serde(rename = "STATE_DONE")]
    Done,
    #[serde(rename = "STATE_WAITING_FOR_USER")]
    WaitingForUser,
    #[serde(rename = "STATE_ERROR")]
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepUpdateSource {
    #[serde(rename = "SOURCE_UNSPECIFIED")]
    Unspecified,
    #[serde(rename = "SOURCE_SYSTEM")]
    System,
    #[serde(rename = "SOURCE_USER")]
    User,
    #[serde(rename = "SOURCE_MODEL")]
    Model,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepUpdateTarget {
    #[serde(rename = "TARGET_UNSPECIFIED")]
    Unspecified,
    #[serde(rename = "TARGET_USER")]
    User,
    #[serde(rename = "TARGET_MODEL")]
    Model,
    #[serde(rename = "TARGET_ENVIRONMENT")]
    Environment,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolConfirmationRequest {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserQuestionsRequest {
    pub questions: Vec<UserQuestion>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserQuestion {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub multiple_choice: Option<MultipleChoice>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultipleChoice {
    pub question: String,
    pub choices: Vec<String>,
    #[serde(default)]
    pub is_multi_select: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FinishPayload {
    /// JSON-encoded structured output, when the run carried a response schema.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub output_string: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrajectoryStateUpdate {
    #[serde(default)]
    pub trajectory_id: String,
    pub state: TrajectoryState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrajectoryState {
    #[serde(rename = "STATE_UNSPECIFIED")]
    Unspecified,
    #[serde(rename = "STATE_RUNNING")]
    Running,
    #[serde(rename = "STATE_IDLE")]
    Idle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireToolCall {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub arguments_json: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub canonical_path: Option<String>,
}
