//! `acp` — serve this identity's agent over the **Agent Client Protocol**
//! (JSON-RPC 2.0, newline-delimited over stdio) so any ACP client — Zed,
//! JetBrains, `vscode-acp`, Buzz's `buzz-acp` bridge — can drive a
//! localharness agent like any other harness on the ACP registry.
//!
//! The agent side of ACP is small: `initialize`, `session/new`,
//! `session/prompt` (streaming `session/update` notifications), and the
//! `session/cancel` notification. We declare `loadSession: false`, no prompt
//! capabilities beyond text, no auth methods — capabilities gate everything
//! else, so this is a compliant v1 agent.
//!
//! Sessions ride the SAME headless metered path as `call`
//! (`start_headless_agent` with `multi_turn: true`): the agent embodies this
//! identity's on-chain persona + lessons + skills, and every prompt is paid
//! from its per-request `$LH` meter. The x402 single-request path is
//! deliberately excluded — its one-shot nonce cannot survive a second
//! `session/prompt` (see `call.rs`).
//!
//! ⛔ STDOUT IS THE WIRE. Every byte we print to stdout must be exactly one
//! JSON-RPC frame per line, flushed (stdout is BLOCK-buffered when piped — an
//! unflushed response deadlocks the client). All human chatter goes to stderr,
//! which `main.rs` already uses for the chain banner.

use std::collections::{HashMap, VecDeque};
use std::io::Write;

use crate::{ensure_meter_funded, resolve_caller_key, resolve_caller_label, start_headless_agent, take_as_flag, take_value_flag, wallet};

/// The single protocol MAJOR version this agent implements. Negotiation rule
/// (spec): if we support the client's version we echo it, else we answer with
/// the latest we support and the client decides whether to proceed.
const PROTOCOL_VERSION: u64 = 1;

// ---------------------------------------------------------------------------
// Pure wire helpers (native-tested below)
// ---------------------------------------------------------------------------

/// One inbound JSON-RPC frame we care about: a request (has `id`) or a
/// notification (no `id`). We never receive responses — this agent issues no
/// client-bound requests in v1 (no fs, no terminal, no permission asks).
struct Frame {
    id: Option<serde_json::Value>,
    method: String,
    params: serde_json::Value,
}

/// Parse one wire line. `Err` carries a human reason; the caller maps a
/// parse failure on a REQUEST to JSON-RPC `-32700`/`-32600` and silently
/// drops undecipherable notifications (per JSON-RPC 2.0).
fn parse_frame(line: &str) -> Result<Frame, String> {
    let v: serde_json::Value =
        serde_json::from_str(line).map_err(|e| format!("parse error: {e}"))?;
    let method = v
        .get("method")
        .and_then(|m| m.as_str())
        .ok_or("missing method")?
        .to_string();
    Ok(Frame {
        id: v.get("id").cloned(),
        method,
        params: v.get("params").cloned().unwrap_or(serde_json::Value::Null),
    })
}

