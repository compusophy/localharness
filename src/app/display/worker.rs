//! WORKER — off-main-thread cartridge containment.
//!
//! The single-cartridge path runs the UNTRUSTED cartridge wasm in a Web Worker
//! (`web/cartridge-worker.js`) so a hung/unbounded `frame()` can only block the
//! worker, never the main thread (chat / studio stay live). The worker posts a
//! transferable framebuffer each frame; this module blits it to the canvas,
//! forwards pointer input + routes host-capability messages to the [`super::
//! bridge`] modules, and runs the WATCHDOG that terminates a worker which stops
//! posting frames — the actual hang defense (synchronous wasm is un-preemptable
//! from JS, so "kill it" is the only cure).
//!
//! This is what un-bricks a subdomain whose persisted public-face cartridge
//! loops forever: a previous build froze the whole tab on every reload; now the
//! reload spawns a worker, the watchdog fires after WATCHDOG_MS, the worker is
//! terminated, and an overlay invites a retry while the rest of the app works.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use js_sys::{ArrayBuffer, Object, Reflect, Uint8Array, Uint8ClampedArray};
use wasm_bindgen::prelude::*;
use wasm_bindgen::{Clamped, JsCast};
use web_sys::{CanvasRenderingContext2d, ImageData, MessageEvent, Worker};

use crate::app::dom;

use super::bridge::{audio, chat, compose, feed, http, mp};
use super::{FB_H, FB_W};

/// No-frame timeout: if the worker doesn't post a frame within this many
/// ms, the watchdog treats the cartridge as hung and terminates it. ~1.5s
/// is well past a normal frame (16ms) yet short enough that a hang is
/// obvious. Re-spawned workers each get the full window.
const WATCHDOG_MS: f64 = 1500.0;
/// How often the watchdog checks the last-frame timestamp.
const WATCHDOG_TICK_MS: i32 = 500;

thread_local! {
    /// The live cartridge worker + its kept-alive closures + watchdog
    /// handle. Replaced on the next cartridge load (terminating the prior
    /// one) and cleared on `stop`/hang.
    static WORKER: RefCell<Option<WorkerHandle>> = const { RefCell::new(None) };
    /// Spawn generation — bumped by every `spawn_cartridge` so a LATE
    /// message from a torn-down worker (its onmessage may already be
    /// queued when we terminate it) can't write an outcome that belongs
    /// to the previous run.
    static RUN_GEN: Cell<u32> = const { Cell::new(0) };
    /// The FIRST lifecycle outcome of the current spawn (issue #7: the
    /// run_cartridge tool used to return "running on display" no matter
    /// what — instantiate failures / traps / watchdog kills only reached
    /// the console + overlay, so the agent saw success on a dead run).
    /// `await_first_outcome` polls this to report the truth instead.
    static RUN_OUTCOME: RefCell<RunOutcome> = const { RefCell::new(RunOutcome::Pending) };
}

/// The first lifecycle signal a spawned cartridge produced.
#[derive(Clone)]
pub(super) enum RunOutcome {
    /// Nothing posted yet (still instantiating / first frame in flight).
    Pending,
    /// A frame (or a one-shot `done`) arrived — the cartridge is live.
    Live,
    /// A fatal error: the worker's coded `{type:'error'}` message, or the
    /// watchdog kill (`LH1001`). `code` is an `LH1xxx` registry value.
    Failed { code: Option<u16>, detail: String },
}

