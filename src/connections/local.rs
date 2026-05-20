//! Local-harness `ConnectionStrategy` implementation.
//!
//! Spawns the bundled `localharness` binary, performs the proto handshake
//! on stdin/stdout, then upgrades to a long-lived WebSocket. Three async
//! tasks supervise the live session:
//!
//! ```text
//!     ┌────────────┐   inbox: mpsc(InputEvent)   ┌─────────────────┐
//!     │  callers   │ ─────────────────────────► │ ws_writer task  │
//!     └────────────┘                             └────────┬────────┘
//!                                                         │
//!                                                  ┌──────▼──────┐
//!                                                  │ websocket   │
//!                                                  └──────▲──────┘
//!                                                         │
//!     ┌────────────┐ broadcast(Step) ◄───────────┌────────┴────────┐
//!     │ subscribers│ ◄──────────────────────────│ ws_reader task  │
//!     └────────────┘                             └─────────────────┘
//! ```
//!
//! The reader publishes to a `tokio::sync::broadcast` so any number of
//! cursors can observe the trajectory in parallel without contention.
//! Backpressure on the writer side is bounded (16 in-flight messages) —
//! deeper queues mean either a stuck WS or a runaway producer, both of
//! which should surface as `Error::Closed` rather than silent memory growth.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use bytes::Bytes;
use futures_util::stream::{BoxStream, StreamExt};
use futures_util::SinkExt;
use parking_lot::Mutex as ParkingMutex;
use prost::Message as ProstMessage;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, mpsc, Notify};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio_stream::wrappers::BroadcastStream;
use tokio_tungstenite::tungstenite::http::Request;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use tracing::{debug, error, trace, warn};

use crate::connections::{Connection, ConnectionStrategy};
use crate::content::{Content, Part};
use crate::error::{Error, Result};
use crate::proto::{
    InputConfig, InputEvent, OutputConfig, OutputEvent, StepUpdateState, StepUpdateSource,
    StepUpdateTarget, ToolResponse, TrajectoryState, UserInput, UserInputMedia, UserInputPart,
};
use crate::types::{
    Step, StepSource, StepStatus, StepTarget, StepType, ToolCall, ToolResult,
};

/// Default size of the broadcast channel that fans Step events out to
/// subscribers. Subscribers that lag past this will receive
/// `Error::Other("lagged …")` and may resubscribe.
const STEP_BROADCAST_CAPACITY: usize = 256;
/// Bounded depth for outbound messages awaiting the WebSocket writer.
const INBOX_CAPACITY: usize = 16;
/// Handshake budget — generous enough for cold start, tight enough to fail
/// loudly when the harness is misconfigured.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Shutdown budget for stdin close + process wait.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

type WsStream = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for a local-harness connection.
#[derive(Debug, Clone, Default)]
pub struct LocalConfig {
    /// Filesystem path to the `localharness` binary. When `None`, the
    /// strategy falls back to `ANTIGRAVITY_HARNESS_PATH` and then a PATH
    /// lookup for `localharness` / `localharness.exe`.
    pub binary_path: Option<PathBuf>,
    /// Directory the harness uses for persistent state. Defaults to a
    /// subdirectory under the OS temp directory.
    pub storage_dir: Option<PathBuf>,
    /// Pre-existing conversation id to resume, or `None` to start fresh.
    pub conversation_id: Option<String>,
}

// =============================================================================
// Strategy
// =============================================================================

pub struct LocalConnectionStrategy {
    config: LocalConfig,
}

