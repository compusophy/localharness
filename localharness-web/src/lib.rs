//! Browser entry point for localharness — now driving the real
//! `Agent` loop, not just `GeminiClient` directly. Each browser tab
//! holds one Agent in a thread_local across calls so the conversation
//! state lives on the Rust side, the way the SDK intends.
//!
//! M2.5 payoff: the full SDK surface (Agent → Conversation →
//! Connection → ToolRunner → built-in tools) is now wasm-portable, so
//! the web demo dogfoods the same code path a CLI host uses.

use std::cell::RefCell;
use std::rc::Rc;

use futures_util::StreamExt;
use wasm_bindgen::prelude::*;

use localharness::{Agent, GeminiAgentConfig, StreamChunk};

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console)]
    fn log(s: &str);
}

#[wasm_bindgen(start)]
fn on_load() {
    console_error_panic_hook::set_once();
}

// One Agent per tab, held across `chat` calls so the SDK's own
// conversation state drives multi-turn dialogue. wasm32 is
// single-threaded so a thread_local + RefCell is the right shape; the
// agent is !Sync on wasm (the relaxed-Send story), so we can't share it
// across a real OS thread anyway.
thread_local! {
    static AGENT: RefCell<Option<Rc<Agent>>> = const { RefCell::new(None) };
}

/// Start (or reset) the chat session. Must be called before `chat`.
/// Subsequent calls drop the previous Agent and create a fresh one.
#[wasm_bindgen]
pub async fn start_session(api_key: String) -> Result<(), JsValue> {
    let key = api_key.trim();
    if key.is_empty() {
        return Err(JsValue::from_str("api_key is empty"));
    }
    if !key.chars().all(|c| c.is_ascii_graphic()) || key.len() > 200 {
        return Err(JsValue::from_str(
            "api_key looks wrong - clear the key field and re-paste.",
        ));
    }
    log(&format!("start_session: key_len={}", key.len()));

    let cfg = GeminiAgentConfig::new(key.to_string());
    let agent = Agent::start_gemini(cfg)
        .await
        .map_err(|e| JsValue::from_str(&format!("start_gemini: {e}")))?;

    AGENT.with(|cell| *cell.borrow_mut() = Some(Rc::new(agent)));
    Ok(())
}

/// Send one turn through the active Agent. Streams text chunks to the
/// `on_chunk` callback as they arrive; returns the full assistant text.
#[wasm_bindgen]
pub async fn chat(prompt: String, on_chunk: js_sys::Function) -> Result<String, JsValue> {
    let agent = AGENT
        .with(|cell| cell.borrow().clone())
        .ok_or_else(|| JsValue::from_str("call start_session(api_key) first"))?;

    let response = agent
        .chat(prompt)
        .await
        .map_err(|e| JsValue::from_str(&format!("agent.chat: {e}")))?;

    let this = JsValue::NULL;
    let mut cursor = response.chunks();
    let mut out = String::new();

    while let Some(item) = cursor.next().await {
        match item {
            Ok(StreamChunk::Text { text, .. }) => {
                if !text.is_empty() {
                    out.push_str(&text);
                    let _ = on_chunk.call1(&this, &JsValue::from_str(&text));
                }
            }
            // Thoughts, tool-call markers, etc - drop silently for now.
            // M3 will surface them via separate callbacks.
            Ok(_) => {}
            Err(e) => return Err(JsValue::from_str(&format!("chunk: {e}"))),
        }
    }

    log(&format!("chat: done reply_len={}", out.len()));

    if out.is_empty() {
        return Err(JsValue::from_str(
            "model returned no text - check the browser console.",
        ));
    }
    Ok(out)
}

/// Drop the active session so the next `start_session` is fresh.
#[wasm_bindgen]
pub fn reset_session() {
    AGENT.with(|cell| *cell.borrow_mut() = None);
}
