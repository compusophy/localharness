//! Deterministic, offline mock backend for testing agents.
//!
//! [`MockConnection`](crate::backends::mock::MockConnection) is a scripted
//! [`ConnectionStrategy`](crate::ConnectionStrategy)
//! / [`Connection`](crate::Connection) that replays a fixed sequence of model
//! turns with **no network, no API key, and no LLM** — so SDK consumers (and
//! the crate's own tests) can unit-test an [`Agent`](crate::Agent)'s behavior
//! (the tool loop, hooks, policies, triggers) deterministically and offline.
//!
//! It is NOT a parallel re-implementation of the turn loop: the mock drives
//! the REAL shared turn engine (`backends::turn_engine` — the same loop the
//! live Gemini/Anthropic/OpenAI backends ride) through a [`TurnProvider`]
//! whose "model stream" is the scripted step sequence. A scripted turn splits
//! into engine ROUNDS at the real model boundary — a round ends after its
//! tool calls; text scripted after a tool call streams as the model's
//! next-round reply to the tool results. So each `agent.chat(...)` exercises
//! the exact shipped scaffold: streamed text-delta steps
//! ([`StepStatus::Active`](crate::StepStatus), `content_delta`), tool-call
//! steps ([`StepType::ToolCall`](crate::StepType)) dispatched inline through
//! the injected [`ToolRunner`](crate::ToolRunner) (running the same hooks +
//! policies), the `finish`-tool special case, and a turn-terminal step
//! ([`StepStatus::Done`](crate::StepStatus), `is_complete_response: true`).
//! Each `agent.chat(...)` / `Connection::send` consumes the next scripted turn.
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

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use parking_lot::Mutex;
use tokio::sync::broadcast;
use tracing::debug;

// Re-exported here so consumers can `use localharness::backends::mock::{
// MockAgentConfig, MockConnection}` in one line. The config itself lives in
// `agent.rs` next to the other per-backend agent configs.
pub use crate::agent::MockAgentConfig;

use crate::backends::turn_engine::{
    self, DispatchedResult, EmitCtx, EngineDeps, ResolvedCall, TurnProvider,
};
use crate::connections::{Connection, ConnectionStrategy, StepStream};
use crate::content::Content;
use crate::error::{Error, Result};
use crate::types::{Step, StepStatus, ToolResult, UsageMetadata};

const STEP_BROADCAST_CAPACITY: usize = 256;

// =============================================================================
// Script: a scripted turn + the actions it replays
// =============================================================================

/// One scripted action the mock replays within a turn, in order.
#[derive(Debug, Clone)]
enum ScriptAction {
    /// Stream a conversational text delta (a `content_delta` step).
    Text(String),
    /// Request a tool call. The engine dispatches it inline through hooks +
    /// policies + the injected [`ToolRunner`](crate::ToolRunner), exactly like
    /// the live backends; without a runner the dispatch surfaces the shared
    /// "no tool runner registered" error result (also like the live backends).
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
    /// the reply a consumer reads via `ChatResponse::text()` (mirrors a
    /// streaming model emitting deltas that sum to the final message).
    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.actions.push(ScriptAction::Text(text.into()));
        self
    }

    /// Append a tool call the model "requests" at this point in the turn.
    /// `args` is the JSON the tool receives; with a [`ToolRunner`](crate::ToolRunner)
    /// injected, it executes inline through the agent's hooks + policies.
    /// Text appended AFTER a tool call streams in the model's next engine
    /// round (its reply to the tool results), exactly like a live backend.
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
/// through the same hooks + policies + [`ToolRunner`](crate::ToolRunner) the live backends use —
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
            state: Arc::new(MockLoopState::new(steps_tx)),
            conversation_id: self.conversation_id.clone().into(),
            runners: self.runners.clone(),
        });
        Ok(Arc::new(MockConnection { inner }))
    }
}

// =============================================================================
// The mock side of the TurnProvider seam
// =============================================================================

/// The mock's wire-history entry: a readable tag (`"user:.."`,
/// `"assistant:{text}:{call count}"`, `"tool:{name}:{value}"`). History is
/// engine bookkeeping only — the mock's turns are scripted, not derived from
/// it, and it exposes no history API (`set_history_bytes` is a no-op).
type MockMsg = String;

/// Per-connection mutable state — the shared generic container the live
/// backends use, specialised to the mock's history tags.
type MockLoopState = crate::backends::state::LoopState<MockMsg>;

/// One scripted "stream event" fed to the engine — a [`ScriptAction`] plus
/// the turn's usage report (folded into the round accumulator so the terminal
/// step carries it, like a live wire's usage chunk).
#[derive(Clone)]
enum MockEvent {
    Text(String),
    Call { name: String, args: serde_json::Value },
    Usage(UsageMetadata),
}

/// One round's accumulator: the tool calls "streamed" this round + usage.
#[derive(Default)]
struct MockAccum {
    calls: Vec<(String, serde_json::Value)>,
    usage: Option<UsageMetadata>,
}

