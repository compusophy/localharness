//! Notifications + Web Push (browser-app).
//!
//! Two surfaces share this module:
//!   * the agent's `notify(title, body?, vibrate?)` closure tool
//!     (`chat::tools::misc::notify_tool`) — in-tab notifications/vibration
//!     for alarms, message-arrived, job-done;
//!   * the admin "notifications" row (`Action::EnableNotifications`) —
//!     subscribes Web Push and publishes the subscription JSON on-chain so
//!     the proxy's scheduler worker can notify the owner with the tab CLOSED
//!     (`proxy/api/scheduler.ts` reads it back per job owner).
//!
//! Notifications are shown through the SERVICE-WORKER registration when one
//! exists (`registration.showNotification`) — the page-level
//! `new Notification(...)` constructor throws in a document context on
//! Android, so the SW path is the one that works everywhere. `web/sw.js` is
//! registered by `boot.js` at boot; `subscribe_push` re-registers it
//! idempotently before subscribing.

use std::cell::RefCell;

use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

thread_local! {
    /// In-app notification bell log (newest first, capped). The header
    /// `#notif-bell-panel` renders from this; `#notif-bell-badge` shows the
    /// unread count. Fed by READY-UP broadcasts (the presser's own ding) and,
    /// later, service-worker pushes relayed to the open page.
    static BELL: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
}

/// Append a notification to the in-app header bell (newest first, cap 30) and
/// bump the unread badge. The panel content is refreshed (kept closed) so it's
/// current when the bell is next opened.
pub(crate) fn push_to_bell(title: &str, body: &str) {
    BELL.with(|b| {
        let mut v = b.borrow_mut();
        v.insert(0, (title.to_string(), body.to_string()));
        v.truncate(30);
    });
    let n = BELL.with(|b| b.borrow().len());
    crate::app::dom::swap_outer(
        "notif-bell-badge",
        &format!("<span id=\"notif-bell-badge\" class=\"notif-badge\">{n}</span>"),
    );
    crate::app::dom::swap_outer(
        "notif-bell-panel",
        &crate::app::templates::notif_list_panel(&bell_items(), None, true).into_string(),
    );
}

/// Snapshot of the in-app bell log (newest first).
pub(crate) fn bell_items() -> Vec<(String, String)> {
    BELL.with(|b| b.borrow().clone())
}

/// Hide the unread badge (called when the bell panel is opened / read).
pub(crate) fn clear_bell_badge() {
    crate::app::dom::swap_outer(
        "notif-bell-badge",
        "<span id=\"notif-bell-badge\" class=\"notif-badge\" hidden></span>",
    );
}

/// VAPID application-server PUBLIC key (base64url, uncompressed P-256 point)
/// — the `applicationServerKey` for `PushManager.subscribe`, pair of the
/// proxy's `VAPID_PRIVATE_KEY`.
///
/// MAINTAINER: this key was generated for this feature branch; the matching
/// private key must be set on the PROXY Vercel project as
/// `VAPID_PRIVATE_KEY` (plus `VAPID_PUBLIC_KEY` = this value and
/// `VAPID_SUBJECT`, e.g. `mailto:compusophy@gmail.com`). Replace BOTH halves
/// together if you rotate — existing on-chain subscriptions die with the key.
pub(crate) const VAPID_PUBLIC_KEY: &str =
    "BHtamLu5RHqMWbV3JyyEmQKL-lweTVq3ePiFOHGu_EBzvrz4w0SzpWpBTI02UgWOkFR9sbAqPrvj8LOtF5R5jow";

fn js_err(context: &str, e: JsValue) -> String {
    format!("{context}: {}", e.as_string().unwrap_or_else(|| format!("{e:?}")))
}

fn window() -> Result<web_sys::Window, String> {
    web_sys::window().ok_or_else(|| "no window".to_string())
}

/// Current Notification permission, requesting it if undecided. `Ok(true)` =
/// granted. Browsers may auto-deny a request outside a user gesture — the
/// admin [enable notifications] button is the gesture path; the agent tool
/// degrades to a permission report.
pub(crate) async fn ensure_permission() -> Result<bool, String> {
    use web_sys::{Notification, NotificationPermission};
    match Notification::permission() {
        NotificationPermission::Granted => return Ok(true),
        NotificationPermission::Denied => return Ok(false),
        _ => {}
    }
    let promise = Notification::request_permission().map_err(|e| js_err("requestPermission", e))?;
    let result = JsFuture::from(promise)
        .await
        .map_err(|e| js_err("requestPermission", e))?;
    Ok(result.as_string().as_deref() == Some("granted"))
}

