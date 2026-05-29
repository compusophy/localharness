//! `call_agent` — inter-agent RPC tool.
//!
//! Lets an agent send a text message to another agent by subdomain name
//! and receive its response. Under the hood, opens a hidden iframe to
//! `<name>.localharness.xyz/?rpc=1`, sends an `lh-agent-call` postMessage,
//! and awaits the `lh-agent-response`.
//!
//! Only available on wasm32 (browser) — native agents don't have iframes.

use std::sync::Arc;
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::Result;
use crate::tools::{Tool, ToolContext};

pub struct CallAgent;

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl Tool for CallAgent {
    fn name(&self) -> &str {
        "call_agent"
    }

    fn description(&self) -> &str {
        "Send a message to another agent by subdomain name and receive its response. \
         The target agent must have an API key configured. Use this to delegate tasks, \
         ask questions, or compose multi-agent workflows."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The subdomain name of the agent to call (e.g. 'alice')"
                },
                "message": {
                    "type": "string",
                    "description": "The message to send to the agent"
                }
            },
            "required": ["name", "message"]
        })
    }

    async fn execute(&self, args: Value, _ctx: Option<Arc<ToolContext>>) -> Result<Value> {
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let message = args
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if name.is_empty() || message.is_empty() {
            return Ok(json!({ "error": "name and message are required" }));
        }

        #[cfg(target_arch = "wasm32")]
        {
            match call_agent_impl(&name, &message).await {
                Ok(text) => Ok(json!({
                    "agent": name,
                    "response": text
                })),
                Err(err) => Ok(json!({
                    "agent": name,
                    "error": err
                })),
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            Ok(json!({ "error": "call_agent is only available in the browser" }))
        }
    }
}

