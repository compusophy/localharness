//! Deterministic, offline mock backend for testing agents.
//!
//! [`MockConnection`](crate::backends::mock::MockConnection) is a scripted
//! [`ConnectionStrategy`](crate::ConnectionStrategy)
//! / [`Connection`](crate::Connection) that replays a fixed sequence of model
//! turns with **no network, no API key, and no LLM** — so SDK consumers (and
//! the crate's own tests) can unit-test an [`Agent`](crate::Agent)'s behavior
//! (the tool loop, hooks, policies, triggers) deterministically and offline.
//!
//! It is a faithful drop-in for a real backend: each scripted turn emits the
//! exact same [`Step`](crate::Step) shapes the live Gemini/Anthropic loops do —
//! streamed text-delta steps ([`StepStatus::Active`](crate::StepStatus),
//! `content_delta`), tool-call steps ([`StepType::ToolCall`](crate::StepType))
//! with inline tool dispatch through the injected [`ToolRunner`](crate::ToolRunner)
//! (running the same hooks + policies), and a turn-terminal step
//! ([`StepStatus::Done`](crate::StepStatus), `is_complete_response: true`). Each
//! `agent.chat(...)` / `Connection::send` consumes the next scripted turn.
//!
//! Always available (no feature flag): the mock pulls no dependencies the core
//! crate doesn't already use, and compiles on `wasm32` exactly like the live
//! backends — so both the crate's own tests and consumers' dev-deps benefit.
//! Use it from your crate's tests via `localharness::backends::mock`.
//!
//! # Example
//!
//! Script a tool-call flow and assert it runs offline against a real
//! [`Agent`](crate::Agent):
//!
//! ```rust,no_run
//! use localharness::{Agent, ClosureTool, policy};
//! use localharness::backends::mock::{MockAgentConfig, MockConnection};
//! use serde_json::json;
//!
//! # async fn run() -> localharness::Result<()> {
//! // A custom tool the script will call.
//! let greet = ClosureTool::new(
//!     "greet",
//!     "Greet someone",
//!     json!({"type": "object", "properties": {"name": {"type": "string"}}}),
//!     |args, _ctx| async move {
//!         let name = args["name"].as_str().unwrap_or("world");
//!         Ok(json!({"text": format!("hello {name}")}))
//!     },
//! );
//!
//! // Script ONE turn: call `greet`, then reply with text.
//! let backend = MockConnection::builder()
//!     .turn(|t| t.tool_call("greet", json!({"name": "ada"})).text("done"))
//!     .build();
//!
//! let agent = Agent::start_mock(
//!     MockAgentConfig::new(backend)
//!         .with_tool(greet)
//!         .with_policies(vec![policy::allow_all()]),
//! )
//! .await?;
//!
//! let reply = agent.chat("hi").await?.text().await?;
//! assert_eq!(reply, "done"); // the tool ran inline; the scripted text replied
//! agent.shutdown().await?;
//! # Ok(())
//! # }
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use async_trait::async_trait;
use futures_util::stream::StreamExt;
use tokio::sync::{broadcast, Notify};
use tokio_stream::wrappers::BroadcastStream;

// Re-exported here so consumers can `use localharness::backends::mock::{
// MockAgentConfig, MockConnection}` in one line. The config itself lives in
// `agent.rs` next to the other per-backend agent configs.
pub use crate::agent::MockAgentConfig;

use crate::connections::{Connection, ConnectionStrategy, StepStream};
use crate::content::Content;
use crate::error::{Error, Result};
use crate::tools::ToolRunner;
use crate::types::{Step, StepStatus, ToolCall, ToolResult, UsageMetadata};

const STEP_BROADCAST_CAPACITY: usize = 256;

// =============================================================================
// Script: a scripted turn + the actions it replays
// =============================================================================