/// Record the FIRST outcome for generation `gen` (later signals and
/// stale-generation writes are ignored — the first frame/error is the
/// truth the tool result wants).
fn record_outcome(generation: u32, outcome: RunOutcome) {
    if RUN_GEN.with(|g| g.get()) != generation {
        return;
    }
    RUN_OUTCOME.with(|o| {
        let mut o = o.borrow_mut();
        if matches!(*o, RunOutcome::Pending) {
            // Auto-report a cartridge FAILURE off-chain (the LH1xxx code +
            // detail + the usual rich context), once per run, gated by the
            // telemetry toggle. Worker traps (LH1002–1004) and the watchdog
            // kill (LH1001) both funnel through here, so this is the one hook
            // — and the Pending guard makes it fire at most once per run.
            if let RunOutcome::Failed { code, detail } = &outcome {
                if crate::app::telemetry::enabled() {
                    let code = *code;
                    let detail = detail.clone();
                    let fp: String = detail
                        .chars()
                        .filter(|c| c.is_ascii_alphanumeric())
                        .take(40)
                        .collect();
                    let signature = format!("cartridge-{fp}");
                    let title = format!(
                        "cartridge failed: {}",
                        detail.chars().take(100).collect::<String>()
                    );
                    // Stamp in WHAT crashed (run_cartridge source / embed_app
                    // name) so the report is REPRODUCIBLE — it rides the
                    // freeform so it's redacted with everything else.
                    let freeform = match super::cartridge_ref() {
                        Some(r) if !r.is_empty() => format!("{detail}\n\n{r}"),
                        _ => detail,
                    };
                    wasm_bindgen_futures::spawn_local(crate::app::telemetry::report_event(
                        "cartridge".to_string(),
                        code,
                        title,
                        signature,
                        freeform,
                        String::new(),
                    ));
                }
            }
            *o = outcome;
        }
    });
}

/// The current spawn's first outcome so far (clone — cheap).
pub(super) fn current_outcome() -> RunOutcome {
    RUN_OUTCOME.with(|o| o.borrow().clone())
}

/// Everything that must outlive a running worker: the `Worker` itself, the
/// `onmessage` closure (JS holds a reference into it), the watchdog interval
/// id + its callback closure, and the `terminated` flag the watchdog/stop
/// path flip so an in-flight tick is a no-op. (The last-frame timestamp is
/// owned by the onmessage + watchdog closures via their own `Rc` clones —
/// it doesn't need a struct slot.)
///
/// The watchdog interval id lives in a SHARED [`Rc<Cell<Option<i32>>>`] —
/// the watchdog callback `take()`s it when it self-clears (on a hang), the
/// `done` handler `take()`s it when a one-shot render disarms it, and `Drop`
/// clears whatever id is still present. Exactly one of those clears the
/// interval; the others see `None`. (A bare `Option<i32>` here was the leak:
/// `stop_worker` set `terminated` BEFORE the drop, so `Drop` wrongly assumed
/// the watchdog had self-cleared and skipped the clear — leaving a live
/// interval whose `Closure` was just dropped, an invoke-after-drop hazard.)
struct WorkerHandle {
    worker: Worker,
    _onmessage: Closure<dyn FnMut(MessageEvent)>,
    watchdog: Rc<Cell<Option<i32>>>,
    _watchdog_cb: Option<Closure<dyn FnMut()>>,
    terminated: Rc<Cell<bool>>,
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        // Clear the watchdog interval if it's still armed. Whoever cleared
        // it first (the watchdog's hang self-clear, or the one-shot `done`
        // disarm) `take()`s the id to `None`, so this is a no-op in those
        // cases and never double-clears a recycled id. terminate() is
        // idempotent.
        if let Some(id) = self.watchdog.take() {
            if let Ok(win) = dom::window() {
                win.clear_interval_with_handle(id);
            }
        }
        self.worker.terminate();
    }
}

/// Spawn a worker for a SINGLE `wasm_bytes` cartridge, wire its message
/// handler to the canvas `ctx`, post the cartridge, and arm the watchdog.
/// Replaces any previous worker (its `Drop` terminates it + clears its
/// interval).
pub(super) fn spawn_cartridge(
    wasm_bytes: &[u8],
    ctx: CanvasRenderingContext2d,
) -> Result<(), JsValue> {
    let bytes = wasm_bytes.to_vec();
    spawn_worker(ctx, move |worker| {
        // Post the wasm. `instantiate` copies the bytes, so we don't need to
        // transfer ownership of this buffer.
        let arr = Uint8Array::from(&bytes[..]);
        let msg = Object::new();
        Reflect::set(&msg, &JsValue::from_str("type"), &JsValue::from_str("load"))?;
        Reflect::set(&msg, &JsValue::from_str("wasm"), &arr.buffer())?;
        attach_viewer_context(&msg)?;
        worker
            .post_message(&msg)
            .map_err(|e| JsValue::from_str(&format!("worker post failed: {e:?}")))
    })
}

