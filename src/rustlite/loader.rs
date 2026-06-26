//! Cartridge loader — instantiate compiled wasm bytes in the browser.
//!
//! Takes the output of `codegen::emit`, instantiates it via
//! `WebAssembly.instantiate`, wires up host imports, and provides
//! a `call` method to invoke exported functions.
//!
//! wasm32-only — on native targets this module compiles but all
//! methods return errors (no browser WebAssembly API).

use crate::rustlite::CompileError;

pub struct Cartridge {
    #[cfg(target_arch = "wasm32")]
    instance: wasm_bindgen::JsValue,
    #[cfg(target_arch = "wasm32")]
    memory: wasm_bindgen::JsValue,
    // Keeps the `host_net` closures + state alive for the cartridge's
    // lifetime (wasm holds JS references into them after instantiation).
    #[cfg(target_arch = "wasm32")]
    _net: NetRuntime,
    #[cfg(not(target_arch = "wasm32"))]
    _phantom: (),
}

impl Cartridge {
    /// Instantiate compiled wasm bytes into a runnable cartridge.
    pub async fn load(wasm_bytes: &[u8]) -> Result<Self, CompileError> {
        #[cfg(target_arch = "wasm32")]
        {
            load_wasm(wasm_bytes).await
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = wasm_bytes;
            Err(CompileError::new("cartridge loading requires a browser environment"))
        }
    }

    /// List all exported function names.
    pub fn exports(&self) -> Vec<String> {
        #[cfg(target_arch = "wasm32")]
        {
            list_exports(&self.instance)
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            Vec::new()
        }
    }

    /// Call an exported function with i32 arguments, returns i32.
    pub fn call_i32(&self, name: &str, args: &[i32]) -> Result<i32, CompileError> {
        #[cfg(target_arch = "wasm32")]
        {
            call_export_i32(&self.instance, name, args)
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (name, args);
            Err(CompileError::new("cartridge execution requires a browser environment"))
        }
    }

    /// Read a string from cartridge memory at the given pointer.
    /// Expects length-prefixed layout: 4 bytes LE length, then UTF-8 payload.
    pub fn read_string(&self, ptr: i32) -> Result<String, CompileError> {
        #[cfg(target_arch = "wasm32")]
        {
            read_string_from_memory(&self.memory, ptr)
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = ptr;
            Err(CompileError::new("requires browser environment"))
        }
    }
}

/// Hard ceiling on cartridge wasm a cartridge loader will instantiate. A
/// cartridge is UNTRUSTED bytes (published by any agent / fetched on-chain),
/// so the runtime must not rely on the publish-time UI cap (16 KB) — refuse
/// oversized bytes here too. 64 KB leaves generous headroom over the publish
/// cap while still bounding the work `instantiate_buffer` is asked to do.
#[cfg(target_arch = "wasm32")]
const MAX_CARTRIDGE_BYTES: usize = 64 * 1024;

#[cfg(target_arch = "wasm32")]
async fn load_wasm(wasm_bytes: &[u8]) -> Result<Cartridge, CompileError> {
    use js_sys::{Reflect, WebAssembly};
    use wasm_bindgen::JsValue;
    use wasm_bindgen_futures::JsFuture;

    // Size gate BEFORE instantiation — a malicious blob can't make us hand an
    // arbitrarily large buffer to the wasm engine.
    if wasm_bytes.len() > MAX_CARTRIDGE_BYTES {
        return Err(CompileError::new(format!(
            "cartridge too large: {} bytes (max {MAX_CARTRIDGE_BYTES})",
            wasm_bytes.len()
        )));
    }

    // The `host_net` closures need the cartridge's linear memory to read
    // outbound strings and write inbound ones, but memory only exists
    // after instantiation. Share a cell the closures read lazily; we fill
    // it in below once the instance is live.
    let mem_cell: SharedMemory = std::rc::Rc::new(std::cell::RefCell::new(JsValue::NULL));

    let (imports, net) = build_host_imports(&mem_cell)?;

    let promise = WebAssembly::instantiate_buffer(wasm_bytes, &imports);
    let result = JsFuture::from(promise)
        .await
        .map_err(|e| CompileError::new(format!("instantiate failed: {e:?}")))?;

    let instance = Reflect::get(&result, &JsValue::from_str("instance"))
        .map_err(|e| CompileError::new(format!("no instance: {e:?}")))?;

    let exports = Reflect::get(&instance, &JsValue::from_str("exports"))
        .map_err(|e| CompileError::new(format!("no exports: {e:?}")))?;
    // NB: the generic loader does NOT require a `frame`/`render` export — it
    // backs `compile_rustlite`, which compiles + calls an ARBITRARY exported
    // function (e.g. `add`). The display path (`run_with_ctx` /
    // `mount_composition`) enforces the `frame`/`render` entry for cartridges
    // it actually animates.
    let memory = Reflect::get(&exports, &JsValue::from_str("memory"))
        .unwrap_or(JsValue::NULL);

    *mem_cell.borrow_mut() = memory.clone();

    Ok(Cartridge { instance, memory, _net: net })
}

