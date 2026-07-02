//! host_agent feed bridge (the "Ready Up" loop, feedback #103).
//!
//! Cartridge feed calls cross the worker→main boundary as messages. WRITES
//! (subscribe/unsubscribe/broadcast) are async on-chain / proxy ops the worker
//! can't do; the main thread runs them and posts the refreshed context back so
//! the cartridge's sync getters (`is_subscribed`, `subscriber_count`,
//! `viewer_has_identity`) catch up next frame. The feed is THIS cartridge's own
//! subdomain — there are no cross-subdomain feeds in v1.

use js_sys::{Object, Reflect};
use wasm_bindgen::prelude::*;

/// The current cartridge's feed tokenId = the tenant subdomain's NFT id.
/// `None` off a tenant (apex / localhost) — a feed needs a subdomain.
async fn feed_token_id() -> Option<u64> {
    let name = crate::app::tenant::current_name()?;
    match crate::app::registry::id_of_name(&name).await {
        Ok(id) if id != 0 => Some(id),
        _ => None,
    }
}

/// Post a partial `agent_context` update to the worker (only the provided
/// fields). The worker's `applyAgentContext` merges them into its cached state.
fn post_agent_context(
    worker: &web_sys::Worker,
    has_identity: Option<bool>,
    is_subscribed: Option<bool>,
    subscriber_count: Option<u32>,
) {
    let msg = Object::new();
    let _ = Reflect::set(&msg, &JsValue::from_str("type"), &JsValue::from_str("agent_context"));
    if let Some(h) = has_identity {
        let _ = Reflect::set(&msg, &JsValue::from_str("viewerHasIdentity"), &JsValue::from_f64(if h { 1.0 } else { 0.0 }));
    }
    if let Some(sub) = is_subscribed {
        let _ = Reflect::set(&msg, &JsValue::from_str("feedIsSubscribed"), &JsValue::from_f64(if sub { 1.0 } else { 0.0 }));
    }
    if let Some(c) = subscriber_count {
        let _ = Reflect::set(&msg, &JsValue::from_str("feedSubscriberCount"), &JsValue::from_f64(c as f64));
    }
    let _ = worker.post_message(&msg);
}

/// Read the live feed context (subscribed? count? identity?) and post it to
/// the worker. Best-effort: any read failure leaves that field unsent.
pub(crate) async fn refresh_feed_context(worker: web_sys::Worker) {
    // Skip for an anonymous visitor (no wallet): they can't subscribe, and this
    // avoids 3 RPC reads on EVERY cartridge launch (the public RPC is
    // rate-limited — a likely source of launch slowness).
    let addr = crate::app::chat::credit_address_existing().await;
    if addr.is_none() {
        return;
    }
    let Some(feed_id) = feed_token_id().await else { return };
    let has_identity = addr.is_some();
    let is_sub = match &addr {
        Some(a) => crate::app::registry::is_subscribed(feed_id, a).await.unwrap_or(false),
        None => false,
    };
    let count = crate::app::registry::subscriber_count(feed_id).await.unwrap_or(0) as u32;
    post_agent_context(&worker, Some(has_identity), Some(is_sub), Some(count));
}

/// subscribe / unsubscribe the viewer to this feed (sponsored), then refresh.
pub(crate) async fn do_feed_subscribe(worker: web_sys::Worker, subscribe: bool) {
    let Some(feed_id) = feed_token_id().await else { return };
    let Some((signer, _)) = crate::app::chat::credit_signer().await else { return };
    let res = if subscribe {
        crate::app::registry::subscribe_sponsored(&signer, feed_id).await
    } else {
        crate::app::registry::unsubscribe_sponsored(&signer, feed_id).await
    };
    if let Err(e) = res {
        web_sys::console::warn_1(&JsValue::from_str(&format!("feed subscribe: {e}")));
    } else if subscribe {
        // The SUBSCRIBE gesture is the right (and only) place to ask for
        // notification permission and register THIS device for Web Push — a
        // subscriber only ever RECEIVES a broadcast if their push subscription
        // is published under their OWN identity's MAIN (the proxy resolves
        // mainOf(subscriber) → push_sub). Removing this earlier left every
        // broadcast unreachable AND never prompted for permission, so nothing
        // ever buzzed. Best-effort: no-ops (silently) for a bare device key
        // with no MAIN tokenId to hang the subscription on.
        if crate::app::notifications::ensure_permission().await.unwrap_or(false) {
            publish_viewer_push_sub().await;
        }
    }
    refresh_feed_context(worker).await;
}

