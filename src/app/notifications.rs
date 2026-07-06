//! Notifications + Web Push (browser-app).
//!
//! Two surfaces share this module:
//!   * the agent's `notify(title, body?, vibrate?)` closure tool
//!     (`chat::tools::misc::notify_tool`) — in-tab notifications/vibration
//!     for alarms, message-arrived, job-done;
//!   * the header notification bell (+ the headless per-load refresh) —
//!     subscribes Web Push and POSTs the subscription JSON to the proxy's
//!     OFF-CHAIN push store (`POST /api/push-sub` → GitHub store, keyed by
//!     this device's address) so the proxy's notify/broadcast/scheduler
//!     workers can buzz the owner with the tab CLOSED. Enrollment used to be
//!     a sponsored ON-CHAIN write (`setPushSub`) — on mainnet it bypassed the
//!     relay and failed with "insufficient funds" for unfunded users; never
//!     reintroduce an on-chain publish here.
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
    /// unread count. Fed by READY-UP broadcasts (the presser's own ding) and
    /// service-worker pushes relayed to the open page (boot.js `lh-push`).
    /// Persisted to OPFS so the inbox survives reloads.
    static BELL: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
}

/// The persisted inbox (written by [`push_to_bell`], read at mount).
const INBOX_FILE: &str = ".lh_notif_inbox.json";
/// Pushes that arrived with NO page open, stashed by `web/sw.js` directly in
/// OPFS; merged + deleted by [`load_inbox`] at the next boot.
//
// NOTE: both notif files are in `filesystem::EXEMPT_FILES`, so `shared_opfs()`
// writes them PLAINTEXT. `web/sw.js` is a seedless service worker that reads
// `PENDING_FILE` with `JSON.parse` — if the Rust side sealed it (`LHE1…`),
// sw.js's parse would throw and clobber closed-tab pushes (the #35 inbox bug).
const PENDING_FILE: &str = ".lh_notif_pending.json";
/// High-water mark (a decimal count) for [`import_onchain_messages`] — the
/// number of on-chain inbox messages already folded into the bell, so a reload
/// only surfaces NEW ones.
const MSG_CURSOR_FILE: &str = ".lh_msg_cursor";
/// Stable per-device id, persisted in OPFS, stamped into every push
/// subscription as the `dev` field so the proxy can collapse the SAME physical
/// device's multiple push endpoints to ONE delivery (R5: a phone registered
/// under two subdomain origins held two different endpoints in the same
/// address-keyed slot → double-buzz). Survives reloads; one per origin.
const DEV_ID_FILE: &str = ".lh_dev_id";
/// High-water mark for [`notify_received_lh`] — the $LH balance (decimal wei)
/// this device last surfaced a "received" note for, so only an INCREASE since
/// the last open fires a fresh note (mirrors the [`MSG_CURSOR_FILE`] pattern).
const LH_BALANCE_MARK_FILE: &str = ".lh_balance_mark";

