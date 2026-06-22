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
        Ok(b) => serde_json::from_slice(&b).unwrap_or_default(),
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
/// Canonical resolutions feed, apex-hosted like `global-lessons.txt`: a JSON list
/// of resolved on-chain feedback `[{index, sender, version, preview}]`, refreshed
/// at each deploy by `scripts/gen-feedback-resolutions.mjs`.
const FEEDBACK_RESOLUTIONS_URL: &str = "https://localharness.xyz/feedback-resolutions.json";

fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
}

/// On mount: if the running bundle's CRATE version (`APP_VERSION` — bumped ONLY on
/// a real release, never a plain web redeploy) differs from the one this device
/// last recorded, drop a one-time bell note linking the changelog. Decentralized —
/// no server push, no user roster: every client self-notifies on the version it
/// loaded. A first-ever visit just records the version (a brand-new device gets no
/// spurious "updated" note).
pub(crate) fn notify_version_change() {
    let Some(storage) = local_storage() else { return };
    let current = crate::app::templates::APP_VERSION;
    let seen = storage.get_item("lh_seen_version").ok().flatten();
    if seen.as_deref() == Some(current) {
        return;
    }
    let _ = storage.set_item("lh_seen_version", current);
    if seen.is_some() {
        push_to_bell(
            &format!("localharness v{current} is live"),
            &format!("A new version shipped — see what changed: {RELEASES_URL}/tag/v{current}"),
        );
    }
}

#[derive(serde::Deserialize)]
struct Resolution {
    index: u32,
    sender: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    preview: String,
}

/// On mount (background): fetch the apex-hosted resolutions feed and, for any
/// resolved feedback whose `sender` is one of THIS user's addresses and that this
/// device hasn't been shown yet, drop a "your feedback was resolved" bell note.
/// Fully client-side (a static fetch + a localStorage seen-set) — the decentralized
/// stand-in for a server push to the submitter. No-op without an identity.
pub(crate) async fn notify_resolved_feedback() {
    // The feedback `sender` is the wallet that signed `submitFeedback` — this
    // user's verified owner / master address.
    let mut mine: Vec<String> = Vec::new();
    crate::app::APP.with(|c| {
        let app = c.borrow();
        if let crate::app::VerifyState::Verified { address } = &app.verify_state {
            mine.push(address.to_lowercase());
        }
        if let Some(w) = &app.wallet {
            mine.push(w.address_hex().to_lowercase());
        }
    });
    if mine.is_empty() {
        return;
    }
    let fetched = crate::app::net::read(async {
        let resp = reqwest::Client::new()
            .get(FEEDBACK_RESOLUTIONS_URL)
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            return None;
        }
        resp.text().await.ok()
    })
    .await
    .ok()
    .flatten();
    let Some(text) = fetched else { return };
    let Ok(items) = serde_json::from_str::<Vec<Resolution>>(&text) else { return };
    let Some(storage) = local_storage() else { return };
    // ABSENT key ⇒ first run on this device: seed the baseline of already-resolved
    // items SILENTLY so we don't dump the whole backlog at once — only items
    // resolved AFTER this point fire a note. (Mirrors the version-change baseline.)
    let seen_existing = storage.get_item("lh_seen_resolutions").ok().flatten();
    let first_run = seen_existing.is_none();
    let mut seen: std::collections::HashSet<u32> = seen_existing
        .unwrap_or_default()
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    let mut changed = false;
    for r in &items {
        if !mine.contains(&r.sender.to_lowercase()) {
            continue;
        }
        if !seen.insert(r.index) {
            continue; // already shown on this device
        }
        changed = true;
        if first_run {
            continue; // baseline seed — record it, but don't notify the backlog
        }
        let preview = if r.preview.is_empty() {
            String::new()
        } else {
            format!("\u{201c}{}\u{201d} — ", r.preview)
        };
        let ver = if r.version.is_empty() { "a recent update" } else { &r.version };
        push_to_bell(
            "Your feedback was resolved",
            &format!("{preview}addressed in {ver}. Changelog: {RELEASES_URL}"),
        );
    }
    // Persist on first run too (even with no matches yet) so the key EXISTS —
    // otherwise a later first-ever resolution would be silently seeded instead of
    // notified. Subsequent runs (key present) notify only genuinely-new items.
    if changed || first_run {
        let joined: Vec<String> = seen.iter().map(u32::to_string).collect();
        let _ = storage.set_item("lh_seen_resolutions", &joined.join(","));
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
        }
        let _ = fs.delete(PENDING_FILE).await;
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
    for i in cursor..count {
        if let Ok((from, _ts, raw)) = crate::registry::message_at(token_id, i).await {
            let (title, body) = parse_note(raw.trim(), &from);
            if title.is_empty() && body.is_empty() {
                continue;
            }
            if existing.iter().any(|(t, b)| *t == title && *b == body) {
                continue; // already shown via the live/stashed push of this note
            }
            push_to_bell(&title, &body);
        }
    }
    // Advance the cursor even if some decodes failed — a permanently-undecodable
    // message must not wedge the poll on every load.
    let _ = fs.write_atomic(MSG_CURSOR_FILE, count.to_string().as_bytes()).await;
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
    let mark: u128 = fs
        .read(LH_BALANCE_MARK_FILE)
        .await
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);

    // FIRST run (no mark yet): seed the baseline SILENTLY so we don't announce a
    // pre-existing balance as "just received" — mirrors the version/resolution
    // baselines. Only an increase AFTER this point fires a note.
    let first_run = fs.read(LH_BALANCE_MARK_FILE).await.is_err();
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
/// yes-confirm — on-chain feedback): wipe the log, hide the badge, re-render
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
    let sub_json = tag_sub_with_dev(&subscribe_push().await?).await;
    let (signer, addr) = crate::app::chat::credit_signer()
        .await
        .ok_or_else(|| "no identity on this device yet".to_string())?;
    // MULTI-DEVICE: merge into the existing slot (array, upsert by `dev` device
    // id when present, else endpoint) instead of overwriting — a phone and a
    // desktop share this address (same seed), and a bare replace silently
    // de-registered the other device.
    let addr_hex = crate::encoding::bytes_to_hex_str(&addr);
    let slot = crate::registry::addr_push_sub_of(&addr_hex).await.ok().flatten();
    let Some(merged) = crate::registry::merge_push_sub(slot.as_deref(), &sub_json) else {
        return Ok("already registered".to_string());
    };
    let sponsor = crate::app::sponsor::signer().map_err(|e| format!("sponsor: {e}"))?;
    let token = crate::registry::ALPHA_USD_ADDRESS();
    crate::registry::set_push_sub_sponsored(&signer, &sponsor, merged.as_bytes(), token).await
}