/// Register the VIEWER's Web Push subscription on-chain keyed by THEIR OWN
/// ADDRESS (`PushFacet.setPushSub`), signed by the viewer's credit key
/// (sponsored) — the slot `/api/broadcast` reads to reach this exact device.
/// Address-keyed so it works for ANY device, INCLUDING a bare device key with
/// no registered MAIN identity (the old MAIN-tokenId slot left such devices
/// unreachable — the cross-device-push bug). Permission already ensured by the
/// caller.
async fn publish_viewer_push_sub() {
    let Ok(sub_json) = crate::app::notifications::subscribe_push().await else { return };
    let Some((signer, _)) = crate::app::chat::credit_signer().await else { return };
    if let Err(e) = crate::registry::set_push_sub_sponsored(&signer, sub_json.as_bytes())
    .await
    {
        web_sys::console::warn_1(&JsValue::from_str(&format!("publish push_sub: {e}")));
    }
}

/// THE READY UP: POST /api/broadcast so the proxy pushes to every subscriber.
pub(crate) async fn do_feed_broadcast(title: String, body: String) {
    let Some(feed_id) = feed_token_id().await else { return };
    let Some((signer, _)) = crate::app::chat::credit_signer().await else { return };
    let now = (js_sys::Date::now() / 1000.0) as u64;
    let token = crate::registry::proxy_auth_token(&signer, now, "broadcast");
    let url = format!(
        "{}api/broadcast",
        crate::registry::CREDIT_PROXY_URL
    );
    let payload = serde_json::json!({ "targetId": feed_id, "title": title, "body": body });
    let send = async {
        reqwest::Client::new()
            .post(&url)
            .header("content-type", "application/json")
            .header("x-goog-api-key", token)
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("broadcast request: {e}"))
    };
    match crate::app::net::with_timeout(20_000, send).await {
        Ok(Ok(_resp)) => {}
        Ok(Err(e)) => web_sys::console::warn_1(&JsValue::from_str(&format!("broadcast: {e}"))),
        Err(e) => web_sys::console::warn_1(&JsValue::from_str(&format!("broadcast timeout: {e}"))),
    }
    // Immediate LOCAL feedback for the presser, on BOTH surfaces the user asked
    // for: (1) the in-app header bell (always — no permission needed), and (2) an
    // OS notification (permission-gated). The proxy fan-out reaches OTHER
    // subscribers' phones via Web Push; this is the presser's own ding.
    crate::app::notifications::push_to_bell(&title, &body);
    if crate::app::notifications::ensure_permission().await.unwrap_or(false) {
        let _ = crate::app::notifications::show(&title, &body).await;
    }
    crate::app::notifications::vibrate(120);
}

/// Ensure the viewer has a wallet (credit_signer generates + persists a device
/// key if none), then refresh context so `viewer_has_identity` flips to 1.
pub(crate) async fn do_feed_request_identity(worker: web_sys::Worker) {
    let _ = crate::app::chat::credit_signer().await;
    refresh_feed_context(worker).await;
}

thread_local! {
    /// True while a cartridge that imports the host::agent FEED is running (set
    /// by the worker's `cartridge_uses_feed` message). Gates permission-priming
    /// so ONLY feed cartridges prompt — a plain game never asks for notifications.
    static FEED_CARTRIDGE_ACTIVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// One-shot guard so priming fires at most once per cartridge load (each tap
    /// would otherwise re-attempt). Reset when a new feed cartridge loads.
    static FEED_PRIMED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(crate) fn set_feed_cartridge_active(on: bool) {
    FEED_CARTRIDGE_ACTIVE.with(|c| c.set(on));
    if on {
        FEED_PRIMED.with(|c| c.set(false));
    }
}

/// Called from the main-thread CANVAS TAP (mousedown / touchstart on a cartridge
/// canvas) — the ONE real user gesture in the cartridge flow. The cartridge's
/// own `subscribe()` tap can't request notification permission (it arrives via a
/// worker postMessage with no user activation); THIS does, on the tap that
/// produced it. Once permission is granted it registers this device for Web Push
/// (address-keyed) so a READY-UP broadcast can reach it. Fires at most once per
/// cartridge load; no-ops for non-feed cartridges and after a hard deny.
pub(crate) fn prime_feed_permission_on_gesture() {
    if !FEED_CARTRIDGE_ACTIVE.with(|c| c.get()) || FEED_PRIMED.with(|c| c.get()) {
        return;
    }
    if matches!(
        web_sys::Notification::permission(),
        web_sys::NotificationPermission::Denied
    ) {
        return; // a denied site can't be re-prompted — needs a manual settings reset
    }
    FEED_PRIMED.with(|c| c.set(true));
    wasm_bindgen_futures::spawn_local(async {
        if crate::app::notifications::ensure_permission().await.unwrap_or(false) {
            publish_viewer_push_sub().await;
        } else {
            // permission not (yet) granted — allow a later tap to try again
            FEED_PRIMED.with(|c| c.set(false));
        }
    });
}