impl LocalConnectionStrategy {
    pub fn new(config: LocalConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl ConnectionStrategy for LocalConnectionStrategy {
    async fn connect(&self) -> Result<Arc<dyn Connection>> {
        let binary = resolve_binary(self.config.binary_path.as_deref())?;
        let storage = resolve_storage(self.config.storage_dir.as_deref())?;
        let conversation_id = self
            .config
            .conversation_id
            .clone()
            .unwrap_or_else(|| format!("conv-{}", short_random_id()));

        debug!(?binary, ?storage, %conversation_id, "spawning local harness");

        let mut child = Command::new(&binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;

        let mut stdin = child.stdin.take().ok_or_else(|| {
            Error::other("harness child returned no stdin handle (unexpected)")
        })?;
        let mut stdout = child.stdout.take().ok_or_else(|| {
            Error::other("harness child returned no stdout handle (unexpected)")
        })?;

        // Stderr is interesting for debugging; pipe each line into tracing.
        if let Some(stderr) = child.stderr.take() {
            spawn_stderr_logger(stderr);
        }

        let handshake = timeout(HANDSHAKE_TIMEOUT, async {
            // Write InputConfig length-prefixed.
            let input = InputConfig {
                storage_directory: storage.display().to_string(),
                port: 0,
                bind_address: "localhost".to_string(),
            };
            let mut buf = Vec::with_capacity(input.encoded_len());
            input.encode(&mut buf)?;
            stdin.write_all(&(buf.len() as u32).to_le_bytes()).await?;
            stdin.write_all(&buf).await?;
            stdin.flush().await?;

            // Read OutputConfig.
            let mut len_buf = [0u8; 4];
            stdout.read_exact(&mut len_buf).await?;
            let out_len = u32::from_le_bytes(len_buf) as usize;
            if out_len > 1 << 20 {
                return Err(Error::other(format!(
                    "harness reported OutputConfig length {} (refusing to allocate)",
                    out_len
                )));
            }
            let mut out_buf = vec![0u8; out_len];
            stdout.read_exact(&mut out_buf).await?;
            let cfg = OutputConfig::decode(out_buf.as_slice())?;
            Ok::<_, Error>(cfg)
        })
        .await
        .map_err(|_| Error::Timeout(HANDSHAKE_TIMEOUT))??;

        debug!(port = handshake.port, "harness handshake complete");

        // Open the WebSocket.
        let ws_url = format!("ws://localhost:{}/", handshake.port);
        let request = Request::builder()
            .uri(&ws_url)
            .header("x-goog-api-key", &handshake.api_key)
            .header("host", format!("localhost:{}", handshake.port))
            .body(())?;
        let (ws_stream, _resp) = timeout(HANDSHAKE_TIMEOUT, connect_async(request))
            .await
            .map_err(|_| Error::Timeout(HANDSHAKE_TIMEOUT))??;

        let (step_tx, _) = broadcast::channel(STEP_BROADCAST_CAPACITY);
        let (inbox_tx, inbox_rx) = mpsc::channel::<InputEvent>(INBOX_CAPACITY);
        let idle = Arc::new(AtomicBool::new(true));
        let idle_notify = Arc::new(Notify::new());
        let shutdown = Arc::new(AtomicBool::new(false));

        let supervisor = supervise_session(
            ws_stream,
            inbox_rx,
            step_tx.clone(),
            idle.clone(),
            idle_notify.clone(),
            shutdown.clone(),
        );

        // Process supervisor — surfaces the harness exit code through tracing
        // and triggers shutdown so the session does not silently leak.
        let process_supervisor = spawn_process_supervisor(child, shutdown.clone(), idle.clone(), idle_notify.clone());

        let conn = LocalConnection {
            conversation_id: conversation_id.into(),
            inbox: inbox_tx,
            steps: step_tx,
            idle,
            idle_notify,
            shutdown,
            tasks: ParkingMutex::new(SessionTasks {
                supervisor: Some(supervisor),
                process_supervisor: Some(process_supervisor),
            }),
        };

        Ok(Arc::new(conn))
    }
}

// =============================================================================
// Connection
// =============================================================================

struct SessionTasks {
    supervisor: Option<JoinHandle<()>>,
    process_supervisor: Option<JoinHandle<()>>,
}

pub struct LocalConnection {
    conversation_id: Arc<str>,
    inbox: mpsc::Sender<InputEvent>,
    steps: broadcast::Sender<Step>,
    idle: Arc<AtomicBool>,
    idle_notify: Arc<Notify>,
    shutdown: Arc<AtomicBool>,
    tasks: ParkingMutex<SessionTasks>,
}

impl LocalConnection {
    async fn send_event(&self, event: InputEvent) -> Result<()> {
        if self.shutdown.load(Ordering::Acquire) {
            return Err(Error::Closed);
        }
        self.inbox.send(event).await.map_err(|_| Error::Closed)
    }
}

#[async_trait]
impl Connection for LocalConnection {
    fn is_idle(&self) -> bool {
        self.idle.load(Ordering::Acquire)
    }

    fn conversation_id(&self) -> &str {
        &self.conversation_id
    }

    async fn send(&self, content: Content) -> Result<()> {
        let event = build_user_input_event(content)?;
        self.idle.store(false, Ordering::Release);
        self.send_event(event).await
    }

    async fn send_trigger(&self, content: String) -> Result<()> {
        let event = InputEvent {
            automated_trigger: Some(content),
            ..Default::default()
        };
        self.send_event(event).await
    }

    async fn send_tool_results(&self, results: Vec<ToolResult>) -> Result<()> {
        for r in results {
            let id = r.id.clone().ok_or_else(|| {
                Error::other("tool result missing id; cannot correlate with call")
            })?;
            let value = if let Some(err) = r.error.as_ref() {
                json!({ "error": err })
            } else {
                r.result.unwrap_or(serde_json::Value::Null)
            };
            let event = InputEvent {
                tool_response: Some(ToolResponse {
                    id,
                    response_json: value.to_string(),
                }),
                ..Default::default()
            };
            self.send_event(event).await?;
        }
        Ok(())
    }

    fn subscribe_steps(&self) -> BoxStream<'static, Result<Step>> {
        let rx = self.steps.subscribe();
        BroadcastStream::new(rx)
            .map(|res| match res {
                Ok(step) => Ok(step),
                Err(e) => Err(Error::other(format!("step stream lag: {e}"))),
            })
            .boxed()
    }

    async fn wait_for_idle(&self) -> Result<()> {
        loop {
            if self.is_idle() {
                return Ok(());
            }
            if self.shutdown.load(Ordering::Acquire) {
                return Err(Error::Closed);
            }
            self.idle_notify.notified().await;
        }
    }

    async fn shutdown(&self) -> Result<()> {
        if self.shutdown.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        // Wake any wait_for_idle parker.
        self.idle_notify.notify_waiters();
        // Closing inbox tells the writer to drop the WS, which cascades into
        // the reader and the process supervisor.
        // Dropping our local sender is enough because each clone in handler
        // tasks dies as the broadcast channel drops its receivers.
        // We don't have access to the inbox to close it explicitly without
        // taking &mut self; close via shutting down the channel by dropping
        // refs is fine because the supervisor watches `shutdown`.

        let tasks = {
            let mut guard = self.tasks.lock();
            SessionTasks {
                supervisor: guard.supervisor.take(),
                process_supervisor: guard.process_supervisor.take(),
            }
        };
        if let Some(handle) = tasks.supervisor {
            let _ = timeout(SHUTDOWN_TIMEOUT, handle).await;
        }
        if let Some(handle) = tasks.process_supervisor {
            let _ = timeout(SHUTDOWN_TIMEOUT, handle).await;
        }
        Ok(())
    }
}

impl Drop for LocalConnection {
    fn drop(&mut self) {
        // Best-effort: mark shutdown so any background tasks notice. We
        // cannot await here; the supervisors poll `shutdown` and the
        // `kill_on_drop` on the child process catches stragglers.
        self.shutdown.store(true, Ordering::Release);
        self.idle_notify.notify_waiters();
    }
}

// =============================================================================
// Supervisors
// =============================================================================

fn supervise_session(
    ws: WsStream,
    mut inbox: mpsc::Receiver<InputEvent>,
    steps: broadcast::Sender<Step>,
    idle: Arc<AtomicBool>,
    idle_notify: Arc<Notify>,
    shutdown: Arc<AtomicBool>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let (mut writer, mut reader) = ws.split();

        loop {
            tokio::select! {
                biased;

                _ = shutdown_signal(&shutdown) => {
                    debug!("session supervisor: shutdown requested");
                    let _ = writer.send(Message::Close(None)).await;
                    break;
                }

                event = inbox.recv() => {
                    match event {
                        Some(event) => {
                            let json = match serde_json::to_string(&event) {
                                Ok(s) => s,
                                Err(e) => {
                                    error!(?e, "failed to encode InputEvent");
                                    continue;
                                }
                            };
                            if let Err(e) = writer.send(Message::Text(json)).await {
                                warn!(?e, "websocket write failed");
                                break;
                            }
                        }
                        None => {
                            debug!("inbox closed; exiting writer loop");
                            let _ = writer.send(Message::Close(None)).await;
                            break;
                        }
                    }
                }

                next = reader.next() => {
                    match next {
                        Some(Ok(Message::Text(text))) => {
                            handle_text_frame(&text, &steps, &idle, &idle_notify);
                        }
                        Some(Ok(Message::Binary(bin))) => {
                            // Some harness builds switch to binary JSON; try
                            // to decode it as UTF-8 before complaining.
                            match std::str::from_utf8(&bin) {
                                Ok(text) => handle_text_frame(text, &steps, &idle, &idle_notify),
                                Err(_) => warn!("binary frame is not utf-8; dropping"),
                            }
                        }
                        Some(Ok(Message::Ping(p))) => {
                            if let Err(e) = writer.send(Message::Pong(p)).await {
                                warn!(?e, "pong failed");
                                break;
                            }
                        }
                        Some(Ok(Message::Close(_))) | None => {
                            debug!("websocket closed by peer");
                            break;
                        }
                        Some(Ok(_)) => {
                            // Pong/Frame: nothing to do.
                        }
                        Some(Err(e)) => {
                            warn!(?e, "websocket read error");
                            break;
                        }
                    }
                }
            }
        }

        shutdown.store(true, Ordering::Release);
        idle.store(true, Ordering::Release);
        idle_notify.notify_waiters();
    })
}

