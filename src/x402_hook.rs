//! App-injected x402 hooks.
//!
//! `call_agent` (the inter-agent RPC tool) lives in the backend layer,
//! but signing an x402 payment — and routing a paid call through the
//! hosted proxy — needs the wallet, which lives in the app layer. To
//! avoid a backend→app dependency, the app installs closures here at
//! mount, and `call_agent` invokes them. Single-threaded (wasm) — `Rc`
//! + local futures.

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

/// A fully caller-decided authorization to sign. The CALLER (not the
/// callee) sets every field — `to` is verified against the agent's
/// on-chain payee, `value` is capped, and the window + nonce are the
/// caller's own — so the hook just signs what it's given.
pub struct X402Challenge {
    pub to: [u8; 20],
    pub value_wei: u128,
    pub valid_after: u64,
    pub valid_before: u64,
    pub nonce: [u8; 32],
}

/// The signed result: the payer address + the 65-byte signature. (The
/// caller already holds the window/nonce it chose.)
pub struct X402Payment {
    pub from: [u8; 20],
    pub signature: [u8; 65],
}

type SignerFut = Pin<Box<dyn Future<Output = Result<X402Payment, String>>>>;
type Signer = Rc<dyn Fn(X402Challenge) -> SignerFut>;

thread_local! {
    static SIGNER: RefCell<Option<Signer>> = const { RefCell::new(None) };
}

/// Install the app's x402 signer (once, at mount).
pub fn install(signer: Signer) {
    SIGNER.with(|s| *s.borrow_mut() = Some(signer));
}

/// Sign an x402 challenge via the installed signer. Errors if the app
/// never installed one (e.g. native builds, or no identity).
pub async fn sign(challenge: X402Challenge) -> Result<X402Payment, String> {
    let signer = SIGNER.with(|s| s.borrow().clone());
    match signer {
        Some(f) => f(challenge).await,
        None => Err("no x402 signer installed".into()),
    }
}

// --- remote (proxy-mediated) paid agent call --------------------------------
//
// The `?rpc=1` iframe path only reaches agents with state on THIS machine
// (OPFS is per-origin but per-DEVICE). For a foreign agent, the app installs
// this route: an x402-paid `ask_agent` call to the hosted MCP endpoint —
// the caller's $LH pays the target's TBA, the proxy settles on-chain and
// answers under the target's published persona.

type RemoteFut = Pin<Box<dyn Future<Output = Result<String, String>>>>;
type RemoteCall = Rc<dyn Fn(String, String) -> RemoteFut>;

thread_local! {
    static REMOTE: RefCell<Option<RemoteCall>> = const { RefCell::new(None) };
}

/// Install the app's proxy-mediated agent-call route (once, at mount).
pub fn install_remote_call(route: RemoteCall) {
    REMOTE.with(|r| *r.borrow_mut() = Some(route));
}

/// Call `target` through the installed proxy route (caller pays in `$LH`).
/// Errors if the app never installed one (e.g. native builds).
pub async fn remote_call(target: &str, message: &str) -> Result<String, String> {
    let route = REMOTE.with(|r| r.borrow().clone());
    match route {
        Some(f) => f(target.to_string(), message.to_string()).await,
        None => Err("no remote agent route installed".into()),
    }
}