/// Load (or generate + persist) this device's stable id. Best-effort: a fresh
/// random uuid on first use; persisted plaintext in OPFS (it identifies a device
/// for dedup, not a secret). Falls back to a fresh ephemeral id if OPFS is
/// unavailable — worst case dedup degrades to endpoint-keyed for this load.
pub(crate) async fn device_id() -> String {
    let fs = crate::app::shared_opfs();
    if let Ok(b) = fs.read(DEV_ID_FILE).await {
        if let Ok(s) = String::from_utf8(b) {
            let t = s.trim();
            if !t.is_empty() {
                return t.to_string();
            }
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    let _ = fs.write_atomic(DEV_ID_FILE, id.as_bytes()).await;
    id
}

/// Stamp this device's stable `dev` id into a push-subscription JSON object so
/// the proxy can dedupe by device (see [`device_id`]). Returns the input
/// unchanged if it isn't a JSON object (defensive — never lose a subscription).
async fn tag_sub_with_dev(sub_json: &str) -> String {
    let dev = device_id().await;
    match serde_json::from_str::<serde_json::Value>(sub_json) {
        Ok(serde_json::Value::Object(mut map)) => {
            map.insert("dev".to_string(), serde_json::Value::String(dev));
            serde_json::Value::Object(map).to_string()
        }
        _ => sub_json.to_string(),
    }
}

/// Append a notification to the in-app header bell (newest first, cap 30),
/// flag the bell unread (a gentle pulse — no count), and persist the inbox.
/// The panel re-render keeps
/// its CURRENT visibility — a push landing mid-read must not yank the
/// dropdown shut (or open it uninvited).
pub(crate) fn push_to_bell(title: &str, body: &str) {
    BELL.with(|b| {
        let mut v = b.borrow_mut();
        v.insert(0, (title.to_string(), body.to_string()));
        v.truncate(30);
    });
    // Present & not [hidden] ⇒ unread; CSS turns that flag into the bell PULSE
    // (no number — a count widened the square icon button).
    crate::app::dom::swap_outer(
        "notif-bell-badge",
        "<span id=\"notif-bell-badge\" class=\"notif-badge\"></span>",
    );
    let hidden = crate::app::dom::by_id("notif-bell-panel")
        .map(|e| e.has_attribute("hidden"))
        .unwrap_or(true);
    crate::app::dom::swap_outer(
        "notif-bell-panel",
        &crate::app::templates::notif_list_panel(&bell_items(), None, hidden, false).into_string(),
    );
    wasm_bindgen_futures::spawn_local(persist_inbox());
}

/// Service-worker push relay (boot.js → the `push_arrived` wasm export):
/// a Web Push that arrived while this page is open lands in the bell inbox.
pub(crate) fn push_arrived(title: &str, body: &str) {
    let title = if title.is_empty() { "localharness" } else { title };
    push_to_bell(title, body);
}

/// MERGE-SAFE inbox delivery for contexts where the in-memory [`BELL`] is NOT
/// the live header bell — e.g. the headless `?rpc=1` endpoint, which never runs
/// [`load_inbox`], so `BELL` is empty and a [`push_to_bell`] would CLOBBER the
/// persisted inbox down to this one entry. Instead head-insert into the SAME
/// `PENDING_FILE` the service worker stashes closed-tab pushes into; the next
/// full-app mount's [`load_inbox`] folds it into the bell (newest, flagged
/// unread) WITHOUT losing prior entries. Best-effort; logs, never surfaces.
///
/// This is the push-INDEPENDENT delivery path (#35): a notification lands in the
/// recipient's in-app inbox even with Web Push disabled.
pub(crate) async fn stash_to_inbox(title: &str, body: &str) {
    let fs = crate::app::shared_opfs();
    let mut items: Vec<(String, String)> = match fs.read(PENDING_FILE).await {
        Ok(b) => serde_json::from_slice(&b).unwrap_or_else(|e| {
            // Surface corruption instead of silently overwriting the prior stashed
            // notifications with just this one. They're unrecoverable from bad JSON,
            // but a silent swallow hid a real data-loss — at least make it visible.
            web_sys::console::warn_1(&JsValue::from_str(&format!(
                "notif stash: pending file corrupted, prior entries lost: {e}"
            )));
            Vec::new()
        }),
        Err(_) => Vec::new(),
    };
    items.insert(0, (title.to_string(), body.to_string()));
    items.truncate(30);
    let Ok(bytes) = serde_json::to_vec(&items) else { return };
    if let Err(e) = fs.write_atomic(PENDING_FILE, &bytes).await {
        web_sys::console::warn_1(&JsValue::from_str(&format!("notif stash: {e}")));
    }
}

/// GitHub releases — every release's notes (the public changelog) land here.
const RELEASES_URL: &str = "https://github.com/compusophy/localharness/releases";

fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
}

/// On mount: if the running bundle's CRATE version (`APP_VERSION` — bumped ONLY on
/// a real release, never a plain web redeploy) differs from the one this device
/// last recorded, self-notify the update. Decentralized — no server push, no user
/// roster: every client self-notifies on the version it loaded (a fully-CLOSED app
/// can't detect its own version change, so there is nothing a deploy-time server
/// broadcast could reach that this on-load path doesn't). A first-ever visit just
/// records the version (a brand-new device gets no spurious "updated" note).
///
/// Delivery is TWO surfaces, mirroring the READY-UP broadcast
/// (`display::do_feed_broadcast`): (1) the in-app header bell (always — no
/// permission needed), and (2) a REAL OS system notification via the service
/// worker's `registration.showNotification` — the SAME delivery surface a Web Push
/// renders through (`web/sw.js` `push` → `showNotification`), so the banner reaches
/// the user even when this tab is backgrounded / not focused, not just as a silent
/// inbox entry they must open the bell to find. The OS banner is permission-gated
/// and NEVER prompts (mount is not a user gesture — a `requestPermission` here
/// would auto-deny on mobile): it fires only when notifications are ALREADY granted
/// (the one-time bell tap), otherwise the bell entry stands alone.
pub(crate) fn notify_version_change() {
    let Some(storage) = local_storage() else { return };
    let current = crate::app::templates::APP_VERSION;
    let seen = storage.get_item("lh_seen_version").ok().flatten();
    if seen.as_deref() == Some(current) {
        return;
    }
    let _ = storage.set_item("lh_seen_version", current);
    if seen.is_some() {
        let title = format!("localharness v{current} is live");
        let body = format!("A new version shipped — see what changed: {RELEASES_URL}/tag/v{current}");
        // In-app bell (always).
        push_to_bell(&title, &body);
        // Escalate to an OS banner via the SW when already permitted — never prompt.
        wasm_bindgen_futures::spawn_local(async move {
            if matches!(
                web_sys::Notification::permission(),
                web_sys::NotificationPermission::Granted
            ) {
                let _ = show(&title, &body).await;
            }
        });
    }
}

/// Persist the bell log to OPFS (best-effort; logs, never surfaces).
async fn persist_inbox() {
    let items = bell_items();
    let Ok(bytes) = serde_json::to_vec(&items) else { return };
    let fs = crate::app::shared_opfs();
    if let Err(e) = fs.write_atomic(INBOX_FILE, &bytes).await {
        web_sys::console::warn_1(&JsValue::from_str(&format!("notif inbox save: {e}")));
    }
}

/// Restore the bell inbox at mount: closed-tab pushes stashed by the service
/// worker (newest, still unread) first, then the persisted log from the last
/// session. ONLY the stashed arrivals flag the bell unread — a reload must
/// not re-flag entries the user already saw.
pub(crate) async fn load_inbox() {
    let fs = crate::app::shared_opfs();
    let mut items: Vec<(String, String)> = Vec::new();
    if let Ok(b) = fs.read(PENDING_FILE).await {
        if let Ok(mut v) = serde_json::from_slice::<Vec<(String, String)>>(&b) {
            items.append(&mut v);
            // Delete ONLY after a successful parse. Deleting on a parse failure
            // (corrupted/partial JSON from a crash mid-write) would permanently lose
            // the pushed notifications the file holds — keep it for a retry next boot.
            let _ = fs.delete(PENDING_FILE).await;
        }
    }
    let fresh = items.len();
    if let Ok(b) = fs.read(INBOX_FILE).await {
        if let Ok(mut v) = serde_json::from_slice::<Vec<(String, String)>>(&b) {
            items.append(&mut v);
        }
    }
    if items.is_empty() {
        return;
    }
    // De-dup identical (title, body), keeping the FIRST (newest — pending is
    // prepended): T5 now ALWAYS stashes a push even when a live page also got
    // it via postMessage (relay → push_to_bell → persist_inbox writes it to
    // INBOX_FILE), so the same note can sit in both PENDING and INBOX. Collapse
    // it here so the always-stash hardening can't double-count.
    {
        let mut seen = std::collections::HashSet::new();
        items.retain(|e| seen.insert(e.clone()));
    }
    items.truncate(30);
    BELL.with(|b| *b.borrow_mut() = items);
    if fresh > 0 {
        // Flag the bell unread (CSS pulse); the count itself isn't shown.
        crate::app::dom::swap_outer(
            "notif-bell-badge",
            "<span id=\"notif-bell-badge\" class=\"notif-badge\"></span>",
        );
        persist_inbox().await; // fold the drained pending file into the log
    }
    crate::app::dom::swap_outer(
        "notif-bell-panel",
        &crate::app::templates::notif_list_panel(&bell_items(), None, true, false).into_string(),
    );
}

/// Poll THIS identity's on-chain MessageFacet inbox and fold any NOT-yet-seen
/// messages into the bell (#35). This is the DURABLE channel for cross-agent
/// `notify`: the proxy records every cross-agent note here (in addition to any
/// Web Push), so it surfaces in-app at next open even when a push to a
/// closed/backgrounded PWA tab never reached the live bell. Run AFTER
/// [`load_inbox`] so it appends to (and dedups against) the loaded log rather
/// than clobbering it. Cursor (`.lh_msg_cursor`) lives in OPFS so each load only
/// processes new indices. Best-effort: any RPC/decode error (or no tenant
/// identity) is swallowed — the inbox is a nicety, never a blocker.
pub(crate) async fn import_onchain_messages() {
    let Some(name) = crate::app::tenant::current_name() else {
        return;
    };
    let token_id = match crate::registry::id_of_name(&name).await {
        Ok(id) if id != 0 => id,
        _ => return,
    };
    let count = match crate::registry::inbox_count(token_id).await {
        Ok(c) => c,
        Err(_) => return,
    };
    let fs = crate::app::shared_opfs();
    let cursor: u64 = fs
        .read(MSG_CURSOR_FILE)
        .await
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    if count <= cursor {
        return;
    }
    // Snapshot the bell BEFORE folding so a recorded note can be deduped against
    // an already-shown live/stashed push of the SAME (title, body): the proxy
    // now records cross-agent notes on-chain AND pushes them with the identical
    // {title,body} payload, so without this the note would appear twice.
    let existing = bell_items();
    // Advance the cursor only up to the highest index we actually FETCHED. A note
    // that was fetched-but-empty/garbage is safe to skip past, but a transient RPC
    // fetch error (`message_at` Err) must NOT advance the cursor — break there and
    // retry that index on the next load, or the message is lost forever (L33).
    let mut next_cursor = cursor;
    for i in cursor..count {
        let Ok((from, _ts, raw)) = crate::registry::message_at(token_id, i).await else {
            break; // transient fetch error — stop; resume from `i` next load
        };
        next_cursor = i + 1;
        let (title, body) = parse_note(raw.trim(), &from);
        if title.is_empty() && body.is_empty() {
            continue;
        }
        if existing.iter().any(|(t, b)| *t == title && *b == body) {
            continue; // already shown via the live/stashed push of this note
        }
        push_to_bell(&title, &body);
    }
    if next_cursor > cursor {
        let _ = fs.write_atomic(MSG_CURSOR_FILE, next_cursor.to_string().as_bytes()).await;
    }
}

/// T13: auto-notify the bell when this identity RECEIVES $LH. Today only the
/// SENDER side piggybacks a notify (platform.rs::notify_recipient_of_incoming_lh);
/// transfers from the CLI / an external wallet / x402 / bounty payouts never
/// reach the receiver. This is a recipient-side, BALANCE-DELTA watcher (NOT
/// event-log scraping — Tempo caps the block range): read the identity's total
/// $LH (owner wallet + this name's TBA, where earnings land), compare against a
/// persisted high-water mark in OPFS (mirrors the MSG_CURSOR pattern), and on an
/// INCREASE push a bundled "received N $LH" note. Best-effort: any RPC error or
/// missing identity is swallowed. Call at a mount/poll point AFTER
/// [`import_onchain_messages`] so it dedups against an already-shown sender-side
/// note via the existing title/body check. wasm32-clean. Wired at mount in
/// `src/app/mod.rs`, chained after [`import_onchain_messages`].
pub(crate) async fn notify_received_lh() {
    let Some(name) = crate::app::tenant::current_name() else {
        return;
    };
    let Ok((_, owner)) = crate::app::tenant::current_tenant_owner().await else {
        return;
    };
    // Total spendable-in $LH: the owner WALLET (send_lh / direct transfers land
    // here) + this name's TBA (x402 earnings + bounty payouts). Either pot rising
    // means funds arrived. Read-only.
    let wallet = crate::registry::token_balance_of(&owner).await.unwrap_or(0);
    let tba = match crate::registry::tba_of_name(&name).await.ok().flatten() {
        Some(addr) => crate::registry::token_balance_of(&addr).await.unwrap_or(0),
        None => 0,
    };
    let total = wallet.saturating_add(tba);

    let fs = crate::app::shared_opfs();
    // Read the mark ONCE: a missing OR present-but-unparseable file both count as
    // a first run (re-seed the baseline) rather than mark=0, which would announce
    // a non-zero balance as fully "received" (L36).
    let parsed: Option<u128> = fs
        .read(LH_BALANCE_MARK_FILE)
        .await
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|s| s.trim().parse().ok());

    // FIRST run (no parseable mark yet): seed the baseline SILENTLY so we don't
    // announce a pre-existing balance as "just received" — mirrors the version/
    // resolution baselines. Only an increase AFTER this point fires a note.
    let first_run = parsed.is_none();
    let mark: u128 = parsed.unwrap_or(0);
    if total != mark || first_run {
        let _ = fs
            .write_atomic(LH_BALANCE_MARK_FILE, total.to_string().as_bytes())
            .await;
    }
    if first_run || total <= mark {
        return;
    }
    let delta = total - mark;
    let amount = crate::app::format_wei_as_test_eth(delta);
    let title = format!("+{amount} $LH received");
    let body = "incoming $LH transfer — check your wallet".to_string();
    // Dedup against a sender-side note already folded into the bell this load
    // (it may carry an `@<from>:` prefix, so match on the body OR an exact title).
    let already = bell_items()
        .iter()
        .any(|(t, b)| (*t == title || t.ends_with(&title)) && *b == body);
    if !already {
        push_to_bell(&title, &body);
    }
}

