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
/// A callee's `lh-payment-required` challenge (raw strings off the wire).
struct ChallengeParts {
    to: String,
    value: String,
    // The caller deliberately does NOT use the callee's validBefore/nonce
    // (security: it sets its own window + nonce), so they aren't stored.
}

#[cfg(target_arch = "wasm32")]
fn parse_hex_n(s: &str, n: usize) -> std::result::Result<Vec<u8>, String> {
    let t = s.trim().trim_start_matches("0x");
    if t.len() != n * 2 {
        return Err(format!("hex len {} != {}", t.len(), n * 2));
    }
    (0..n)
        .map(|i| u8::from_str_radix(&t[i * 2..i * 2 + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

#[cfg(target_arch = "wasm32")]
fn hex0x(b: &[u8]) -> String {
    let mut s = String::from("0x");
    for x in b {
        s.push_str(&format!("{x:02x}"));
    }
    s
}

/// Sign the challenge via the app-injected x402 hook and build the
/// `payment` object to re-post.
/// Max `$LH` (wei) this caller will pay for a single agent call — a hard
/// ceiling so a malicious callee can't drain the wallet (C1/H1).
#[cfg(target_arch = "wasm32")]
const MAX_PAY_PER_CALL_WEI: u128 = 100_000_000_000_000_000_000; // 100 LH

#[cfg(target_arch = "wasm32")]
async fn pay_and_build(
    name: &str,
    ch: &ChallengeParts,
) -> std::result::Result<js_sys::Object, String> {
    use wasm_bindgen::JsValue;
    let to: [u8; 20] = parse_hex_n(&ch.to, 20)?
        .try_into()
        .map_err(|_| "bad to".to_string())?;

    // C1: the recipient MUST be the callee's on-chain registered payee
    // (its TBA), NOT an arbitrary address the callee named — otherwise a
    // malicious agent could redirect funds anywhere.
    let payee = crate::registry::tba_of_name(name)
        .await
        .map_err(|e| format!("payee lookup: {e}"))?
        .ok_or_else(|| "callee has no on-chain wallet — refusing to pay".to_string())?;
    let payee_b: [u8; 20] = parse_hex_n(&payee, 20)?
        .try_into()
        .map_err(|_| "bad payee".to_string())?;
    if to != payee_b {
        return Err("payment recipient ≠ agent's registered wallet — refusing".into());
    }

    // C1/H1: cap the per-call amount.
    let value_wei: u128 = ch.value.parse().map_err(|_| "bad value".to_string())?;
    if value_wei > MAX_PAY_PER_CALL_WEI {
        return Err("requested price exceeds the per-call cap".into());
    }

    // C2/L2: WE pick a short window + fresh nonce — never trust the callee's.
    let now = (js_sys::Date::now() / 1000.0) as u64;
    let valid_after = now.saturating_sub(60);
    let valid_before = now + 300;
    let mut nonce = [0u8; 32];
    if let Some(c) = web_sys::window().and_then(|w| w.crypto().ok()) {
        let _ = c.get_random_values_with_u8_array(&mut nonce);
    }

    let payment = crate::x402_hook::sign(crate::x402_hook::X402Challenge {
        to,
        value_wei,
        valid_after,
        valid_before,
        nonce,
    })
    .await?;

    let obj = js_sys::Object::new();
    let set = |k: &str, v: &str| {
        let _ = js_sys::Reflect::set(&obj, &JsValue::from_str(k), &JsValue::from_str(v));
    };
    set("from", &hex0x(&payment.from));
    set("value", &value_wei.to_string());
    set("nonce", &hex0x(&nonce));
    set("signature", &hex0x(&payment.signature));
    let _ = js_sys::Reflect::set(&obj, &JsValue::from_str("validAfter"), &JsValue::from_f64(valid_after as f64));
    let _ = js_sys::Reflect::set(&obj, &JsValue::from_str("validBefore"), &JsValue::from_f64(valid_before as f64));
    Ok(obj)
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
    // x402: a callee may answer with `lh-payment-required` before serving.
    let payment_slot: Rc<RefCell<Option<ChallengeParts>>> = Rc::new(RefCell::new(None));

    let result_c = result_slot.clone();
    let payment_c = payment_slot.clone();
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

        if msg_type == "lh-payment-required" {
            let msg_id = js_sys::Reflect::get(&data, &JsValue::from_str("id"))
                .ok()
                .and_then(|v| v.as_string())
                .unwrap_or_default();
            if msg_id != id_c {
                return;
            }
            let gs = |k: &str| {
                js_sys::Reflect::get(&data, &JsValue::from_str(k))
                    .ok()
                    .and_then(|v| v.as_string())
                    .unwrap_or_default()
            };
            *payment_c.borrow_mut() = Some(ChallengeParts {
                to: gs("to"),
                value: gs("value"),
            });
            if let Some(w) = waker_c.borrow_mut().take() {
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

    // Wait for the response — or, if the callee charges, pay (x402) and
    // retry once. 60s timeout per wait (LLM calls are slow).
    let mut paid = false;
    loop {
        let promise = js_sys::Promise::new(&mut |resolve, _| {
            *waker_slot.borrow_mut() = Some(resolve.clone());
            if let Some(w) = web_sys::window() {
                let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 60_000);
            }
        });
        let _ = JsFuture::from(promise).await;

        if result_slot.borrow().is_some() {
            break;
        }
        let challenge = payment_slot.borrow_mut().take();
        match challenge {
            Some(ch) if !paid => {
                paid = true;
                match pay_and_build(name, &ch).await {
                    Ok(payment_obj) => {
                        let retry = js_sys::Object::new();
                        let s = |k: &str, v: &str| {
                            let _ = js_sys::Reflect::set(&retry, &JsValue::from_str(k), &JsValue::from_str(v));
                        };
                        s("type", "lh-agent-call");
                        s("id", &id);
                        s("message", message);
                        s("from", &my_name);
                        let _ = js_sys::Reflect::set(&retry, &JsValue::from_str("payment"), &payment_obj);
                        if let Err(e) = target.post_message(&retry, &rpc_origin) {
                            *result_slot.borrow_mut() = Some(Err(format!("retry postMessage: {e:?}")));
                            break;
                        }
                        continue; // await the post-payment response
                    }
                    Err(e) => {
                        *result_slot.borrow_mut() = Some(Err(format!("x402 payment: {e}")));
                        break;
                    }
                }
            }
            _ => break, // timeout (or an unexpected second challenge)
        }
    }

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
