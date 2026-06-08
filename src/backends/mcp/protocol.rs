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
    Text {
        text: String,
    },
    Image {
        data: String,
        // MCP sends camelCase `mimeType`; `rename_all = "lowercase"` above
        // only renames the variant tags (the `type` value), NOT struct
        // fields — so this needs its own rename or every image-bearing
        // tool result fails to decode and the whole call errors out.
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    Resource {
        resource: Value,
    },
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ---- Response envelope decoding (untrusted child stdout) ----

    #[test]
    fn decodes_result_response() {
        let resp: Response =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":7,"result":{"ok":true}}"#).unwrap();
        assert_eq!(resp.id, Some(7));
        assert_eq!(resp.result, Some(json!({"ok": true})));
        assert!(resp.error.is_none());
    }

    #[test]
    fn decodes_error_response() {
        let resp: Response = serde_json::from_str(
            r#"{"jsonrpc":"2.0","id":3,"error":{"code":-32601,"message":"Method not found"}}"#,
        )
        .unwrap();
        assert_eq!(resp.id, Some(3));
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32601);
        assert_eq!(err.message, "Method not found");
    }

    #[test]
    fn error_response_carries_optional_data() {
        let resp: Response = serde_json::from_str(
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":1,"message":"x","data":{"detail":42}}}"#,
        )
        .unwrap();
        assert_eq!(resp.error.unwrap().data, Some(json!({"detail": 42})));
    }

    #[test]
    fn missing_data_field_in_error_is_none() {
        let resp: Response = serde_json::from_str(
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":1,"message":"x"}}"#,
        )
        .unwrap();
        assert!(resp.error.unwrap().data.is_none());
    }

    #[test]
    fn notification_decodes_with_no_id() {
        // A server-initiated notification has a `method`, no `id`. It must
        // decode (unknown fields ignored) and surface id=None so the
        // dispatcher drops it rather than mistaking it for a response.
        let resp: Response = serde_json::from_str(
            r#"{"jsonrpc":"2.0","method":"notifications/message","params":{"level":"info"}}"#,
        )
        .unwrap();
        assert_eq!(resp.id, None);
        assert!(resp.result.is_none());
        assert!(resp.error.is_none());
    }

    #[test]
    fn response_with_extra_unknown_fields_decodes() {
        // Servers may add fields we don't model; must not break decoding.
        let resp: Response = serde_json::from_str(
            r#"{"jsonrpc":"2.0","id":9,"result":{},"meta":{"trace":"abc"},"_extra":1}"#,
        )
        .unwrap();
        assert_eq!(resp.id, Some(9));
    }

    #[test]
    fn response_missing_jsonrpc_field_fails() {
        // `jsonrpc` is required; a line without it is not a valid Response
        // and is treated as undecodable noise by the dispatcher.
        let r: std::result::Result<Response, _> =
            serde_json::from_str(r#"{"id":1,"result":{}}"#);
        assert!(r.is_err());
    }

    #[test]
    fn non_json_line_fails_to_decode() {
        // A server logging plain text to stdout must not parse as a Response.
        let r: std::result::Result<Response, _> =
            serde_json::from_str("INFO: server started, listening on stdio");
        assert!(r.is_err());
    }

    // ---- initialize / tools/list decoding ----

    #[test]
    fn initialize_result_decodes_camelcase() {
        let r: InitializeResult = serde_json::from_str(
            r#"{"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"demo","version":"1.0"}}"#,
        )
        .unwrap();
        assert_eq!(r.protocol_version.as_deref(), Some("2025-03-26"));
        assert_eq!(r.server_info.unwrap().name, "demo");
    }

    #[test]
    fn initialize_result_tolerates_missing_server_info() {
        let r: InitializeResult =
            serde_json::from_str(r#"{"protocolVersion":"2025-03-26","capabilities":{}}"#)
                .unwrap();
        assert!(r.server_info.is_none());
    }

    #[test]
    fn tools_list_decodes_with_and_without_optional_fields() {
        let r: ToolsListResult = serde_json::from_str(
            r#"{"tools":[
                {"name":"a","description":"does a","inputSchema":{"type":"object"}},
                {"name":"b"}
            ]}"#,
        )
        .unwrap();
        assert_eq!(r.tools.len(), 2);
        assert_eq!(r.tools[0].name, "a");
        assert_eq!(r.tools[0].description.as_deref(), Some("does a"));
        assert!(r.tools[0].input_schema.is_some());
        // Tool "b" has no description / schema — both optional.
        assert_eq!(r.tools[1].name, "b");
        assert!(r.tools[1].description.is_none());
        assert!(r.tools[1].input_schema.is_none());
    }

    #[test]
    fn tools_list_empty_is_ok() {
        let r: ToolsListResult = serde_json::from_str(r#"{"tools":[]}"#).unwrap();
        assert!(r.tools.is_empty());
    }

    // ---- tools/call result + flatten ----

    #[test]
    fn flatten_joins_multiple_text_blocks() {
        let r: ToolCallResult = serde_json::from_str(
            r#"{"content":[{"type":"text","text":"line1"},{"type":"text","text":"line2"}]}"#,
        )
        .unwrap();
        let v = r.flatten();
        assert_eq!(v["text"], json!("line1\nline2"));
        assert!(v.get("is_error").is_none());
    }

    #[test]
    fn flatten_marks_error_results() {
        let r: ToolCallResult = serde_json::from_str(
            r#"{"content":[{"type":"text","text":"boom"}],"isError":true}"#,
        )
        .unwrap();
        let v = r.flatten();
        assert_eq!(v["text"], json!("boom"));
        assert_eq!(v["is_error"], json!(true));
    }

    #[test]
    fn flatten_empty_content_is_empty_object() {
        let r: ToolCallResult = serde_json::from_str(r#"{"content":[]}"#).unwrap();
        assert_eq!(r.flatten(), json!({}));
    }

    #[test]
    fn tool_call_result_tolerates_missing_content() {
        // `content` is #[serde(default)] — a result object with no content
        // field must still decode to an empty vec, not error.
        let r: ToolCallResult = serde_json::from_str(r#"{}"#).unwrap();
        assert!(r.content.is_empty());
        assert_eq!(r.flatten(), json!({}));
    }

    #[test]
    fn flatten_unknown_content_block_is_skipped_not_fatal() {
        // A future/unsupported block type must not abort decoding of the
        // whole result (would drop a real tool response on the floor).
        let r: ToolCallResult = serde_json::from_str(
            r#"{"content":[{"type":"audio","data":"...","mimeType":"audio/wav"},{"type":"text","text":"hi"}]}"#,
        )
        .unwrap();
        let v = r.flatten();
        assert_eq!(v["text"], json!("hi"));
    }

    #[test]
    fn flatten_resource_block_serialized_into_text() {
        let r: ToolCallResult = serde_json::from_str(
            r#"{"content":[{"type":"resource","resource":{"uri":"file:///x","text":"body"}}]}"#,
        )
        .unwrap();
        let v = r.flatten();
        assert!(v["text"].as_str().unwrap().contains("file:///x"));
    }

    #[test]
    fn flatten_image_block_uses_camelcase_mime_type() {
        // The MCP wire format sends `mimeType` (camelCase). The image block
        // must decode and surface the mime type. Regression guard for a
        // field-rename bug that silently dropped the whole tool response.
        let r: ToolCallResult = serde_json::from_str(
            r#"{"content":[{"type":"image","data":"BASE64","mimeType":"image/png"}]}"#,
        )
        .unwrap();
        let v = r.flatten();
        let imgs = v["images"].as_array().expect("images array present");
        assert_eq!(imgs.len(), 1);
        assert_eq!(imgs[0]["mime_type"], json!("image/png"));
        assert_eq!(imgs[0]["data_base64"], json!("BASE64"));
    }
}