/// Spawn a worker for a ROOTLESS `?compose=` composition (issue #77): the
/// grid-laid-out named modules run in the SAME isolated worker + watchdog as
/// a single cartridge, so a hung child can't freeze the tab. `slots` carries
/// `(name, x, y, w, h)` viewport tiles the main thread computed via
/// [`crate::compose::grid_viewports`]; the worker mounts each as a compose
/// child and resolves its on-chain `app.wasm` through the SAME
/// `compose_spawn` / `compose_bytes` round-trip a recursive spawn uses.
pub(super) fn spawn_composition(
    slots: Vec<(String, crate::raster::Viewport)>,
    ctx: CanvasRenderingContext2d,
) -> Result<(), JsValue> {
    spawn_worker(ctx, move |worker| {
        let arr = js_sys::Array::new();
        for (name, vp) in &slots {
            let s = Object::new();
            Reflect::set(&s, &JsValue::from_str("name"), &JsValue::from_str(name))?;
            Reflect::set(&s, &JsValue::from_str("x"), &JsValue::from_f64(vp.ox as f64))?;
            Reflect::set(&s, &JsValue::from_str("y"), &JsValue::from_f64(vp.oy as f64))?;
            Reflect::set(&s, &JsValue::from_str("w"), &JsValue::from_f64(vp.w as f64))?;
            Reflect::set(&s, &JsValue::from_str("h"), &JsValue::from_f64(vp.h as f64))?;
            arr.push(&s);
        }
        let msg = Object::new();
        Reflect::set(&msg, &JsValue::from_str("type"), &JsValue::from_str("compose_load"))?;
        Reflect::set(&msg, &JsValue::from_str("slots"), &arr)?;
        attach_viewer_context(&msg)?;
        worker
            .post_message(&msg)
            .map_err(|e| JsValue::from_str(&format!("worker post failed: {e:?}")))
    })
}

/// Stamp the load message with this device's viewer context (verified owner?
/// has a wallet?) for `host_agent`. Both read sync from APP state — cheap, no
/// RPC. A cartridge gates host-only controls on `viewer_is_owner`.
fn attach_viewer_context(msg: &Object) -> Result<(), JsValue> {
    let (is_owner, has_identity) = crate::app::APP.with(|c| {
        let app = c.borrow();
        (
            matches!(app.verify_state, crate::app::VerifyState::Verified { .. }),
            app.wallet.is_some(),
        )
    });
    Reflect::set(
        msg,
        &JsValue::from_str("viewerIsOwner"),
        &JsValue::from_f64(if is_owner { 1.0 } else { 0.0 }),
    )?;
    Reflect::set(
        msg,
        &JsValue::from_str("viewerHasIdentity"),
        &JsValue::from_f64(if has_identity { 1.0 } else { 0.0 }),
    )?;
    Ok(())
}