/// Shared handle to the cartridge's linear memory, filled in after
/// instantiation so the `host_net` closures can read/write strings.
#[cfg(target_arch = "wasm32")]
type SharedMemory = std::rc::Rc<std::cell::RefCell<wasm_bindgen::JsValue>>;

#[cfg(target_arch = "wasm32")]
use net::NetRuntime;

#[cfg(target_arch = "wasm32")]
fn build_host_imports(mem: &SharedMemory) -> Result<(js_sys::Object, NetRuntime), CompileError> {
    use js_sys::{Object, Reflect};
    use wasm_bindgen::prelude::*;

    let imports = Object::new();

    // host_log module — ambient, always available
    let host_log = Object::new();
    let log_info = Closure::<dyn Fn(i32)>::new(|_ptr: i32| {
        web_sys::console::log_1(&wasm_bindgen::JsValue::from_str("[cartridge] log"));
    });
    let _ = Reflect::set(&host_log, &JsValue::from_str("info"), log_info.as_ref());
    let _ = Reflect::set(&host_log, &JsValue::from_str("warn"), log_info.as_ref());
    let _ = Reflect::set(&host_log, &JsValue::from_str("error"), log_info.as_ref());
    let _ = Reflect::set(&host_log, &JsValue::from_str("debug"), log_info.as_ref());
    log_info.forget();
    let _ = Reflect::set(&imports, &JsValue::from_str("host_log"), &host_log);

    // host_time module — ambient
    let host_time = Object::new();
    let now_fn = Closure::<dyn Fn() -> f64>::new(|| {
        js_sys::Date::now()
    });
    let _ = Reflect::set(&host_time, &JsValue::from_str("now_unix_ms"), now_fn.as_ref());
    let _ = Reflect::set(&host_time, &JsValue::from_str("monotonic_ms"), now_fn.as_ref());
    now_fn.forget();
    let _ = Reflect::set(&imports, &JsValue::from_str("host_time"), &host_time);

    // host_abort module — ambient
    let host_abort = Object::new();
    let panic_fn = Closure::<dyn Fn(i32)>::new(|_ptr: i32| {
        web_sys::console::error_1(&wasm_bindgen::JsValue::from_str("[cartridge] panic"));
    });
    let _ = Reflect::set(&host_abort, &JsValue::from_str("panic"), panic_fn.as_ref());
    panic_fn.forget();

    // fuel_remaining: a constant in this BARE loader (SDK / compile-check path)
    // because it runs no frame loop — there's no budget to decrement. The DISPLAY
    // path runs cartridges in a Web Worker (web/cartridge-worker.js), whose
    // host_abort.fuel_remaining IS a real per-frame decreasing budget. Note that
    // fuel is only a courtesy for cartridges that voluntarily poll it: the
    // rustlite compiler emits no fuel checks, so the actual hang defense against
    // an unbounded frame() is the worker + the main-thread WATCHDOG that
    // terminates a worker which stops posting frames (display.rs `mod worker`).
    let fuel_fn = Closure::<dyn Fn() -> f64>::new(|| 1_000_000.0);
    let _ = Reflect::set(&host_abort, &JsValue::from_str("fuel_remaining"), fuel_fn.as_ref());
    fuel_fn.forget();

    let mem_fn = Closure::<dyn Fn() -> i32>::new(|| 0);
    let _ = Reflect::set(&host_abort, &JsValue::from_str("memory_bytes"), mem_fn.as_ref());
    mem_fn.forget();
    let _ = Reflect::set(&imports, &JsValue::from_str("host_abort"), &host_abort);

    // host_audio module — ambient stub. The real Web Audio engine lives in
    // src/app/display.rs (browser-app); this no-op keeps the bare loader
    // (SDK / tests) able to instantiate a cartridge that imports host_audio
    // instead of failing instantiation with a missing-import LinkError.
    let host_audio = Object::new();
    let audio_ret3 = Closure::<dyn Fn(i32, i32, i32) -> i32>::new(|_a, _b, _c| -1);
    let audio_ret4 = Closure::<dyn Fn(i32, i32, i32, i32) -> i32>::new(|_a, _b, _c, _d| -1);
    let audio_noise = Closure::<dyn Fn(i32) -> i32>::new(|_a| -1);
    let audio_void = Closure::<dyn Fn(i32)>::new(|_a| {});
    let _ = Reflect::set(&host_audio, &JsValue::from_str("tone"), audio_ret3.as_ref());
    let _ = Reflect::set(&host_audio, &JsValue::from_str("tone_at"), audio_ret4.as_ref());
    let _ = Reflect::set(&host_audio, &JsValue::from_str("noise"), audio_noise.as_ref());
    let _ = Reflect::set(&host_audio, &JsValue::from_str("stop"), audio_void.as_ref());
    let _ = Reflect::set(&host_audio, &JsValue::from_str("set_volume"), audio_void.as_ref());
    audio_ret3.forget();
    audio_ret4.forget();
    audio_noise.forget();
    audio_void.forget();
    let _ = Reflect::set(&imports, &JsValue::from_str("host_audio"), &host_audio);

    // host_compose module — cartridge-in-cartridge composition (the real
    // compositor lives in the Web Worker / src/app/display.rs). This BARE loader
    // (SDK / compile-check path) runs no compositor context, so every op is an
    // inert stub: spawn_module / status / focused return a negative "no compositor
    // / bad handle" code, the rest return 0. Without these a cartridge that
    // imports host_compose would fail instantiation with a missing-import
    // LinkError. Children mounted by the live compositor get a similarly inert
    // host_compose (recursion is the parent's job, capped by ComposeBudget).
    let host_compose = Object::new();
    let compose_spawn = Closure::<dyn Fn(i32, i32, i32, i32, i32) -> i32>::new(|_a, _b, _c, _d, _e| -1);
    let compose_one = Closure::<dyn Fn(i32) -> i32>::new(|_a| -1);
    let compose_move = Closure::<dyn Fn(i32, i32, i32, i32, i32) -> i32>::new(|_a, _b, _c, _d, _e| 0);
    let compose_none = Closure::<dyn Fn() -> i32>::new(|| -1);
    let _ = Reflect::set(&host_compose, &JsValue::from_str("spawn_module"), compose_spawn.as_ref());
    let _ = Reflect::set(&host_compose, &JsValue::from_str("status"), compose_one.as_ref());
    let _ = Reflect::set(&host_compose, &JsValue::from_str("move_module"), compose_move.as_ref());
    let _ = Reflect::set(&host_compose, &JsValue::from_str("focus_module"), compose_one.as_ref());
    let _ = Reflect::set(&host_compose, &JsValue::from_str("focused"), compose_none.as_ref());
    let _ = Reflect::set(&host_compose, &JsValue::from_str("close_module"), compose_one.as_ref());
    let _ = Reflect::set(&host_compose, &JsValue::from_str("module_count"), compose_none.as_ref());
    compose_spawn.forget();
    compose_one.forget();
    compose_move.forget();
    compose_none.forget();
    let _ = Reflect::set(&imports, &JsValue::from_str("host_compose"), &host_compose);

    // host_display module — ambient stub. The REAL framebuffer host lives in
    // the Web Worker (web/cartridge-worker.js) / src/app/display.rs; this bare
    // loader (SDK / `compile_rustlite` compile-check path) runs no canvas, so
    // every draw op is a no-op and the queries return 0. Without this, a
    // cartridge that imports host_display (any clear/set_pixel/draw_* call)
    // failed instantiation with "Import #0 host_display: module is not an
    // object or function" — forcing run_cartridge just to compile-check
    // (on-chain feedback). ABI mirrors src/rustlite/typecheck.rs display::*.
    let host_display = Object::new();
    let disp_clear = Closure::<dyn Fn(i32)>::new(|_a| {});
    let disp_void3 = Closure::<dyn Fn(i32, i32, i32)>::new(|_a, _b, _c| {});
    let disp_void5 = Closure::<dyn Fn(i32, i32, i32, i32, i32)>::new(|_a, _b, _c, _d, _e| {});
    let disp_tri = Closure::<dyn Fn(i32, i32, i32, i32, i32, i32, i32)>::new(
        |_a, _b, _c, _d, _e, _f, _g| {},
    );
    let disp_present = Closure::<dyn Fn()>::new(|| {});
    let disp_query = Closure::<dyn Fn() -> i32>::new(|| 0);
    let disp_state_get = Closure::<dyn Fn(i32) -> i32>::new(|_a| 0);
    let disp_state_set = Closure::<dyn Fn(i32, i32)>::new(|_a, _b| {});
    let _ = Reflect::set(&host_display, &JsValue::from_str("clear"), disp_clear.as_ref());
    let _ = Reflect::set(&host_display, &JsValue::from_str("set_pixel"), disp_void3.as_ref());
    let _ = Reflect::set(&host_display, &JsValue::from_str("fill_rect"), disp_void5.as_ref());
    let _ = Reflect::set(&host_display, &JsValue::from_str("draw_char"), disp_void5.as_ref());
    let _ = Reflect::set(&host_display, &JsValue::from_str("draw_number"), disp_void5.as_ref());
    let _ = Reflect::set(&host_display, &JsValue::from_str("draw_line"), disp_void5.as_ref());
    let _ = Reflect::set(&host_display, &JsValue::from_str("fill_triangle"), disp_tri.as_ref());
    let _ = Reflect::set(&host_display, &JsValue::from_str("present"), disp_present.as_ref());
    let _ = Reflect::set(&host_display, &JsValue::from_str("width"), disp_query.as_ref());
    let _ = Reflect::set(&host_display, &JsValue::from_str("height"), disp_query.as_ref());
    let _ = Reflect::set(&host_display, &JsValue::from_str("pointer_x"), disp_query.as_ref());
    let _ = Reflect::set(&host_display, &JsValue::from_str("pointer_y"), disp_query.as_ref());
    let _ = Reflect::set(&host_display, &JsValue::from_str("pointer_down"), disp_query.as_ref());
    let _ = Reflect::set(&host_display, &JsValue::from_str("state_get"), disp_state_get.as_ref());
    let _ = Reflect::set(&host_display, &JsValue::from_str("state_set"), disp_state_set.as_ref());
    disp_clear.forget();
    disp_void3.forget();
    disp_void5.forget();
    disp_tri.forget();
    disp_present.forget();
    disp_query.forget();
    disp_state_get.forget();
    disp_state_set.forget();
    let _ = Reflect::set(&imports, &JsValue::from_str("host_display"), &host_display);

    // host_http module — inert stub. The REAL HTTP host (fetch through the
    // platform's `/api/fetch` proxy + HTML→text) lives in the Web Worker
    // (web/cartridge-worker.js); this bare loader (SDK / `compile_rustlite`
    // compile-check path) has no proxy, so `get` refuses (-1), every poller
    // reports a bad handle, and `parse_text` writes nothing (0). Without this a
    // cartridge that imports host_http would fail instantiation with a missing-
    // import LinkError. ABI mirrors src/rustlite/typecheck.rs http::*.
    let host_http = Object::new();
    let http_get = Closure::<dyn Fn(i32, i32) -> i32>::new(|_a, _b| -1);
    let http_one = Closure::<dyn Fn(i32) -> i32>::new(|_a| -1);
    let http_read = Closure::<dyn Fn(i32, i32, i32) -> i32>::new(|_a, _b, _c| -1);
    let http_parse = Closure::<dyn Fn(i32, i32, i32, i32) -> i32>::new(|_a, _b, _c, _d| 0);
    // body_lines / draw_line: inert here (the native loader has no real HTTP host);
    // the browser worker implements them over the held body. Stubs so a cartridge
    // importing them still instantiates natively. ABI mirrors typecheck.rs http::*.
    let http_lines = Closure::<dyn Fn(i32) -> i32>::new(|_a| 0);
    let http_draw = Closure::<dyn Fn(i32, i32, i32, i32, i32, i32) -> i32>::new(|_a, _b, _c, _d, _e, _f| 0);
    let _ = Reflect::set(&host_http, &JsValue::from_str("get"), http_get.as_ref());
    let _ = Reflect::set(&host_http, &JsValue::from_str("ready"), http_one.as_ref());
    let _ = Reflect::set(&host_http, &JsValue::from_str("status"), http_one.as_ref());
    let _ = Reflect::set(&host_http, &JsValue::from_str("body_len"), http_one.as_ref());
    let _ = Reflect::set(&host_http, &JsValue::from_str("read_body"), http_read.as_ref());
    let _ = Reflect::set(&host_http, &JsValue::from_str("parse_text"), http_parse.as_ref());
    let _ = Reflect::set(&host_http, &JsValue::from_str("body_lines"), http_lines.as_ref());
    let _ = Reflect::set(&host_http, &JsValue::from_str("draw_line"), http_draw.as_ref());
    http_get.forget();
    http_one.forget();
    http_read.forget();
    http_parse.forget();
    http_lines.forget();
    http_draw.forget();
    let _ = Reflect::set(&imports, &JsValue::from_str("host_http"), &host_http);

    // host_mp module — browser-to-browser MULTIPLAYER over a WebRTC data channel
    // (off-chain-signaled). The REAL impl is the Web Worker's host_mp +
    // src/app/display.rs (which wires the proven webrtc.rs Peer + the relay); this
    // bare loader (compile-check path) has no peer, so everything is inert:
    // open/connected/peer_count/event_* → 0, self_index → -1, join/set/send no-op,
    // get → 0. Without this a cartridge importing host_mp fails instantiation. ABI
    // mirrors src/rustlite/typecheck.rs mp::* AND web/cartridge-worker.js host_mp.
    let host_mp = Object::new();
    let mp_zero = Closure::<dyn Fn() -> i32>::new(|| 0);
    let mp_self = Closure::<dyn Fn() -> i32>::new(|| -1);
    let mp_void1 = Closure::<dyn Fn(i32)>::new(|_a| {});
    let mp_void2 = Closure::<dyn Fn(i32, i32)>::new(|_a, _b| {});
    let mp_get = Closure::<dyn Fn(i32, i32) -> i32>::new(|_a, _b| 0);
    let _ = Reflect::set(&host_mp, &JsValue::from_str("open"), mp_zero.as_ref());
    let _ = Reflect::set(&host_mp, &JsValue::from_str("join"), mp_void1.as_ref());
    let _ = Reflect::set(&host_mp, &JsValue::from_str("auto"), mp_void1.as_ref());
    let _ = Reflect::set(&host_mp, &JsValue::from_str("connected"), mp_zero.as_ref());
    let _ = Reflect::set(&host_mp, &JsValue::from_str("self_index"), mp_self.as_ref());
    let _ = Reflect::set(&host_mp, &JsValue::from_str("peer_count"), mp_zero.as_ref());
    let _ = Reflect::set(&host_mp, &JsValue::from_str("set"), mp_void2.as_ref());
    let _ = Reflect::set(&host_mp, &JsValue::from_str("get"), mp_get.as_ref());
    let _ = Reflect::set(&host_mp, &JsValue::from_str("send"), mp_void1.as_ref());
    let _ = Reflect::set(&host_mp, &JsValue::from_str("event_count"), mp_zero.as_ref());
    let _ = Reflect::set(&host_mp, &JsValue::from_str("event_next"), mp_zero.as_ref());
    mp_zero.forget();
    mp_self.forget();
    mp_void1.forget();
    mp_void2.forget();
    mp_get.forget();
    let _ = Reflect::set(&imports, &JsValue::from_str("host_mp"), &host_mp);

    // host_chat module — off-chain open CHATROOM (per-subdomain text message log).
    // The REAL impl is the Web Worker's host_chat + src/app/display.rs (holds the
    // received-line ring + compose buffer, polls /api/chat); this bare loader has no
    // relay, so it's inert (empty ring / no-op compose). Integer-only ABI — mirrors
    // typecheck.rs chat::* AND web/cartridge-worker.js host_chat.
    let host_chat = Object::new();
    let chat_poll = Closure::<dyn Fn() -> i32>::new(|| 0);
    let chat_line_count = Closure::<dyn Fn() -> i32>::new(|| 0);
    let chat_line_len = Closure::<dyn Fn(i32) -> i32>::new(|_a| 0);
    let chat_line_char = Closure::<dyn Fn(i32, i32) -> i32>::new(|_a, _b| -1);
    let chat_key = Closure::<dyn Fn(i32)>::new(|_a| {});
    let chat_backspace = Closure::<dyn Fn()>::new(|| {});
    let chat_compose_len = Closure::<dyn Fn() -> i32>::new(|| 0);
    let chat_compose_char = Closure::<dyn Fn(i32) -> i32>::new(|_a| -1);
    let chat_send = Closure::<dyn Fn() -> i32>::new(|| 0);
    let _ = Reflect::set(&host_chat, &JsValue::from_str("poll"), chat_poll.as_ref());
    let _ = Reflect::set(&host_chat, &JsValue::from_str("line_count"), chat_line_count.as_ref());
    let _ = Reflect::set(&host_chat, &JsValue::from_str("line_len"), chat_line_len.as_ref());
    let _ = Reflect::set(&host_chat, &JsValue::from_str("line_char"), chat_line_char.as_ref());
    let _ = Reflect::set(&host_chat, &JsValue::from_str("key"), chat_key.as_ref());
    let _ = Reflect::set(&host_chat, &JsValue::from_str("backspace"), chat_backspace.as_ref());
    let _ = Reflect::set(&host_chat, &JsValue::from_str("compose_len"), chat_compose_len.as_ref());
    let _ = Reflect::set(&host_chat, &JsValue::from_str("compose_char"), chat_compose_char.as_ref());
    let _ = Reflect::set(&host_chat, &JsValue::from_str("send"), chat_send.as_ref());
    chat_poll.forget();
    chat_line_count.forget();
    chat_line_len.forget();
    chat_line_char.forget();
    chat_key.forget();
    chat_backspace.forget();
    chat_compose_len.forget();
    chat_compose_char.forget();
    chat_send.forget();
    let _ = Reflect::set(&imports, &JsValue::from_str("host_chat"), &host_chat);

    // host_net module — WebSocket-backed multiplayer / sync I/O. Mirrors
    // host_display: integer-only host functions a rustlite cartridge calls,
    // strings passed as length-prefixed pointers into cartridge memory.
    let net = net::build_host_net(&imports, mem)?;

    Ok((imports, net))
}

