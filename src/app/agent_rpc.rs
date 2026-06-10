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
    // Was `starts_with("http://localhost")`, which also trusted
    // `http://localhost.evil.com`. Centralised host-exact check now;
    // localhost honoured only in dev.
    super::tenant::is_trusted_lh_origin(origin)
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
        let payment = extract_payment(&data);

        wasm_bindgen_futures::spawn_local(async move {
            let response = handle_agent_call(&id, &message, &from, payment).await;
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

/// An incoming x402 `payment` payload (the caller's signed authorization).
struct PaymentParts {
    from_hex: String,
    value_dec: String,
    valid_after: u64,
    valid_before: u64,
    nonce_hex: String,
    sig_hex: String,
}

fn extract_payment(data: &JsValue) -> Option<PaymentParts> {
    let p = js_sys::Reflect::get(data, &JsValue::from_str("payment")).ok()?;
    if p.is_undefined() || p.is_null() {
        return None;
    }
    let get_str = |k: &str| {
        js_sys::Reflect::get(&p, &JsValue::from_str(k))
            .ok()
            .and_then(|v| v.as_string())
    };
    let get_num = |k: &str| {
        js_sys::Reflect::get(&p, &JsValue::from_str(k))
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as u64
    };
    Some(PaymentParts {
        from_hex: get_str("from")?,
        value_dec: get_str("value")?,
        valid_after: get_num("validAfter"),
        valid_before: get_num("validBefore"),
        nonce_hex: get_str("nonce")?,
        sig_hex: get_str("signature")?,
    })
}

/// This agent's per-call x402 price in `$LH` wei (`.lh_x402_price` in OPFS;
/// 0 / missing = free, current behavior).
async fn x402_price() -> u128 {
    use crate::filesystem::Filesystem;
    let fs = super::shared_opfs();
    match fs.read(".lh_x402_price").await {
        Ok(bytes) => String::from_utf8(bytes)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0),
        Err(_) => 0,
    }
}

/// Fixed-length hex decode — thin wrapper over [`crate::encoding::hex_to_bytes`]
/// that also enforces an exact byte count (addresses, nonces, signatures).
fn parse_hex(s: &str, n: usize) -> Result<Vec<u8>, String> {
    let bytes = crate::encoding::hex_to_bytes(s)?;
    if bytes.len() != n {
        return Err(format!("expected {n} bytes, got {}", bytes.len()));
    }
    Ok(bytes)
}

fn build_response(id: &str, result: Result<String, String>) -> JsValue {
    let response = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&response, &JsValue::from_str("type"), &JsValue::from_str("lh-agent-response"));
    let _ = js_sys::Reflect::set(&response, &JsValue::from_str("id"), &JsValue::from_str(id));
    match result {
        Ok(text) => {
            let _ = js_sys::Reflect::set(&response, &JsValue::from_str("text"), &JsValue::from_str(&text));
        }
        Err(err) => {
            let _ = js_sys::Reflect::set(&response, &JsValue::from_str("error"), &JsValue::from_str(&err));
        }
    }
    response.into()
}

/// Build the `lh-payment-required` challenge: pay `price` `$LH` to this
/// agent's address, with a fresh nonce + 5-minute validity.
async fn build_payment_required(id: &str, price: u128, my_name: &str) -> JsValue {
    // Bill to the agent's on-chain payee (its TBA). The caller verifies
    // this against the registry before paying, so an honest agent's
    // address resolves and a spoofed one is rejected caller-side.
    let to = match super::registry::tba_of_name(my_name).await {
        Ok(Some(a)) => a,
        _ => return build_response(id, Err("agent has no on-chain wallet to bill to".into())),
    };
    let mut nonce = [0u8; 32];
    rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut nonce);
    let now = (js_sys::Date::now() / 1000.0) as u64;
    let obj = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&obj, &JsValue::from_str("type"), &JsValue::from_str("lh-payment-required"));
    let _ = js_sys::Reflect::set(&obj, &JsValue::from_str("id"), &JsValue::from_str(id));
    let _ = js_sys::Reflect::set(&obj, &JsValue::from_str("to"), &JsValue::from_str(&to));
    let _ = js_sys::Reflect::set(&obj, &JsValue::from_str("value"), &JsValue::from_str(&price.to_string()));
    let _ = js_sys::Reflect::set(&obj, &JsValue::from_str("validBefore"), &JsValue::from_f64((now + 300) as f64));
    let _ = js_sys::Reflect::set(&obj, &JsValue::from_str("nonce"), &JsValue::from_str(&crate::encoding::bytes_to_hex_str(&nonce)));
    obj.into()
}