pub(crate) async fn enable_and_publish() -> Result<String, String> {
    if !ensure_permission().await? {
        return Err("notification permission denied — allow notifications for this site in the browser settings".to_string());
    }
    let sub_json = tag_sub_with_dev(&subscribe_push().await?).await;

    let (name, owner) = crate::app::tenant::current_tenant_owner().await?;
    let token_id = match crate::registry::main_of(&owner).await {
        Ok(id) if id != 0 => id,
        _ => match crate::registry::id_of_name(&name).await {
            Ok(id) if id != 0 => id,
            Ok(_) => return Err("this subdomain isn't registered on-chain yet".to_string()),
            Err(e) => return Err(format!("id_of_name: {e}")),
        },
    };

    // MULTI-DEVICE merge (see enable_device_push) — never overwrite siblings.
    let slot = crate::registry::push_sub_of(token_id).await.ok().flatten();
    let Some(merged) = crate::registry::merge_push_sub(slot.as_deref(), &sub_json) else {
        return Ok("already registered".to_string());
    };

    let registry_addr = crate::encoding::parse_address(crate::registry::REGISTRY_ADDRESS())?;
    let call = crate::tempo_tx::TempoCall {
        to: registry_addr,
        value_wei: 0,
        input: crate::registry::encode_set_push_sub(token_id, merged.as_bytes()),
    };
    let gas = crate::app::gas::set_metadata_gas(merged.len());
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
    let current = tag_sub_with_dev(&current).await;
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
    let published = crate::registry::push_sub_of(token_id).await.ok().flatten();
    // MULTI-DEVICE merge: only write when THIS device's entry is missing or
    // stale in the slot array (None = already current, no tx).
    let Some(merged) = crate::registry::merge_push_sub(published.as_deref(), &current) else {
        return;
    };
    let publish = async {
        let registry_addr = crate::encoding::parse_address(crate::registry::REGISTRY_ADDRESS())?;
        let call = crate::tempo_tx::TempoCall {
            to: registry_addr,
            value_wei: 0,
            input: crate::registry::encode_set_push_sub(token_id, merged.as_bytes()),
        };
        let gas = crate::app::gas::set_metadata_gas(merged.len());
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

/// HEADLESS auto-registration: on every app load, if notification permission is
/// ALREADY granted AND this device already has an identity, (re)publish its
/// ADDRESS-KEYED Web Push subscription — no gesture, no prompt, works for any
/// visitor (no MAIN needed). After the ONE-TIME permission grant (via the bell),
/// this keeps the device registered so a READY-UP broadcast always reaches it.
/// Idempotent: skips the sponsored write when the on-chain sub already matches.
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
    let Some((signer, addr)) = crate::app::chat::credit_signer().await else {
        return;
    };
    let Ok(current) = subscribe_push().await else {
        return;
    };
    let current = tag_sub_with_dev(&current).await;
    let addr_hex = crate::encoding::bytes_to_hex_str(&addr);
    let slot = crate::registry::addr_push_sub_of(&addr_hex).await.ok().flatten();
    // MULTI-DEVICE merge: None = this device is already in the slot array.
    let Some(merged) = crate::registry::merge_push_sub(slot.as_deref(), &current) else {
        return; // already up to date
    };
    let Ok(sponsor) = crate::app::sponsor::signer() else {
        return;
    };
    let token = crate::registry::ALPHA_USD_ADDRESS();
    match crate::registry::set_push_sub_sponsored(&signer, &sponsor, merged.as_bytes(), token).await
    {
        Ok(_) => web_sys::console::log_1(&wasm_bindgen::JsValue::from_str(
            "[push] device auto-registered (address-keyed)",
        )),
        Err(e) => web_sys::console::warn_1(&wasm_bindgen::JsValue::from_str(&format!(
            "[push] auto-register failed: {e}"
        ))),
    }
}