#[cfg(target_arch = "wasm32")]
async fn call_agent_impl(name: &str, message: &str) -> std::result::Result<String, String> {
    use std::cell::RefCell;
    use std::rc::Rc;
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{HtmlIFrameElement, MessageEvent};

    // The name is model-supplied and interpolated into the RPC URL host.
    // Restrict it to the registry's DNS-label charset so a crafted value
    // (`.`, `/`, `@`, `:`…) can't reshape the host and aim the iframe at an
    // unintended origin.
    if name.is_empty()
        || name.len() > 63
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(format!(
            "invalid agent name '{name}' — must be lowercase a-z, 0-9, '-'"
        ));
    }

    let rpc_url = format!("https://{name}.localharness.xyz/?rpc=1");
    let rpc_origin = format!("https://{name}.localharness.xyz");

    let doc = web_sys::window()
        .and_then(|w| w.document())
        .ok_or_else(|| "no document".to_string())?;
    let body = doc.body().ok_or_else(|| "no body".to_string())?;

    let iframe: HtmlIFrameElement = doc
        .create_element("iframe")
        .map_err(|e| format!("create iframe: {e:?}"))?
        .dyn_into()
        .map_err(|_| "not an iframe".to_string())?;
    iframe.set_src(&rpc_url);
    let _ = iframe.set_attribute(
        "style",
        "display:none;width:0;height:0;border:0;position:absolute;",
    );
    body.append_child(&iframe)
        .map_err(|e| format!("append: {e:?}"))?;

    let id = format!("rpc-{:08x}", js_sys::Math::random().to_bits() as u32);

    let result_slot: Rc<RefCell<Option<std::result::Result<String, String>>>> =
        Rc::new(RefCell::new(None));
    let waker_slot: Rc<RefCell<Option<js_sys::Function>>> = Rc::new(RefCell::new(None));
    let ready_slot: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
    let ready_waker: Rc<RefCell<Option<js_sys::Function>>> = Rc::new(RefCell::new(None));

    let result_c = result_slot.clone();
    let waker_c = waker_slot.clone();
    let ready_c = ready_slot.clone();
    let ready_waker_c = ready_waker.clone();
    let id_c = id.clone();
    let origin_c = rpc_origin.clone();

    let handler = Closure::<dyn FnMut(_)>::new(move |event: MessageEvent| {
        let data = event.data();
        if data.is_null() || data.is_undefined() {
            return;
        }
        if event.origin() != origin_c {
            return;
        }
        let msg_type = js_sys::Reflect::get(&data, &JsValue::from_str("type"))
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default();

        if msg_type == "lh-rpc-ready" {
            *ready_c.borrow_mut() = true;
            if let Some(w) = ready_waker_c.borrow_mut().take() {
                let _ = w.call0(&JsValue::NULL);
            }
            return;
        }

        if msg_type != "lh-agent-response" {
            return;
        }
        let msg_id = js_sys::Reflect::get(&data, &JsValue::from_str("id"))
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default();
        if msg_id != id_c {
            return;
        }

        let outcome =
            if let Some(err) = js_sys::Reflect::get(&data, &JsValue::from_str("error"))
                .ok()
                .and_then(|v| v.as_string())
            {
                Err(err)
            } else {
                let text = js_sys::Reflect::get(&data, &JsValue::from_str("text"))
                    .ok()
                    .and_then(|v| v.as_string())
                    .unwrap_or_default();
                Ok(text)
            };
        *result_c.borrow_mut() = Some(outcome);
        if let Some(w) = waker_c.borrow_mut().take() {
            let _ = w.call0(&JsValue::NULL);
        }
    });

    let window = web_sys::window().ok_or_else(|| "no window".to_string())?;
    window
        .add_event_listener_with_callback("message", handler.as_ref().unchecked_ref())
        .map_err(|e| format!("listener: {e:?}"))?;

    // Wait for iframe content window
    let mut cw: Option<web_sys::Window> = None;
    for _ in 0..50 {
        if let Some(w) = iframe.content_window() {
            cw = Some(w);
            break;
        }
        sleep_ms(50).await;
    }
    let target = cw.ok_or_else(|| "iframe content window unavailable".to_string())?;

    // Wait for rpc-ready signal (15s timeout)
    if !*ready_slot.borrow() {
        let ready_p = js_sys::Promise::new(&mut |resolve, _| {
            *ready_waker.borrow_mut() = Some(resolve.clone());
            if let Some(w) = web_sys::window() {
                let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 15_000);
            }
        });
        let _ = JsFuture::from(ready_p).await;
    }

    // Send the RPC request
    let payload = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&payload, &JsValue::from_str("type"), &JsValue::from_str("lh-agent-call"));
    let _ = js_sys::Reflect::set(&payload, &JsValue::from_str("id"), &JsValue::from_str(&id));
    let _ = js_sys::Reflect::set(&payload, &JsValue::from_str("message"), &JsValue::from_str(message));

    let my_name = web_sys::window()
        .and_then(|w| w.location().hostname().ok())
        .unwrap_or_else(|| "unknown".to_string());
    let _ = js_sys::Reflect::set(&payload, &JsValue::from_str("from"), &JsValue::from_str(&my_name));

    target
        .post_message(&payload, &rpc_origin)
        .map_err(|e| format!("postMessage: {e:?}"))?;

    // Wait for response (60s timeout for LLM calls)
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        let resolve_c = resolve.clone();
        *waker_slot.borrow_mut() = Some(resolve_c);
        if let Some(w) = web_sys::window() {
            let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 60_000);
        }
    });
    let _ = JsFuture::from(promise).await;

    // Cleanup
    let _ = window.remove_event_listener_with_callback("message", handler.as_ref().unchecked_ref());
    let _ = body.remove_child(&iframe);
    drop(handler);

    result_slot
        .borrow()
        .clone()
        .unwrap_or_else(|| Err("timeout waiting for agent response".into()))
}

#[cfg(target_arch = "wasm32")]
async fn sleep_ms(ms: u32) {
    use wasm_bindgen_futures::JsFuture;
    let p = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(w) = web_sys::window() {
            let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms as i32);
        }
    });
    let _ = JsFuture::from(p).await;
}
