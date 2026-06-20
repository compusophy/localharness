//! Public boundary types for the SDK.
//!
//! Provider-neutral, wire-adjacent types every backend maps onto — the
//! model-agnostic contract above the connection layer. Owned data
//! (`String`/`Vec`) at the boundary for ergonomic cloning; hot paths in the
//! connection layer use `Bytes` where it pays.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

// Default chat model FOR THE GEMINI BACKEND. Verify ids against the live API
// before changing (model ids flip; re-test 400s). Other backends carry their
// own default (e.g. `claude-haiku-4-5-20251001` for the Anthropic backend).
/// Default chat model ID (Gemini backend).
pub const DEFAULT_MODEL: &str = "gemini-3.5-flash";
/// Default image generation model ID.
pub const DEFAULT_IMAGE_GENERATION_MODEL: &str = "gemini-2.0-flash-exp-image-generation";

// =============================================================================
// Model configuration
// =============================================================================

/// How much "thinking" (chain-of-thought reasoning) the model produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingLevel {
    /// Least reasoning output.
    Minimal,
    /// Low reasoning output.
    Low,
    /// Moderate reasoning output.
    Medium,
    /// Maximum reasoning output.
    High,
}

// =============================================================================
// System instructions
// =============================================================================

/// One titled section within templated system instructions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemInstructionSection {
    /// The instruction text.
    pub content: String,
    /// Section title shown to the model.
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

/// Plain-text system instructions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomSystemInstructions {
    /// The raw instruction text.
    pub text: String,
}

/// Structured system instructions with identity and titled sections.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplatedSystemInstructions {
    /// Optional identity string for the agent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    /// Ordered instruction sections.
    #[serde(default)]
    pub sections: Vec<SystemInstructionSection>,
}

/// System instructions: either a plain string or structured sections.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SystemInstructions {
    /// Free-form text instructions.
    Custom(CustomSystemInstructions),
    /// Structured instructions with identity and sections.
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

/// Identifiers for the SDK's built-in tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuiltinTool {
    /// List a directory's contents.
    ListDirectory,
    /// Regex search within files.
    SearchDirectory,
    /// Find files by glob pattern.
    FindFile,
    /// Read a file's contents.
    ViewFile,
    /// Create a new file.
    CreateFile,
    /// Apply edits to an existing file.
    EditFile,
    /// Delete a file or directory.
    DeleteFile,
    /// Rename or move a file.
    RenameFile,
    /// Execute a shell command.
    RunCommand,
    /// Prompt the user with a question.
    AskQuestion,
    /// Spawn a sub-agent.
    StartSubagent,
    /// Generate an image via the image model.
    GenerateImage,
    /// Call another agent via inter-agent RPC.
    CallAgent,
    /// Compile and run a rustlite program.
    CompileRustlite,
    /// Compile a rustlite cartridge and run it on the visual display.
    RunCartridge,
    /// Render an HTML document onto the visual display (framebuffer).
    RenderHtml,
    /// Read/update this agent's own config manifest (system prompt + tool
    /// allowlist), or reset it to defaults.
    ConfigureAgent,
    /// Signal that the agent's turn is complete.
    Finish,
}

impl BuiltinTool {
    /// Every built-in tool variant.
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
        Self::CallAgent,
        Self::CompileRustlite,
        Self::RunCartridge,
        Self::RenderHtml,
        Self::ConfigureAgent,
        Self::Finish,
    ];

    /// Tools that only read state (no side effects).
    pub const READ_ONLY: &'static [BuiltinTool] = &[
        Self::ListDirectory,
        Self::SearchDirectory,
        Self::FindFile,
        Self::ViewFile,
        Self::Finish,
    ];

    /// Tools that operate on individual files.
    pub const FILE_TOOLS: &'static [BuiltinTool] = &[
        Self::ViewFile,
        Self::CreateFile,
        Self::EditFile,
        Self::DeleteFile,
        Self::RenameFile,
    ];

    /// The snake_case wire name the model uses to invoke this tool.
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
            Self::CallAgent => "call_agent",
            Self::CompileRustlite => "compile_rustlite",
            Self::RunCartridge => "run_cartridge",
            Self::RenderHtml => "render_html",
            Self::ConfigureAgent => "configure_agent",
            Self::Finish => "finish",
        }
    }
}

