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
