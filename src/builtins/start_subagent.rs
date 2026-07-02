//! `start_subagent` — spawn a one-shot, TOOL-BEARING subagent.
//!
//! The parent agent calls `start_subagent` to delegate a self-contained
//! task: an isolated context (no shared history), a single user prompt,
//! its own system instructions. The subagent runs against the same
//! Gemini client + model and returns its final response.
//!
//! Unlike the v1 text-only spawner, the subagent gets its OWN
//! [`ToolRunner`] with a REDUCED tool surface: the filesystem builtins over
//! the SAME [`Filesystem`](crate::filesystem::Filesystem) the parent uses
//! (so a subagent can actually read/write/search the shared OPFS), plus
//! `finish`. It NEVER receives value-moving, owner-only, or
//! cartridge/agent-spawning tools — and crucially it does NOT receive
//! `start_subagent` itself, so a subagent can never spawn further
//! subagents (depth is bounded to one). Cost is bounded by
//! [`MAX_SUBAGENT_ROUNDS`] model↔tool round-trips.
//!
//! The model↔tool loop reuses the SAME backend plumbing the main agent
//! loop uses — the Gemini wire types and the shared
//! [`dispatch_tool_call`](crate::backends::dispatch::dispatch_tool_call)
//! pipeline over a [`ToolRunner`] — so policies, hooks, and the
//! `{"error": ...}` error-lift convention behave identically here. This is
//! WIRING, not new infrastructure.

use std::sync::Arc;

use async_trait::async_trait;
use futures_util::stream::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::backends::dispatch::dispatch_tool_call;
use crate::backends::gemini::api::SharedClient;
use crate::backends::gemini::wire::{
    Content, ContentRole, FinishReason, FunctionCall, FunctionResponse, FunctionDeclaration,
    GenerateContentRequest, Part, ToolDecl,
};
use crate::builtins::{register_builtins, BuiltinDeps, FINISH_TOOL_NAME};
use crate::error::{Error, Result};
use crate::filesystem::SharedFilesystem;
use crate::hooks::{HookRunner, TurnContext};
use crate::tools::{Tool, ToolContext, ToolRunner};
use crate::types::{BuiltinTool, CapabilitiesConfig, ToolCall};

/// Maximum model↔tool round-trips a single subagent runs before the loop is
/// force-ended. Bounds cost: a subagent that keeps calling tools without
/// finishing can't run away. Mirrors the main loop's `MAX_TOOL_ROUNDS`
/// intent, smaller because a delegated sub-task should be focused.
const MAX_SUBAGENT_ROUNDS: u32 = 8;

/// The subagent's REDUCED tool allowlist: the filesystem builtins (over the
/// SAME filesystem the parent uses) plus `finish`. Deliberately EXCLUDES
/// `start_subagent` (no nested subagents → depth bounded to one),
/// `run_command`, `call_agent`, `generate_image`, `ask_question`,
/// `configure_agent`, the cartridge/render tools, and anything value-moving
/// or owner-only. Mirrors `app::chat::tools::misc::spawn_recursive_subagent`'s
/// reduced surface, minus the self-spawn (recursion is the parent's job, not
/// a builtin's).
const SUBAGENT_TOOLS: &[BuiltinTool] = &[
    BuiltinTool::ListDirectory,
    BuiltinTool::SearchDirectory,
    BuiltinTool::FindFile,
    BuiltinTool::ViewFile,
    BuiltinTool::CreateFile,
    BuiltinTool::EditFile,
    BuiltinTool::DeleteFile,
    BuiltinTool::RenameFile,
    BuiltinTool::Finish,
];

pub struct StartSubagent {
    client: SharedClient,
    model: String,
    /// The shared filesystem the parent agent's fs builtins write to. When
    /// present the subagent gets the fs builtins over the SAME store (so it
    /// can do real work); when `None` (no filesystem supplied to the parent)
    /// the subagent is text-only + `finish`.
    fs: Option<SharedFilesystem>,
    /// The parent's hooks/policies. Threaded into the subagent's
    /// `dispatch_tool_call` so its tool calls inherit the parent's PreToolCall
    /// deny/containment policies (e.g. `policy::workspace_only`). `None` =
    /// no inheritance (then only the PROTECTED_FILES basename guard bounds the
    /// subagent's fs tools). See M8.
    hooks: Option<Arc<HookRunner>>,
}