/// One scripted action the mock replays within a turn, in order.
#[derive(Debug, Clone)]
enum ScriptAction {
    /// Stream a conversational text delta (a `content_delta` step). The
    /// terminal step's `content` is the concatenation of every `Text`.
    Text(String),
    /// Request a tool call. When a [`ToolRunner`] is injected (the Agent path)
    /// the mock dispatches it inline through hooks + policies, exactly like the
    /// live backends; otherwise it only surfaces the call on the step stream.
    ToolCall {
        name: String,
        args: serde_json::Value,
    },
}

/// A single scripted model turn: an ordered list of text deltas and/or
/// tool-calls, optionally reporting token usage on its terminal step.
///
/// Build one with the closure passed to [`MockConnectionBuilder::turn`], or
/// construct directly and pass a `Vec<ScriptedTurn>` to
/// [`MockConnectionBuilder::turns`].
#[derive(Debug, Clone, Default)]
pub struct ScriptedTurn {
    actions: Vec<ScriptAction>,
    usage: Option<UsageMetadata>,
}

impl ScriptedTurn {
    /// An empty turn. Add actions with [`Self::text`] / [`Self::tool_call`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a streamed text delta. Multiple `text` calls concatenate into
    /// the turn-terminal step's `content` (mirrors a streaming model emitting
    /// deltas that sum to the final message).
    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.actions.push(ScriptAction::Text(text.into()));
        self
    }

    /// Append a tool call the model "requests" at this point in the turn.
    /// `args` is the JSON the tool receives. With a [`ToolRunner`] injected,
    /// the mock executes it inline through the agent's hooks + policies.
    pub fn tool_call(mut self, name: impl Into<String>, args: serde_json::Value) -> Self {
        self.actions.push(ScriptAction::ToolCall {
            name: name.into(),
            args,
        });
        self
    }

    /// Report token usage on this turn's terminal step (the only step that
    /// carries usage — matching the live backends, so `cumulative_usage`
    /// counts each turn exactly once).
    pub fn with_usage(mut self, usage: UsageMetadata) -> Self {
        self.usage = Some(usage);
        self
    }

    /// The concatenated text content of the turn (the terminal step's body).
    fn content(&self) -> String {
        let mut out = String::new();
        for a in &self.actions {
            if let ScriptAction::Text(t) = a {
                out.push_str(t);
            }
        }
        out
    }
}

// =============================================================================
// Builder
// =============================================================================

/// Fluent builder for a scripted [`MockConnectionStrategy`].
///
/// See the [module docs](self) for an end-to-end example.
#[derive(Default)]
pub struct MockConnectionBuilder {
    turns: Vec<ScriptedTurn>,
    conversation_id: Option<String>,
}

impl MockConnectionBuilder {
    /// Start an empty builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a scripted turn built by the closure. Each call adds one turn;
    /// the Nth `agent.chat(...)` replays the Nth turn.
    ///
    /// ```rust
    /// use localharness::backends::mock::MockConnection;
    /// use serde_json::json;
    ///
    /// let backend = MockConnection::builder()
    ///     .turn(|t| t.text("first answer"))
    ///     .turn(|t| t.tool_call("search", json!({"q": "rust"})).text("found it"))
    ///     .build();
    /// # let _ = backend;
    /// ```
    pub fn turn(mut self, f: impl FnOnce(ScriptedTurn) -> ScriptedTurn) -> Self {
        self.turns.push(f(ScriptedTurn::new()));
        self
    }

    /// Append a pre-built scripted turn (use when you already hold a
    /// [`ScriptedTurn`], e.g. one built in a loop).
    pub fn push_turn(mut self, turn: ScriptedTurn) -> Self {
        self.turns.push(turn);
        self
    }

    /// Replace the whole script with an ordered list of turns.
    pub fn turns(mut self, turns: Vec<ScriptedTurn>) -> Self {
        self.turns = turns;
        self
    }

    /// Set a fixed conversation id (default: `"mock-conversation"`).
    pub fn conversation_id(mut self, id: impl Into<String>) -> Self {
        self.conversation_id = Some(id.into());
        self
    }