/// The mock side of the [`TurnProvider`] seam — the engine is monomorphized
/// over it exactly as over the live providers, so mock-driven tests exercise
/// the shipped turn loop.
struct MockProvider;

impl TurnProvider for MockProvider {
    type Message = MockMsg;
    type Config = ();
    type Request = ();
    type Event = MockEvent;
    type Accum = MockAccum;

    fn build_request(_config: &(), _history: &[MockMsg]) {}

    fn compaction_threshold(_config: &()) -> Option<u32> {
        None
    }

    fn fold_event(
        acc: &mut MockAccum,
        ctx: &mut EmitCtx<'_, MockMsg>,
        ev: MockEvent,
    ) -> Result<()> {
        match ev {
            MockEvent::Text(t) => ctx.push_text(&t),
            MockEvent::Call { name, args } => acc.calls.push((name, args)),
            MockEvent::Usage(u) => acc.usage = Some(u),
        }
        Ok(())
    }

    /// Scripted args are already-parsed `Value`s — never a parse error, and
    /// no wire correlation id (like Gemini, the mock correlates by name).
    fn resolve_pending_calls(acc: &mut MockAccum) -> Vec<ResolvedCall> {
        std::mem::take(&mut acc.calls)
            .into_iter()
            .map(|(name, args)| ResolvedCall {
                id: None,
                name,
                args,
                parse_error: None,
            })
            .collect()
    }

    fn round_usage(acc: &MockAccum) -> UsageMetadata {
        acc.usage.clone().unwrap_or_default()
    }

    fn map_finish_reason(_acc: &MockAccum) -> (StepStatus, &'static str) {
        (StepStatus::Done, "")
    }

    fn assemble_assistant_message(
        _acc: MockAccum,
        text: &str,
        calls: &[ResolvedCall],
    ) -> Option<MockMsg> {
        (!text.is_empty() || !calls.is_empty())
            .then(|| format!("assistant:{text}:{}", calls.len()))
    }

    fn tool_result_messages(results: Vec<DispatchedResult>) -> Vec<MockMsg> {
        results
            .into_iter()
            .map(|r| format!("tool:{}:{}", r.call.name, r.value))
            .collect()
    }
}

/// Split one scripted turn into engine ROUNDS at the real model boundary: a
/// streamed round ends after its tool calls, so text scripted AFTER a tool
/// call becomes the model's next-round reply to the tool results (a live
/// model can't keep talking in the same response after requesting tools).
/// The turn's usage report rides the first round (merged once — the engine
/// folds per-round usage into the terminal step's total).
fn split_rounds(turn: ScriptedTurn) -> VecDeque<Vec<MockEvent>> {
    let mut rounds: VecDeque<Vec<MockEvent>> = VecDeque::new();
    let mut cur: Vec<MockEvent> = Vec::new();
    let mut prev_was_call = false;
    for action in turn.actions {
        match action {
            ScriptAction::Text(t) => {
                if prev_was_call {
                    rounds.push_back(std::mem::take(&mut cur));
                    prev_was_call = false;
                }
                cur.push(MockEvent::Text(t));
            }
            ScriptAction::ToolCall { name, args } => {
                cur.push(MockEvent::Call { name, args });
                prev_was_call = true;
            }
        }
    }
    rounds.push_back(cur);
    if let Some(u) = turn.usage {
        if let Some(first) = rounds.front_mut() {
            first.insert(0, MockEvent::Usage(u));
        }
    }
    rounds
}

// =============================================================================
// Connection
// =============================================================================