/// Decode an on-chain inbox message into a `(title, body)` bell entry. Notes the
/// proxy records carry the SAME `{title,body}` JSON a Web Push does — so a
/// folded entry is byte-identical to a live/stashed push and dedups against it.
/// Any non-JSON / legacy string is shown verbatim as the body under a generic
/// `message · <from>` title. Returns `("", "")` only when there is nothing to
/// show (caller skips).
fn parse_note(raw: &str, from: &str) -> (String, String) {
    if raw.is_empty() {
        return (String::new(), String::new());
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) {
        let t = v.get("title").and_then(|x| x.as_str()).unwrap_or("");
        let b = v.get("body").and_then(|x| x.as_str()).unwrap_or("");
        if !t.is_empty() || !b.is_empty() {
            let short = from.get(..8).unwrap_or(from);
            let title = if t.is_empty() {
                format!("message · {short}…")
            } else {
                t.to_string()
            };
            return (title, b.to_string());
        }
    }
    let short = from.get(..8).unwrap_or(from);
    (format!("message · {short}…"), raw.to_string())
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

/// Empty the in-app bell inbox (the "clear all" control, after its inline
/// yes-confirm): wipe the log, hide the badge, re-render
/// the (now empty) panel OPEN, and persist the cleared inbox to OPFS so it
/// stays empty across reloads.
pub(crate) fn clear_all() {
    BELL.with(|b| b.borrow_mut().clear());
    clear_bell_badge();
    crate::app::dom::swap_outer(
        "notif-bell-panel",
        &crate::app::templates::notif_list_panel(&[], None, false, false).into_string(),
    );
    wasm_bindgen_futures::spawn_local(persist_inbox());
}

/// VAPID application-server PUBLIC key (base64url, uncompressed P-256 point)
/// — the `applicationServerKey` for `PushManager.subscribe`, pair of the
/// proxy's `VAPID_PRIVATE_KEY`.
///
/// MAINTAINER: this key was generated for this feature branch; the matching
/// private key must be set on the PROXY Vercel project as
/// `VAPID_PRIVATE_KEY` (plus `VAPID_PUBLIC_KEY` = this value and
/// `VAPID_SUBJECT`, e.g. `mailto:compusophy@gmail.com`). Replace BOTH halves
/// together if you rotate — existing enrolled subscriptions die with the key.
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

/// POST this device's push-subscription JSON to the proxy's OFF-CHAIN store
/// (`/api/push-sub`, personal-sign authed, keyed server-side by the signer's
/// address). The server upserts by the stable `dev` id (else endpoint) into the
/// address's device array — MULTI-DEVICE safe (a phone and a desktop on the
/// same seed each keep an entry) and idempotent (an unchanged sub is a no-op).
/// This REPLACED the sponsored on-chain `setPushSub` publish, which bypassed
/// the mainnet relay and failed with "insufficient funds" for unfunded users.
async fn post_push_sub(
    signer: &k256::ecdsa::SigningKey,
    sub_json: &str,
) -> Result<String, String> {
    let sub: serde_json::Value =
        serde_json::from_str(sub_json).map_err(|e| format!("subscription JSON: {e}"))?;
    let now = (js_sys::Date::now() / 1000.0) as u64;
    let token = crate::registry::proxy_auth_token(signer, now, "push-sub");
    let url = format!("{}api/push-sub", crate::registry::CREDIT_PROXY_URL);
    let send = async {
        reqwest::Client::new()
            .post(&url)
            .header("content-type", "application/json")
            .header("x-goog-api-key", token)
            .json(&serde_json::json!({ "sub": sub }))
            .send()
            .await
            .map_err(|e| format!("push-sub request: {e}"))
    };
    let resp = crate::app::net::with_timeout(20_000, send).await??;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        let detail = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
            .unwrap_or(body);
        return Err(format!("push-sub {status}: {detail}"));
    }
    Ok("registered".to_string())
}