fn spawn_process_supervisor(
    mut child: Child,
    shutdown: Arc<AtomicBool>,
    idle: Arc<AtomicBool>,
    idle_notify: Arc<Notify>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        tokio::select! {
            _ = shutdown_signal(&shutdown) => {
                let _ = child.start_kill();
                let _ = timeout(SHUTDOWN_TIMEOUT, child.wait()).await;
            }
            status = child.wait() => {
                match status {
                    Ok(s) => debug!(?s, "harness exited"),
                    Err(e) => warn!(?e, "harness wait failed"),
                }
                shutdown.store(true, Ordering::Release);
                idle.store(true, Ordering::Release);
                idle_notify.notify_waiters();
            }
        }
    })
}

fn spawn_stderr_logger(mut stderr: tokio::process::ChildStderr) {
    tokio::spawn(async move {
        let mut buf = Vec::with_capacity(8 * 1024);
        let mut scratch = [0u8; 4096];
        loop {
            match stderr.read(&mut scratch).await {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend_from_slice(&scratch[..n]);
                    while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
                        let line: Vec<u8> = buf.drain(..=pos).collect();
                        let line = String::from_utf8_lossy(&line[..line.len() - 1]);
                        debug!(target: "antig::harness", "{}", line.trim_end_matches('\r'));
                    }
                }
                Err(e) => {
                    warn!(?e, "harness stderr read failed");
                    break;
                }
            }
        }
        if !buf.is_empty() {
            let tail = String::from_utf8_lossy(&buf);
            debug!(target: "antig::harness", "{tail}");
        }
    });
}

