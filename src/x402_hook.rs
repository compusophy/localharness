//! App-injected x402 signing hook.
//!
//! `call_agent` (the inter-agent RPC tool) lives in the backend layer,
//! but signing an x402 payment needs the wallet, which lives in the app
//! layer. To avoid a backend→app dependency, the app installs a signer
//! closure here at mount, and `call_agent` invokes it when a callee
//! demands payment. Single-threaded (wasm) — `Rc` + a local future.

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