/// Reject any WebSocket URL a cartridge must NOT open. A cartridge is
/// UNTRUSTED wasm (published by any agent / fetched on-chain) run in the
/// visitor's tab, so `open(url)` is an SSRF surface: without this gate it
/// could reach loopback / LAN / internal hosts from inside the victim's
/// network, or beacon to an arbitrary host. Policy: `wss://` only (no
/// cleartext `ws://`, which is also the loopback vector browsers don't
/// mixed-content-block), and the host must not be empty, an IP literal (in
/// ANY notation the WHATWG URL parser accepts — dotted-decimal, hex, octal,
/// or a bare 32-bit integer), `localhost`/`localhost.`/`*.localhost`, or a
/// `.local` mDNS name.
///
/// ⚠ This is HAND-PORTED to `web/cartridge-worker.js::urlIsAllowed` (the
/// browser cartridge host). Any change here MUST be mirrored there in
/// lockstep or the two hosts disagree on what a cartridge may reach.
//
// Lives at module level (not inside the wasm-only `net` module) so it is a
// pure, native-unit-testable core; `#[allow(dead_code)]` because the only
// caller (`net::build_host_net`) is wasm32-only, while the regression test
// and the JS-parity reasoning need it on native too.
#[allow(dead_code)]
fn url_is_allowed(url: &str) -> bool {
    let rest = match url
        .split_once("://")
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("wss"))
    {
        Some((_, rest)) => rest,
        None => return false,
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let hostport = authority.rsplit_once('@').map(|(_, h)| h).unwrap_or(authority);
    if hostport.starts_with('[') {
        return false; // IPv6 literal
    }
    let host = hostport.split(':').next().unwrap_or("");
    if host.is_empty() {
        return false;
    }
    let lower = host.to_ascii_lowercase();
    // WHATWG drops a single trailing dot before classifying the host, so
    // `localhost.` and `127.0.0.1.` are the loopback names the resolver sees.
    let normalized = lower.strip_suffix('.').unwrap_or(&lower);
    if normalized.is_empty() {
        return false;
    }
    if normalized == "localhost"
        || normalized.ends_with(".localhost")
        || normalized.ends_with(".local")
    {
        return false;
    }
    // Reject anything the WHATWG host parser would treat as an IPv4 address:
    // it parses the host as IPv4 iff its LAST label "is a number" — all
    // decimal digits (covers decimal & octal forms) OR a `0x`/`0X` hex
    // literal. This catches a bare single-number host (e.g. `2130706433` ==
    // 127.0.0.1) and hex-dotted forms (`0x7f.0.0.1`) the old per-octet
    // all-decimal-digits check missed.
    let last_label = normalized.rsplit('.').next().unwrap_or("");
    let looks_numeric = !last_label.is_empty()
        && (last_label.starts_with("0x")
            || last_label.bytes().all(|b| b.is_ascii_digit()));
    if looks_numeric {
        return false;
    }
    // Require a dotted name — a bare single-label host is an internal name.
    normalized.contains('.')
}

