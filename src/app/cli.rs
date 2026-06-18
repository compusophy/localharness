//! CLI SANDBOX — run a compiled wasm "CLI" program under a WASI-SUBSET host
//! and capture its TEXT output (on-chain feedback #6, the extensibility POC).
//!
//! This is the text counterpart of [`crate::app::display`]: where `display`
//! runs a `host_display` framebuffer cartridge, this runs a `wasi_snapshot_
//! preview1` COMMAND (a module exporting `_start`) and surfaces its stdout /
//! stderr / exit code as monochrome terminal text.
//!
//! ## Model: untrusted wasm in a sibling worker
//! Like the cartridge runtime, the module runs OFF the main thread in a Web
//! Worker (`web/wasi-worker.js`) so a hung/unbounded `_start` can only block
//! the worker — the main-thread [`WATCHDOG_MS`] timeout terminates it. The
//! worker implements the WASI subset (`fd_write` → captured text, `proc_exit`,
//! `args_*`, `environ_*`, `fd_read` = EOF, `clock_time_get`, `random_get`, plus
//! defined-errno stubs for the wider surface). The single source of truth for
//! the host is the worker JS; this module is just the spawn + await + paint
//! half, mirroring `display::mod worker`.
//!
//! ## What this is NOT (the honest boundary)
//! A WASI-subset stdout sandbox — NOT a real filesystem, network, or x86 PC.
//! `path_open` returns NOTCAPABLE, there are no preopened dirs, stdin is empty.
//! A full Linux/x86 machine (v86/WebVM) needs iframes + multi-MB blobs, which
//! violate this project's no-iframe + framebuffer design rules.

use std::cell::RefCell;

use js_sys::{Array, Object, Reflect, Uint8Array};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{MessageEvent, Worker};

use super::{dom, templates};

/// How long the main thread waits for the worker to post a `done`/`error`
/// before terminating it as hung. WASI commands here are bounded little
/// programs; 4s is generous for one that prints + exits, short enough that a
/// runaway loop is killed promptly. The worker's `_start` is synchronous, so
/// "kill it" (terminate) is the only containment, exactly as for cartridges.
const WATCHDOG_MS: i32 = 4000;

/// Default + hard cap on captured output per stream (bytes), mirrored in the
/// worker. Keeps a runaway printer from ballooning the postMessage payload.
const MAX_OUTPUT_BYTES: u32 = 256 * 1024;

/// The structured outcome of one CLI run — what [`run_wasm_cli`] returns to the
/// tool and paints into the terminal surface.
pub(crate) struct CliRun {
    /// The program's process exit code (`proc_exit`, or 0 if `_start` returned).
    pub exit_code: i32,
    /// Captured stdout as UTF-8 text.
    pub stdout: String,
    /// Captured stderr as UTF-8 text.
    pub stderr: String,
    /// True if either stream hit the output cap and was cut.
    pub truncated: bool,
}

/// A run that never produced output — the worker's instantiate/trap error, or a
/// watchdog timeout. The human-readable reason for the tool's error result.
pub(crate) struct CliFailure {
    pub detail: String,
}

thread_local! {
    /// The live CLI worker + its kept-alive onmessage closure + watchdog id, so
    /// they outlive the spawning call. Replaced/cleared per run. ONE at a time
    /// (a CLI run is short and modal; no concurrency need).
    static CLI: RefCell<Option<CliHandle>> = const { RefCell::new(None) };
}

struct CliHandle {
    worker: Worker,
    _onmessage: Closure<dyn FnMut(MessageEvent)>,
    watchdog: Option<i32>,
}

impl Drop for CliHandle {
    fn drop(&mut self) {
        if let Some(id) = self.watchdog.take() {
            if let Ok(win) = dom::window() {
                win.clear_interval_with_handle(id);
            }
        }
        self.worker.terminate();
    }
}

/// Terminate + drop any live CLI worker (idempotent).
fn stop_worker() {
    CLI.with(|c| *c.borrow_mut() = None);
}