/// Spawn + wire a cartridge worker against canvas `ctx`, run `post_load` to
/// send the initial work (a single `load` or a `compose_load`), and arm the
/// watchdog. The message handler, watchdog, and teardown are IDENTICAL for
/// both the single-cartridge and `?compose=` paths — only the load message
/// differs — so both share this core. Replaces any previous worker (its
/// `Drop` terminates it + clears its interval).
fn spawn_worker(
    ctx: CanvasRenderingContext2d,
    post_load: impl FnOnce(&Worker) -> Result<(), JsValue>,
) -> Result<(), JsValue> {
    // Tear down the previous worker first (idempotent).
    stop_worker();

    // New spawn generation: reset the first-outcome slot so this run's
    // signals (and only this run's) populate it.
    let run_gen = RUN_GEN.with(|g| {
        let n = g.get().wrapping_add(1);
        g.set(n);
        n
    });
    RUN_OUTCOME.with(|o| *o.borrow_mut() = RunOutcome::Pending);

    let worker = Worker::new(&worker_url())
        .map_err(|e| JsValue::from_str(&format!("worker spawn failed: {e:?}")))?;

    let last_frame = Rc::new(Cell::new(js_sys::Date::now()));
    let terminated = Rc::new(Cell::new(false));
    // Shared watchdog interval id. `arm_watchdog` fills it; the watchdog
    // self-clears it on a hang, the `done` handler disarms it after a
    // one-shot render completes, and `WorkerHandle::Drop` clears whatever
    // remains. `take()` makes exactly one of those the real clear.
    let watchdog_id: Rc<Cell<Option<i32>>> = Rc::new(Cell::new(None));

    // Message handler: blit frames, play forwarded audio, surface errors.
    let onmessage = {
        let ctx = ctx.clone();
        let last_frame = last_frame.clone();
        let watchdog_id = watchdog_id.clone();
        let worker_for_msg = worker.clone();
        Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
            let data = e.data();
            let ty = Reflect::get(&data, &JsValue::from_str("type"))
                .ok()
                .and_then(|v| v.as_string())
                .unwrap_or_default();
            match ty.as_str() {
                "frame" => {
                    last_frame.set(js_sys::Date::now());
                    record_outcome(run_gen, RunOutcome::Live);
                    blit_frame(&data, &ctx);
                }
                "audio" => handle_audio(&data),
                "error" => {
                    let detail = Reflect::get(&data, &JsValue::from_str("detail"))
                        .ok()
                        .and_then(|v| v.as_string())
                        .unwrap_or_default();
                    // The worker tags each fatal error with a stable LH1xxx
                    // runtime code (trap / instantiate / no-entry); paint it
                    // into the overlay so the canvas shows the coded reason
                    // instead of just going dark. The watchdog handles the
                    // hang code (LH1001) on its own path — DISARM it here
                    // (same one-shot `take()` as the `done` arm), or it
                    // fires ~1.5s later (no frames after a fatal error) and
                    // repaints the coded overlay as a false LH1001.
                    if let Some(id) = watchdog_id.take() {
                        if let Ok(win) = dom::window() {
                            win.clear_interval_with_handle(id);
                        }
                    }
                    let code = Reflect::get(&data, &JsValue::from_str("code"))
                        .ok()
                        .and_then(|v| v.as_f64())
                        .map(|n| n as u16);
                    // Surface the failure to the agent too: the
                    // run_cartridge tool awaits this outcome, so the
                    // coded reason lands in the TOOL RESULT instead of
                    // only the canvas overlay + console (issue #7).
                    record_outcome(
                        run_gen,
                        RunOutcome::Failed { code, detail: detail.clone() },
                    );
                    if let Some(code) = code {
                        paint_stopped_overlay_coded(&ctx, code);
                    }
                    web_sys::console::warn_1(&JsValue::from_str(&format!(
                        "cartridge error{}: {detail}",
                        code.map(|c| format!(" {}", crate::error_codes::fmt_label(c)))
                            .unwrap_or_default()
                    )));
                }
                "log" => {
                    let msg = Reflect::get(&data, &JsValue::from_str("msg"))
                        .ok()
                        .and_then(|v| v.as_string())
                        .unwrap_or_default();
                    web_sys::console::log_1(&JsValue::from_str(&msg));
                }
                // host_agent::notify — a cartridge asked to buzz the viewer.
                // Permission-GATED (never prompts: `show` only renders if
                // already granted, and we skip entirely otherwise) and the
                // worker already rate-limited. Local to THIS viewer only.
                "agent_notify" => {
                    if matches!(
                        web_sys::Notification::permission(),
                        web_sys::NotificationPermission::Granted
                    ) {
                        let title = Reflect::get(&data, &JsValue::from_str("title"))
                            .ok()
                            .and_then(|v| v.as_string())
                            .unwrap_or_default();
                        let body = Reflect::get(&data, &JsValue::from_str("body"))
                            .ok()
                            .and_then(|v| v.as_string())
                            .unwrap_or_default();
                        if !title.is_empty() {
                            wasm_bindgen_futures::spawn_local(async move {
                                let _ = crate::app::notifications::show(&title, &body).await;
                            });
                        }
                    }
                }
                // The running cartridge imports the host::agent feed surface
                // → arm permission-priming so the NEXT canvas tap (a real
                // main-thread gesture) can request notification permission,
                // which the cartridge's own subscribe() tap cannot.
                "cartridge_uses_feed" => feed::set_feed_cartridge_active(true),
                "agent_subscribe" => {
                    let w = worker_for_msg.clone();
                    wasm_bindgen_futures::spawn_local(feed::do_feed_subscribe(w, true));
                }
                "agent_unsubscribe" => {
                    let w = worker_for_msg.clone();
                    wasm_bindgen_futures::spawn_local(feed::do_feed_subscribe(w, false));
                }
                "agent_broadcast" => {
                    let title = Reflect::get(&data, &JsValue::from_str("title"))
                        .ok().and_then(|v| v.as_string()).unwrap_or_default();
                    let body = Reflect::get(&data, &JsValue::from_str("body"))
                        .ok().and_then(|v| v.as_string()).unwrap_or_default();
                    if !title.is_empty() {
                        wasm_bindgen_futures::spawn_local(feed::do_feed_broadcast(title, body));
                    }
                }
                // The cartridge wants a CUSTOM broadcast message — open the
                // host-side text input over the canvas (the cartridge can't
                // summon a keyboard from pixels). [send] runs the same
                // do_feed_broadcast as agent_broadcast, with the typed body.
                "agent_broadcast_compose" => {
                    let title = Reflect::get(&data, &JsValue::from_str("title"))
                        .ok().and_then(|v| v.as_string()).unwrap_or_default();
                    let body = Reflect::get(&data, &JsValue::from_str("body"))
                        .ok().and_then(|v| v.as_string()).unwrap_or_default();
                    if !title.is_empty() {
                        super::surface::open_broadcast_composer(&title, &body);
                    }
                }
                "agent_request_identity" => {
                    let w = worker_for_msg.clone();
                    wasm_bindgen_futures::spawn_local(feed::do_feed_request_identity(w));
                }
                // host::compose — a cartridge ANYWHERE in the compose tree
                // spawned a child. The worker can't read the on-chain
                // registry, so it posted the child's name + its global uid
                // here; resolve the published app.wasm on the MAIN thread and
                // post the bytes back (or a FAILED signal). The worker
                // instantiates it into the matching node.
                "compose_spawn" => {
                    let uid = Reflect::get(&data, &JsValue::from_str("uid"))
                        .ok().and_then(|v| v.as_f64()).map(|n| n as i32).unwrap_or(-1);
                    let name = Reflect::get(&data, &JsValue::from_str("name"))
                        .ok().and_then(|v| v.as_string()).unwrap_or_default();
                    if uid >= 0 && !name.is_empty() {
                        let w = worker_for_msg.clone();
                        wasm_bindgen_futures::spawn_local(compose::do_compose_spawn(w, uid, name));
                    }
                }
                // host::http — a cartridge called http::get. The worker can't
                // sign the proxy token or fetch cross-origin, so it posted the
                // url + a global id here; run the authed /api/fetch proxy POST
                // on the MAIN thread and post the body back as `http_result`.
                "http_fetch" => {
                    let id = Reflect::get(&data, &JsValue::from_str("id"))
                        .ok().and_then(|v| v.as_f64()).map(|n| n as i32).unwrap_or(-1);
                    let url = Reflect::get(&data, &JsValue::from_str("url"))
                        .ok().and_then(|v| v.as_string()).unwrap_or_default();
                    if id >= 0 && !url.is_empty() {
                        let w = worker_for_msg.clone();
                        wasm_bindgen_futures::spawn_local(http::do_http_fetch(w, id, url));
                    }
                }
                // host::mp — a multiplayer cartridge wants to connect (open as
                // HOST / JOIN a code), or has buffered state to broadcast. The
                // proven webrtc.rs Peer + the relay live HERE on main (the
                // worker can't sign the relay token or hold an RtcPeerConnection
                // cheaply); incoming peer frames come back as `mp:peer`.
                "mp:host" | "mp:join" => {
                    let code = Reflect::get(&data, &JsValue::from_str("room"))
                        .ok().and_then(|v| v.as_f64()).map(|n| n as i32).unwrap_or(0);
                    let is_host = ty == "mp:host";
                    wasm_bindgen_futures::spawn_local(mp::mp_connect(
                        worker_for_msg.clone(), code, is_host,
                    ));
                }
                "mp:auto" => {
                    // Single shared room: FULL P2P MESH (no host) — claim a slot
                    // + connect directly to every peer.
                    let code = Reflect::get(&data, &JsValue::from_str("room"))
                        .ok().and_then(|v| v.as_f64()).map(|n| n as i32).unwrap_or(0);
                    wasm_bindgen_futures::spawn_local(mp::mp_connect_mesh(
                        worker_for_msg.clone(), code,
                    ));
                }
                "mp:deltas" => mp::mp_send(Some(mp::mp_read_int_array(&data, "deltas")), None),
                "mp:events" => mp::mp_send(None, Some(mp::mp_read_int_array(&data, "events"))),
                "mp:leave" => mp::mp_teardown(),
                // host::chat — an open-chatroom cartridge wants to start
                // receiving (begin polling the /api/chat relay for this
                // subdomain) or post a line. The relay + personal-sign auth
                // live HERE on main; new lines come back as `chat:msg`.
                "chat:start" => chat::chat_start(worker_for_msg.clone()),
                "chat:send" => {
                    let text = Reflect::get(&data, &JsValue::from_str("text"))
                        .ok().and_then(|v| v.as_string()).unwrap_or_default();
                    if !text.is_empty() {
                        chat::chat_send(text);
                    }
                }
                "done" => {
                    record_outcome(run_gen, RunOutcome::Live);
                    // A one-shot `render()` finished and posted its single
                    // frame — it can't hang now (it already returned), so
                    // DISARM the watchdog. Otherwise it would fire ~1.5s
                    // later and falsely paint "CARTRIDGE STOPPED LH1001"
                    // over a perfectly-good static render. The worker stays
                    // alive (a re-load reuses it). Animated cartridges never
                    // send `done`, so they keep the watchdog.
                    if let Some(id) = watchdog_id.take() {
                        if let Ok(win) = dom::window() {
                            win.clear_interval_with_handle(id);
                        }
                    }
                }
                _ => {}
            }
        })
    };
    worker.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));

    // Post the initial work — a single `load` or a `compose_load` — both
    // already stamped with the viewer context via `attach_viewer_context`.
    post_load(&worker)?;

    // Best-effort: resolve the live feed context (subscribed? count?) and
    // post it so host_agent::is_subscribed / subscriber_count reflect
    // reality a beat after launch. Off a tenant this is a no-op.
    {
        let w = worker.clone();
        wasm_bindgen_futures::spawn_local(feed::refresh_feed_context(w));
    }

    // Arm the watchdog: terminate the worker if no frame lands in time.
    let watchdog_cb = arm_watchdog(
        worker.clone(),
        ctx,
        last_frame.clone(),
        terminated.clone(),
        watchdog_id.clone(),
        run_gen,
    );

    WORKER.with(|cell| {
        *cell.borrow_mut() = Some(WorkerHandle {
            worker,
            _onmessage: onmessage,
            watchdog: watchdog_id,
            _watchdog_cb: watchdog_cb,
            terminated,
        });
    });
    Ok(())
}