/// The current service-worker registration, if any (resolves `undefined`
/// when boot.js hasn't registered / SW unsupported).
async fn sw_registration() -> Option<web_sys::ServiceWorkerRegistration> {
    let sw = web_sys::window()?.navigator().service_worker();
    let v = JsFuture::from(sw.get_registration()).await.ok()?;
    v.dyn_into::<web_sys::ServiceWorkerRegistration>().ok()
}

/// Show a notification (caller has already ensured permission). Prefers the
/// service-worker registration; falls back to the page constructor where no
/// SW exists (desktop without sw.js — e.g. localhost dev).
pub(crate) async fn show(title: &str, body: &str) -> Result<(), String> {
    let opts = web_sys::NotificationOptions::new();
    opts.set_body(body);
    opts.set_icon("/icons/icon-192.png");
    // Same-content notifications COLLAPSE instead of stacking (Android
    // shows untagged notifications separately — feedback #55 reported
    // doubles): tag = the content itself, so an accidental second render
    // replaces the first instead of buzzing twice.
    opts.set_tag(&format!("lh-{title}-{body}"));
    if let Some(reg) = sw_registration().await {
        let promise = reg
            .show_notification_with_options(title, &opts)
            .map_err(|e| js_err("showNotification", e))?;
        JsFuture::from(promise)
            .await
            .map_err(|e| js_err("showNotification", e))?;
        return Ok(());
    }
    web_sys::Notification::new_with_options(title, &opts)
        .map(|_| ())
        .map_err(|e| js_err("Notification", e))
}

/// Vibrate the device for `ms` milliseconds. Best-effort: returns false where
/// unsupported (desktop) or blocked (no user activation) — never errors.
pub(crate) fn vibrate(ms: u32) -> bool {
    match web_sys::window() {
        Some(win) => win.navigator().vibrate_with_duration(ms),
        None => false,
    }
}

/// Subscribe this browser to Web Push (registering `sw.js` if needed) and
/// return the subscription JSON (`{endpoint, keys: {p256dh, auth}}`) — the
/// exact shape the proxy's push sender consumes.
pub(crate) async fn subscribe_push() -> Result<String, String> {
    let container = window()?.navigator().service_worker();
    // Idempotent re-register: boot.js already did this on a normal boot, but
    // an old tab from before the SW shipped (or a failed first attempt)
    // would otherwise hang on `ready` forever.
    let _ = JsFuture::from(container.register("/sw.js")).await;
    let ready = container.ready().map_err(|e| js_err("serviceWorker.ready", e))?;
    let reg: web_sys::ServiceWorkerRegistration = JsFuture::from(ready)
        .await
        .map_err(|e| js_err("serviceWorker.ready", e))?
        .dyn_into()
        .map_err(|_| "serviceWorker.ready: not a registration".to_string())?;
    let manager = reg.push_manager().map_err(|e| js_err("pushManager", e))?;

    let opts = web_sys::PushSubscriptionOptionsInit::new();
    opts.set_user_visible_only(true);
    let key_bytes = {
        use base64::Engine as _;
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(VAPID_PUBLIC_KEY)
            .map_err(|e| format!("bad VAPID_PUBLIC_KEY constant: {e}"))?
    };
    let key_js: JsValue = js_sys::Uint8Array::from(key_bytes.as_slice()).into();
    opts.set_application_server_key(&key_js);

    let sub: web_sys::PushSubscription = JsFuture::from(
        manager
            .subscribe_with_options(&opts)
            .map_err(|e| js_err("push subscribe", e))?,
    )
    .await
    .map_err(|e| js_err("push subscribe", e))?
    .dyn_into()
    .map_err(|_| "push subscribe: not a PushSubscription".to_string())?;

    // PushSubscription has a toJSON, so JSON.stringify yields the canonical
    // {endpoint, expirationTime, keys} shape.
    js_sys::JSON::stringify(&sub)
        .map_err(|e| js_err("subscription stringify", e))?
        .as_string()
        .ok_or_else(|| "subscription stringify: empty".to_string())
}

