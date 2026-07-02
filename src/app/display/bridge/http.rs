//! host_http main-thread bridge (issue #19): the authed `/api/fetch` proxy
//! round-trip a cartridge's poll-model `http::get` rides on.

use js_sys::{Object, Reflect};
use wasm_bindgen::prelude::*;

/// host_http main-thread half (issue #19): the worker asked to GET `url` (the
/// cartridge has no auth signer + can't fetch cross-origin). Run the SAME authed
/// `/api/fetch` proxy POST the `web_fetch` tool uses (signed token, https-only,
/// private hosts denied, 200KB cap, textual content only), then post an
/// `http_result { id, status, body }` (or `{ id, error:true }`) back so the
/// worker's poll-model handle flips READY/ERROR. Mirrors `do_compose_spawn`.
pub(crate) async fn do_http_fetch(worker: web_sys::Worker, id: i32, url: String) {
    // A FRESH per-request proxy token (same scheme as web_fetch). No identity =>
    // the cartridge can't fetch; mark the handle errored.
    let Some((signer, _)) = crate::app::chat::credit_signer().await else {
        post_http_result(&worker, id, None);
        return;
    };
    let now = (js_sys::Date::now() / 1000.0) as u64;
    let token = crate::registry::proxy_auth_token(&signer, now, "fetch");
    let endpoint = format!(
        "{}api/fetch",
        crate::registry::CREDIT_PROXY_URL
    );
    let send = async {
        let resp = reqwest::Client::new()
            .post(&endpoint)
            .header("content-type", "application/json")
            .header("x-goog-api-key", token)
            .json(&serde_json::json!({ "url": url }))
            .send()
            .await
            .map_err(|e| format!("proxy request: {e}"))?;
        let status = resp.status();
        let body = resp
            .json::<serde_json::Value>()
            .await
            .map_err(|e| format!("proxy response decode: {e}"))?;
        Ok::<_, String>((status, body))
    };
    match crate::app::net::with_timeout(20_000, send).await {
        Ok(Ok((status, body))) if status.is_success() => {
            // `/api/fetch` returns { status, contentType, truncated, body } for a
            // textual hit, or { status, contentType, note } for binary (no body).
            let upstream = body.get("status").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let text = body.get("body").and_then(|v| v.as_str()).unwrap_or("");
            post_http_result_ok(&worker, id, upstream, text);
        }
        _ => post_http_result(&worker, id, None),
    }
}

/// Post a successful `http_result` to the worker (upstream status + body text).
fn post_http_result_ok(worker: &web_sys::Worker, id: i32, status: i32, body: &str) {
    let msg = Object::new();
    let _ = Reflect::set(&msg, &JsValue::from_str("type"), &JsValue::from_str("http_result"));
    let _ = Reflect::set(&msg, &JsValue::from_str("id"), &JsValue::from_f64(id as f64));
    let _ = Reflect::set(&msg, &JsValue::from_str("status"), &JsValue::from_f64(status as f64));
    let _ = Reflect::set(&msg, &JsValue::from_str("body"), &JsValue::from_str(body));
    let _ = worker.post_message(&msg);
}

/// Post an `http_result` to the worker. `None` => mark the request ERRORED
/// (`error: true`); the worker's `ready(handle)` then returns -2.
fn post_http_result(worker: &web_sys::Worker, id: i32, ok: Option<(i32, &str)>) {
    match ok {
        Some((status, body)) => post_http_result_ok(worker, id, status, body),
        None => {
            let msg = Object::new();
            let _ = Reflect::set(&msg, &JsValue::from_str("type"), &JsValue::from_str("http_result"));
            let _ = Reflect::set(&msg, &JsValue::from_str("id"), &JsValue::from_f64(id as f64));
            let _ = Reflect::set(&msg, &JsValue::from_str("error"), &JsValue::TRUE);
            let _ = worker.post_message(&msg);
        }
    }
}
