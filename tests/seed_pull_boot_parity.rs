//! Parity guard for the seed-pull fast bounce in `web/boot.js`.
//!
//! boot.js short-circuits the apex `?seed_export=1` leg for a browser with NO
//! apex identity: a definitive OPFS miss of `.lh_wallet` → `history.back()`
//! before the wasm loads (the visitor's already-painted face restores from
//! bfcache with zero repaint). That JS hand-copies TWO facts from Rust and
//! this test reddens if either drifts:
//!
//! 1. `.lh_wallet` is the exact file `wallet_store.rs` reads (a rename would
//!    make the fast path bounce REAL OWNERS and silently break mobile seed
//!    adoption — the one semantic this fix must never touch);
//! 2. the shortcut stays gated on the definitive `NotFoundError` + a
//!    `history.length` check, with `history.back()` as the bounce.

use std::path::Path;

#[test]
fn boot_js_fast_bounce_matches_wallet_store() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let boot = std::fs::read_to_string(root.join("web").join("boot.js")).expect("read web/boot.js");
    let store = std::fs::read_to_string(root.join("src").join("app").join("wallet_store.rs"))
        .expect("read src/app/wallet_store.rs");

    // Rust side: the wallet file constant the fast path mirrors.
    assert!(
        store.contains("const WALLET_FILE: &str = \".lh_wallet\";"),
        "wallet_store.rs no longer pins WALLET_FILE = \".lh_wallet\" — update the \
         boot.js seed-pull fast bounce (and this guard) IN THE SAME COMMIT, or the \
         fast path will history.back() real owners and break mobile seed adoption."
    );

    // JS side: the fast path exists and keeps its safety gates.
    for (needle, why) in [
        ("lhSeedExportFastBounce", "the fast-bounce function"),
        ("seed_export", "the ?seed_export=1 gate"),
        (".lh_wallet", "the exact wallet_store.rs OPFS filename"),
        ("NotFoundError", "the definitive-miss-only guard"),
        ("history.length > 1", "the nothing-behind-us fallback gate"),
        ("history.back()", "the bfcache bounce itself"),
    ] {
        assert!(
            boot.contains(needle),
            "web/boot.js lost `{needle}` ({why}) — the seed-pull fast bounce must keep \
             its exact gates or be removed together with this guard."
        );
    }
}