/// Terminate + drop the current worker (clears its watchdog). Idempotent.
pub(super) fn stop_worker() {
    mp::mp_teardown(); // close any multiplayer Peer this cartridge held
    chat::chat_stop(); // halt the chatroom relay-poll loop
    WORKER.with(|cell| {
        // Mark terminated so an in-flight watchdog tick is a no-op, then
        // drop the handle (its `Drop` terminates the worker + clears the
        // interval).
        if let Some(h) = cell.borrow().as_ref() {
            h.terminated.set(true);
        }
        *cell.borrow_mut() = None;
    });
}


/// Forward the latest pointer to the worker (poll model). No-op if no
/// worker is live.
pub(super) fn post_input(x: i32, y: i32, down: i32) {
    WORKER.with(|cell| {
        if let Some(h) = cell.borrow().as_ref() {
            let msg = Object::new();
            let _ = Reflect::set(&msg, &JsValue::from_str("type"), &JsValue::from_str("input"));
            let _ = Reflect::set(&msg, &JsValue::from_str("x"), &JsValue::from_f64(x as f64));
            let _ = Reflect::set(&msg, &JsValue::from_str("y"), &JsValue::from_f64(y as f64));
            let _ = Reflect::set(&msg, &JsValue::from_str("down"), &JsValue::from_f64(down as f64));
            let _ = h.worker.post_message(&msg);
        }
    });
}

