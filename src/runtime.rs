//! Per-target async runtime helpers.
//!
//! On native, this delegates to tokio's multi-threaded runtime. On
//! wasm32 the only available scheduler is the browser event loop, which
//! `wasm_bindgen_futures::spawn_local` drives. The two APIs differ on
//! `Send` bounds — tokio requires futures be `Send`; the wasm scheduler
//! is single-threaded so it does not — so each platform gets its own
//! signature.
//!
//! Use `crate::runtime::spawn` instead of `tokio::spawn` anywhere the
//! call site needs to compile on both targets.

use std::future::Future;

/// A marker that is `Send + Sync` on native and a no-op on wasm32.
///
/// Use as a supertrait — `pub trait Tool: MaybeSendSync { ... }` — so
/// concrete implementations whose internals (e.g., reqwest's browser
/// fetch client) aren't `Send` can still satisfy the bound on wasm.
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSendSync: Send + Sync {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: ?Sized + Send + Sync> MaybeSendSync for T {}

#[cfg(target_arch = "wasm32")]
pub trait MaybeSendSync {}
#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> MaybeSendSync for T {}

/// Spawn a fire-and-forget future onto the platform's async runtime.
///
/// The future's output is discarded; there is no `JoinHandle` because
/// wasm_bindgen_futures doesn't return one. If you need to await the
/// result, await the future directly instead of spawning it.
#[cfg(not(target_arch = "wasm32"))]
pub fn spawn<F>(f: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    tokio::spawn(f);
}

#[cfg(target_arch = "wasm32")]
pub fn spawn<F>(f: F)
where
    F: Future<Output = ()> + 'static,
{
    wasm_bindgen_futures::spawn_local(f);
}

/// Current UNIX time in whole seconds, on either target. Native: `SystemTime`.
/// wasm32: `js_sys::Date::now()`. Used to stamp freshness-windowed personal-sign
/// auth tokens (`registry::proxy_auth_token`) from code that runs on BOTH targets
/// (e.g. bashlite `lh-publish`, which is the CLI `sh` AND browser `execute_script`).
// Gated on `wallet`: the ONLY caller is `bashlite::platform` (the `lh-*`
// platform reads/writes, feature = wallet). The default (no-wallet) build never
// references it, so compiling it there is dead code under clippy `-D warnings`.
#[cfg(all(not(target_arch = "wasm32"), feature = "wallet"))]
pub fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(all(target_arch = "wasm32", feature = "wallet"))]
pub fn now_unix_secs() -> u64 {
    (js_sys::Date::now() / 1000.0) as u64
}

/// Sleep for `ms` milliseconds on either target.
///
/// Native: `tokio::time::sleep`. wasm32: a `setTimeout`-backed Promise on the
/// browser event loop. The wasm flavor requires a `window` — inside a Web
/// Worker the promise never resolves (identical to the historical per-module
/// copies this canonicalizes; workers schedule via their own runtime).
#[cfg(not(target_arch = "wasm32"))]
pub async fn sleep_ms(ms: u32) {
    tokio::time::sleep(std::time::Duration::from_millis(ms as u64)).await;
}

#[cfg(target_arch = "wasm32")]
pub async fn sleep_ms(ms: u32) {
    use wasm_bindgen_futures::JsFuture;
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(window) = web_sys::window() {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
                &resolve,
                ms as i32,
            );
        }
    });
    let _ = JsFuture::from(promise).await;
}
