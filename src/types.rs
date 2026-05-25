//! Public boundary types for the SDK.
//!
//! These mirror the Pydantic models in `google/antigravity/types.py` so a
//! payload that round-trips through JSON looks identical on the wire. The
//! Rust port uses owned data (`String`/`Vec`) at the boundary for ergonomic
//! cloning; hot paths in the connection layer use `Bytes` where it pays.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

// Google's chat model. `gemini-3.x` doesn't exist on the public API
// yet — using a model id the API actually accepts. Bump this when
// Google publishes newer ids (and re-test 400s).
pub const DEFAULT_MODEL: &str = "gemini-2.5-flash";
pub const DEFAULT_IMAGE_GENERATION_MODEL: &str = "gemini-2.0-flash-exp-image-generation";

// =============================================================================
// Model configuration
// =============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingLevel {
    Minimal,
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<ThinkingLevel>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelEntry {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default)]
    pub generation: GenerationConfig,
}

impl ModelEntry {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            api_key: None,
            generation: GenerationConfig::default(),
        }
    }
}

impl Default for ModelEntry {
    fn default() -> Self {
        Self::new(DEFAULT_MODEL)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelConfig {
    pub default: ModelEntry,
    pub image_generation: ModelEntry,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            default: ModelEntry::new(DEFAULT_MODEL),
            image_generation: ModelEntry::new(DEFAULT_IMAGE_GENERATION_MODEL),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeminiConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default)]
    pub models: ModelConfig,
}

// =============================================================================
// System instructions
// =============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemInstructionSection {
    pub content: String,
    #[serde(default = "default_section_title")]
    pub title: String,
}

fn default_section_title() -> String {
    "user_system_instructions".to_string()
}

impl Default for SystemInstructionSection {
    fn default() -> Self {
        Self {
            content: String::new(),
            title: default_section_title(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomSystemInstructions {
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplatedSystemInstructions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    #[serde(default)]
    pub sections: Vec<SystemInstructionSection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SystemInstructions {
    Custom(CustomSystemInstructions),
    Templated(TemplatedSystemInstructions),
}

impl From<&str> for SystemInstructions {
    fn from(text: &str) -> Self {
        Self::Custom(CustomSystemInstructions { text: text.into() })
    }
}

impl From<String> for SystemInstructions {
    fn from(text: String) -> Self {
        Self::Custom(CustomSystemInstructions { text })
    }
}

// =============================================================================
// Builtin tools + capability flags
// =============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuiltinTool {
    ListDirectory,
    SearchDirectory,
    FindFile,
    ViewFile,
    CreateFile,
    EditFile,
    DeleteFile,
    RenameFile,
    RunCommand,
    AskQuestion,
    StartSubagent,
    GenerateImage,
    Finish,
}

impl BuiltinTool {
    pub const ALL: &'static [BuiltinTool] = &[
        Self::ListDirectory,
        Self::SearchDirectory,
        Self::FindFile,
        Self::ViewFile,
        Self::CreateFile,
        Self::EditFile,
        Self::DeleteFile,
        Self::RenameFile,
        Self::RunCommand,
        Self::AskQuestion,
        Self::StartSubagent,
        Self::GenerateImage,
        Self::Finish,
    ];

    pub const READ_ONLY: &'static [BuiltinTool] = &[
        Self::ListDirectory,
        Self::SearchDirectory,
        Self::FindFile,
        Self::ViewFile,
        Self::Finish,
    ];

    pub const FILE_TOOLS: &'static [BuiltinTool] = &[
        Self::ViewFile,
        Self::CreateFile,
        Self::EditFile,
        Self::DeleteFile,
        Self::RenameFile,
    ];

    pub fn wire_name(self) -> &'static str {
        match self {
            Self::ListDirectory => "list_directory",
            Self::SearchDirectory => "search_directory",
            Self::FindFile => "find_file",
            Self::ViewFile => "view_file",
            Self::CreateFile => "create_file",
            Self::EditFile => "edit_file",
            Self::DeleteFile => "delete_file",
            Self::RenameFile => "rename_file",
            Self::RunCommand => "run_command",
            Self::AskQuestion => "ask_question",
            Self::StartSubagent => "start_subagent",
            Self::GenerateImage => "generate_image",
            Self::Finish => "finish",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilitiesConfig {
    pub enable_subagents: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled_tools: Option<Vec<BuiltinTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_tools: Option<Vec<BuiltinTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compaction_threshold: Option<u32>,
    pub image_model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_tool_schema_json: Option<String>,
}

impl Default for CapabilitiesConfig {
    fn default() -> Self {
        // Python defaults to read-only safety mode unless the caller opts in.
        Self {
            enable_subagents: true,
            enabled_tools: Some(BuiltinTool::READ_ONLY.to_vec()),
            disabled_tools: None,
            compaction_threshold: None,
            image_model: DEFAULT_IMAGE_GENERATION_MODEL.to_string(),
            finish_tool_schema_json: None,
        }
    }
}

impl CapabilitiesConfig {
    /// Unrestricted: exposes every builtin tool to the model. Matches the
    /// Python `CapabilitiesConfig()` no-arg constructor.
    pub fn unrestricted() -> Self {
        Self {
            enable_subagents: true,
            enabled_tools: None,
            disabled_tools: None,
            compaction_threshold: None,
            image_model: DEFAULT_IMAGE_GENERATION_MODEL.to_string(),
            finish_tool_schema_json: None,
        }
    }

    /// Resolve the effective enabled-tool set, given that `enabled_tools` and
    /// `disabled_tools` are mutually exclusive (the validator enforces it).
    pub fn effective_tools(&self) -> HashSet<BuiltinTool> {
        match (&self.enabled_tools, &self.disabled_tools) {
            (Some(en), _) => en.iter().copied().collect(),
            (None, Some(dis)) => {
                let disabled: HashSet<_> = dis.iter().copied().collect();
                BuiltinTool::ALL
                    .iter()
                    .copied()
                    .filter(|t| !disabled.contains(t))
                    .collect()
            }
            (None, None) => BuiltinTool::ALL.iter().copied().collect(),
        }
    }

    pub fn validate(&self) -> Result<(), crate::error::Error> {
        if self.enabled_tools.is_some() && self.disabled_tools.is_some() {
            return Err(crate::error::Error::config(
                "enabled_tools and disabled_tools are mutually exclusive",
            ));
        }
        Ok(())
    }
}

// =============================================================================
// MCP server configuration
// =============================================================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum McpServerConfig {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
    Sse {
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        headers: Option<std::collections::BTreeMap<String, String>>,
    },
    Http {
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        headers: Option<std::collections::BTreeMap<String, String>>,
        #[serde(default = "default_http_timeout")]
        timeout_secs: f64,
        #[serde(default = "default_sse_read_timeout")]
        sse_read_timeout_secs: f64,
        #[serde(default = "default_terminate_on_close")]
        terminate_on_close: bool,
    },
}

fn default_http_timeout() -> f64 {
    30.0
}
fn default_sse_read_timeout() -> f64 {
    300.0
}
fn default_terminate_on_close() -> bool {
    true
}

// =============================================================================
// Tool calls + results
// =============================================================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    #[serde(default)]
    pub args: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub canonical_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error: Option<String>,
}

impl ToolResult {
    pub fn ok(name: impl Into<String>, id: Option<String>, value: serde_json::Value) -> Self {
        Self {
            name: name.into(),
            id,
            result: Some(value),
            error: None,
        }
    }