/// WebSocket-backed networking imports for cartridges (`host_net`).
///
/// A cartridge is a sandbox: it has linear memory + the host imports we
/// grant it, and no DOM. This module grants it a *poll-model* WebSocket —
/// `open` returns an integer handle, `send`/`poll` move length-prefixed
/// strings through cartridge memory, and the cartridge drains its inbox
/// each frame. That's enough to build multi-device sync and multiplayer
/// apps without ever touching the DOM or the network stack directly.
#[cfg(target_arch = "wasm32")]
mod net {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    use js_sys::{Object, Reflect};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;
    use web_sys::{MessageEvent, WebSocket};

    use crate::rustlite::CompileError;

    use super::{url_is_allowed, SharedMemory};

    /// One open socket: the live `WebSocket` plus a bounded inbox of
    /// received text messages the cartridge has not yet polled.
    struct Socket {
        ws: WebSocket,
        inbox: Rc<RefCell<VecDeque<String>>>,
        // Keeps the `onmessage` closure alive for the socket's lifetime.
        _on_message: Closure<dyn FnMut(MessageEvent)>,
    }

    /// Handle-indexed socket table. A handle is an index into `Vec`;
    /// closed sockets become `None` so handles never alias.
    type SocketTable = Rc<RefCell<Vec<Option<Socket>>>>;