/// Cached "this device verified an enrollment" flag (localStorage) — the
/// INSTANT bell-panel status line reads it; only [`register_device_push`]'s
/// verified outcome writes it. A hint, never a source of truth.
fn set_enrolled_hint(on: bool) {
    if let Some(s) = local_storage() {
        if on {
            let _ = s.set_item("lh_push_enrolled", "1");
        } else {
            let _ = s.remove_item("lh_push_enrolled");
        }
    }
}

fn enrolled_hint() -> bool {
    local_storage()
        .and_then(|s| s.get_item("lh_push_enrolled").ok().flatten())
        .as_deref()
        == Some("1")
}

/// The bell panel's push-state line (permission + cached enrolled hint) for
/// the instant paint on open; the async enroll result then overwrites it.
pub(crate) fn bell_status_line() -> &'static str {
    let perm = match web_sys::Notification::permission() {
        web_sys::NotificationPermission::Granted => "granted",
        web_sys::NotificationPermission::Denied => "denied",
        _ => "default",
    };
    crate::push_enroll::bell_status(perm, enrolled_hint())
}

/// Subscribe this browser to Web Push, stamp the stable `dev` id, enroll the
/// subscription in the proxy's off-chain store, and VERIFY it actually landed
/// (telemetry #40: enrollment was fire-and-forget — a sub that silently never
/// landed meant every closed-tab push died while the user believed they were
/// enrolled). Caller has already handled notification permission; identity
/// comes from `credit_signer` (the personal-sign token needs a key).
pub(crate) async fn register_device_push() -> Result<String, String> {
    let sub_json = tag_sub_with_dev(&subscribe_push().await?).await;
    let (signer, addr) = crate::app::chat::credit_signer()
        .await
        .ok_or_else(|| "no identity on this device yet".to_string())?;
    if let Err(e) = post_push_sub(&signer, &sub_json).await {
        set_enrolled_hint(false);
        return Err(e);
    }
    // Read the store back and require THIS device's entry (endpoint or `dev`).
    let address = crate::encoding::bytes_to_hex_str(&addr);
    let endpoint = serde_json::from_str::<serde_json::Value>(&sub_json)
        .ok()
        .and_then(|v| v.get("endpoint").and_then(|e| e.as_str()).map(str::to_string))
        .unwrap_or_default();
    let dev = device_id().await;
    let url = format!("{}api/push-sub?address={address}", crate::registry::CREDIT_PROXY_URL);
    let fetch = async {
        let resp = reqwest::Client::new()
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("push-sub verify: {e}"))?;
        resp.text().await.map_err(|e| format!("push-sub verify: {e}"))
    };
    let body = crate::app::net::with_timeout(20_000, fetch).await??;
    match crate::push_enroll::verify_enrolled(&body, &endpoint, &dev) {
        Some(n) => {
            set_enrolled_hint(true);
            Ok(format!(
                "push: enrolled — {n} device{} will get alerts with the tab closed",
                if n == 1 { "" } else { "s" }
            ))
        }
        None => {
            set_enrolled_hint(false);
            Err("enrollment did not land in the push store — closed-tab pushes will NOT reach this device; tap the bell to retry".to_string())
        }
    }
}