/// Controls which built-in tools are exposed to the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilitiesConfig {
    /// Whether sub-agent spawning is allowed.
    pub enable_subagents: bool,
    /// Explicit allowlist (mutually exclusive with `disabled_tools`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled_tools: Option<Vec<BuiltinTool>>,
    /// Explicit denylist (mutually exclusive with `enabled_tools`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_tools: Option<Vec<BuiltinTool>>,
    /// History-entry count that triggers auto-compaction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compaction_threshold: Option<u32>,
    /// Model ID used for image generation.
    pub image_model: String,
    /// JSON schema for the `finish` tool's structured output.
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

    /// Verify that `enabled_tools` and `disabled_tools` are not both set.
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

/// How to connect to an MCP server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum McpServerConfig {
    /// Launch a child process and communicate over stdin/stdout.
    Stdio {
        /// Command to spawn.
        command: String,
        /// Command-line arguments.
        #[serde(default)]
        args: Vec<String>,
    },
    /// Connect to an SSE-based MCP server (not yet implemented).
    Sse {
        /// Server URL.
        url: String,
        /// Optional HTTP headers.
        #[serde(skip_serializing_if = "Option::is_none")]
        headers: Option<std::collections::BTreeMap<String, String>>,
    },
    /// Connect to an HTTP-based MCP server (not yet implemented).
    Http {
        /// Server URL.
        url: String,
        /// Optional HTTP headers.
        #[serde(skip_serializing_if = "Option::is_none")]
        headers: Option<std::collections::BTreeMap<String, String>>,
        /// Per-call timeout in seconds.
        #[serde(default = "default_http_timeout")]
        timeout_secs: f64,
        /// SSE read timeout in seconds.
        #[serde(default = "default_sse_read_timeout")]
        sse_read_timeout_secs: f64,
        /// Whether to kill the server when the client closes.
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

/// A tool invocation requested by the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Wire name of the tool.
    pub name: String,
    /// JSON arguments the model supplied.
    #[serde(default)]
    pub args: serde_json::Value,
    /// Backend-assigned call ID for correlating results.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub id: Option<String>,
    /// Resolved filesystem path (set by file tools for policy evaluation).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub canonical_path: Option<String>,
}

/// The outcome of executing a [`ToolCall`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    /// Wire name of the tool that was called.
    pub name: String,
    /// Call ID this result corresponds to.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub id: Option<String>,
    /// JSON value on success.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub result: Option<serde_json::Value>,
    /// Error message on failure.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error: Option<String>,
}

impl ToolResult {
    /// Build a successful tool result.
    pub fn ok(name: impl Into<String>, id: Option<String>, value: serde_json::Value) -> Self {
        Self {
            name: name.into(),
            id,
            result: Some(value),
            error: None,
        }
    }

    /// Build a failed tool result with an error message.
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

/// Token usage statistics from the backend.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageMetadata {
    /// Tokens in the prompt.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_token_count: Option<i32>,
    /// Tokens served from cache.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cached_content_token_count: Option<i32>,
    /// Tokens in the model's response.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub candidates_token_count: Option<i32>,
    /// Tokens used for chain-of-thought reasoning.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub thoughts_token_count: Option<i32>,
    /// Total tokens (prompt + response + thoughts).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub total_token_count: Option<i32>,
}