    /// Finish building the strategy.
    pub fn build(self) -> MockConnectionStrategy {
        MockConnectionStrategy {
            turns: Arc::new(self.turns),
            conversation_id: self
                .conversation_id
                .unwrap_or_else(|| "mock-conversation".to_string()),
            runners: MockRunners::default(),
        }
    }
}

// =============================================================================
// Runners (injected by Agent::start_mock)
// =============================================================================

/// Runners the Agent injects so the mock can dispatch tool calls inline
/// through the same hooks + policies + [`ToolRunner`] the live backends use —
/// an alias of the shared [`BackendRunners`](crate::backends::BackendRunners).
pub type MockRunners = crate::backends::BackendRunners;

// =============================================================================
// Strategy
// =============================================================================

/// Factory that opens a scripted [`MockConnection`]. Build one via
/// [`MockConnection::builder`].
pub struct MockConnectionStrategy {
    turns: Arc<Vec<ScriptedTurn>>,
    conversation_id: String,
    runners: MockRunners,
}

impl MockConnectionStrategy {
    /// Inject the runners the Agent owns so scripted tool calls dispatch
    /// inline through hooks + policies + the tool runner. Parallels
    /// `GeminiConnectionStrategy::with_runners`.
    pub fn with_runners(mut self, runners: MockRunners) -> Self {
        self.runners = runners;
        self
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl ConnectionStrategy for MockConnectionStrategy {
    async fn connect(&self) -> Result<Arc<dyn Connection>> {
        let (steps_tx, _) = broadcast::channel::<Step>(STEP_BROADCAST_CAPACITY);
        let inner = Arc::new(MockInner {
            turns: self.turns.clone(),
            next_turn: AtomicUsize::new(0),
            step_index: AtomicUsize::new(0),
            steps: steps_tx,
            idle: AtomicBool::new(true),
            idle_notify: Notify::new(),
            conversation_id: self.conversation_id.clone().into(),
            runners: self.runners.clone(),
        });
        Ok(Arc::new(MockConnection { inner }))
    }
}

// =============================================================================
// Connection
// =============================================================================

/// A live, scripted session implementing [`Connection`]. Replays one
/// [`ScriptedTurn`] per [`Connection::send`] / `agent.chat(...)`.
///
/// Construct it via the [builder](MockConnection::builder); the
/// [`MockConnectionStrategy`] it produces is what [`Agent::start_mock`]
/// consumes.
///
/// [`Agent::start_mock`]: crate::Agent::start_mock
pub struct MockConnection {
    inner: Arc<MockInner>,
}

/// Shared, cheaply-cloneable turn-running state. Held behind an `Arc` so
/// [`Connection::send`] can hand a clone to the spawned turn task (the live
/// backends clone an `Arc`-backed `deps_template` the same way).
struct MockInner {
    turns: Arc<Vec<ScriptedTurn>>,
    next_turn: AtomicUsize,
    step_index: AtomicUsize,
    steps: broadcast::Sender<Step>,
    idle: AtomicBool,
    idle_notify: Notify,
    conversation_id: Arc<str>,
    runners: MockRunners,
}

impl MockConnection {
    /// Start building a scripted mock backend.
    pub fn builder() -> MockConnectionBuilder {
        MockConnectionBuilder::new()
    }
}

impl MockInner {
    fn alloc_step_index(&self) -> u32 {
        self.step_index.fetch_add(1, Ordering::Relaxed) as u32
    }

    fn emit(&self, step: Step) {
        let _ = self.steps.send(step);
    }

    /// Replay one scripted turn: stream its text deltas, dispatch its tool
    /// calls inline through hooks + policies + the tool runner (when injected),
    /// then emit the turn-terminal step. Faithful to the live `run_turn` shape:
    /// streamed `Active` deltas + a single `Done` terminal carrying the full
    /// `content` (and, if scripted, the turn's usage).
    async fn run_turn(&self, turn: ScriptedTurn) {
        self.idle.store(false, Ordering::Release);
        let traj = uuid::Uuid::new_v4().to_string();

        for action in &turn.actions {
            match action {
                ScriptAction::Text(delta) => {
                    self.emit(Step::text_delta(&traj, self.alloc_step_index(), delta));
                }
                ScriptAction::ToolCall { name, args } => {
                    let tool_call = ToolCall {
                        name: name.clone(),
                        args: args.clone(),
                        id: None,
                        canonical_path: None,
                    };
                    // Surface the call on the stream (UIs flip the tool block to
                    // "running"), exactly like the live loop's tool-call step.
                    // `Done` (not `Active`) is deliberate: the mock ALREADY
                    // dispatches this tool call inline (matching the live
                    // backends), so the step exists only for OBSERVABILITY —
                    // `ChatResponse::tool_calls()` reads `tool_calls` regardless
                    // of status. The Agent's `spawn_tool_dispatcher` SKIPS
                    // `Done` steps, so this step does NOT trigger a redundant
                    // second dispatch. (It is non-terminal —
                    // `is_complete_response: Some(false)` and
                    // `target: Environment` — so it never ends the
                    // `ChatResponse`.)
                    self.emit(Step::tool_call(
                        self.alloc_step_index(),
                        tool_call.clone(),
                        StepStatus::Done,
                    ));

                    // Dispatch inline through hooks + policies + the runner when
                    // the Agent injected them. This is what makes a scripted
                    // tool-call flow actually RUN the tool offline — like the
                    // live backends, the result feeds the conversation rather
                    // than being re-broadcast as its own step.
                    if let Some(runner) = self.runners.tool_runner.as_ref() {
                        let _result = self.dispatch_tool(&tool_call, runner).await;
                    }
                }
            }
        }

        self.emit(Step::turn_complete(
            traj,
            self.alloc_step_index(),
            StepStatus::Done,
            turn.content(),
            "",
            None,
            turn.usage,
        ));

        self.idle.store(true, Ordering::Release);
        self.idle_notify.notify_waiters();
    }

    /// Run a scripted tool call through the injected hooks + policies + tool
    /// runner — the same pipeline the live backends use — and return the typed
    /// result. A denied call (policy / pre-tool-call hook) yields an error
    /// result without executing the tool.
    async fn dispatch_tool(&self, call: &ToolCall, runner: &ToolRunner) -> ToolResult {
        let turn_ctx = self
            .runners
            .session_ctx
            .as_ref()
            .map(|s| s.child())
            .unwrap_or_default();

        let (decision, op_ctx) = if let Some(hooks) = self.runners.hook_runner.as_ref() {
            hooks.dispatch_pre_tool_call(&turn_ctx, call).await
        } else {
            (crate::types::HookResult::allow(), turn_ctx.clone())
        };

        let result = if !decision.allow {
            ToolResult::err(call.name.clone(), call.id.clone(), decision.message.clone())
        } else {
            match runner.execute(&call.name, call.args.clone()).await {
                Ok(v) => ToolResult::ok(call.name.clone(), call.id.clone(), v),
                Err(e) => ToolResult::err(call.name.clone(), call.id.clone(), e.to_string()),
            }
        };

        if let Some(hooks) = self.runners.hook_runner.as_ref() {
            hooks.dispatch_post_tool_call(&op_ctx, &result).await;
        }
        result
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl Connection for MockConnection {
    fn is_idle(&self) -> bool {
        self.inner.idle.load(Ordering::Acquire)
    }

    fn conversation_id(&self) -> &str {
        &self.inner.conversation_id
    }

    async fn send(&self, _content: Content) -> Result<()> {
        // Consume the next scripted turn. Past the end of the script, every
        // send is an empty terminal turn (a model with nothing left to say) —
        // so an over-sending test terminates cleanly instead of hanging.
        let idx = self.inner.next_turn.fetch_add(1, Ordering::Relaxed);
        let turn = self.inner.turns.get(idx).cloned().unwrap_or_default();
        // Spawn the turn so `send` returns once dispatched (the live backends
        // do the same), letting streaming consumers subscribe before steps land.
        let inner = self.inner.clone();
        crate::runtime::spawn(async move {
            inner.run_turn(turn).await;
        });
        Ok(())
    }

    async fn send_trigger(&self, content: String) -> Result<()> {
        self.send(Content::text(content)).await
    }

    async fn send_tool_results(&self, _results: Vec<ToolResult>) -> Result<()> {
        // The mock dispatches scripted tool calls inline (like the Gemini
        // backend), so out-of-band results are a no-op.
        Ok(())
    }

    fn subscribe_steps(&self) -> StepStream {
        let rx = self.inner.steps.subscribe();
        let mapped = BroadcastStream::new(rx)
            .map(|r| r.map_err(|e| Error::other(format!("mock step lag: {e}"))));
        #[cfg(not(target_arch = "wasm32"))]
        {
            mapped.boxed()
        }
        #[cfg(target_arch = "wasm32")]
        {
            mapped.boxed_local()
        }
    }

    async fn wait_for_idle(&self) -> Result<()> {
        loop {
            if self.is_idle() {
                return Ok(());
            }
            self.inner.idle_notify.notified().await;
        }
    }

    async fn shutdown(&self) -> Result<()> {
        self.inner.idle.store(true, Ordering::Release);
        self.inner.idle_notify.notify_waiters();
        Ok(())
    }
}

// =============================================================================
// Tests — these read like the example a consumer would write.
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
    use crate::policy;
    use crate::tools::ClosureTool;
    use parking_lot::Mutex;
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    /// THE demonstrating test: a consumer scripts a tool-call flow and asserts
    /// it runs deterministically OFFLINE — no network, no key, no LLM.
    ///
    /// Script ONE turn: `{ tool_call(record_fact) -> text("logged") }`.
    /// Assert: the tool actually RAN (its side effect fired, with the scripted
    /// args) AND the agent's final text is the scripted reply. This is the
    /// whole point — agent logic (the tool loop) tested with a mock model.
    #[tokio::test]
    async fn scripted_tool_call_flow_runs_offline() {
        // A tool whose side effect we can observe: it COUNTS each invocation
        // (so a double-dispatch would be caught) and records the args it saw.
        let count = Arc::new(AtomicUsize::new(0));
        let recorded: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let count_c = count.clone();
        let recorded_c = recorded.clone();
        let record_fact = ClosureTool::new(
            "record_fact",
            "Persist a fact",
            json!({"type": "object", "properties": {"fact": {"type": "string"}}}),
            move |args, _ctx| {
                let count_c = count_c.clone();
                let recorded_c = recorded_c.clone();
                async move {
                    count_c.fetch_add(1, Ordering::SeqCst);
                    let fact = args["fact"].as_str().unwrap_or_default().to_string();
                    *recorded_c.lock() = Some(fact);
                    Ok(json!({"ok": true}))
                }
            },
        );

        // Script the model's behavior: call the tool, then answer.
        let backend = MockConnection::builder()
            .turn(|t| {
                t.tool_call("record_fact", json!({"fact": "the sky is blue"}))
                    .text("logged")
            })
            .build();

        let agent = Agent::start_mock(
            MockAgentConfig::new(backend)
                .with_tool(record_fact)
                .with_policies(vec![policy::allow_all()]),
        )
        .await
        .expect("mock agent starts");

        let reply = agent
            .chat("remember a fact")
            .await
            .expect("chat starts")
            .text()
            .await
            .expect("turn completes");

        // 1. The scripted tool executed EXACTLY ONCE, with the scripted args.
        //    Exactly-once proves the mock dispatches inline (like the live
        //    backends) without the Agent's step dispatcher double-firing it.
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "the scripted tool must run exactly once",
        );
        assert_eq!(
            recorded.lock().as_deref(),
            Some("the sky is blue"),
            "the tool received the scripted args",
        );
        // 2. The agent's final text is the scripted reply.
        assert_eq!(reply, "logged", "the scripted terminal text is returned");

        agent.shutdown().await.expect("clean shutdown");
    }

    /// A policy that denies the scripted tool must BLOCK it: the tool's side
    /// effect never fires, but the turn still completes (the model's text
    /// still streams). Proves the mock drives the real hooks/policy pipeline.
    #[tokio::test]
    async fn denied_tool_call_does_not_execute() {
        let ran = Arc::new(AtomicBool::new(false));
        let ran_c = ran.clone();
        let tool = ClosureTool::new(
            "danger",
            "A blocked tool",
            json!({"type": "object"}),
            move |_args, _ctx| {
                let ran_c = ran_c.clone();
                async move {
                    ran_c.store(true, Ordering::SeqCst);
                    Ok(json!({"ok": true}))
                }
            },
        );

        let backend = MockConnection::builder()
            .turn(|t| t.tool_call("danger", json!({})).text("attempted"))
            .build();

        // deny_all → the pre-tool-call policy hook blocks every call.
        let agent = Agent::start_mock(
            MockAgentConfig::new(backend)
                .with_tool(tool)
                .with_policies(vec![policy::deny_all()]),
        )
        .await
        .expect("mock agent starts");

        let reply = agent.chat("go").await.unwrap().text().await.unwrap();

        assert!(
            !ran.load(Ordering::SeqCst),
            "a denied tool must NOT execute its body",
        );
        assert_eq!(reply, "attempted", "the turn still completes");
        agent.shutdown().await.unwrap();
    }

    /// The scripted tool call is also OBSERVABLE on the response stream — a
    /// consumer can assert which tool the (mock) model dispatched via the
    /// public `ChatResponse::tool_calls()` cursor, with the scripted args.
    #[tokio::test]
    async fn scripted_tool_call_is_visible_on_the_stream() {
        use futures_util::StreamExt;

        let tool = ClosureTool::new(
            "search",
            "Search",
            json!({"type": "object", "properties": {"q": {"type": "string"}}}),
            |_args, _ctx| async move { Ok(json!({"hits": 0})) },
        );
        let backend = MockConnection::builder()
            .turn(|t| t.tool_call("search", json!({"q": "rust"})).text("none found"))
            .build();
        let agent = Agent::start_mock(
            MockAgentConfig::new(backend)
                .with_tool(tool)
                .with_policies(vec![policy::allow_all()]),
        )
        .await
        .unwrap();

        let resp = agent.chat("find rust").await.unwrap();
        let mut calls = resp.tool_calls();
        let first = calls
            .next()
            .await
            .expect("a tool call is surfaced")
            .expect("ok");
        assert_eq!(first.name, "search");
        assert_eq!(first.args, json!({"q": "rust"}));
        agent.shutdown().await.unwrap();
    }

    /// Multi-turn determinism: the Nth `chat` replays the Nth scripted turn,
    /// and per-turn usage accumulates exactly like the live backends.
    #[tokio::test]
    async fn turns_replay_in_order_with_usage() {
        let backend = MockConnection::builder()
            .turn(|t| {
                t.text("first").with_usage(UsageMetadata {
                    total_token_count: Some(10),
                    ..Default::default()
                })
            })
            .turn(|t| {
                t.text("second").with_usage(UsageMetadata {
                    total_token_count: Some(20),
                    ..Default::default()
                })
            })
            .build();

        let agent = Agent::start_mock(MockAgentConfig::new(backend))
            .await
            .expect("mock agent starts");

        let r1 = agent.chat("a").await.unwrap().text().await.unwrap();
        assert_eq!(r1, "first");
        let r2 = agent.chat("b").await.unwrap().text().await.unwrap();
        assert_eq!(r2, "second");

        // Usage summed across both turns, counted once each.
        assert_eq!(
            agent.cumulative_usage().total_token_count,
            Some(30),
            "10 + 20, each turn counted once",
        );
        agent.shutdown().await.unwrap();
    }
}