/// `true` while a cartridge worker is live AND not terminated (used by the
/// input handlers to decide whether to forward pointer events). A hung
/// worker the watchdog killed reports `false` even though its inert handle
/// is still in the slot.
pub(super) fn is_active() -> bool {
    WORKER.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|h| !h.terminated.get())
            .unwrap_or(false)
    })
}

/// Resolve the worker script URL relative to the page. `web/cartridge-
/// worker.js` ships next to `index.html`, so an absolute `/cartridge-
/// worker.js` resolves on apex, tenants, and Vercel previews alike.
fn worker_url() -> String {
    "/cartridge-worker.js".to_string()
}

/// Blit a `{ type:'frame', fb:ArrayBuffer, w, h }` message to the canvas.
/// The framebuffer is RGBA8888 already in the worker's packing
/// (0xAABBGGRR little-endian == ImageData byte order R,G,B,A).
///
/// VARIABLE RESOLUTION: the worker stamps every frame with the cartridge's
/// actual `w`/`h` (its `dims()` export, or the 512×512 default). We size the
/// canvas backing store to those dims (idempotent: a no-op once it matches,
/// so steady-state frames don't thrash) and build the `ImageData` at `w`×`h`
/// — the canvas adapts to the cartridge's chosen size on the FIRST frame.
/// Falls back to [`FB_W`]/[`FB_H`] if the message omits/garbles the dims.
fn blit_frame(data: &JsValue, ctx: &CanvasRenderingContext2d) {
    let Ok(fb) = Reflect::get(data, &JsValue::from_str("fb")) else { return };
    let Ok(buffer) = fb.dyn_into::<ArrayBuffer>() else { return };
    let w = Reflect::get(data, &JsValue::from_str("w"))
        .ok()
        .and_then(|v| v.as_f64())
        .map(|n| n as u32)
        .filter(|&n| n > 0)
        .unwrap_or(FB_W);
    let h = Reflect::get(data, &JsValue::from_str("h"))
        .ok()
        .and_then(|v| v.as_f64())
        .map(|n| n as u32)
        .filter(|&n| n > 0)
        .unwrap_or(FB_H);
    let clamped = Uint8ClampedArray::new(&buffer);
    // Bound the transferred buffer before allocating: the worker clamps dims to
    // [16,1024] (<= 1024*1024*4 = 4 MiB), so anything larger is a worker bug or a
    // compromised cartridge — drop the frame rather than OOM-panic the whole tab.
    if clamped.length() as usize > 4 * 1024 * 1024 {
        return;
    }
    // ImageData wants a &Clamped<&[u8]>; copy the transferred buffer out.
    let mut bytes = vec![0u8; clamped.length() as usize];
    clamped.copy_to(&mut bytes[..]);
    // Resize the canvas backing store to the cartridge's framebuffer dims
    // (setting width/height to the same value is cheap; only a real change
    // reallocates + clears). The CSS keeps the element aspect-scaled.
    let canvas = ctx.canvas();
    if let Some(canvas) = canvas {
        if canvas.width() != w {
            canvas.set_width(w);
        }
        if canvas.height() != h {
            canvas.set_height(h);
        }
    }
    if let Ok(img) =
        ImageData::new_with_u8_clamped_array_and_sh(Clamped(&bytes[..]), w, h)
    {
        let _ = ctx.put_image_data(&img, 0.0, 0.0);
    }
}