impl UsageMetadata {
    /// Fold a SUCCESSIVE ROUND of the SAME turn into `self` — the per-turn
    /// accumulator the backend loops use. `prompt_token_count` /
    /// `cached_content_token_count` describe the LIVE CONTEXT SIZE, which the
    /// model reports afresh each round of a multi-round tool turn, so they
    /// take the LATEST value (summing them quadruple-counted the context on a
    /// 4-round turn, made the context-fullness bar "fluctuate wildly", and
    /// fired auto-compaction early — on-chain feedback #73). Output-side
    /// counts (`candidates` / `thoughts` / `total`) are genuinely cumulative
    /// and sum. For cross-turn billing-style totals use [`Self::accumulate`].
    pub fn merge_round(&mut self, other: &UsageMetadata) {
        fn add(a: &mut Option<i32>, b: Option<i32>) {
            if let Some(v) = b {
                *a = Some(a.unwrap_or(0).saturating_add(v));
            }
        }
        fn latest(a: &mut Option<i32>, b: Option<i32>) {
            if b.is_some() {
                *a = b;
            }
        }
        latest(&mut self.prompt_token_count, other.prompt_token_count);
        latest(
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

    /// Fold `other` into `self`, summing EVERY field — the cross-turn
    /// billing-style accumulator (`Conversation::cumulative_usage`). Missing
    /// fields on either side are treated as zero so the accumulator only
    /// advances when the backend reports usage.
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

/// The kind of event a step represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepType {
    /// Model-generated text response.
    #[serde(rename = "TEXT_RESPONSE")]
    TextResponse,
    /// Model requesting a tool call.
    #[serde(rename = "TOOL_CALL")]
    ToolCall,
    /// System-generated message.
    #[serde(rename = "SYSTEM_MESSAGE")]
    SystemMessage,
    /// History compaction event.
    #[serde(rename = "COMPACTION")]
    Compaction,
    /// Agent signaling completion.
    #[serde(rename = "FINISH")]
    Finish,
    /// Unrecognized step type.
    #[serde(rename = "UNKNOWN")]
    Unknown,
}

/// Who produced a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepSource {
    /// Generated by the system/SDK.
    #[serde(rename = "SYSTEM")]
    System,
    /// Originated from the user.
    #[serde(rename = "USER")]
    User,
    /// Produced by the model.
    #[serde(rename = "MODEL")]
    Model,
    /// Unknown origin.
    #[serde(rename = "UNKNOWN")]
    Unknown,
}

/// Intended audience of a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepTarget {
    /// Addressed to the user.
    #[serde(rename = "TARGET_USER")]
    User,
    /// Addressed to the environment (tools).
    #[serde(rename = "TARGET_ENVIRONMENT")]
    Environment,
    /// No specific target.
    #[serde(rename = "TARGET_UNSPECIFIED")]
    Unspecified,
    /// Unknown target.
    #[serde(rename = "UNKNOWN")]
    Unknown,
}

/// Lifecycle state of a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepStatus {
    /// Turn is still in progress.
    #[serde(rename = "ACTIVE")]
    Active,
    /// Turn completed successfully.
    #[serde(rename = "DONE")]
    Done,
    /// Waiting for user input.
    #[serde(rename = "WAITING_FOR_USER")]
    WaitingForUser,
    /// Turn ended with an error.
    #[serde(rename = "ERROR")]
    Error,
    /// Turn was canceled.
    #[serde(rename = "CANCELED")]
    Canceled,
    /// Unknown status.
    #[serde(rename = "UNKNOWN")]
    Unknown,
}

