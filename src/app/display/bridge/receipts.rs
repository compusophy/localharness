//! Call-receipt main-thread bridge: bind the worker's per-frame
//! `compose::call` records to their module content hashes and persist them as
//! execution receipts (`localharness::receipt`) in a bounded OPFS ring.
//!
//! The split of labor: the WORKER knows what ran (uid, export, args, result,
//! wire status) but must stay hash-free (keccak in JS would be a hand-ported
//! parity liability); the MAIN thread saw every child's bytes go by in the
//! `compose_bytes` reply, so it computes `keccak(bytes)` ONCE per spawn and
//! joins the two on `uid` here. Records whose status maps to no execution are
//! dropped by the SSOT mapping (`CallStatus::from_compose_status`), never by
//! ad-hoc worker logic.
//!
//! N=1 rent: `.lh_receipts.jsonl` is a deterministic record of what your
//! compositions actually called and what came back — readable in the files
//! modal and by the agent's own fs tools, no counterparty required.

use std::cell::RefCell;
use std::collections::HashMap;

use wasm_bindgen::{JsCast, JsValue};

/// The OPFS ring file. One JSON object per line, newest last.
pub(crate) const RECEIPTS_FILE: &str = ".lh_receipts.jsonl";

/// Ring cap in lines — at ~300 bytes/receipt this bounds the file to ~75 KB.
const RECEIPTS_CAP: usize = 256;

thread_local! {
    /// uid → (child name, module keccak). Written when a `compose_spawn`
    /// resolves bytes; read when that child's call records arrive with a
    /// frame. Session-lived like the compose wasm cache.
    static MODULE_HASHES: RefCell<HashMap<i32, (String, [u8; 32])>> =
        RefCell::new(HashMap::new());
}

/// Record a spawned child's module hash (called from the `compose_spawn`
/// resolution path with the exact bytes the worker will instantiate).
pub(crate) fn note_module(uid: i32, name: &str, bytes: &[u8]) {
    let hash = crate::registry::keccak32(bytes);
    MODULE_HASHES.with(|m| {
        m.borrow_mut().insert(uid, (name.to_string(), hash));
    });
}

/// Handle the `calls` batch riding a frame message: bind each record to its
/// module hash, build receipts, and append them to the OPFS ring in ONE write.
/// Best-effort end to end — a malformed record or a missing hash drops that
/// record (a receipt that can't name its module binds nothing), and storage
/// failures log rather than disturb the frame path.
pub(crate) fn handle_frame_calls(data: &JsValue) {
    use js_sys::Reflect;
    let Ok(calls) = Reflect::get(data, &JsValue::from_str("calls")) else { return };
    if calls.is_undefined() || calls.is_null() {
        return;
    }
    let Ok(arr) = calls.dyn_into::<js_sys::Array>() else { return };
    let mut lines: Vec<String> = Vec::new();
    for entry in arr.iter() {
        if let Some(line) = record_to_line(&entry) {
            lines.push(line);
        }
    }
    // A dropped-records count is worth surfacing once per batch (a cartridge
    // calling >256 exports per frame is unusual enough to want visible).
    if let Ok(dropped) = Reflect::get(data, &JsValue::from_str("callsDropped")) {
        if let Some(n) = dropped.as_f64() {
            if n > 0.0 {
                web_sys::console::warn_1(&JsValue::from_str(&format!(
                    "receipts: {n} call record(s) dropped this frame (per-frame cap)"
                )));
            }
        }
    }
    if lines.is_empty() {
        return;
    }
    wasm_bindgen_futures::spawn_local(async move {
        let fs = crate::app::shared_opfs();
        let existing = fs
            .read(RECEIPTS_FILE)
            .await
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
            .unwrap_or_default();
        let mut blob = existing;
        for line in &lines {
            blob = crate::receipt::ring_append(&blob, line, RECEIPTS_CAP);
        }
        if let Err(e) = fs.write_atomic(RECEIPTS_FILE, blob.as_bytes()).await {
            web_sys::console::warn_1(&JsValue::from_str(&format!("receipts: write: {e}")));
        }
    });
}

/// One worker record → one receipt JSONL line. `None` drops it: unknown uid
/// (no module hash to bind), a wire status that maps to no execution, or a
/// shape that doesn't parse (defensive — the worker is ours, but the frame
/// path must never panic on a message field).
fn record_to_line(entry: &JsValue) -> Option<String> {
    use js_sys::Reflect;
    let get = |k: &str| Reflect::get(entry, &JsValue::from_str(k)).ok();
    let uid = get("uid")?.as_f64()? as i32;
    let export = get("fn")?.as_string()?;
    let status_code = get("status")?.as_f64()? as i32;
    let status = crate::receipt::CallStatus::from_compose_status(status_code)?;
    let (name, module_keccak) = MODULE_HASHES.with(|m| m.borrow().get(&uid).cloned())?;
    let args: Vec<i32> = get("args")
        .and_then(|v| v.dyn_into::<js_sys::Array>().ok())
        .map(|a| a.iter().filter_map(|x| x.as_f64().map(|f| f as i32)).collect())
        .unwrap_or_default();
    let result = get("result").and_then(|v| v.as_f64()).map(|f| f as i32);
    let receipt = crate::receipt::Receipt::call_on_module(
        module_keccak,
        crate::receipt::CallRecord { export, args, result, status, fuel: 0 },
    );
    // The line is the receipt's JSON view plus routing context (the child
    // NAME) — context lives outside the hash: names are mutable routing,
    // the module hash is the truth.
    let mut json = receipt.to_json();
    json["module"] = serde_json::Value::String(name);
    Some(json.to_string())
}