    /// Keeps the `host_net` import closures + the socket table alive for
    /// the cartridge's lifetime. wasm holds JS references into the closures.
    #[allow(dead_code)]
    pub(crate) struct NetRuntime {
        sockets: SocketTable,
        open: Closure<dyn FnMut(i32) -> i32>,
        send: Closure<dyn FnMut(i32, i32) -> i32>,
        poll: Closure<dyn FnMut(i32, i32, i32) -> i32>,
        status: Closure<dyn FnMut(i32) -> i32>,
        close: Closure<dyn FnMut(i32)>,
    }

    /// Cap the inbox so a chatty peer can't grow memory unbounded; oldest
    /// messages are dropped first.
    const MAX_INBOX: usize = 256;

    /// Cap live sockets per cartridge. A `frame` loop calling `open` every
    /// tick would otherwise flood connections (fd exhaustion / connection-
    /// flood amplifier). Once at the cap, `open` refuses until one is closed.
    const MAX_SOCKETS: usize = 8;

    /// Build the `host_net` import object and return the runtime that owns
    /// the closures + socket table (must outlive the wasm instance).
    pub(crate) fn build_host_net(
        imports: &Object,
        mem: &SharedMemory,
    ) -> Result<NetRuntime, CompileError> {
        let sockets: SocketTable = Rc::new(RefCell::new(Vec::new()));

        let open = {
            let sockets = sockets.clone();
            let mem = mem.clone();
            Closure::<dyn FnMut(i32) -> i32>::new(move |url_ptr: i32| {
                let url = match read_string(&mem.borrow(), url_ptr) {
                    Some(u) => u,
                    None => return -1,
                };
                // SSRF/abuse gate — only public `wss://` hosts.
                if !url_is_allowed(&url) {
                    return -1;
                }
                // Connection cap; reuse a freed slot so handles stay bounded.
                let free_slot = {
                    let table = sockets.borrow();
                    if table.iter().filter(|s| s.is_some()).count() >= MAX_SOCKETS {
                        return -1;
                    }
                    table.iter().position(|s| s.is_none())
                };
                let ws = match WebSocket::new(&url) {
                    Ok(ws) => ws,
                    Err(_) => return -1,
                };
                ws.set_binary_type(web_sys::BinaryType::Arraybuffer);

                let inbox: Rc<RefCell<VecDeque<String>>> =
                    Rc::new(RefCell::new(VecDeque::new()));
                let on_message = {
                    let inbox = inbox.clone();
                    Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
                        if let Some(text) = e.data().as_string() {
                            let mut q = inbox.borrow_mut();
                            if q.len() >= MAX_INBOX {
                                q.pop_front();
                            }
                            q.push_back(text);
                        }
                    })
                };
                ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));

