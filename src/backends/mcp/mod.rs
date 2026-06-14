//! Model Context Protocol (MCP) client + tool bridge.
//!
//! Connects to one or more MCP servers, discovers their tool sets, and
//! adapts each remote tool to a local [`Tool`] so the agent can call
//! it transparently — the model never sees the difference between an
//! in-process tool and an MCP-served one.
//!
//! ## Scope (0.4.0-alpha.2)
//!
//! * **Stdio transport only.** SSE / Streamable HTTP land in a later
//!   release (`McpServerConfig::Sse` and `Http` return an error today).
//! * **Tools surface only.** Prompts, resources, sampling,
//!   subscriptions are out of scope.
//! * **Eager registration.** Tools are fetched once at `connect` and
//!   registered into the runner. Server-side tool changes are not
//!   re-discovered.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex as ParkingMutex;
use serde_json::{json, Value};
use tokio::sync::{oneshot, Mutex};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tracing::{debug, trace, warn};

use crate::error::{Error, Result};
use crate::tools::{Tool, ToolContext, ToolRunner};
use crate::types::McpServerConfig;

mod protocol;
mod transport;

pub use protocol::McpToolDecl;

use protocol::{
    ClientInfo, InitializeParams, InitializeResult, Notification, Request, Response,
    ToolCallParams, ToolCallResult, ToolsListResult, MCP_PROTOCOL_VERSION,
};
use transport::StdioTransport;

/// Per-call timeout. MCP servers can be slow; this is a hard ceiling.
const CALL_TIMEOUT: Duration = Duration::from_secs(60);
/// Handshake timeout — keep tighter so we fail fast on misconfigured
/// servers.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);

// =============================================================================
// Client
// =============================================================================

/// A single live MCP connection.
pub struct McpClient {
    transport: Arc<StdioTransport>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Response>>>>,
    next_id: AtomicU64,
    dispatcher: ParkingMutex<Option<JoinHandle<()>>>,
    /// The server's self-reported name.
    pub server_name: String,
    /// Tools discovered during the handshake.
    pub tools: Vec<McpToolDecl>,
}

impl McpClient {
    /// Spawn an MCP server over stdio, complete the initialize
    /// handshake, fetch its tool list.
    pub async fn connect_stdio(command: &str, args: &[String]) -> Result<Arc<Self>> {
        // Build the moving parts outside of `Self` so we can do the
        // handshake against them, then assemble the final struct with
        // populated `server_name` + `tools`.
        let transport = Arc::new(StdioTransport::spawn(command, args).await?);
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Response>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let dispatcher = spawn_dispatcher(transport.clone(), pending.clone());
        let next_id = AtomicU64::new(1);

        let init_result = match timeout(
            HANDSHAKE_TIMEOUT,
            initialize_via(&transport, &pending, &next_id),
        )
        .await
        {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                dispatcher.abort();
                transport.shutdown().await;
                return Err(e);
            }
            Err(_) => {
                dispatcher.abort();
                transport.shutdown().await;
                return Err(Error::Timeout(HANDSHAKE_TIMEOUT));
            }
        };

        let tools = match timeout(
            HANDSHAKE_TIMEOUT,
            list_tools_via(&transport, &pending, &next_id),
        )
        .await
        {
            Ok(Ok(t)) => t,
            Ok(Err(e)) => {
                dispatcher.abort();
                transport.shutdown().await;
                return Err(e);
            }
            Err(_) => {
                dispatcher.abort();
                transport.shutdown().await;
                return Err(Error::Timeout(HANDSHAKE_TIMEOUT));
            }
        };

        let server_name = init_result
            .server_info
            .map(|s| s.name)
            .unwrap_or_else(|| command.to_string());
        debug!(server = %server_name, count = tools.len(), "mcp connected");