/// Verify + settle an incoming x402 payment on-chain. The payer signed
/// `to = this agent's address`, so we settle to that same address; a
/// mismatched `to` makes the on-chain signature check fail.
async fn settle_incoming(price: u128, p: &PaymentParts, my_name: &str) -> Result<(), String> {
    let value: u128 = p.value_dec.parse().map_err(|_| "bad value".to_string())?;
    if value < price {
        return Err("underpaid".into());
    }
    // Settle TO this agent's registered payee (the TBA the caller signed
    // over). A mismatch would fail the on-chain signature check anyway.
    let payee = super::registry::tba_of_name(my_name)
        .await
        .map_err(|e| format!("payee: {e}"))?
        .ok_or_else(|| "no payee".to_string())?;
    let to: [u8; 20] = parse_hex(&payee, 20)?.try_into().unwrap();
    // The local credit key SUBMITS the sponsored settle (it isn't the payee).
    let (signer, _) = super::chat::credit_signer()
        .await
        .ok_or_else(|| "no identity".to_string())?;
    let fee_payer = super::sponsor::signer()?;
    let from: [u8; 20] = parse_hex(&p.from_hex, 20)?.try_into().unwrap();
    let nonce: [u8; 32] = parse_hex(&p.nonce_hex, 32)?.try_into().unwrap();
    let sig: [u8; 65] = parse_hex(&p.sig_hex, 65)?.try_into().unwrap();
    super::registry::settle_x402_sponsored(
        &signer,
        &fee_payer,
        &from,
        &to,
        value,
        p.valid_after,
        p.valid_before,
        &nonce,
        &sig,
        super::registry::ALPHA_USD_ADDRESS,
    )
    .await?;
    // H2: confirm the settlement actually consumed the nonce on-chain
    // before serving — a reverted/dropped settle must NOT yield free service.
    if !super::registry::x402_authorization_state(&p.from_hex, &nonce)
        .await
        .unwrap_or(false)
    {
        return Err("settlement not confirmed".into());
    }
    Ok(())
}

async fn handle_agent_call(
    id: &str,
    message: &str,
    from: &str,
    payment: Option<PaymentParts>,
) -> JsValue {
    web_sys::console::log_1(&JsValue::from_str(&format!(
        "rpc: call from {from}: {message}"
    )));

    // x402 gate: if this agent charges, require a settled $LH payment.
    let price = x402_price().await;
    if price > 0 {
        // Charging needs a registered identity to bill to.
        let Some(my_name) = super::tenant::current_name() else {
            return build_response(
                id,
                Err("agent is not a registered subdomain — cannot charge".into()),
            );
        };
        match payment {
            None => return build_payment_required(id, price, &my_name).await,
            Some(p) => {
                if let Err(e) = settle_incoming(price, &p, &my_name).await {
                    return build_response(id, Err(format!("payment: {e}")));
                }
            }
        }
    }

    build_response(id, process_message(message).await)
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
        // The shared const is the caller's fallback trigger: `call_agent`
        // matches on it and reroutes through the hosted x402 path.
        Err(format!(
            "{} — no model key on this device",
            crate::builtins::NO_SESSION_ERR
        ))
    }
}

/// Paint the minimal RPC endpoint chrome. Starts a headless agent
/// session (same config as the chat UI but no transcript rendering).
pub(crate) async fn paint_rpc() {
    let name = super::tenant::current_name().unwrap_or_else(|| "rpc".to_string());

    if let Some(root) = dom::by_id("root") {
        // maud escapes `name` (a hostname-derived subdomain). DNS labels
        // can't contain HTML metacharacters, but keep the escaped path so
        // no raw-HTML-with-interpolation sink exists anywhere in the app.
        root.set_inner_html(
            &maud::html! {
                main style="padding:24px;color:#7a8493;font:14px ui-monospace,Menlo,Consolas,monospace" {
                    (name) " · rpc endpoint · listening"
                }
            }
            .into_string(),
        );
    }

    // Start a headless agent session if we have an API key
    if let Some(key) = super::key_store::load().await {
        // Headless RPC uses the loaded key directly (BYOK): no proxy,
        // identity == key.
        match super::chat::start_session(&key, None, &key).await {
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