                let socket = Socket { ws, inbox, _on_message: on_message };
                let mut table = sockets.borrow_mut();
                match free_slot {
                    Some(i) => {
                        table[i] = Some(socket);
                        i as i32
                    }
                    None => {
                        let handle = table.len() as i32;
                        table.push(Some(socket));
                        handle
                    }
                }
            })
        };

        let send = {
            let sockets = sockets.clone();
            let mem = mem.clone();
            Closure::<dyn FnMut(i32, i32) -> i32>::new(move |handle: i32, ptr: i32| {
                let msg = match read_string(&mem.borrow(), ptr) {
                    Some(m) => m,
                    None => return 0,
                };
                let table = sockets.borrow();
                match table.get(handle as usize).and_then(|s| s.as_ref()) {
                    Some(sock) => match sock.ws.send_with_str(&msg) {
                        Ok(()) => 1,
                        Err(_) => 0,
                    },
                    None => 0,
                }
            })
        };

        let poll = {
            let sockets = sockets.clone();
            let mem = mem.clone();
            Closure::<dyn FnMut(i32, i32, i32) -> i32>::new(
                move |handle: i32, out_ptr: i32, max: i32| {
                    let table = sockets.borrow();
                    let sock = match table.get(handle as usize).and_then(|s| s.as_ref()) {
                        Some(s) => s,
                        None => return -1,
                    };
                    let msg = match sock.inbox.borrow_mut().pop_front() {
                        Some(m) => m,
                        None => return 0,
                    };
                    write_string(&mem.borrow(), out_ptr, &msg, max.max(0) as usize)
                },
            )
        };

        let status = {
            let sockets = sockets.clone();
            Closure::<dyn FnMut(i32) -> i32>::new(move |handle: i32| {
                let table = sockets.borrow();
                match table.get(handle as usize).and_then(|s| s.as_ref()) {
                    // web_sys ready_state: 0 CONNECTING, 1 OPEN, 2 CLOSING, 3 CLOSED.
                    Some(sock) => sock.ws.ready_state() as i32,
                    None => -1,
                }
            })
        };

        let close = {
            let sockets = sockets.clone();
            Closure::<dyn FnMut(i32)>::new(move |handle: i32| {
                let mut table = sockets.borrow_mut();
                if let Some(slot) = table.get_mut(handle as usize) {
                    if let Some(sock) = slot.take() {
                        let _ = sock.ws.close();
                    }
                }
            })
        };

        let host_net = Object::new();
        set_fn(&host_net, "open", open.as_ref())?;
        set_fn(&host_net, "send", send.as_ref())?;
        set_fn(&host_net, "poll", poll.as_ref())?;
        set_fn(&host_net, "status", status.as_ref())?;
        set_fn(&host_net, "close", close.as_ref())?;
        Reflect::set(imports, &JsValue::from_str("host_net"), &host_net)
            .map_err(|_| CompileError::new("failed to set host_net import"))?;

        Ok(NetRuntime { sockets, open, send, poll, status, close })
    }

    fn set_fn(obj: &Object, name: &str, f: &JsValue) -> Result<(), CompileError> {
        Reflect::set(obj, &JsValue::from_str(name), f)
            .map(|_| ())
            .map_err(|_| CompileError::new(format!("failed to set host_net.{name}")))
    }

    /// Read a length-prefixed UTF-8 string from cartridge memory at `ptr`
    /// (4 bytes LE length, then payload) — same layout as the loader's
    /// `read_string`. Returns `None` on a missing memory or bad length.
    fn read_string(memory: &JsValue, ptr: i32) -> Option<String> {
        if ptr < 0 || memory.is_null() {
            return None;
        }
        let buffer = Reflect::get(memory, &JsValue::from_str("buffer")).ok()?;
        let array = js_sys::Uint8Array::new(&buffer);
        let cap = array.length() as u64;
        let ptr = ptr as u64;
        // Bound the read region against the cartridge's own memory (an OOB
        // Uint8Array read yields 0 in JS — never host memory — but we check so
        // the read is well-defined and the `u32` adds below can't wrap).
        if ptr + 4 > cap {
            return None;
        }
        let mut len_bytes = [0u8; 4];
        for (i, b) in len_bytes.iter_mut().enumerate() {
            *b = array.get_index(ptr as u32 + i as u32);
        }
        let len = u32::from_le_bytes(len_bytes) as u64;
        if len > 65536 || ptr + 4 + len > cap {
            return None;
        }
        let mut bytes = vec![0u8; len as usize];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = array.get_index(ptr as u32 + 4 + i as u32);
        }
        String::from_utf8(bytes).ok()
    }

    /// Write `s` into cartridge memory at `out_ptr` as a length-prefixed
    /// UTF-8 string (4 bytes LE length, then payload), truncating the
    /// payload to `max` bytes (on a UTF-8 char boundary). Returns the
    /// payload byte length written, or -1 on a missing memory.
    fn write_string(memory: &JsValue, out_ptr: i32, s: &str, max: usize) -> i32 {
        if out_ptr < 0 || memory.is_null() {
            return -1;
        }
        let buffer = match Reflect::get(memory, &JsValue::from_str("buffer")) {
            Ok(b) => b,
            Err(_) => return -1,
        };
        let array = js_sys::Uint8Array::new(&buffer);
        let cap = array.length() as u64;
        let ptr = out_ptr as u64;

        // Truncate to `max` bytes without splitting a UTF-8 codepoint.
        let mut end = s.len().min(max);
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        let bytes = &s.as_bytes()[..end];
        let len = bytes.len() as u32;
        // The full write region must fit the cartridge's own memory (an OOB
        // `set_index` is a JS no-op — never reaches host memory — but check so
        // a partial write can't land and the `u32` adds can't wrap).
        if ptr + 4 + len as u64 > cap {
            return -1;
        }
        let ptr = ptr as u32;
        for (i, b) in len.to_le_bytes().iter().enumerate() {
            array.set_index(ptr + i as u32, *b);
        }
        for (i, b) in bytes.iter().enumerate() {
            array.set_index(ptr + 4 + i as u32, *b);
        }
        len as i32
    }
}