/// The admin [enable notifications] flow: permission → push subscription →
/// publish the subscription JSON on-chain under
/// `keccak256("localharness.push_sub")` for the owner's MAIN tokenId
/// (fallback: this name's own id — mirrors the Gemini-key-sync slot rule, so
/// ONE subscription serves every subdomain of the identity). Sponsored write,
/// zero-click. Returns the tx hash.
///
/// KNOWN TRADEOFF (v1): the subscription is stored PLAINTEXT on-chain. The
/// endpoint is a bearer capability URL — anyone reading chain state can send
/// this device (unauthenticated-origin) pushes until the user unsubscribes.
/// Payloads are still E2E-encrypted to this browser (p256dh/auth), so no
/// third party can read OUR pushes; the exposure is spam/identification.
/// Follow-up: ECIES-seal the JSON to a proxy-held key so only the scheduler
/// can read it.
/// Enable Web Push for THIS DEVICE keyed by its OWN ADDRESS (PushFacet), not a
/// MAIN tokenId — so ANY visitor (a bare device key, `mainOf == 0`) can receive
/// cross-device pushes. MUST be called from a DIRECT user gesture (the header
/// notification bell): the cartridge subscribe tap runs through a worker
/// postMessage that loses user activation, so its `requestPermission` never
/// prompts on mobile and the device silently never registers — THE
/// cross-device-push bug. This path prompts, subscribes, and publishes the
/// address-keyed subscription (signed by the device's credit key, sponsored).
/// Returns the tx hash. Idempotent — safe to tap again to refresh a stale sub.
pub(crate) async fn enable_device_push() -> Result<String, String> {
    if !ensure_permission().await? {
        return Err("notification permission is blocked — allow notifications for this site in your browser settings, then tap again".to_string());
    }
    let sub_json = subscribe_push().await?;
    let (signer, _) = crate::app::chat::credit_signer()
        .await
        .ok_or_else(|| "no identity on this device yet".to_string())?;
    let sponsor = crate::app::sponsor::signer().map_err(|e| format!("sponsor: {e}"))?;
    let token = crate::registry::ALPHA_USD_ADDRESS;
    crate::registry::set_push_sub_sponsored(&signer, &sponsor, sub_json.as_bytes(), token).await
}

pub(crate) async fn enable_and_publish() -> Result<String, String> {
    if !ensure_permission().await? {
        return Err("notification permission denied — allow notifications for this site in the browser settings".to_string());
    }
    let sub_json = subscribe_push().await?;

    let (name, owner) = crate::app::tenant::current_tenant_owner().await?;
    let token_id = match crate::registry::main_of(&owner).await {
        Ok(id) if id != 0 => id,
        _ => match crate::registry::id_of_name(&name).await {
            Ok(id) if id != 0 => id,
            Ok(_) => return Err("this subdomain isn't registered on-chain yet".to_string()),
            Err(e) => return Err(format!("id_of_name: {e}")),
        },
    };

    let registry_addr = crate::encoding::parse_address(crate::registry::REGISTRY_ADDRESS)?;
    let call = crate::tempo_tx::TempoCall {
        to: registry_addr,
        value_wei: 0,
        input: crate::registry::encode_set_push_sub(token_id, sub_json.as_bytes()),
    };
    let gas = crate::app::gas::set_metadata_gas(sub_json.len());
    crate::app::events::run_sponsored_tempo_call(&owner, vec![call], gas, "publish push subscription")
        .await
}

/// Silently refresh a STALE push subscription on app open. Reinstalling the
/// PWA / clearing site data invalidates the browser's old push endpoint, but
/// the on-chain slot keeps serving it — every worker push then dies with an
/// FCM 410 and the owner silently stops getting buzzed (seen live 2026-06-12).
/// When permission is already granted, re-subscribe (idempotent) and, if the
/// endpoint differs from the published one, re-publish. Best-effort: any
/// failure leaves the existing state untouched; never prompts.
pub(crate) async fn refresh_subscription_if_stale() {
    if !matches!(
        web_sys::Notification::permission(),
        web_sys::NotificationPermission::Granted
    ) {
        return;
    }
    let Ok(current) = subscribe_push().await else {
        return;
    };
    let Ok((name, owner)) = crate::app::tenant::current_tenant_owner().await else {
        return;
    };
    let token_id = match crate::registry::main_of(&owner).await {
        Ok(id) if id != 0 => id,
        _ => match crate::registry::id_of_name(&name).await {
            Ok(id) if id != 0 => id,
            _ => return,
        },
    };
    let published = crate::registry::push_sub_of(token_id)
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    if published == current {
        return;
    }
    let publish = async {
        let registry_addr = crate::encoding::parse_address(crate::registry::REGISTRY_ADDRESS)?;
        let call = crate::tempo_tx::TempoCall {
            to: registry_addr,
            value_wei: 0,
            input: crate::registry::encode_set_push_sub(token_id, current.as_bytes()),
        };
        let gas = crate::app::gas::set_metadata_gas(current.len());
        crate::app::events::run_sponsored_tempo_call(
            &owner,
            vec![call],
            gas,
            "refresh push subscription",
        )
        .await
    };
    match publish.await {
        Err(e) => web_sys::console::warn_1(&wasm_bindgen::JsValue::from_str(&format!(
            "push subscription refresh failed: {e}"
        ))),
        Ok(_) => web_sys::console::log_1(&wasm_bindgen::JsValue::from_str(
            "stale push subscription refreshed on-chain",
        )),
    }
}
