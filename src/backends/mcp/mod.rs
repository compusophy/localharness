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
    pub server_name: String,
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

    pub async fn shutdown(&self) {
        let h = self.dispatcher.lock().take();
        if let Some(h) = h {
            h.abort();
        }
        self.transport.shutdown().await;
    }
}

async fn request_via(
    transport: &StdioTransport,
    pending: &Mutex<HashMap<u64, oneshot::Sender<Response>>>,
    next_id: &AtomicU64,
    method: &str,
    params: Option<Value>,
) -> Result<Option<Value>> {
    let id = next_id.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = oneshot::channel();
    pending.lock().await.insert(id, tx);

    let req = Request::new(id, method, params);
    let payload = serde_json::to_string(&req)
        .map_err(|e| Error::other(format!("mcp encode: {e}")))?;
    trace!(method, %payload, "mcp request");

    if let Err(e) = transport.send(&payload).await {
        pending.lock().await.remove(&id);
        return Err(e);
    }

    let resp = match rx.await {
        Ok(r) => r,
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
    pending: &Mutex<HashMap<u64, oneshot::Sender<Response>>>,
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
    pending: &Mutex<HashMap<u64, oneshot::Sender<Response>>>,
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
            let resp: Response = match serde_json::from_str(&line) {
                Ok(r) => r,
                Err(e) => {
                    trace!(?e, %line, "mcp: undecodable line (likely a notification)");
                    continue;
                }
            };
            if let Some(id) = resp.id {
                if let Some(tx) = pending.lock().await.remove(&id) {
                    let _ = tx.send(resp);
                }
            }
        }
    })
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