/// Run `wasm_bytes` as a WASI command with `args`, awaiting its captured
/// output. Spawns the sibling worker, posts the module + argv, and races a
/// `done`/`error` message against the [`WATCHDOG_MS`] timeout (a hung `_start`
/// stops posting → the watchdog terminates the worker and reports the timeout).
///
/// `Ok(CliRun)` once the program exits (any exit code — a nonzero exit is a
/// successful RUN that the tool reports, not a tool error); `Err(CliFailure)`
/// for an instantiate failure, a trap, or a watchdog kill.
pub(crate) async fn run_wasm_cli(
    wasm_bytes: &[u8],
    args: &[String],
) -> Result<CliRun, CliFailure> {
    stop_worker();

    // Shared outcome slot the onmessage + watchdog closures write; the await
    // loop below polls it. `None` = still running.
    let outcome: std::rc::Rc<RefCell<Option<Result<CliRun, CliFailure>>>> =
        std::rc::Rc::new(RefCell::new(None));

    let worker = Worker::new("/wasi-worker.js")
        .map_err(|e| CliFailure { detail: format!("worker spawn failed: {e:?}") })?;

    let onmessage = {
        let outcome = outcome.clone();
        Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
            let data = e.data();
            let ty = Reflect::get(&data, &JsValue::from_str("type"))
                .ok()
                .and_then(|v| v.as_string())
                .unwrap_or_default();
            // First signal wins; ignore anything after.
            if outcome.borrow().is_some() {
                return;
            }
            match ty.as_str() {
                "done" => {
                    let getf = |k: &str| {
                        Reflect::get(&data, &JsValue::from_str(k)).ok().and_then(|v| v.as_f64())
                    };
                    let gets = |k: &str| {
                        Reflect::get(&data, &JsValue::from_str(k))
                            .ok()
                            .and_then(|v| v.as_string())
                            .unwrap_or_default()
                    };
                    let getb = |k: &str| {
                        Reflect::get(&data, &JsValue::from_str(k))
                            .ok()
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false)
                    };
                    *outcome.borrow_mut() = Some(Ok(CliRun {
                        exit_code: getf("exitCode").unwrap_or(0.0) as i32,
                        stdout: gets("stdout"),
                        stderr: gets("stderr"),
                        truncated: getb("truncated"),
                    }));
                }
                "error" => {
                    let detail = Reflect::get(&data, &JsValue::from_str("detail"))
                        .ok()
                        .and_then(|v| v.as_string())
                        .unwrap_or_else(|| "unknown worker error".into());
                    *outcome.borrow_mut() = Some(Err(CliFailure { detail }));
                }
                _ => {}
            }
        })
    };
    worker.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));

    // Post the run: transfer the wasm ArrayBuffer (zero-copy; the worker copies
    // it into a Module). argv is a JS string array.
    let arr = Uint8Array::from(wasm_bytes);
    let buf = arr.buffer();
    let argv = Array::new();
    for a in args {
        argv.push(&JsValue::from_str(a));
    }
    let msg = Object::new();
    let _ = Reflect::set(&msg, &JsValue::from_str("type"), &JsValue::from_str("run"));
    let _ = Reflect::set(&msg, &JsValue::from_str("wasm"), &buf);
    let _ = Reflect::set(&msg, &JsValue::from_str("args"), &argv);
    let _ = Reflect::set(
        &msg,
        &JsValue::from_str("maxOutput"),
        &JsValue::from_f64(MAX_OUTPUT_BYTES as f64),
    );
    let transfer = Array::new();
    transfer.push(&buf);
    if let Err(e) = worker.post_message_with_transfer(&msg, &transfer) {
        return Err(CliFailure { detail: format!("worker post failed: {e:?}") });
    }

    // Stash the handle so the closure stays alive while we await.
    CLI.with(|c| {
        *c.borrow_mut() = Some(CliHandle { worker, _onmessage: onmessage, watchdog: None });
    });

    // Poll the outcome up to the watchdog window. Sleeping in short chunks
    // yields to the event loop so the worker's onmessage can fire.
    let mut waited = 0i32;
    loop {
        if let Some(result) = outcome.borrow_mut().take() {
            stop_worker();
            return result;
        }
        if waited >= WATCHDOG_MS {
            stop_worker(); // terminate the hung worker
            return Err(CliFailure {
                detail: format!(
                    "the program did not finish within {}ms — terminated (likely an \
                     unbounded loop; bound your work or print + exit promptly)",
                    WATCHDOG_MS
                ),
            });
        }
        crate::runtime::sleep_ms(50).await;
        waited += 50;
    }
}

/// Open the terminal overlay (fullscreen, dismissable) showing one CLI run's
/// argv + stdout + stderr + exit code. Mirrors `opfs::toggle_display` /
/// `display::mount_canvas` — a `swap_outer` over the `#terminal-overlay` shell.
pub(crate) fn show_terminal(argv: &str, run: &CliRun) {
    dom::remember_focus();
    dom::swap_outer(
        "terminal-overlay",
        &templates::terminal_overlay(argv, run).into_string(),
    );
    dom::focus_first_in("terminal-overlay");
}

/// `Action::ToggleTerminal` (terminal card [show] / overlay ×): tear down the
/// overlay if open, else re-open the LAST run from its stashed snapshot. The
/// snapshot lets the [show] button on an old transcript card re-open that run.
pub(crate) fn toggle_terminal() {
    if dom::by_id("terminal-surface").is_some() {
        close_terminal();
    } else if let Some((argv, run)) = LAST_RUN.with(|c| c.borrow().clone()) {
        show_terminal(&argv, &run);
    }
}

/// Dismiss the terminal overlay.
pub(crate) fn close_terminal() {
    dom::swap_outer(
        "terminal-overlay",
        &templates::terminal_overlay_closed().into_string(),
    );
    dom::restore_focus();
}

thread_local! {
    /// The most recent CLI run (argv line + outcome), so the overlay can be
    /// re-opened from a transcript card's [show] after the run completed.
    /// One slot — the latest run wins, like the single-worker display path.
    static LAST_RUN: RefCell<Option<(String, CliRun)>> = const { RefCell::new(None) };
}

impl Clone for CliRun {
    fn clone(&self) -> Self {
        CliRun {
            exit_code: self.exit_code,
            stdout: self.stdout.clone(),
            stderr: self.stderr.clone(),
            truncated: self.truncated,
        }
    }
}

/// Remember the latest run so the overlay re-opens from a card's [show].
pub(crate) fn remember_run(argv: &str, run: &CliRun) {
    LAST_RUN.with(|c| *c.borrow_mut() = Some((argv.to_string(), run.clone())));
}
