//! host::chat open-chatroom bridge (worker ↔ the /api/chat relay).
//!
//! An open-chatroom cartridge (worker) posts chat:start (begin polling) and
//! chat:send (post a line). The relay GET/POST + personal-sign auth live HERE on
//! main; the ROOM is this subdomain. The poll loop is ADAPTIVE: it polls fast
//! (CHAT_FAST_MS) for a bounded burst after activity (you sent, or a poll
//! returned new lines) so a live conversation feels snappy, and backs off to
//! CHAT_IDLE_MS when the room is quiet — keeping average GitHub load (the relay
//! reads GitHub per poll; ~5000/hr shared) low. CHAT_ACTIVE gates the loop.

use js_sys::{Object, Reflect};
use wasm_bindgen::prelude::*;
use web_sys::Worker;

thread_local! {
    static CHAT_ACTIVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    // Fast-poll cycles remaining; refilled to CHAT_FAST_REFILL on activity.
    static CHAT_FAST: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
}
const CHAT_FAST_MS: u32 = 500; // poll cadence while a conversation is live
const CHAT_IDLE_MS: u32 = 2500; // poll cadence when the room is quiet
const CHAT_FAST_REFILL: i32 = 12; // fast cycles after activity (~6s at 500ms)

/// Begin polling the chatroom relay for this subdomain (idempotent — a
/// second chat:start while already active is a no-op). Spawned on the first
/// `host::chat` use. No tenant name (apex/preview) → inert.
pub(crate) fn chat_start(worker: Worker) {
    if CHAT_ACTIVE.with(|a| a.get()) {
        return;
    }
    let Some(room) = crate::app::tenant::current_name() else {
        return;
    };
    CHAT_ACTIVE.with(|a| a.set(true));
    wasm_bindgen_futures::spawn_local(chat_poll_loop(worker, room));
}

/// Poll `room` for new lines while CHAT_ACTIVE, posting each as `chat:msg`.
/// First poll (cursor -1) pulls the backlog; then only `n > cursor`. Adaptive
/// cadence: fast for a bounded burst after activity, idle floor otherwise.
async fn chat_poll_loop(worker: Worker, room: String) {
    let mut cursor: i64 = -1;
    while CHAT_ACTIVE.with(|a| a.get()) {
        if let Ok(msgs) = crate::registry::chat_poll(&room, cursor).await {
            if !msgs.is_empty() {
                CHAT_FAST.with(|f| f.set(CHAT_FAST_REFILL)); // live convo → stay fast
            }
            for (n, name, text) in msgs {
                if n > cursor {
                    cursor = n;
                }
                chat_post_to_worker(&worker, &format!("{name}: {text}"));
            }
        }
        // Burn one fast cycle if we have any, else idle.
        let ms = CHAT_FAST.with(|f| {
            let n = f.get();
            if n > 0 {
                f.set(n - 1);
                CHAT_FAST_MS
            } else {
                CHAT_IDLE_MS
            }
        });
        crate::runtime::sleep_ms(ms).await;
    }
}

fn chat_post_to_worker(worker: &Worker, line: &str) {
    let m = Object::new();
    let _ = Reflect::set(&m, &JsValue::from_str("type"), &JsValue::from_str("chat:msg"));
    let _ = Reflect::set(&m, &JsValue::from_str("text"), &JsValue::from_str(line));
    let _ = worker.post_message(&m);
}

/// POST a line to the chatroom relay for this subdomain (personal-sign authed
/// off the viewer's identity). No identity → silently dropped (the cartridge
/// can still READ the room).
pub(crate) fn chat_send(text: String) {
    CHAT_FAST.with(|f| f.set(CHAT_FAST_REFILL)); // poll fast right after we post
    wasm_bindgen_futures::spawn_local(async move {
        let Some(room) = crate::app::tenant::current_name() else {
            return;
        };
        let Some((signer, _)) = crate::app::chat::credit_signer().await else {
            return;
        };
        let now = (js_sys::Date::now() / 1000.0) as u64;
        let _ = crate::registry::chat_post(&signer, now, &room, &text).await;
    });
}

/// Halt the chatroom relay-poll loop (the loop exits on its next tick).
pub(crate) fn chat_stop() {
    CHAT_ACTIVE.with(|a| a.set(false));
    CHAT_FAST.with(|f| f.set(0));
}