/// A JSON-RPC success response as one wire line (no trailing newline).
fn rpc_result(id: &serde_json::Value, result: serde_json::Value) -> String {
    serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

/// A JSON-RPC error response as one wire line.
fn rpc_error(id: &serde_json::Value, code: i64, message: &str) -> String {
    serde_json::json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
        .to_string()
}

/// A JSON-RPC notification as one wire line.
fn notification(method: &str, params: serde_json::Value) -> String {
    serde_json::json!({"jsonrpc": "2.0", "method": method, "params": params}).to_string()
}

/// The `initialize` result: version + our (deliberately minimal) capability
/// surface. Everything not declared here is something a compliant client will
/// never call.
fn initialize_result() -> serde_json::Value {
    serde_json::json!({
        "protocolVersion": PROTOCOL_VERSION,
        "agentCapabilities": {
            "loadSession": false,
            "promptCapabilities": {
                "image": false,
                "audio": false,
                "embeddedContext": false
            }
        },
        "agentInfo": {
            "name": "localharness",
            "title": "localharness",
            "version": env!("CARGO_PKG_VERSION")
        },
        "authMethods": []
    })
}

/// Extract the user text from a `session/prompt` content-block array: all
/// `text` blocks concatenated in order. Unknown block types are skipped —
/// we declared no image/audio/embeddedContext capability, so a compliant
/// client never sends them, and an over-eager one degrades gracefully.
fn prompt_text(params: &serde_json::Value) -> String {
    let mut out = String::new();
    if let Some(blocks) = params.get("prompt").and_then(|p| p.as_array()) {
        for b in blocks {
            if b.get("type").and_then(|t| t.as_str()) == Some("text") {
                if let Some(t) = b.get("text").and_then(|t| t.as_str()) {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(t);
                }
            }
        }
    }
    out
}

/// Map ONE agent stream chunk to its `session/update` payload (the `update`
/// object). `pending` correlates tool results to tool-call ids the same way
/// the browser transcript does — chunks arrive in call order, results in the
/// same order, so a FIFO pairs them exactly. `next_id` synthesizes ids for
/// backends that emit `ToolCall.id: None`.
fn chunk_update(
    chunk: &localharness::StreamChunk,
    pending: &mut VecDeque<String>,
    next_id: &mut u32,
) -> Option<serde_json::Value> {
    use localharness::StreamChunk;
    match chunk {
        StreamChunk::Text { text, .. } if !text.is_empty() => Some(serde_json::json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": text}
        })),
        StreamChunk::Thought { text, .. } if !text.is_empty() => Some(serde_json::json!({
            "sessionUpdate": "agent_thought_chunk",
            "content": {"type": "text", "text": text}
        })),
        StreamChunk::ToolCall(call) => {
            let id = call.id.clone().unwrap_or_else(|| {
                *next_id += 1;
                format!("call_{next_id}")
            });
            pending.push_back(id.clone());
            Some(serde_json::json!({
                "sessionUpdate": "tool_call",
                "toolCallId": id,
                "title": call.name,
                "kind": "other",
                "status": "in_progress",
                "rawInput": call.args
            }))
        }
        StreamChunk::ToolResult(result) => {
            let id = pending.pop_front()?;
            let (status, raw) = match &result.error {
                Some(e) => ("failed", serde_json::Value::String(e.clone())),
                None => (
                    "completed",
                    result.result.clone().unwrap_or(serde_json::Value::Null),
                ),
            };
            Some(serde_json::json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": id,
                "status": status,
                "rawOutput": raw
            }))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// The server loop
// ---------------------------------------------------------------------------