/// A live, scripted session implementing [`Connection`]. Replays one
/// [`ScriptedTurn`] per [`Connection::send`] / `agent.chat(...)` — through
/// the REAL shared turn engine.
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
    state: Arc<MockLoopState>,
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
    /// Drive one turn through the shared engine, the scripted rounds standing
    /// in for the model stream (`open` pops the next round). The rounds are
    /// materialized LAZILY on the first stream open: the engine gates pre-turn
    /// hooks BEFORE opening, so a denied prompt consumes NO scripted turn.
    async fn run_turn(&self, prompt: Content) {
        let deps = EngineDeps::<MockProvider> {
            config: (),
            state: self.state.clone(),
            tool_runner: self.runners.tool_runner.clone(),
            hook_runner: self.runners.hook_runner.clone(),
            session_ctx: self.runners.session_ctx.clone(),
        };
        let user = format!("user:{}", prompt.as_text().unwrap_or_default());
        let rounds: Mutex<Option<VecDeque<Vec<MockEvent>>>> = Mutex::new(None);
        let res = turn_engine::run_turn::<MockProvider, _, _, _, _, _>(
            deps,
            user,
            prompt,
            |_req| {
                // Consume the next scripted turn on the FIRST open of this
                // turn; each later open (a new round after tool dispatch)
                // pops the next round. Past the end of the script — or past
                // the turn's last round — the "model" streams nothing, so an
                // over-sending test terminates cleanly.
                let evs = rounds
                    .lock()
                    .get_or_insert_with(|| {
                        let idx = self.next_turn.fetch_add(1, Ordering::Relaxed);
                        split_rounds(self.turns.get(idx).cloned().unwrap_or_default())
                    })
                    .pop_front()
                    .unwrap_or_default();
                async move {
                    Ok(futures_util::stream::iter(
                        evs.into_iter().map(Ok::<_, Error>),
                    ))
                }
            },
            || async {},
        )
        .await;
        // A deny / turn failure already surfaced as a turn_error Step (which
        // `subscribe_step_stream` turns into a stream `Err`).
        if let Err(e) = res {
            debug!(error = %e, "mock turn ended with error");
        }
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl Connection for MockConnection {
    fn is_idle(&self) -> bool {
        self.inner.state.idle.load(Ordering::Acquire)
    }

    fn conversation_id(&self) -> &str {
        &self.inner.conversation_id
    }

    async fn send(&self, content: Content) -> Result<()> {
        // Spawn the turn so `send` returns once dispatched (the live backends
        // do the same), letting streaming consumers subscribe before steps
        // land. The engine gates the prompt through the pre-turn hooks and
        // the mock only consumes the next scripted turn when the gate allows.
        let inner = self.inner.clone();
        crate::runtime::spawn(async move {
            inner.run_turn(content).await;
        });
        Ok(())
    }

    async fn send_trigger(&self, content: String) -> Result<()> {
        self.send(Content::text(content)).await
    }

    async fn send_tool_results(&self, _results: Vec<ToolResult>) -> Result<()> {
        // The engine dispatches scripted tool calls inline (like the live
        // backends), so out-of-band results are a no-op.
        Ok(())
    }

    fn subscribe_steps(&self) -> StepStream {
        // Turn-failure Steps surface as stream `Err` (uniform across
        // backends) — see `backends::subscribe_step_stream`.
        crate::backends::subscribe_step_stream(self.inner.state.steps.subscribe(), "mock")
    }

    async fn wait_for_idle(&self) -> Result<()> {
        loop {
            if self.is_idle() {
                return Ok(());
            }
            self.inner.state.idle_notify.notified().await;
        }
    }

    async fn shutdown(&self) -> Result<()> {
        self.inner.state.idle.store(true, Ordering::Release);
        self.inner.state.idle_notify.notify_waiters();
        Ok(())
    }

    /// Deliberate no-op: the mock's turns are scripted, not derived from
    /// history, so there is nothing to restore into. (`history_bytes` keeps
    /// the trait default `Ok(None)` — no snapshot either.)
    fn set_history_bytes(&self, _bytes: &[u8]) -> Result<()> {
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

    /// Round-splitting fidelity: text scripted AFTER a tool call streams as
    /// the model's NEXT engine round (the live backends' shape), and the
    /// consumer-visible reply is unchanged — `text()` concatenates the
    /// deltas across rounds; the tool still runs exactly once.
    #[tokio::test]
    async fn text_after_tool_call_rides_a_second_engine_round() {
        let count = Arc::new(AtomicUsize::new(0));
        let count_c = count.clone();
        let tool = ClosureTool::new(
            "ping",
            "Ping",
            json!({"type": "object"}),
            move |_args, _ctx| {
                let count_c = count_c.clone();
                async move {
                    count_c.fetch_add(1, Ordering::SeqCst);
                    Ok(json!({"ok": true}))
                }
            },
        );
        let backend = MockConnection::builder()
            .turn(|t| t.text("a").tool_call("ping", json!({})).text("b"))
            .build();
        let agent = Agent::start_mock(
            MockAgentConfig::new(backend)
                .with_tool(tool)
                .with_policies(vec![policy::allow_all()]),
        )
        .await
        .unwrap();

        let reply = agent.chat("go").await.unwrap().text().await.unwrap();
        assert_eq!(reply, "ab", "deltas from both rounds concatenate");
        assert_eq!(count.load(Ordering::SeqCst), 1, "the tool ran exactly once");
        agent.shutdown().await.unwrap();
    }

    /// A scripted `finish` call rides the ENGINE's finish special-case now:
    /// the terminal step is `Finish` and carries the summary + structured
    /// output (the old parallel mock loop dropped both).
    #[tokio::test]
    async fn scripted_finish_captures_summary_and_structured_output() {
        use crate::types::StepType;
        use futures_util::StreamExt;

        let strategy = MockConnection::builder()
            .turn(|t| {
                t.text("working").tool_call(
                    crate::builtins::FINISH_TOOL_NAME,
                    json!({"summary": "all done", "output": {"x": 1}}),
                )
            })
            .build();
        let conn = strategy.connect().await.expect("connects");
        let mut steps = conn.subscribe_steps();
        conn.send(Content::text("go")).await.expect("send dispatches");

        loop {
            let step = steps
                .next()
                .await
                .expect("steps flow")
                .expect("no turn error");
            if step.is_complete_response == Some(true) {
                assert_eq!(step.kind, StepType::Finish);
                assert_eq!(step.finish_summary.as_deref(), Some("all done"));
                assert_eq!(step.structured_output, Some(json!({"x": 1})));
                assert_eq!(step.content, "working");
                break;
            }
        }
        conn.shutdown().await.expect("clean shutdown");
    }
}