impl StartSubagent {
    /// Construct a text-only spawner (no filesystem tools, no inherited
    /// policies). Kept for callers that don't have a filesystem to share.
    pub fn new(client: SharedClient, model: impl Into<String>) -> Self {
        Self {
            client,
            model: model.into(),
            fs: None,
            hooks: None,
        }
    }

    /// Construct a tool-bearing spawner whose subagents get the filesystem
    /// builtins over `fs` (the same store the parent's fs builtins use) and
    /// whose tool calls run through `hooks` (the parent's PreToolCall
    /// deny/containment policies), so the subagent can't escape the parent's
    /// confinement.
    pub fn with_filesystem(
        client: SharedClient,
        model: impl Into<String>,
        fs: Option<SharedFilesystem>,
        hooks: Option<Arc<HookRunner>>,
    ) -> Self {
        Self {
            client,
            model: model.into(),
            fs,
            hooks,
        }
    }

    /// Build the subagent's isolated [`ToolRunner`] from the reduced
    /// allowlist, reusing the crate's `register_builtins` so the subagent's
    /// tools are constructed and gated exactly like the main agent's. The fs
    /// builtins register only when a filesystem is present (same rule as the
    /// main path); `finish` always registers.
    fn build_runner(&self) -> ToolRunner {
        let caps = CapabilitiesConfig {
            enable_subagents: false,
            enabled_tools: Some(SUBAGENT_TOOLS.to_vec()),
            disabled_tools: None,
            compaction_threshold: None,
            image_model: String::new(),
            finish_tool_schema_json: None,
        };
        let deps = BuiltinDeps {
            // No chat/image client: the subagent must NOT get start_subagent
            // (nested) or generate_image even were they in the allowlist — so
            // no nested-subagent path consumes `hooks` here.
            chat_client: None,
            chat_model: String::new(),
            image_client: None,
            image_model: String::new(),
            fs: self.fs.clone(),
            hooks: None,
        };
        let runner = ToolRunner::new();
        register_builtins(&runner, &caps, &deps);
        runner
    }

    /// Open the model stream with a bounded retry — the SAME shared
    /// [`open_stream_with_retry`](crate::backends::retry::open_stream_with_retry)
    /// wrapper the gemini/anthropic/openai turn loops use (one policy, one impl).
    /// Transient transport/5xx/timeout failures retry with a short backoff;
    /// auth/credits/rate-limit fail FAST — retrying those just burns time and quota.
    async fn stream_with_retry(
        &self,
        req: &GenerateContentRequest,
    ) -> Result<crate::backends::gemini::api::GeminiSseStream> {
        crate::backends::retry::open_stream_with_retry(|| self.client.stream_generate(&self.model, req))
            .await
    }
}