/// One event in the agent's response stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    /// Backend-assigned step identifier.
    #[serde(default)]
    pub id: String,
    /// Zero-based index within the current turn.
    #[serde(default)]
    pub step_index: u32,
    /// What kind of event this step represents.
    #[serde(rename = "type", default = "Step::default_type")]
    pub kind: StepType,
    /// Who produced this step.
    #[serde(default = "Step::default_source")]
    pub source: StepSource,
    /// Intended audience.
    #[serde(default = "Step::default_target")]
    pub target: StepTarget,
    /// Lifecycle state.
    #[serde(default = "Step::default_status")]
    pub status: StepStatus,
    /// Accumulated text content.
    #[serde(default)]
    pub content: String,
    /// Incremental text delta since the last step.
    #[serde(default)]
    pub content_delta: String,
    /// Accumulated thinking (reasoning) text.
    #[serde(default)]
    pub thinking: String,
    /// Incremental thinking delta.
    #[serde(default)]
    pub thinking_delta: String,
    /// Tool calls the model is requesting.
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    /// Dispatched tool RESULTS (a [`Step::tool_result`] observability step).
    /// Stream consumers (`ChatResponse::chunks` → `StreamChunk::ToolResult`)
    /// read these to flip a tool block from "running" to ok/err and render
    /// inline result cards. Empty on every other step kind; omitted from the
    /// wire when empty (old-shape compatible).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_results: Vec<ToolResult>,
    /// Error message, if any.
    #[serde(default)]
    pub error: String,
    /// Explicit flag that this step ends the turn.
    #[serde(default)]
    pub is_complete_response: Option<bool>,
    /// Structured JSON output from the `finish` tool.
    #[serde(default)]
    pub structured_output: Option<serde_json::Value>,
    /// The `finish` tool's optional `summary` arg — a short closing message the
    /// model passes so a turn that only showed tool activity still ends with a
    /// final reply. Set ONLY on a terminal `Finish` step; surfaced via
    /// [`crate::conversation::ChatResponse::finish_summary`] so the UI can paint
    /// it as the last assistant segment. Omitted from the wire when empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_summary: Option<String>,
    /// Token usage for this step.
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

    // -------------------------------------------------------------------------
    // Constructors — the backend loops all emit the same handful of Step
    // shapes. Building them here (field-for-field what the loops used to
    // hand-roll as 16-field literals) keeps a new backend from drifting.
    // -------------------------------------------------------------------------

    /// Common skeleton: every field empty/`None` except the four enums.
    fn base(kind: StepType, source: StepSource, target: StepTarget, status: StepStatus) -> Self {
        Self {
            id: String::new(),
            step_index: 0,
            kind,
            source,
            target,
            status,
            content: String::new(),
            content_delta: String::new(),
            thinking: String::new(),
            thinking_delta: String::new(),
            tool_calls: Vec::new(),
            tool_results: Vec::new(),
            error: String::new(),
            is_complete_response: None,
            structured_output: None,
            finish_summary: None,
            usage_metadata: None,
        }
    }

    /// A streamed text delta (model → user, `Active`, non-terminal).
    pub fn text_delta(trajectory_id: &str, step_index: u32, delta: &str) -> Self {
        let mut s = Self::base(
            StepType::TextResponse,
            StepSource::Model,
            StepTarget::User,
            StepStatus::Active,
        );
        s.id = trajectory_id.to_string();
        s.step_index = step_index;
        s.content_delta = delta.to_string();
        s.is_complete_response = Some(false);
        s
    }

    /// A streamed thinking (reasoning) delta (model → user, `Active`,
    /// non-terminal).
    pub fn thought_delta(trajectory_id: &str, step_index: u32, delta: &str) -> Self {
        let mut s = Self::base(
            StepType::TextResponse,
            StepSource::Model,
            StepTarget::User,
            StepStatus::Active,
        );
        s.id = trajectory_id.to_string();
        s.step_index = step_index;
        s.thinking_delta = delta.to_string();
        s.is_complete_response = Some(false);
        s
    }

    /// A tool-call step (model → environment, non-terminal) surfacing `call`
    /// on the stream. `status` is [`StepStatus::Active`] when emitted ahead of
    /// dispatch (the live backends) or [`StepStatus::Done`] when the call was
    /// already dispatched inline and the step is observability-only (the mock
    /// backend — the Agent's step dispatcher skips `Done` steps).
    pub fn tool_call(step_index: u32, call: ToolCall, status: StepStatus) -> Self {
        let mut s = Self::base(
            StepType::ToolCall,
            StepSource::Model,
            StepTarget::Environment,
            status,
        );
        s.step_index = step_index;
        s.tool_calls = vec![call];
        s.is_complete_response = Some(false);
        s
    }

    /// A resolved-tool-result step (model → environment, `Done`, non-terminal,
    /// NO tool_calls) carrying the full dispatched [`ToolResult`] in
    /// `tool_results` — `ChatResponse::chunks` surfaces it as
    /// [`StreamChunk::ToolResult`] so a UI can flip the tool block from
    /// "running" to ok/err and render the inline result card. `Done` +
    /// Model→Environment so it is observability-only: the step dispatcher
    /// skips `Done` steps, and it never trips the System+Error turn-failure
    /// translation in `subscribe_steps` (a *tool* error must not abort the
    /// turn — it also rides in `error` for step-level consumers).
    pub fn tool_result(step_index: u32, result: ToolResult) -> Self {
        let mut s = Self::base(
            StepType::ToolCall,
            StepSource::Model,
            StepTarget::Environment,
            StepStatus::Done,
        );
        s.step_index = step_index;
        s.error = result.error.clone().unwrap_or_default();
        s.tool_results = vec![result];
        s.is_complete_response = Some(false);
        s
    }

    /// The turn-terminating step (model → user, terminal). `kind` is
    /// [`StepType::Finish`] when the model called the `finish` tool
    /// (`finished == true`) OR carried structured output (a bare `finish`
    /// with no `output` arg still flags `Finish`), else
    /// [`StepType::TextResponse`]. The `Finish` kind is the canonical
    /// "the model said it's done" signal — consumers (the in-tab loop)
    /// read it to stop auto-continuing and to suppress an empty-response
    /// bubble on a pure-tool/finish turn.
    #[allow(clippy::too_many_arguments)] // a terminal-step constructor — fields, not flags
    pub fn turn_complete(
        trajectory_id: impl Into<String>,
        step_index: u32,
        status: StepStatus,
        content: impl Into<String>,
        error: impl Into<String>,
        finished: bool,
        structured_output: Option<serde_json::Value>,
        usage_metadata: Option<UsageMetadata>,
    ) -> Self {
        let kind = if finished || structured_output.is_some() {
            StepType::Finish
        } else {
            StepType::TextResponse
        };
        let mut s = Self::base(kind, StepSource::Model, StepTarget::User, status);
        s.id = trajectory_id.into();
        s.step_index = step_index;
        s.content = content.into();
        s.error = error.into();
        s.is_complete_response = Some(true);
        s.structured_output = structured_output;
        s.usage_metadata = usage_metadata;
        s
    }

    /// Attach a `finish`-tool `summary` to a terminal step (builder-style), so
    /// backends can thread the model's closing message through WITHOUT widening
    /// the already-broad [`Self::turn_complete`] signature. A no-op for an empty
    /// summary or a non-`Finish` terminal (a summary only ever rides a real
    /// completion). Surfaced to the UI via
    /// [`crate::conversation::ChatResponse::finish_summary`].
    pub fn with_finish_summary(mut self, summary: Option<String>) -> Self {
        if self.kind == StepType::Finish {
            self.finish_summary = summary.filter(|sm| !sm.is_empty());
        }
        self
    }

    /// A System-sourced turn-failure step (terminal, status `Error`) carrying
    /// `message`. Backends whose `subscribe_steps` translates turn failures
    /// convert exactly this shape into a stream `Err`.
    pub fn turn_error(step_index: u32, message: impl Into<String>) -> Self {
        let mut s = Self::base(
            StepType::TextResponse,
            StepSource::System,
            StepTarget::User,
            StepStatus::Error,
        );
        s.step_index = step_index;
        s.error = message.into();
        s.is_complete_response = Some(true);
        s
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

/// The decision from a decide-hook: allow or deny, with a reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookResult {
    /// `true` to proceed, `false` to block.
    pub allow: bool,
    /// Human-readable reason for the decision.
    #[serde(default)]
    pub message: String,
}

