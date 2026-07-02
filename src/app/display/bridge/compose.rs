//! host::compose main-thread bridge: resolve a spawned child's PUBLISHED
//! `app.wasm` (the worker can't do the on-chain registry read) and post the
//! bytes back into the worker's compose tree.

use std::cell::RefCell;

use wasm_bindgen::prelude::*;

thread_local! {
    /// Session-lived cache of published child `app.wasm` bytes, keyed by name
    /// (host::compose). The worker can't do the on-chain registry read, so the
    /// main thread resolves the bytes for a `compose_spawn` and posts them back;
    /// repeat spawns of the same name reuse the cache instead of re-hitting the
    /// chain. Cache lifetime = the page session (a parent reload re-spawns from
    /// scratch); staleness only matters across an on-chain republish, which is a
    /// reload-scale event. (The compose-core `WasmCache` is content-addressed;
    /// this main-thread cache is name-keyed — the cheaper of the two for the
    /// "same name, same session" hit path.)
    static COMPOSE_WASM_CACHE: RefCell<std::collections::HashMap<String, Vec<u8>>> =
        RefCell::new(std::collections::HashMap::new());
}

/// host::compose main-thread half: the worker asked to mount `name` as a child
/// (handle already allocated worker-side in the LOADING state). Resolve that
/// subdomain's PUBLISHED on-chain `app.wasm` (cached per session) and post it
/// back as `compose_bytes`; the worker instantiates it into its slot. A
/// `wasm: null` reply marks the child FAILED (unregistered / no published app).
pub(crate) async fn do_compose_spawn(worker: web_sys::Worker, uid: i32, name: String) {
    // Cache hit → reuse; else fetch the published bytes and remember them.
    let cached = COMPOSE_WASM_CACHE.with(|c| c.borrow().get(&name).cloned());
    let bytes = match cached {
        Some(b) => Some(b),
        None => {
            let fetched = crate::app::compose_module_wasm(&name).await;
            if let Some(ref b) = fetched {
                COMPOSE_WASM_CACHE.with(|c| {
                    c.borrow_mut().insert(name.clone(), b.clone());
                });
            }
            fetched
        }
    };
    post_compose_bytes(&worker, uid, bytes.as_deref());
}

/// Post a `compose_bytes` reply to the worker: the resolved child `app.wasm`
/// (transferred zero-copy) or `wasm: null` to mark the slot FAILED. Keyed by the
/// child's global `uid` (compose is a tree now — a flat handle can't address a
/// node nested under another node's table).
fn post_compose_bytes(worker: &web_sys::Worker, uid: i32, bytes: Option<&[u8]>) {
    use js_sys::{Object, Reflect, Uint8Array};
    let msg = Object::new();
    let _ = Reflect::set(&msg, &JsValue::from_str("type"), &JsValue::from_str("compose_bytes"));
    let _ = Reflect::set(&msg, &JsValue::from_str("uid"), &JsValue::from_f64(uid as f64));
    match bytes {
        Some(b) => {
            let arr = Uint8Array::from(b);
            let buf = arr.buffer();
            let _ = Reflect::set(&msg, &JsValue::from_str("wasm"), &buf);
            // Transfer the ArrayBuffer (zero-copy); instantiate copies it worker-side.
            let transfer = js_sys::Array::new();
            transfer.push(&buf);
            let _ = worker.post_message_with_transfer(&msg, &transfer);
        }
        None => {
            let _ = Reflect::set(&msg, &JsValue::from_str("wasm"), &JsValue::NULL);
            let _ = worker.post_message(&msg);
        }
    }
}
