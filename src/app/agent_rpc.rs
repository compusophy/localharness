//! Inter-agent RPC — the actor-model nervous system.
//!
//! When a subdomain loads with `?rpc=1`, it starts as a lightweight
//! agent endpoint instead of the full chat UI. It loads the agent
//! (same as normal — api key from OPFS, system prompt, tool allowlist)
//! and instens for `lh-agent-call` postMessage requests from other
//! subdomains. Each request is routed through the agent's chat loop,
//! and the response is sent back as `lh-agent-response`.
//!
//! **Message protocol:**
//! ```text
//! caller  → agent: { type: "lh-agent-call", id, message, from }
//! agent  → caller: { type: "lh-agent-response", id, text }
//!              or: { type: "lh-agent-response", id, error }
//! ```
//!
//! `from` is the caller's subdomain name (for logging/trust).
//! The agent processes one request at a time (sequential, not parallel).
//!
//! Origin validation: only accepts messages from `*.localharness.xyz`
//! or `localhost` origins.

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::MessageEvent;

use super::dom;

pub(crate) fn has_rpc_hint() -> bool {
    web_sys::window()
        .and_then(|w| w.location().search().ok())
        .map(|s| s.contains("rpc=1"))
        .unwrap_or(false)
}

fn is_trusted_origin(origin: &str) -> bool {
    origin.ends_with(".localharness.xyz")
        || origin == "https://localharness.xyz"
        || origin.starts_with("http://localhost")
        || origin.starts_with("http://127.0.0.1")
}

pub(crate) fn install_rpc_listener() -> Result<(), JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;

    let handler = Closure::<dyn FnMut(_)>::new(move |event: MessageEvent| {
        let data = event.data();
        if data.is_null() || data.is_undefined() {
            return;
        }
        let origin = event.origin();
        if !is_trusted_origin(&origin) {
            return;
        }
        let msg_type = js_sys::Reflect::get(&data, &JsValue::from_str("type"))
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default();
        if msg_type != "lh-agent-call" {
            return;
        }

        let id = js_sys::Reflect::get(&data, &JsValue::from_str("id"))
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default();
        let message = js_sys::Reflect::get(&data, &JsValue::from_str("message"))
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default();
        let from = js_sys::Reflect::get(&data, &JsValue::from_str("from"))
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_else(|| "unknown".to_string());

        if id.is_empty() || message.is_empty() {
            return;
        }

        let source = event.source();
        let reply_origin = origin.clone();

        wasm_bindgen_futures::spawn_local(async move {
            let response = handle_agent_call(&id, &message, &from).await;
            if let Some(source) = source {
                let _ = js_sys::Reflect::get(&source, &JsValue::from_str("postMessage"))
                    .ok()
                    .and_then(|pm| pm.dyn_ref::<js_sys::Function>().cloned())
                    .map(|pm| {
                        let _ = pm.call2(
                            &source,
                            &response,
                            &JsValue::from_str(&reply_origin),
                        );
                    });
            }
        });
    });

    window
        .add_event_listener_with_callback("message", handler.as_ref().unchecked_ref())
        .map_err(|e| JsValue::from_str(&format!("rpc listener: {e:?}")))?;
    handler.forget();

    // Announce readiness so callers can start sending immediately
    let ready = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &ready,
        &JsValue::from_str("type"),
        &JsValue::from_str("lh-rpc-ready"),
    );
    if let Some(parent) = window.parent().ok().flatten() {
        let _ = parent.post_message(&ready, "*");
    }

    Ok(())
}

async fn handle_agent_call(id: &str, message: &str, from: &str) -> JsValue {
    web_sys::console::log_1(&JsValue::from_str(&format!(
        "rpc: call from {from}: {message}"
    )));

    let result = process_message(message).await;

    let response = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &response,
        &JsValue::from_str("type"),
        &JsValue::from_str("lh-agent-response"),
    );
    let _ = js_sys::Reflect::set(
        &response,
        &JsValue::from_str("id"),
        &JsValue::from_str(id),
    );
    match result {
        Ok(text) => {
            let _ = js_sys::Reflect::set(
                &response,
                &JsValue::from_str("text"),
                &JsValue::from_str(&text),
            );
        }
        Err(err) => {
            let _ = js_sys::Reflect::set(
                &response,
                &JsValue::from_str("error"),
                &JsValue::from_str(&err),
            );
        }
    }
    response.into()
}

async fn process_message(message: &str) -> Result<String, String> {
    // Check if we have an active agent session
    let agent = super::APP.with(|cell| {
        cell.borrow().agent.as_ref().cloned()
    });

    if let Some(agent) = agent {
        let response = agent
            .chat(message)
            .await
            .map_err(|e| format!("agent error: {e}"))?;
        let text = response
            .text()
            .await
            .map_err(|e| format!("text error: {e}"))?;
        Ok(text)
    } else {
        Err("no agent session active — set a Gemini API key first".into())
    }
}

/// Paint the minimal RPC endpoint chrome. Starts a headless agent
/// session (same config as the chat UI but no transcript rendering).
pub(crate) async fn paint_rpc() {
    let name = match super::tenant::current() {
        super::tenant::Host::Tenant(n) => n,
        _ => "rpc".to_string(),
    };

    if let Some(root) = dom::by_id("root") {
        root.set_inner_html(&format!(
            "<main style=\"padding:24px;color:#7a8493;font:14px ui-monospace,Menlo,Consolas,monospace\">\
             {name} · rpc endpoint · listening\
             </main>"
        ));
    }

    // Start a headless agent session if we have an API key
    if let Some(key) = super::key_store::load().await {
        match super::chat::start_session(&key).await {
            Ok(()) => {
                web_sys::console::log_1(&JsValue::from_str("rpc: agent session started"));
            }
            Err(e) => {
                web_sys::console::warn_1(&JsValue::from_str(&format!(
                    "rpc: failed to start agent: {e:?}"
                )));
            }
        }
    }
}