impl HookResult {
    /// Allow with no message.
    pub fn allow() -> Self {
        Self {
            allow: true,
            message: String::new(),
        }
    }

    /// Allow with a diagnostic message.
    pub fn allow_with(message: impl Into<String>) -> Self {
        Self {
            allow: true,
            message: message.into(),
        }
    }

    /// Deny with a reason message.
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

/// One selectable option in an interactive question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskQuestionOption {
    /// Machine-readable identifier.
    pub id: String,
    /// Display text.
    pub text: String,
}

/// A single question with a set of options.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskQuestionEntry {
    /// The question text.
    pub question: String,
    /// Available answer choices.
    pub options: Vec<AskQuestionOption>,
    /// Whether multiple options can be selected.
    #[serde(default)]
    pub is_multi_select: bool,
}

/// A batch of interactive questions for the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskQuestionInteractionSpec {
    /// The questions to present.
    pub questions: Vec<AskQuestionEntry>,
}

/// The user's answer to an interactive question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionResponse {
    /// IDs of the selected options.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub selected_option_ids: Option<Vec<String>>,
    /// Free-text answer.
    #[serde(default)]
    pub freeform_response: String,
    /// Whether the user skipped the question.
    #[serde(default)]
    pub skipped: bool,
}

// =============================================================================
// Trigger delivery semantics
// =============================================================================