/// Play a `{ type:'audio', op, args }` message on the main-thread audio
/// engine (AudioContext can't run in a worker). Mirrors the `host_audio`
/// ABI; the return handle is dropped (the worker tracks its own local
/// handles).
fn handle_audio(data: &JsValue) {
    let op = Reflect::get(data, &JsValue::from_str("op"))
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default();
    let args = Reflect::get(data, &JsValue::from_str("args")).unwrap_or(JsValue::NULL);
    let arg = |i: u32| -> i32 {
        Reflect::get_u32(&args, i)
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as i32
    };
    match op.as_str() {
        "tone" => { audio::play_tone(arg(0), arg(1), arg(2), 0); }
        "tone_at" => { audio::play_tone(arg(0), arg(1), arg(2), arg(3)); }
        "noise" => { audio::play_noise(arg(0)); }
        "stop" => audio::stop_handle(arg(0)),
        "set_volume" => audio::set_master_volume(arg(0)),
        _ => {}
    }
}

/// Arm a polling watchdog: every `WATCHDOG_TICK_MS`, if the last frame is
/// older than `WATCHDOG_MS`, the cartridge is hung → terminate the worker,
/// paint a "stopped" overlay, and clear its own interval. Crucially this
/// runs on the MAIN thread, which is never blocked (the cartridge runs in
/// the worker), so it can always fire — that is what makes a hung cartridge
/// terminable + the subdomain un-brickable.
///
/// SAFETY: the watchdog must NOT drop its own `WorkerHandle` from inside the
/// callback (that would drop the `Closure` currently executing). So on a
/// hang it sets `terminated` (making the slot inert + `is_active()` false),
/// terminates the worker, and clears its interval via the SHARED `interval_id`
/// cell — but leaves the `WorkerHandle` in place. The next `spawn_cartridge`
/// / `stop_worker` drops it safely (off the callback stack); `Drop` clears
/// whatever id remains, and the `take()` here ensures it sees `None`.
fn arm_watchdog(
    worker: Worker,
    ctx: CanvasRenderingContext2d,
    last_frame: Rc<Cell<f64>>,
    terminated: Rc<Cell<bool>>,
    interval_id: Rc<Cell<Option<i32>>>,
    run_gen: u32,
) -> Option<Closure<dyn FnMut()>> {
    let cb = {
        let interval_id = interval_id.clone();
        Closure::<dyn FnMut()>::new(move || {
            if terminated.get() {
                return;
            }
            if js_sys::Date::now() - last_frame.get() > WATCHDOG_MS {
                terminated.set(true);
                worker.terminate();
                // LH1001 = frame timeout (the hang the watchdog catches).
                // Record it as the run outcome too, so a cartridge that
                // never produced its FIRST frame reports the kill to the
                // awaiting run_cartridge tool, not just the overlay.
                record_outcome(
                    run_gen,
                    RunOutcome::Failed {
                        code: Some(crate::error_codes::FRAME_TIMEOUT),
                        detail: format!(
                            "no frame within {WATCHDOG_MS}ms — the watchdog \
                             terminated the hung cartridge"
                        ),
                    },
                );
                paint_stopped_overlay_coded(&ctx, crate::error_codes::FRAME_TIMEOUT);
                // Self-clear the interval (don't drop the handle here). The
                // shared `take()` leaves `None` so `Drop` won't re-clear a
                // recycled id.
                if let Some(id) = interval_id.take() {
                    if let Ok(win) = dom::window() {
                        win.clear_interval_with_handle(id);
                    }
                }
            }
        })
    };
    let id = dom::window().ok().and_then(|win| {
        win.set_interval_with_callback_and_timeout_and_arguments_0(
            cb.as_ref().unchecked_ref(),
            WATCHDOG_TICK_MS,
        )
        .ok()
    });
    interval_id.set(id);
    Some(cb)
}