/// Enable Web Push for THIS DEVICE, keyed by its OWN ADDRESS in the proxy's
/// off-chain store — so ANY visitor (a bare device key, no MAIN) can receive
/// cross-device pushes. MUST be called from a DIRECT user gesture (the header
/// notification bell): the cartridge subscribe tap runs through a worker
/// postMessage that loses user activation, so its `requestPermission` never
/// prompts on mobile and the device silently never registers — THE
/// cross-device-push bug. Idempotent — safe to tap again to refresh a stale sub.
pub(crate) async fn enable_device_push() -> Result<String, String> {
    if !ensure_permission().await? {
        set_enrolled_hint(false);
        return Err("notification permission is blocked — allow notifications for this site in your browser settings, then tap again".to_string());
    }
    register_device_push().await
}

/// HEADLESS auto-registration: on every app load, if notification permission is
/// ALREADY granted AND this device already has an identity, (re)enroll its
/// current subscription in the off-chain store — no gesture, no prompt. This
/// both keeps the device registered after the ONE-TIME bell-tap grant AND
/// self-heals a STALE endpoint (PWA reinstall / cleared site data invalidates
/// the old one; the store kept serving it → every push died with an FCM 410,
/// seen live 2026-06-12): re-subscribing yields the fresh endpoint and the
/// server upserts it over the stale entry by `dev` id. Best-effort; idempotent
/// (the server skips the write when the stored sub already matches).
pub(crate) async fn auto_register_device_push() {
    if !matches!(
        web_sys::Notification::permission(),
        web_sys::NotificationPermission::Granted
    ) {
        return; // no permission yet — the one-time bell tap must grant it first
    }
    // Only if an identity ALREADY exists — never silently mint a wallet on load.
    if crate::app::chat::credit_address_existing().await.is_none() {
        return;
    }
    match register_device_push().await {
        Ok(_) => web_sys::console::log_1(&wasm_bindgen::JsValue::from_str(
            "[push] device registered (off-chain, address-keyed)",
        )),
        Err(e) => web_sys::console::warn_1(&wasm_bindgen::JsValue::from_str(&format!(
            "[push] auto-register failed: {e}"
        ))),
    }
}