        Ok(Arc::new(Self {
            transport,
            pending,
            next_id,
            dispatcher: ParkingMutex::new(Some(dispatcher)),
            server_name,
            tools,
        }))
    }

    /// Invoke a tool on the remote MCP server.
    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value> {
        let params = serde_json::to_value(ToolCallParams {
            name,
            arguments,
        })
        .map_err(|e| Error::other(format!("tools/call encode: {e}")))?;

        let resp = timeout(CALL_TIMEOUT, self.request("tools/call", Some(params)))
            .await
            .map_err(|_| Error::Timeout(CALL_TIMEOUT))??;
        let result: ToolCallResult = serde_json::from_value(
            resp.ok_or_else(|| Error::other("tools/call returned no result"))?,
        )
        .map_err(|e| Error::other(format!("tools/call decode: {e}")))?;
        Ok(result.flatten())
    }

    async fn request(&self, method: &str, params: Option<Value>) -> Result<Option<Value>> {
        request_via(&self.transport, &self.pending, &self.next_id, method, params).await
    }

    /// Kill the child process and clean up. Idempotent.
    pub async fn shutdown(&self) {
        let h = self.dispatcher.lock().take();
        if let Some(h) = h {
            h.abort();
        }
        self.transport.shutdown().await;
    }
}

/// RAII guard that removes the `pending` entry for `id` when dropped —
/// closing the map-entry leak that a caller-side `timeout()` (or any cancel)
/// would otherwise cause by dropping the [`request_via`] future mid-await,
/// leaving its oneshot sender stranded in the table forever. Disarm via
/// [`PendingGuard::disarm`] on the paths where the entry is already gone
/// (delivered, or removed on a send error), so the common case spawns
/// nothing. Drop uses a spawned removal because the table is behind an async
/// mutex (a sync lock in `Drop` could deadlock the dispatcher).
struct PendingGuard {
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Response>>>>,
    id: u64,
    armed: bool,
}

impl PendingGuard {
    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let pending = self.pending.clone();
        let id = self.id;
        tokio::spawn(async move {
            pending.lock().await.remove(&id);
        });
    }
}

async fn request_via(
    transport: &StdioTransport,
    pending: &Arc<Mutex<HashMap<u64, oneshot::Sender<Response>>>>,
    next_id: &AtomicU64,
    method: &str,
    params: Option<Value>,
) -> Result<Option<Value>> {
    let id = next_id.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = oneshot::channel();
    pending.lock().await.insert(id, tx);
    // Armed from here: any early return / dropped-future (timeout) below pops
    // the entry on the way out.
    let guard = PendingGuard {
        pending: pending.clone(),
        id,
        armed: true,
    };

    let req = Request::new(id, method, params);
    let payload = serde_json::to_string(&req)
        .map_err(|e| Error::other(format!("mcp encode: {e}")))?;
    trace!(method, %payload, "mcp request");

    if let Err(e) = transport.send(&payload).await {
        // Remove synchronously and disarm — nothing else can race a not-yet-
        // sent id.
        pending.lock().await.remove(&id);
        guard.disarm();
        return Err(e);
    }

    let resp = match rx.await {
        // The dispatcher already removed the entry to deliver this response.
        Ok(r) => {
            guard.disarm();
            r
        }
        Err(_) => return Err(Error::Closed),
    };
    if let Some(err) = resp.error {
        return Err(Error::other(format!(
            "mcp '{method}' rpc error {}: {}",
            err.code, err.message
        )));
    }
    Ok(resp.result)
}

async fn initialize_via(
    transport: &StdioTransport,
    pending: &Arc<Mutex<HashMap<u64, oneshot::Sender<Response>>>>,
    next_id: &AtomicU64,
) -> Result<InitializeResult> {
    let params = serde_json::to_value(InitializeParams {
        protocol_version: MCP_PROTOCOL_VERSION,
        capabilities: json!({}),
        client_info: ClientInfo {
            name: "localharness",
            version: env!("CARGO_PKG_VERSION"),
        },
    })
    .map_err(|e| Error::other(format!("mcp initialize encode: {e}")))?;

    let resp = request_via(transport, pending, next_id, "initialize", Some(params)).await?;
    let result: InitializeResult = serde_json::from_value(
        resp.ok_or_else(|| Error::other("mcp initialize returned no result"))?,
    )
    .map_err(|e| Error::other(format!("mcp initialize decode: {e}")))?;

    let notif = Notification::new("notifications/initialized", None);
    let payload = serde_json::to_string(&notif)
        .map_err(|e| Error::other(format!("mcp notify encode: {e}")))?;
    transport.send(&payload).await?;

    Ok(result)
}

