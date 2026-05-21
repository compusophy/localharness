//! JSON-RPC 2.0 + MCP wire types.
//!
//! Just enough to do `initialize`, `tools/list`, and `tools/call`.
//! Subscriptions, sampling, prompts, resources — all deferred.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Pin the protocol revision we negotiate with the server.
pub const MCP_PROTOCOL_VERSION: &str = "2025-03-26";

// =============================================================================
// JSON-RPC envelope
// =============================================================================

#[derive(Debug, Clone, Serialize)]
pub struct Request<'a> {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl<'a> Request<'a> {
    pub fn new(id: u64, method: &'a str, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            method,
            params,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Notification<'a> {
    pub jsonrpc: &'static str,
    pub method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl<'a> Notification<'a> {
    pub fn new(method: &'a str, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0",
            method,
            params,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Response {
    #[allow(dead_code)]
    pub jsonrpc: String,
    pub id: Option<u64>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<RpcError>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[allow(dead_code)]
    #[serde(default)]
    pub data: Option<Value>,
}

// =============================================================================
// MCP message bodies
// =============================================================================

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams<'a> {
    pub protocol_version: &'a str,
    pub capabilities: Value,
    pub client_info: ClientInfo<'a>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClientInfo<'a> {
    pub name: &'a str,
    pub version: &'a str,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    /// Kept for diagnostics; we don't currently enforce protocol-version mismatches.
    #[allow(dead_code)]
    #[serde(default)]
    pub protocol_version: Option<String>,
    /// Kept for diagnostics; we don't gate features on declared server capabilities yet.
    #[allow(dead_code)]
    #[serde(default)]
    pub capabilities: Value,
    #[serde(default)]
    pub server_info: Option<ServerInfo>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    #[allow(dead_code)]
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolsListResult {
    pub tools: Vec<McpToolDecl>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolDecl {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// JSON schema describing the tool's args. May be absent for
    /// no-arg tools.
    #[serde(default)]
    pub input_schema: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolCallParams<'a> {
    pub name: &'a str,
    pub arguments: Value,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallResult {
    #[serde(default)]
    pub content: Vec<ContentBlock>,
    #[serde(default)]
    pub is_error: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ContentBlock {
    Text { text: String },
    Image { data: String, mime_type: String },
    Resource { resource: Value },
    #[serde(other)]
    Unknown,
}

impl ToolCallResult {
    /// Best-effort flattening of the response into a JSON value
    /// suitable to feed back to Gemini as a function_response.
    pub fn flatten(self) -> Value {
        let mut text = String::new();
        let mut images: Vec<Value> = Vec::new();
        for block in self.content {
            match block {
                ContentBlock::Text { text: t } => {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(&t);
                }
                ContentBlock::Image { data, mime_type } => {
                    images.push(serde_json::json!({
                        "mime_type": mime_type,
                        "data_base64": data,
                    }));
                }
                ContentBlock::Resource { resource } => {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(&resource.to_string());
                }
                ContentBlock::Unknown => {}
            }
        }
        let mut out = serde_json::Map::new();
        if !text.is_empty() {
            out.insert("text".into(), Value::String(text));
        }
        if !images.is_empty() {
            out.insert("images".into(), Value::Array(images));
        }
        if let Some(true) = self.is_error {
            out.insert("is_error".into(), Value::Bool(true));
        }
        Value::Object(out)
    }
}