async fn shutdown_signal(flag: &AtomicBool) {
    loop {
        if flag.load(Ordering::Acquire) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// =============================================================================
// Wire-frame handling
// =============================================================================

fn handle_text_frame(
    text: &str,
    steps: &broadcast::Sender<Step>,
    idle: &AtomicBool,
    idle_notify: &Notify,
) {
    let event: OutputEvent = match serde_json::from_str(text) {
        Ok(e) => e,
        Err(e) => {
            trace!(?e, frame = %text, "discarding unparseable OutputEvent");
            return;
        }
    };

    if let Some(update) = event.step_update {
        let status = match update.state {
            StepUpdateState::Done => StepStatus::Done,
            StepUpdateState::Error => StepStatus::Error,
            StepUpdateState::WaitingForUser => StepStatus::WaitingForUser,
            StepUpdateState::Active => StepStatus::Active,
            StepUpdateState::Unspecified => StepStatus::Unknown,
        };

        let source = match update.source {
            StepUpdateSource::System => StepSource::System,
            StepUpdateSource::User => StepSource::User,
            StepUpdateSource::Model => StepSource::Model,
            StepUpdateSource::Unspecified => StepSource::Unknown,
        };

        let target = match update.target {
            StepUpdateTarget::User => StepTarget::User,
            StepUpdateTarget::Environment => StepTarget::Environment,
            StepUpdateTarget::Model => StepTarget::Unspecified,
            StepUpdateTarget::Unspecified => StepTarget::Unspecified,
        };

        let kind = match &update.finish {
            Some(_) => StepType::Finish,
            None => {
                let has_text = update.text_delta.is_some() || update.text.is_some();
                let has_thinking =
                    update.thinking_delta.is_some() || update.thinking.is_some();
                if has_text || has_thinking {
                    StepType::TextResponse
                } else {
                    StepType::ToolCall
                }
            }
        };

        let structured_output: Option<serde_json::Value> = update
            .finish
            .as_ref()
            .and_then(|f| f.output_string.as_ref())
            .and_then(|s| serde_json::from_str(s).ok());

        let usage_opt = event.usage_metadata.clone();

        let mut tool_calls: Vec<ToolCall> = Vec::new();
        if let Some(wc) = event.tool_call.clone() {
            let args: serde_json::Value =
                serde_json::from_str(&wc.arguments_json).unwrap_or(serde_json::Value::Null);
            tool_calls.push(ToolCall {
                name: wc.name,
                args,
                id: Some(wc.id),
                canonical_path: wc.canonical_path,
            });
        }

        let step = Step {
            id: update.trajectory_id,
            step_index: update.step_index,
            kind,
            source,
            target,
            status,
            content: update.text.unwrap_or_default(),
            content_delta: update.text_delta.unwrap_or_default(),
            thinking: update.thinking.unwrap_or_default(),
            thinking_delta: update.thinking_delta.unwrap_or_default(),
            tool_calls,
            error: update.error_message.unwrap_or_default(),
            is_complete_response: Some(matches!(update.state, StepUpdateState::Done)),
            structured_output,
            usage_metadata: usage_opt,
        };

        if matches!(
            update.state,
            StepUpdateState::Done | StepUpdateState::Error
        ) {
            idle.store(true, Ordering::Release);
            idle_notify.notify_waiters();
        } else {
            idle.store(false, Ordering::Release);
        }

        // `send` returns Err when there are no subscribers; that's expected
        // when no caller has subscribed yet, so we swallow it.
        let _ = steps.send(step);
    } else if let Some(state) = event.trajectory_state_update {
        if matches!(state.state, TrajectoryState::Idle) {
            idle.store(true, Ordering::Release);
            idle_notify.notify_waiters();
        }
    }
}

// =============================================================================
// Helpers
// =============================================================================

fn build_user_input_event(content: Content) -> Result<InputEvent> {
    if content.parts.is_empty() {
        return Err(Error::config("empty content"));
    }

    if let Some(text) = content.as_text() {
        return Ok(InputEvent {
            user_input: Some(text),
            ..Default::default()
        });
    }

    let mut parts = Vec::with_capacity(content.parts.len());
    for p in content.parts {
        let part = match p {
            Part::Text(t) => UserInputPart {
                text: Some(t),
                media: None,
            },
            Part::Media(m) => UserInputPart {
                text: None,
                media: Some(UserInputMedia {
                    mime_type: m.mime_type,
                    description: m.description.unwrap_or_default(),
                    data: BASE64.encode(m.data.as_ref() as &[u8]),
                }),
            },
        };
        parts.push(part);
    }

    Ok(InputEvent {
        complex_user_input: Some(UserInput { parts }),
        ..Default::default()
    })
}

fn resolve_binary(explicit: Option<&std::path::Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        if p.exists() {
            return Ok(p.to_path_buf());
        }
        return Err(Error::other(format!(
            "binary path does not exist: {}",
            p.display()
        )));
    }
    if let Ok(env) = std::env::var("ANTIGRAVITY_HARNESS_PATH") {
        let p = PathBuf::from(env);
        if p.exists() {
            return Ok(p);
        }
    }
    let exe_names: &[&str] = if cfg!(windows) {
        &["localharness.exe", "localharness"]
    } else {
        &["localharness"]
    };
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            for name in exe_names {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
    }
    Err(Error::BinaryNotFound)
}

fn resolve_storage(explicit: Option<&std::path::Path>) -> Result<PathBuf> {
    let dir = explicit
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::temp_dir().join("antigravity"));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// 8 hex chars sourced from a per-process nondeterministic counter. We do
/// not need cryptographic randomness — the harness assigns the real id; this
/// is just a friendly fallback when the caller did not pass one in.
fn short_random_id() -> String {
    use std::sync::atomic::AtomicU64;
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let bump = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mixed = nanos.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(bump);
    format!("{:08x}", mixed as u32)
}

// `Bytes` is used in `build_user_input_event` via `m.data` — the import is
// re-exported here to make the path obvious in code review.
#[allow(dead_code)]
fn _bytes_anchor(_: &Bytes) {}