/// Paint a monochrome "cartridge stopped" message into the framebuffer
/// (pixels, via the shared 5x7 font — no DOM), so the canvas shows WHY it
/// went dark instead of just freezing — now carrying the stable LH1xxx
/// runtime code + its registry meaning. The rest of the app is reachable.
/// The font has only uppercase/digits/limited punctuation, so we uppercase
/// the meaning and keep the lines short.
fn paint_stopped_overlay_coded(ctx: &CanvasRenderingContext2d, code: u16) {
    let mut buf = vec![0u8; (FB_W * FB_H * 4) as usize];
    for px in buf.chunks_exact_mut(4) {
        px[3] = 255; // opaque black
    }
    let vp = crate::raster::Viewport::full(FB_W as i32, FB_H as i32);
    // Line 1: "CARTRIDGE STOPPED LHxxxx"; line 2: the code's meaning.
    let label = crate::error_codes::fmt_label(code);
    let meaning = crate::error_codes::lookup(code)
        .map(|e| e.meaning.to_uppercase())
        .unwrap_or_else(|| "RELOAD TO RETRY".to_string());
    let header = format!("CARTRIDGE STOPPED {label}");
    let owned = [header, meaning];
    let lines: [&str; 2] = [owned[0].as_str(), owned[1].as_str()];
    let mut y = (FB_H as i32) / 2 - 8;
    for line in lines {
        let advance = 6; // 5px glyph + 1px gap at scale 1
        let width = line.len() as i32 * advance;
        let mut x = ((FB_W as i32) - width) / 2;
        for ch in line.chars() {
            crate::raster::blit_glyph(
                &mut buf, FB_W as i32, &vp, x, y, ch as u32, (200, 200, 200), 1,
            );
            x += advance;
        }
        y += 12;
    }
    if let Ok(img) =
        ImageData::new_with_u8_clamped_array_and_sh(Clamped(&buf[..]), FB_W, FB_H)
    {
        let _ = ctx.put_image_data(&img, 0.0, 0.0);
    }
}