async fn list_tools_via(
    transport: &StdioTransport,
    pending: &Arc<Mutex<HashMap<u64, oneshot::Sender<Response>>>>,
    next_id: &AtomicU64,
) -> Result<Vec<McpToolDecl>> {
    let resp = request_via(transport, pending, next_id, "tools/list", None).await?;
    let result: ToolsListResult = serde_json::from_value(
        resp.ok_or_else(|| Error::other("tools/list returned no result"))?,
    )
    .map_err(|e| Error::other(format!("tools/list decode: {e}")))?;
    Ok(result.tools)
}

fn spawn_dispatcher(
    transport: Arc<StdioTransport>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Response>>>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let line = {
                let mut rx = transport.inbound.lock().await;
                match rx.recv().await {
                    Some(l) => l,
                    None => return,
                }
            };
            route_line(&line, &pending).await;
        }
    })
}

/// Decode one inbound line and, if it is a response with a matching
/// pending `id`, deliver it. Undecodable lines (server log noise) and
/// notifications (no `id`) are dropped. A response whose `id` matches no
/// pending request is dropped — never panics, never blocks. Factored out
/// of `spawn_dispatcher` so the framing/correlation logic is unit-testable
/// without a live child process.
async fn route_line(line: &str, pending: &Mutex<HashMap<u64, oneshot::Sender<Response>>>) {
    let resp: Response = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(e) => {
            trace!(?e, %line, "mcp: undecodable line (likely a notification)");
            return;
        }
    };
    if let Some(id) = resp.id {
        if let Some(tx) = pending.lock().await.remove(&id) {
            let _ = tx.send(resp);
        }
    }
}

// =============================================================================
// Bridge
// =============================================================================

/// Owns a set of [`McpClient`]s. Registering the bridge into a
/// [`ToolRunner`] exposes every server's tools to the agent.
#[derive(Default)]
pub struct McpBridge {
    clients: Vec<Arc<McpClient>>,
}

impl McpBridge {
    /// Create an empty bridge with no connected servers.
    pub fn new() -> Self {
        Self::default()
    }

    /// Spawn the configured server and stash the client. Stdio only
    /// today; SSE and HTTP variants return `Error::Config`.
    pub async fn connect(&mut self, config: &McpServerConfig) -> Result<()> {
        let client = match config {
            McpServerConfig::Stdio { command, args } => {
                McpClient::connect_stdio(command, args).await?
            }
            McpServerConfig::Sse { .. } => {
                return Err(Error::config(
                    "MCP SSE transport not implemented yet (use Stdio)",
                ))
            }
            McpServerConfig::Http { .. } => {
                return Err(Error::config(
                    "MCP HTTP transport not implemented yet (use Stdio)",
                ))
            }
        };
        self.clients.push(client);
        Ok(())
    }

    /// Register every server's tools into `runner`. Returns the names
    /// registered. Custom tools already registered under the same name
    /// **win** (no overwrite).
    pub fn register_into(&self, runner: &ToolRunner) -> Vec<String> {
        let existing = runner.names();
        let mut registered = Vec::new();
        for client in &self.clients {
            for decl in &client.tools {
                if existing.iter().any(|n| n == &decl.name) {
                    debug!(name = %decl.name, "mcp: skipping (already registered)");
                    continue;
                }
                let tool: Arc<dyn Tool> = Arc::new(McpTool {
                    client: client.clone(),
                    decl: decl.clone(),
                });
                runner.register(tool);
                registered.push(decl.name.clone());
            }
        }
        registered
    }

    /// Shut down all connected MCP servers.
    pub async fn shutdown(&self) {
        for c in &self.clients {
            c.shutdown().await;
        }
    }
}

// =============================================================================
// Tool adapter
// =============================================================================