#[derive(Deserialize)]
struct Args {
    system_instructions: String,
    prompt: String,
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl Tool for StartSubagent {
    fn name(&self) -> &str {
        "start_subagent"
    }

    fn description(&self) -> &str {
        "Spawn a one-shot subagent with an ISOLATED context to do a self-contained \
         task and return the result. The subagent receives the given \
         `system_instructions` and `prompt`, runs against the same model as the \
         parent, and gets its OWN reduced tool surface — the filesystem tools \
         (list/view/find/search/create/edit/delete/rename over the SAME files you \
         see) plus `finish` — so it can actually DO work, not just reason. It \
         CANNOT spawn further subagents, move value, run commands, or call other \
         agents. It cannot see your conversation history. Use it to delegate a \
         focused unit of work (research a directory, refactor a file, draft \
         content) and get back the finished result."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "system_instructions": { "type": "string", "description": "System instructions for the subagent's persona / role (e.g. \"you are a focused worker that does X and returns just the result\")." },
                "prompt": { "type": "string", "description": "The task / user message to send to the subagent." }
            },
            "required": ["system_instructions", "prompt"]
        })
    }

    async fn execute(&self, args: Value, _ctx: Option<Arc<ToolContext>>) -> Result<Value> {
        let args: Args = serde_json::from_value(args)
            .map_err(|e| Error::other(format!("start_subagent args: {e}")))?;

        // The subagent's isolated tool runner (reduced allowlist) + the wire
        // function declarations the model sees — built from the SAME tools, so
        // a tool's schema is never out of sync with what actually runs.
        let runner = Arc::new(self.build_runner());
        let tool_declarations: Vec<FunctionDeclaration> = runner
            .iter_tools()
            .into_iter()
            .map(|tool| FunctionDeclaration {
                name: tool.name().to_string(),
                description: tool.description().to_string(),
                parameters: tool.input_schema(),
            })
            .collect();
        let tools = if tool_declarations.is_empty() {
            Vec::new()
        } else {
            vec![ToolDecl {
                function_declarations: tool_declarations,
            }]
        };

        let system_instruction = Some(Content {
            role: ContentRole::User,
            parts: vec![Part::Text {
                text: args.system_instructions,
            }],
        });

        // Isolated history — the subagent starts fresh with only its prompt.
        let mut history: Vec<Content> = vec![Content {
            role: ContentRole::User,
            parts: vec![Part::Text { text: args.prompt }],
        }];

        // A fresh turn context — the subagent has no parent session, but it
        // DOES inherit the parent's hooks (`self.hooks`) so the shared dispatch
        // pipeline enforces the parent's PreToolCall deny/containment policies
        // (e.g. workspace_only) on every subagent tool call.
        let turn_ctx = TurnContext::default();

        let mut last_text = String::new();
        let mut finish_reason: Option<FinishReason> = None;
        let mut finished = false;
        let mut rounds = 0u32;

        loop {
            rounds += 1;
            if rounds > MAX_SUBAGENT_ROUNDS {
                break;
            }

            let req = GenerateContentRequest {
                system_instruction: system_instruction.clone(),
                contents: history.clone(),
                tools: tools.clone(),
                ..Default::default()
            };

            let mut stream = self.stream_with_retry(&req).await?;
            let mut text = String::new();
            // Each call rides with its `thoughtSignature` — Gemini 3.x stamps
            // functionCall parts and 400s replayed history missing it.
            let mut pending_calls: Vec<(FunctionCall, Option<String>)> = Vec::new();
            while let Some(chunk_res) = stream.next().await {
                let chunk = chunk_res?;
                for cand in chunk.candidates {
                    if let Some(content) = cand.content {
                        for part in content.parts {
                            match part {
                                // Gemini 3.x tags normal text as
                                // `Thought { thought: false, text: Some(_) }`
                                // as well as plain `Text` — collect both.
                                Part::Text { text: t } => text.push_str(&t),
                                Part::Thought {
                                    thought: false,
                                    text: Some(t),
                                    ..
                                } => text.push_str(&t),
                                Part::FunctionCall {
                                    function_call,
                                    thought_signature,
                                } => pending_calls.push((function_call, thought_signature)),
                                _ => {}
                            }
                        }
                    }
                    if let Some(r) = cand.finish_reason {
                        finish_reason = Some(r);
                    }
                }
            }

            // Persist the model turn (text + functionCalls, with signatures).
            let mut model_parts: Vec<Part> = Vec::new();
            if !text.is_empty() {
                model_parts.push(Part::Text { text: text.clone() });
            }
            for (call, signature) in &pending_calls {
                model_parts.push(Part::FunctionCall {
                    function_call: call.clone(),
                    thought_signature: signature.clone(),
                });
            }
            if !model_parts.is_empty() {
                history.push(Content {
                    role: ContentRole::Model,
                    parts: model_parts,
                });
            }
            if !text.is_empty() {
                last_text = text;
            }

            // No tool calls → the subagent's turn is done.
            if pending_calls.is_empty() {
                break;
            }

            // Dispatch each call through the SHARED pipeline (ToolRunner +
            // dispatch_tool_call), build the functionResponse turn, loop.
            let mut response_parts: Vec<Part> = Vec::with_capacity(pending_calls.len());
            let mut saw_finish = false;
            for (call, _signature) in pending_calls {
                if call.name == FINISH_TOOL_NAME {
                    saw_finish = true;
                    response_parts.push(Part::FunctionResponse {
                        function_response: FunctionResponse {
                            name: call.name.clone(),
                            response: json!({ "ok": true }),
                        },
                    });
                    continue;
                }
                let tool_call = ToolCall {
                    name: call.name.clone(),
                    args: call.args.clone(),
                    id: None,
                    canonical_path: None,
                };
                // Inherit the parent's hooks/policies: a PreToolCall deny
                // (workspace_only / a custom containment policy) short-circuits
                // before the tool runs, with the same execute + error-lift
                // semantics as the main loop.
                let result =
                    dispatch_tool_call(Some(&runner), self.hooks.as_ref(), &turn_ctx, &tool_call)
                        .await;
                let value = result.result.clone().unwrap_or(Value::Null);
                response_parts.push(Part::FunctionResponse {
                    function_response: FunctionResponse {
                        name: call.name,
                        response: value,
                    },
                });
            }
            history.push(Content {
                role: ContentRole::User,
                parts: response_parts,
            });

            if saw_finish {
                finished = true;
                break;
            }
        }

        Ok(json!({
            "final_response": last_text,
            "finished": finished,
            "finish_reason": format!("{:?}", finish_reason.unwrap_or(FinishReason::Stop)),
        }))
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::filesystem::NativeFilesystem;

    /// The reduced runner registers EXACTLY the filesystem builtins + finish
    /// when a filesystem is supplied — and never `start_subagent` (no nested
    /// spawning, depth bounded to one), `run_command`, `call_agent`, or any
    /// value-moving / owner-only tool. This is the safety invariant: a
    /// subagent's tool surface is a strict reduction of the parent's.
    #[test]
    fn reduced_runner_grants_only_fs_and_finish() {
        let client = Arc::new(
            crate::backends::gemini::api::GeminiClient::new("offline-test-key")
                .expect("client builds"),
        );
        let fs: SharedFilesystem = Arc::new(NativeFilesystem::new());
        let sub = StartSubagent::with_filesystem(client, "gemini-test", Some(fs), None);
        let runner = sub.build_runner();
        let mut names = runner.names();
        names.sort();

        let mut expected = vec![
            "list_directory",
            "search_directory",
            "find_file",
            "view_file",
            "create_file",
            "edit_file",
            "delete_file",
            "rename_file",
            "finish",
        ];
        expected.sort();
        assert_eq!(names, expected, "subagent must get exactly the reduced fs + finish set");

        // Hard negatives: the tools a subagent must NEVER get.
        for forbidden in [
            "start_subagent",
            "run_command",
            "call_agent",
            "generate_image",
            "configure_agent",
            "ask_question",
        ] {
            assert!(
                !names.iter().any(|n| n == forbidden),
                "subagent must NOT get `{forbidden}`"
            );
        }
    }

    /// Without a filesystem the subagent is text-only + finish — the fs
    /// builtins gate on a supplied `Filesystem`, never silently appearing.
    #[test]
    fn no_filesystem_means_finish_only() {
        let client = Arc::new(
            crate::backends::gemini::api::GeminiClient::new("offline-test-key")
                .expect("client builds"),
        );
        let sub = StartSubagent::new(client, "gemini-test");
        let runner = sub.build_runner();
        assert_eq!(runner.names(), vec!["finish".to_string()]);
    }

    /// The subagent's wire tool declarations are built from the SAME tools the
    /// runner dispatches, so a declared tool always has a live implementation
    /// (no schema/impl drift) and every declared schema is Gemini-safe.
    #[test]
    fn declarations_match_runner_and_have_single_type_schemas() {
        fn assert_single_type(v: &Value, tool: &str) {
            match v {
                Value::Object(map) => {
                    if let Some(t) = map.get("type") {
                        assert!(!t.is_array(), "tool `{tool}` has a union `type` — Gemini 400s");
                    }
                    for val in map.values() {
                        assert_single_type(val, tool);
                    }
                }
                Value::Array(arr) => {
                    for val in arr {
                        assert_single_type(val, tool);
                    }
                }
                _ => {}
            }
        }

        let client = Arc::new(
            crate::backends::gemini::api::GeminiClient::new("offline-test-key")
                .expect("client builds"),
        );
        let fs: SharedFilesystem = Arc::new(NativeFilesystem::new());
        let sub = StartSubagent::with_filesystem(client, "gemini-test", Some(fs), None);
        let runner = sub.build_runner();
        let runner_names = runner.names();
        for tool in runner.iter_tools() {
            assert!(
                runner_names.iter().any(|n| n == tool.name()),
                "declared tool `{}` must have a live impl in the runner",
                tool.name()
            );
            assert_single_type(&tool.input_schema(), tool.name());
        }
    }
}