#[cfg(target_arch = "wasm32")]
fn list_exports(instance: &wasm_bindgen::JsValue) -> Vec<String> {
    use js_sys::Reflect;
    use wasm_bindgen::JsValue;

    let exports = match Reflect::get(instance, &JsValue::from_str("exports")) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let keys = js_sys::Object::keys(&js_sys::Object::from(exports));
    let mut names = Vec::new();
    for i in 0..keys.length() {
        if let Some(key) = keys.get(i).as_string() {
            if key != "memory" {
                names.push(key);
            }
        }
    }
    names
}

#[cfg(target_arch = "wasm32")]
fn call_export_i32(
    instance: &wasm_bindgen::JsValue,
    name: &str,
    args: &[i32],
) -> Result<i32, CompileError> {
    use js_sys::Reflect;
    use wasm_bindgen::JsCast;
    use wasm_bindgen::JsValue;

    let exports = Reflect::get(instance, &JsValue::from_str("exports"))
        .map_err(|_| CompileError::new("no exports"))?;
    let func = Reflect::get(&exports, &JsValue::from_str(name))
        .map_err(|_| CompileError::new(format!("export '{name}' not found")))?;
    let func: js_sys::Function = func
        .dyn_into()
        .map_err(|_| CompileError::new(format!("'{name}' is not a function")))?;

    let js_args = js_sys::Array::new();
    for &arg in args {
        js_args.push(&JsValue::from(arg));
    }

    let result = func
        .apply(&JsValue::NULL, &js_args)
        .map_err(|e| CompileError::new(format!("call failed: {e:?}")))?;

    result
        .as_f64()
        .map(|v| v as i32)
        .ok_or_else(|| CompileError::new("function did not return a number"))
}