    pub fn err(name: impl Into<String>, id: Option<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            id,
            result: None,
            error: Some(message.into()),
        }
    }
}

// =============================================================================
// Usage metadata
// =============================================================================

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageMetadata {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_token_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cached_content_token_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub candidates_token_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub thoughts_token_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub total_token_count: Option<i32>,
}

impl UsageMetadata {
    /// Fold `other` into `self`. Missing fields on either side are treated as
    /// zero so the accumulator only advances when the backend reports usage.
    pub fn accumulate(&mut self, other: &UsageMetadata) {
        fn add(a: &mut Option<i32>, b: Option<i32>) {
            if let Some(v) = b {
                *a = Some(a.unwrap_or(0).saturating_add(v));
            }
        }
        add(&mut self.prompt_token_count, other.prompt_token_count);
        add(
            &mut self.cached_content_token_count,
            other.cached_content_token_count,
        );
        add(
            &mut self.candidates_token_count,
            other.candidates_token_count,
        );
        add(&mut self.thoughts_token_count, other.thoughts_token_count);
        add(&mut self.total_token_count, other.total_token_count);
    }
}

// =============================================================================
// Steps
// =============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepType {
    #[serde(rename = "TEXT_RESPONSE")]
    TextResponse,
    #[serde(rename = "TOOL_CALL")]
    ToolCall,
    #[serde(rename = "SYSTEM_MESSAGE")]
    SystemMessage,
    #[serde(rename = "COMPACTION")]
    Compaction,
    #[serde(rename = "FINISH")]
    Finish,
    #[serde(rename = "UNKNOWN")]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepSource {
    #[serde(rename = "SYSTEM")]
    System,
    #[serde(rename = "USER")]
    User,
    #[serde(rename = "MODEL")]
    Model,
    #[serde(rename = "UNKNOWN")]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepTarget {
    #[serde(rename = "TARGET_USER")]
    User,
    #[serde(rename = "TARGET_ENVIRONMENT")]
    Environment,
    #[serde(rename = "TARGET_UNSPECIFIED")]
    Unspecified,
    #[serde(rename = "UNKNOWN")]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepStatus {
    #[serde(rename = "ACTIVE")]
    Active,
    #[serde(rename = "DONE")]
    Done,
    #[serde(rename = "WAITING_FOR_USER")]
    WaitingForUser,
    #[serde(rename = "ERROR")]
    Error,
    #[serde(rename = "CANCELED")]
    Canceled,
    #[serde(rename = "UNKNOWN")]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub step_index: u32,
    #[serde(rename = "type", default = "Step::default_type")]
    pub kind: StepType,
    #[serde(default = "Step::default_source")]
    pub source: StepSource,
    #[serde(default = "Step::default_target")]
    pub target: StepTarget,
    #[serde(default = "Step::default_status")]
    pub status: StepStatus,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub content_delta: String,
    #[serde(default)]
    pub thinking: String,
    #[serde(default)]
    pub thinking_delta: String,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub is_complete_response: Option<bool>,
    #[serde(default)]
    pub structured_output: Option<serde_json::Value>,
    #[serde(default)]
    pub usage_metadata: Option<UsageMetadata>,
}