/// Write one frame line to stdout and FLUSH — see the module note on why an
/// unflushed frame deadlocks the client.
fn emit(line: &str) {
    let mut out = std::io::stdout();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// `localharness acp [--as <name>] [--model <id>]` — serve until stdin EOF.
pub(crate) async fn acp(rest: &[String]) -> i32 {
    const USAGE: &str = "usage: localharness acp [--as <name>] [--model <id>]";
    let (as_name, rest) = match take_as_flag(rest) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let (model, rest) = match take_value_flag(&rest, "--model", USAGE) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    if !rest.is_empty() {
        eprintln!("{USAGE}");
        return 2;
    }

    // Resolve the acting identity ONCE, before serving — a key problem should
    // fail fast on stderr, not surface as a cryptic session/new error.
    let (_key_file, key_hex) = match resolve_caller_key(as_name.as_deref()) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let name = match resolve_caller_label(as_name.as_deref()) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let caller = match wallet::from_private_key_hex(&key_hex) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("bad identity key: {e}");
            return 1;
        }
    };
    eprintln!("acp: serving {name}.localharness.xyz over stdio (ctrl-d to stop)");

    // stdin → channel on a plain thread: tokio's async stdin still blocks the
    // runtime on read, and a thread keeps EOF handling trivial.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut line = String::new();
        loop {
            line.clear();
            match std::io::BufRead::read_line(&mut stdin.lock(), &mut line) {
                Ok(0) | Err(_) => break, // EOF / broken pipe → channel drops → loop ends
                Ok(_) => {
                    let t = line.trim();
                    if !t.is_empty() && tx.send(t.to_string()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let mut sessions: HashMap<String, localharness::Agent> = HashMap::new();
    let mut session_seq: u32 = 0;
    // Frames that arrived while a prompt turn was streaming (anything but the
    // cancel that ends it) — replayed in order once the turn responds.
    let mut deferred: VecDeque<String> = VecDeque::new();

    loop {
        let line = match deferred.pop_front() {
            Some(l) => l,
            None => match rx.recv().await {
                Some(l) => l,
                None => break, // stdin EOF — clean shutdown
            },
        };
        let frame = match parse_frame(&line) {
            Ok(f) => f,
            Err(e) => {
                // A malformed REQUEST gets a -32700; we can't know the id, so
                // JSON-RPC says id: null. Malformed notifications drop silently.
                emit(&rpc_error(&serde_json::Value::Null, -32700, &e));
                continue;
            }
        };

        match (frame.method.as_str(), frame.id) {
            ("initialize", Some(id)) => {
                emit(&rpc_result(&id, initialize_result()));
            }
            ("session/new", Some(id)) => {
                // `cwd` + `mcpServers` are accepted and unused: the headless
                // agent is conversational + read-only EVM tools — it gets no
                // filesystem and connects to no client-supplied MCP servers.
                session_seq += 1;
                match start_headless_agent(&key_hex, &name, model.as_deref(), None, true).await {
                    Ok(agent) => {
                        let sid = format!("sess_{}_{session_seq}", &name);
                        sessions.insert(sid.clone(), agent);
                        emit(&rpc_result(&id, serde_json::json!({"sessionId": sid})));
                    }
                    Err(e) => emit(&rpc_error(&id, -32603, &e)),
                }
            }
            ("session/prompt", Some(id)) => {
                let sid = frame
                    .params
                    .get("sessionId")
                    .and_then(|s| s.as_str())
                    .unwrap_or("")
                    .to_string();
                let Some(agent) = sessions.get(&sid) else {
                    emit(&rpc_error(&id, -32602, "unknown sessionId"));
                    continue;
                };
                let text = prompt_text(&frame.params);
                if text.trim().is_empty() {
                    emit(&rpc_error(&id, -32602, "prompt has no text content"));
                    continue;
                }
                // Multi-turn metering: each prompt debits ~1 $LH, so re-fund
                // the meter lazily before every turn, not just at start.
                ensure_meter_funded(&caller).await;

                let response = match agent.chat(text).await {
                    Ok(r) => r,
                    Err(e) => {
                        emit(&rpc_error(&id, -32603, &e.to_string()));
                        continue;
                    }
                };
                let mut cursor = response.chunks();
                let mut pending: VecDeque<String> = VecDeque::new();
                let mut next_tool_id: u32 = 0;
                let mut stop_reason = "end_turn";
                // stdin EOF mid-turn means "no more REQUESTS", not "abandon the
                // in-flight one" (a scripted `printf … | localharness acp`
                // closes stdin the instant its frames are written): stop
                // selecting on the channel and drain the turn to completion.
                let mut stdin_open = true;
                loop {
                    let chunk = if stdin_open {
                        tokio::select! {
                            chunk = futures_util::StreamExt::next(&mut cursor) => chunk,
                            inbound = rx.recv() => {
                                match inbound {
                                    Some(l) => {
                                        // Only a cancel for THIS session interrupts
                                        // the turn; everything else replays after it.
                                        let is_cancel = parse_frame(&l).ok().is_some_and(|f| {
                                            f.method == "session/cancel"
                                                && f.params.get("sessionId").and_then(|s| s.as_str())
                                                    == Some(sid.as_str())
                                        });
                                        if is_cancel {
                                            stop_reason = "cancelled";
                                            break;
                                        }
                                        deferred.push_back(l);
                                    }
                                    None => stdin_open = false,
                                }
                                continue;
                            }
                        }
                    } else {
                        futures_util::StreamExt::next(&mut cursor).await
                    };
                    match chunk {
                        Some(Ok(c)) => {
                            if let Some(update) = chunk_update(&c, &mut pending, &mut next_tool_id)
                            {
                                emit(&notification(
                                    "session/update",
                                    serde_json::json!({"sessionId": sid, "update": update}),
                                ));
                            }
                        }
                        Some(Err(e)) => {
                            eprintln!("acp: stream error: {e}");
                            break;
                        }
                        None => break, // turn complete
                    }
                }
                emit(&rpc_result(&id, serde_json::json!({"stopReason": stop_reason})));
            }
            ("session/cancel", None) => {} // no turn in flight — nothing to cancel
            (_, Some(id)) => emit(&rpc_error(&id, -32601, "method not found")),
            (_, None) => {} // unknown notification — ignore per JSON-RPC 2.0
        }
    }

    // Clean shutdown: close every live session's agent.
    for (_, agent) in sessions.drain() {
        let _ = agent.shutdown().await;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::{chunk_update, initialize_result, parse_frame, prompt_text, rpc_error, rpc_result};
    use std::collections::VecDeque;

    /// Requests parse with id + method + params; notifications with no id;
    /// garbage errors (the loop maps it to -32700).
    #[test]
    fn frames_parse_by_shape() {
        let req = parse_frame(r#"{"jsonrpc":"2.0","id":3,"method":"initialize","params":{"protocolVersion":1}}"#).unwrap();
        assert_eq!(req.method, "initialize");
        assert_eq!(req.id, Some(serde_json::json!(3)));
        assert_eq!(req.params["protocolVersion"], 1);
        let notif = parse_frame(r#"{"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":"s"}}"#).unwrap();
        assert!(notif.id.is_none());
        assert!(parse_frame("not json").is_err());
        assert!(parse_frame(r#"{"jsonrpc":"2.0","id":1}"#).is_err()); // no method
    }

    /// The declared capability surface is the MINIMAL compliant one: v1, no
    /// loadSession, no rich prompt blocks, no auth. Anything widened here must
    /// actually be implemented in the loop.
    #[test]
    fn initialize_declares_the_minimal_surface() {
        let r = initialize_result();
        assert_eq!(r["protocolVersion"], 1);
        assert_eq!(r["agentCapabilities"]["loadSession"], false);
        assert_eq!(r["agentCapabilities"]["promptCapabilities"]["image"], false);
        assert_eq!(r["agentCapabilities"]["promptCapabilities"]["embeddedContext"], false);
        assert_eq!(r["authMethods"], serde_json::json!([]));
        assert_eq!(r["agentInfo"]["name"], "localharness");
    }

    /// Text blocks concatenate in order; non-text blocks (which we never
    /// advertised support for) are skipped, not fatal.
    #[test]
    fn prompt_text_takes_text_blocks_only() {
        let params = serde_json::json!({"prompt": [
            {"type": "text", "text": "hello"},
            {"type": "image", "data": "…"},
            {"type": "text", "text": "world"},
        ]});
        assert_eq!(prompt_text(&params), "hello\nworld");
        assert_eq!(prompt_text(&serde_json::json!({})), "");
    }

    /// Chunk mapping: text → message chunk, tool call/result pair correlate
    /// FIFO (synthesized ids when the backend gives none), error results map
    /// to `failed`.
    #[test]
    fn chunks_map_to_session_updates() {
        use localharness::types::{ToolCall, ToolResult};
        use localharness::StreamChunk;
        let mut pending = VecDeque::new();
        let mut next = 0;

        let text = StreamChunk::Text { step_index: 0, text: "hi".into() };
        let u = chunk_update(&text, &mut pending, &mut next).unwrap();
        assert_eq!(u["sessionUpdate"], "agent_message_chunk");
        assert_eq!(u["content"]["text"], "hi");

        let call = StreamChunk::ToolCall(ToolCall {
            name: "evm_balance".into(),
            id: None,
            args: serde_json::json!({"address": "0x0"}),
            canonical_path: None,
        });
        let u = chunk_update(&call, &mut pending, &mut next).unwrap();
        assert_eq!(u["sessionUpdate"], "tool_call");
        assert_eq!(u["status"], "in_progress");
        let tool_id = u["toolCallId"].as_str().unwrap().to_string();

        let result = StreamChunk::ToolResult(ToolResult {
            name: "evm_balance".into(),
            id: None,
            result: Some(serde_json::json!({"wei": "1"})),
            error: None,
        });
        let u = chunk_update(&result, &mut pending, &mut next).unwrap();
        assert_eq!(u["sessionUpdate"], "tool_call_update");
        assert_eq!(u["toolCallId"].as_str().unwrap(), tool_id);
        assert_eq!(u["status"], "completed");

        // An errored result maps to failed.
        pending.push_back("call_9".into());
        let errored = StreamChunk::ToolResult(ToolResult {
            name: "t".into(),
            id: None,
            result: None,
            error: Some("boom".into()),
        });
        let u = chunk_update(&errored, &mut pending, &mut next).unwrap();
        assert_eq!(u["status"], "failed");
        assert_eq!(u["rawOutput"], "boom");

        // Empty text chunks are suppressed (no empty message frames).
        let empty = StreamChunk::Text { step_index: 0, text: String::new() };
        assert!(chunk_update(&empty, &mut pending, &mut next).is_none());
    }

    /// Wire lines are single-line JSON-RPC 2.0 frames.
    #[test]
    fn wire_lines_are_single_line_jsonrpc() {
        let ok = rpc_result(&serde_json::json!(1), serde_json::json!({"a": 1}));
        assert!(ok.contains("\"jsonrpc\":\"2.0\"") && !ok.contains('\n'));
        let err = rpc_error(&serde_json::json!(2), -32601, "method not found");
        assert!(err.contains("-32601") && !err.contains('\n'));
    }
}