#[cfg(target_arch = "wasm32")]
fn read_string_from_memory(
    memory: &wasm_bindgen::JsValue,
    ptr: i32,
) -> Result<String, CompileError> {
    use js_sys::Reflect;
    use wasm_bindgen::JsValue;

    let buffer = Reflect::get(memory, &JsValue::from_str("buffer"))
        .map_err(|_| CompileError::new("no memory buffer"))?;
    let array = js_sys::Uint8Array::new(&buffer);

    let ptr = ptr as u32;
    let mut len_bytes = [0u8; 4];
    for (i, b) in len_bytes.iter_mut().enumerate() {
        *b = array.get_index(ptr + i as u32) as u8;
    }
    let len = u32::from_le_bytes(len_bytes);
    if len > 65536 {
        return Err(CompileError::new(format!("string too long: {len}")));
    }

    let mut bytes = vec![0u8; len as usize];
    for i in 0..len {
        bytes[i as usize] = array.get_index(ptr + 4 + i) as u8;
    }

    String::from_utf8(bytes)
        .map_err(|e| CompileError::new(format!("invalid utf-8: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cartridge_exports_list() {
        // On native, load returns an error — just verify the API compiles
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(Cartridge::load(&[]));
        assert!(result.is_err());
    }

    #[test]
    fn ssrf_gate_blocks_loopback_and_ip_notations() {
        // Loopback in every notation the WHATWG host parser accepts.
        assert!(!url_is_allowed("wss://127.0.0.1/"));
        assert!(!url_is_allowed("wss://0x7f.0.0.1/")); // hex-dotted (L19 bypass)
        assert!(!url_is_allowed("wss://0x7f000001/")); // bare hex
        assert!(!url_is_allowed("wss://2130706433/")); // bare decimal
        assert!(!url_is_allowed("wss://localhost/"));
        assert!(!url_is_allowed("wss://localhost./")); // trailing dot (L19 bypass)
        assert!(!url_is_allowed("wss://api.localhost/"));
        assert!(!url_is_allowed("wss://printer.local/"));
        // Non-wss / cleartext / IPv6 / empty / bare host are all rejected.
        assert!(!url_is_allowed("ws://example.com/"));
        assert!(!url_is_allowed("wss://[::1]/"));
        assert!(!url_is_allowed("wss:///path"));
        assert!(!url_is_allowed("wss://intranet/"));
        // A normal public host is still allowed.
        assert!(url_is_allowed("wss://relay.example.com/"));
        assert!(url_is_allowed("wss://relay.example.com:443/path"));
    }
}