struct McpTool {
    client: Arc<McpClient>,
    decl: McpToolDecl,
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.decl.name
    }

    fn description(&self) -> &str {
        self.decl
            .description
            .as_deref()
            .unwrap_or("(no description provided by MCP server)")
    }

    fn input_schema(&self) -> Value {
        self.decl
            .input_schema
            .clone()
            .unwrap_or_else(|| json!({ "type": "object", "properties": {} }))
    }

    async fn execute(&self, args: Value, _ctx: Option<Arc<ToolContext>>) -> Result<Value> {
        match self.client.call_tool(&self.decl.name, args).await {
            Ok(v) => Ok(v),
            Err(e) => {
                warn!(
                    server = %self.client.server_name,
                    tool = %self.decl.name,
                    error = %e,
                    "mcp tool call failed"
                );
                Err(e)
            }
        }
    }
}

// =============================================================================
// Tests — framing / correlation / error edges (no live child process)
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Fresh empty pending table.
    fn pending_map() -> Arc<Mutex<HashMap<u64, oneshot::Sender<Response>>>> {
        Arc::new(Mutex::new(HashMap::new()))
    }

    /// Register a waiter for `id`, returning the receiver the caller awaits.
    async fn register(
        pending: &Mutex<HashMap<u64, oneshot::Sender<Response>>>,
        id: u64,
    ) -> oneshot::Receiver<Response> {
        let (tx, rx) = oneshot::channel();
        pending.lock().await.insert(id, tx);
        rx
    }

    #[tokio::test]
    async fn routes_result_response_to_waiter() {
        let pending = pending_map();
        let rx = register(&pending, 1).await;
        route_line(r#"{"jsonrpc":"2.0","id":1,"result":{"v":42}}"#, &pending).await;
        let resp = rx.await.expect("delivered");
        assert_eq!(resp.result, Some(serde_json::json!({"v": 42})));
        // The entry is consumed (a duplicate would find nothing).
        assert!(pending.lock().await.is_empty());
    }

    #[tokio::test]
    async fn routes_error_response_to_waiter() {
        let pending = pending_map();
        let rx = register(&pending, 5).await;
        route_line(
            r#"{"jsonrpc":"2.0","id":5,"error":{"code":-32000,"message":"nope"}}"#,
            &pending,
        )
        .await;
        let resp = rx.await.expect("delivered");
        let err = resp.error.expect("error present");
        assert_eq!(err.code, -32000);
        assert_eq!(err.message, "nope");
    }

    #[tokio::test]
    async fn out_of_order_responses_match_by_id() {
        // Two concurrent requests; the server answers id=2 before id=1.
        let pending = pending_map();
        let rx1 = register(&pending, 1).await;
        let rx2 = register(&pending, 2).await;
        route_line(r#"{"jsonrpc":"2.0","id":2,"result":"second"}"#, &pending).await;
        route_line(r#"{"jsonrpc":"2.0","id":1,"result":"first"}"#, &pending).await;
        assert_eq!(rx1.await.unwrap().result, Some(serde_json::json!("first")));
        assert_eq!(rx2.await.unwrap().result, Some(serde_json::json!("second")));
    }

    #[tokio::test]
    async fn unmatched_id_is_dropped_without_panic() {
        // A response for an id we never sent (or already answered) must be
        // silently ignored — not panic, not block.
        let pending = pending_map();
        let rx = register(&pending, 1).await;
        route_line(r#"{"jsonrpc":"2.0","id":999,"result":"ghost"}"#, &pending).await;
        // Our real waiter is untouched.
        assert!(pending.lock().await.contains_key(&1));
        // Now answer it properly.
        route_line(r#"{"jsonrpc":"2.0","id":1,"result":"ok"}"#, &pending).await;
        assert_eq!(rx.await.unwrap().result, Some(serde_json::json!("ok")));
    }

    #[tokio::test]
    async fn duplicate_response_for_same_id_does_not_panic() {
        // A buggy/malicious server sends two responses with the same id.
        // The first is delivered; the second finds no pending entry and is
        // dropped. Must not panic on the orphaned second send.
        let pending = pending_map();
        let rx = register(&pending, 1).await;
        route_line(r#"{"jsonrpc":"2.0","id":1,"result":"a"}"#, &pending).await;
        route_line(r#"{"jsonrpc":"2.0","id":1,"result":"b"}"#, &pending).await;
        assert_eq!(rx.await.unwrap().result, Some(serde_json::json!("a")));
        assert!(pending.lock().await.is_empty());
    }

    #[tokio::test]
    async fn notification_without_id_does_not_consume_a_waiter() {
        // Server-initiated notification (method, no id) must be dropped and
        // must NOT be mistaken for a response to any pending request.
        let pending = pending_map();
        let rx = register(&pending, 1).await;
        route_line(
            r#"{"jsonrpc":"2.0","method":"notifications/message","params":{"level":"info"}}"#,
            &pending,
        )
        .await;
        assert!(pending.lock().await.contains_key(&1));
        // The waiter is still live and answerable.
        route_line(r#"{"jsonrpc":"2.0","id":1,"result":"done"}"#, &pending).await;
        assert_eq!(rx.await.unwrap().result, Some(serde_json::json!("done")));
    }

    #[tokio::test]
    async fn undecodable_noise_line_is_ignored() {
        // A server logging plain text / a partial line to stdout must not
        // disturb pending requests or panic the dispatcher.
        let pending = pending_map();
        let rx = register(&pending, 1).await;
        route_line("INFO server ready", &pending).await;
        route_line("{ not valid json", &pending).await;
        route_line("", &pending).await;
        assert!(pending.lock().await.contains_key(&1));
        route_line(r#"{"jsonrpc":"2.0","id":1,"result":1}"#, &pending).await;
        assert_eq!(rx.await.unwrap().result, Some(serde_json::json!(1)));
    }

    #[tokio::test]
    async fn response_with_null_id_is_dropped() {
        // JSON-RPC parse-error responses carry id:null. We can't correlate
        // them, so they're dropped — pending waiters untouched.
        let pending = pending_map();
        let rx = register(&pending, 1).await;
        route_line(
            r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"parse error"}}"#,
            &pending,
        )
        .await;
        assert!(pending.lock().await.contains_key(&1));
        drop(rx);
    }

    #[tokio::test]
    async fn dropped_sender_yields_recv_error_for_caller() {
        // Models "child died mid-request": the dispatcher never delivers,
        // and on shutdown the pending sender is dropped. The awaiting
        // caller observes RecvError, which request_via maps to Error::Closed
        // (never an infinite hang at this layer — the hang ceiling is the
        // outer timeout()).
        let pending = pending_map();
        let rx = register(&pending, 1).await;
        // Simulate teardown dropping all pending senders.
        pending.lock().await.clear();
        assert!(rx.await.is_err());
    }

    #[tokio::test]
    async fn armed_pending_guard_removes_entry_on_drop() {
        // A timed-out / cancelled request_via drops its PendingGuard with the
        // entry still in the table. The guard must reap it (the map-entry leak
        // bug). Drop spawns the removal, so yield until the table drains.
        let pending = pending_map();
        let _rx = register(&pending, 7).await;
        assert!(pending.lock().await.contains_key(&7));
        {
            let _guard = PendingGuard {
                pending: pending.clone(),
                id: 7,
                armed: true,
            };
        } // dropped here → spawns the removal
        // Give the spawned removal task a turn to run.
        for _ in 0..100 {
            if !pending.lock().await.contains_key(&7) {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(
            !pending.lock().await.contains_key(&7),
            "armed guard must remove the pending entry on drop"
        );
    }

    #[tokio::test]
    async fn disarmed_pending_guard_leaves_entry_alone() {
        // The delivered / send-error paths disarm the guard (the entry is
        // already gone or owned elsewhere) — drop must NOT spawn a spurious
        // removal that could reap a re-used id.
        let pending = pending_map();
        let _rx = register(&pending, 9).await;
        {
            let guard = PendingGuard {
                pending: pending.clone(),
                id: 9,
                armed: true,
            };
            guard.disarm();
        }
        // Yield a few times; a disarmed guard spawns nothing, so the entry
        // stays.
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert!(
            pending.lock().await.contains_key(&9),
            "disarmed guard must not touch the table"
        );
    }
}