impl Step {
    fn default_type() -> StepType {
        StepType::Unknown
    }
    fn default_source() -> StepSource {
        StepSource::Unknown
    }
    fn default_target() -> StepTarget {
        StepTarget::Unknown
    }
    fn default_status() -> StepStatus {
        StepStatus::Unknown
    }

    /// A turn-terminating step: model-sourced, user-facing, status DONE, with
    /// content. Mirrors `is_complete_response` semantics in the Python SDK.
    pub fn is_terminal_response(&self) -> bool {
        self.is_complete_response.unwrap_or(false)
            || (self.source == StepSource::Model
                && self.target == StepTarget::User
                && self.status == StepStatus::Done
                && !self.content.is_empty())
    }
}

// =============================================================================
// Hooks
// =============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookResult {
    pub allow: bool,
    #[serde(default)]
    pub message: String,
}

impl HookResult {
    pub fn allow() -> Self {
        Self {
            allow: true,
            message: String::new(),
        }
    }

    pub fn allow_with(message: impl Into<String>) -> Self {
        Self {
            allow: true,
            message: message.into(),
        }
    }

    pub fn deny(message: impl Into<String>) -> Self {
        Self {
            allow: false,
            message: message.into(),
        }
    }
}

// =============================================================================
// Ask-question (interactive) primitives
// =============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskQuestionOption {
    pub id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskQuestionEntry {
    pub question: String,
    pub options: Vec<AskQuestionOption>,
    #[serde(default)]
    pub is_multi_select: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskQuestionInteractionSpec {
    pub questions: Vec<AskQuestionEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionResponse {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub selected_option_ids: Option<Vec<String>>,
    #[serde(default)]
    pub freeform_response: String,
    #[serde(default)]
    pub skipped: bool,
}

// =============================================================================
// Trigger delivery semantics
// =============================================================================

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerDelivery {
    SendImmediately,
    #[default]
    WaitIdle,
}

// =============================================================================
// Stream chunks
// =============================================================================

/// A single semantic event emitted while a turn is in flight.
///
/// `Text` and `Thought` carry incremental deltas (already split into
/// human-readable chunks). `ToolCall` and `ToolResult` carry strongly typed
/// dispatches so consumers can render spinners without parsing JSON.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamChunk {
    Text { step_index: u32, text: String },
    Thought { step_index: u32, text: String },
    ToolCall(ToolCall),
    ToolResult(ToolResult),
}

// =============================================================================
// Persisted transcript
// =============================================================================

/// User-visible role of a [`TranscriptEntry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TranscriptRole {
    User,
    Assistant,
}

impl TranscriptRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            TranscriptRole::User => "user",
            TranscriptRole::Assistant => "assistant",
        }
    }
}

/// One turn-or-message in a flattened, user-visible transcript.
///
/// Produced by [`Agent::transcript`] — text-only summary of the
/// internal Gemini history, useful for repainting a UI after a session
/// resume. Tool-call activity is intentionally dropped: this is a
/// human-readable view, not a fidelity-preserving snapshot.
///
/// [`Agent::transcript`]: crate::Agent::transcript
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptEntry {
    pub role: TranscriptRole,
    pub text: String,
}