/// When a trigger's message is delivered to the agent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerDelivery {
    /// Deliver immediately, even mid-turn.
    SendImmediately,
    /// Wait until the agent is idle before delivering.
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
    /// Incremental conversational text delta.
    Text {
        /// Index of the step that produced this chunk.
        step_index: u32,
        /// The text fragment.
        text: String,
    },
    /// Incremental reasoning/thinking delta.
    Thought {
        /// Index of the step that produced this chunk.
        step_index: u32,
        /// The thinking fragment.
        text: String,
    },
    /// The model is requesting a tool call.
    ToolCall(ToolCall),
    /// A tool call completed with a result.
    ToolResult(ToolResult),
}

// =============================================================================
// Persisted transcript
// =============================================================================

/// User-visible role of a [`TranscriptEntry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TranscriptRole {
    /// Message from the user.
    User,
    /// Message from the model.
    Assistant,
}

impl TranscriptRole {
    /// Lowercase string representation (`"user"` or `"assistant"`).
    pub fn as_str(&self) -> &'static str {
        match self {
            TranscriptRole::User => "user",
            TranscriptRole::Assistant => "assistant",
        }
    }
}

/// One turn-or-message in a flattened, user-visible transcript.
///
/// Produced by [`Agent::transcript`] — includes tool-call activity
/// so restored sessions show what the agent did, not just what it said.
///
/// [`Agent::transcript`]: crate::Agent::transcript
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptEntry {
    /// Who sent this message.
    pub role: TranscriptRole,
    /// The textual content of the message.
    pub text: String,
    /// Tool calls made during this turn (assistant only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<TranscriptToolCall>,
}

/// A tool call as recorded in the transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptToolCall {
    /// Tool name.
    pub name: String,
    /// Arguments the model supplied.
    pub args: serde_json::Value,
    /// Result value, if the call succeeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// Error message, if the call failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[cfg(test)]
mod transcript_tests {
    use super::*;

    /// BACKWARD COMPAT: a transcript entry persisted by an OLDER build — one
    /// with NO `tool_calls` field at all — must still deserialize. `tool_calls`
    /// is `#[serde(default)]`, so an absent field restores as an empty Vec and
    /// the old entry replays exactly as it did before (text only). This is the
    /// load-bearing guarantee: existing `.lh_history.json` projections (the UI
    /// repaint path never decoded the raw history, but a forward-compat reader
    /// must tolerate both shapes) keep loading.
    #[test]
    fn old_format_entry_without_tool_calls_deserializes() {
        let old = r#"{"role":"assistant","text":"hi there"}"#;
        let entry: TranscriptEntry = serde_json::from_str(old).expect("old format must deserialize");
        assert_eq!(entry.role, TranscriptRole::Assistant);
        assert_eq!(entry.text, "hi there");
        assert!(entry.tool_calls.is_empty(), "absent tool_calls → empty Vec");
    }

    /// A NEW entry carrying tool calls (with result + error) round-trips
    /// losslessly. `tool_calls` is `skip_serializing_if = "Vec::is_empty"`, so
    /// a text-only entry serializes WITHOUT the field — staying byte-compatible
    /// with the old shape that older readers expect.
    #[test]
    fn new_format_with_tool_calls_round_trips() {
        let entry = TranscriptEntry {
            role: TranscriptRole::Assistant,
            text: "checking your files".into(),
            tool_calls: vec![
                TranscriptToolCall {
                    name: "view_file".into(),
                    args: serde_json::json!({"path": "main.rs"}),
                    result: Some(serde_json::json!({"contents": "fn main() {}"})),
                    error: None,
                },
                TranscriptToolCall {
                    name: "view_file".into(),
                    args: serde_json::json!({"path": "missing"}),
                    result: None,
                    error: Some("no such file".into()),
                },
            ],
        };
        let bytes = serde_json::to_vec(&entry).unwrap();
        let back: TranscriptEntry = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(entry, back);

        // Text-only entries omit the field entirely (old-shape compatible).
        let text_only = TranscriptEntry {
            role: TranscriptRole::User,
            text: "hello".into(),
            tool_calls: Vec::new(),
        };
        let json = serde_json::to_string(&text_only).unwrap();
        assert!(
            !json.contains("tool_calls"),
            "empty tool_calls must be omitted, got: {json}"
        );
    }
}
