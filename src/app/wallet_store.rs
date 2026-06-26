//! Master wallet persistence — one seed per ORIGIN.
//!
//! The per-origin OPFS sandbox scopes the `.lh_wallet` file: the apex
//! origin always holds the seed, and a subdomain holds its OWN copy
//! after pulling it in via [`super::seed_pull`] (the local-seed-per-origin
//! model that keeps mobile working — cross-origin iframe storage is
//! partitioned there, so seed ops must run LOCAL-FIRST off this store,
//! never through an iframe-only path).
//!
//! Storage format is a single line of 12 BIP-39 words. **Plain text, by
//! design, forever**: the seed is the root every at-rest key derives
//! from — sealing it under a key derived from itself bricks the identity
//! (the 2026-06-05 reset-brick class of bug). It is on
//! [`crate::filesystem::encrypted::EXEMPT_FILES`], and every function
//! here that yields a `MasterWallet` also installs the seed-keyed
//! at-rest encryption layer over the shared OPFS handle (see
//! [`install_at_rest`]), so the REST of OPFS stops being plaintext the
//! moment an identity exists.

use crate::wallet;

const WALLET_FILE: &str = ".lh_wallet";

/// Derive the at-rest OPFS key from this wallet's BIP-39 entropy (tag
/// `localharness/v0/opfs-at-rest`, pinned in `crate::wallet`) and install
/// the [`crate::filesystem::EncryptedFilesystem`] wrapper. A no-op when the
/// SAME seed is re-derived; a DIFFERENT seed (in-tab [`import`]) REWRAPS the
/// layer so later writes seal under the new key (see
/// [`super::install_at_rest_encryption`]).
fn install_at_rest(mnemonic: &bip39::Mnemonic) {
    use zeroize::Zeroize as _;
    let mut entropy = mnemonic.to_entropy();
    let key = wallet::at_rest_key_from_entropy(&entropy);
    entropy.zeroize();
    super::install_at_rest_encryption(key);
}

pub(crate) struct MasterWallet {
    pub(crate) mnemonic: bip39::Mnemonic,
    /// Signs owner proofs, sponsored Tempo txs, and key seal/open —
    /// local-first off `APP.wallet` (see `verify.rs` / `signer.rs`).
    pub(crate) signer: k256::ecdsa::SigningKey,
    pub(crate) address: [u8; 20],
}

impl MasterWallet {
    pub(crate) fn address_hex(&self) -> String {
        crate::encoding::bytes_to_hex_str(&self.address)
    }
}

/// Load the master wallet for this device if one exists. Returns
/// `None` on a fresh device — never generates a wallet implicitly.
/// Wallet creation must come from an explicit user action via
/// [`create_and_persist`] or [`import`].
pub(crate) async fn load() -> Option<MasterWallet> {
    let fs = super::shared_opfs();
    let bytes = fs.read(WALLET_FILE).await.ok()?;
    if bytes.is_empty() {
        return None;
    }
    let phrase = String::from_utf8(bytes).ok()?;
    let w = restore_from_phrase(&phrase).ok()?;
    install_at_rest(&w.mnemonic);
    Some(w)
}

/// Generate a fresh keypair, persist its mnemonic to OPFS, and return
/// the wallet. Caller is responsible for confirming intent — this
/// overwrites any existing wallet file at the apex origin.
pub(crate) async fn create_and_persist() -> Result<MasterWallet, String> {
    super::debuglog::log("create wallet: generating mnemonic");
    let fs = super::shared_opfs();
    let (mnemonic, signer) = wallet::generate_with_mnemonic();
    super::debuglog::log("create wallet: writing seed (opfs write)");
    fs.write_atomic(WALLET_FILE, mnemonic.to_string().as_bytes())
        .await
        .map_err(|e| format!("wallet save: {e}"))?;
    super::debuglog::log("create wallet: seed written — installing at-rest key");
    install_at_rest(&mnemonic);
    let address = wallet::address(&signer);
    Ok(MasterWallet {
        mnemonic,
        signer,
        address,
    })
}

/// Pay-first onboarding step 1: generate a fresh keypair **IN MEMORY ONLY**
/// and install it as `APP.wallet`, writing NOTHING to disk and NOT installing
/// the at-rest encryption layer. The seed exists only in process memory until
/// the user has paid — see [`persist_current_seed`], called on a confirmed mint.
///
/// This honors the rule that no mnemonic is written to disk before payment, and
/// also moves the eventual OPFS write out of the busy landing paint (where a
/// concurrent task tripped the iOS WebKit "RefCell already borrowed" panic) into
/// the quiet post-payment moment. Keygen is synchronous; `async` only so callers
/// can await it uniformly alongside the persist step.
///
/// If the user abandons before paying, the in-memory wallet is simply discarded
/// on reload — no orphan seed is ever persisted.
pub(crate) async fn generate_in_memory() -> Result<MasterWallet, String> {
    super::debuglog::log("create wallet: generating mnemonic (in memory, no disk)");
    let (mnemonic, signer) = wallet::generate_with_mnemonic();
    let address = wallet::address(&signer);
    // Install a clone into APP so `chat::credit_signer` (which reads APP.wallet
    // FIRST) authenticates the Stripe checkout as THIS new keypair and binds the
    // mint recipient to its address. `Mnemonic`/`SigningKey` are both `Clone`.
    let app_copy = MasterWallet {
        mnemonic: mnemonic.clone(),
        signer: signer.clone(),
        address,
    };
    super::APP.with(|cell| cell.borrow_mut().wallet = Some(app_copy));
    Ok(MasterWallet {
        mnemonic,
        signer,
        address,
    })
}

/// Pay-first onboarding step 2: persist the IN-MEMORY seed (the wallet set by
/// [`generate_in_memory`]) to OPFS and install the at-rest encryption layer.
/// Called ONLY after a payment confirms, so the seed lands the moment the user
/// has actually bought. Idempotent — a second call rewrites the same seed.
///
/// Errors (non-silently — the caller surfaces it) if there is no in-memory
/// wallet to persist, or if the OPFS write fails. After a successful payment the
/// seed is still in memory, so the caller can retry / reveal it.
pub(crate) async fn persist_current_seed() -> Result<(), String> {
    let mnemonic = super::APP
        .with(|cell| cell.borrow().wallet.as_ref().map(|w| w.mnemonic.clone()))
        .ok_or_else(|| "no in-memory wallet to persist".to_string())?;
    super::debuglog::log("persist seed: writing seed (opfs write, post-payment)");
    let fs = super::shared_opfs();
    fs.write_atomic(WALLET_FILE, mnemonic.to_string().as_bytes())
        .await
        .map_err(|e| format!("wallet save: {e}"))?;
    super::debuglog::log("persist seed: seed written — installing at-rest key");
    install_at_rest(&mnemonic);
    Ok(())
}

/// Import an existing wallet from a user-supplied seed phrase.
/// Overwrites whatever's on disk — the caller is responsible for
/// confirming the user really wants to replace.
pub(crate) async fn import(phrase: &str) -> Result<MasterWallet, String> {
    let mnemonic = wallet::mnemonic_from_phrase(phrase)?;
    let fs = super::shared_opfs();
    fs.write_atomic(WALLET_FILE, mnemonic.to_string().as_bytes())
        .await
        .map_err(|e| format!("wallet save: {e}"))?;
    install_at_rest(&mnemonic);
    let signer = wallet::signer_from_mnemonic(&mnemonic);
    let address = wallet::address(&signer);
    Ok(MasterWallet {
        mnemonic,
        signer,
        address,
    })
}

/// Per-origin signer key for seedless origins — the CREDIT identity
/// (`chat::credit_signer`) on a device/origin that doesn't hold the
/// master seed, and the signer key a previously-linked device persisted.
/// NOT the master seed — a fresh random key, stored as raw hex (no
/// mnemonic; it's not meant for human backup).
const DEVICE_KEY_FILE: &str = ".lh_device_key";

/// Persist a device signer's private key (hex) to this origin's OPFS.
///
/// Encrypted at rest with the per-origin device key (see
/// [`super::encryption`]) — same model as the API key / history: OPFS
/// holds ciphertext, doesn't defend against XSS, and is safe to lose
/// (a fresh key is generated on demand, not an identity-loss like the
/// seed would be — which is why the seed is deliberately left plaintext).
pub(crate) async fn persist_device_key(private_key_hex: &str) -> Result<(), String> {
    let fs = super::shared_opfs();
    let bytes = private_key_hex.as_bytes();
    let data = super::encryption::seal(bytes)
        .await
        .unwrap_or_else(|| bytes.to_vec());
    fs.write_atomic(DEVICE_KEY_FILE, &data)
        .await
        .map_err(|e| format!("device key save: {e}"))
}

/// Load this origin's device signer key, if one was persisted here.
pub(crate) async fn load_device_key() -> Option<k256::ecdsa::SigningKey> {
    let fs = super::shared_opfs();
    let bytes = fs.read(DEVICE_KEY_FILE).await.ok()?;
    if bytes.is_empty() {
        return None;
    }
    // Decrypt if it's our ciphertext; else treat as legacy plaintext (it
    // re-encrypts on the next persist).
    let plain = super::encryption::open(&bytes).await.unwrap_or(bytes);
    let hex = String::from_utf8(plain).ok()?;
    wallet::from_private_key_hex(hex.trim()).ok()
}

/// Pointer to the on-chain OWNER address this device is linked to. A
/// linked device (second browser holding only a per-origin signer key,
/// not the seed) gives the apex no master wallet to key on. This tiny
/// PUBLIC pointer (a plaintext 0x address) tells the apex which identity
/// to render; everything shown is then read live on-chain (subdomains,
/// linked devices, MAIN). Written by the apex `?link_device=` hand-off.
const LINKED_OWNER_FILE: &str = ".lh_linked_owner";

/// Persist the on-chain owner address this device is linked to.
pub(crate) async fn persist_linked_owner(owner_hex: &str) -> Result<(), String> {
    let fs = super::shared_opfs();
    fs.write_atomic(LINKED_OWNER_FILE, owner_hex.trim().as_bytes())
        .await
        .map_err(|e| format!("linked owner save: {e}"))
}

/// Read this origin's linked-owner pointer, if any.
pub(crate) async fn load_linked_owner() -> Option<String> {
    let fs = super::shared_opfs();
    let bytes = fs.read(LINKED_OWNER_FILE).await.ok()?;
    let s = String::from_utf8(bytes).ok()?;
    let t = s.trim();
    if t.is_empty() || !t.starts_with("0x") {
        None
    } else {
        Some(t.to_string())
    }
}

/// Whether this origin's OPFS is at risk of being WIPED on tab close —
/// the private/incognito-window case (kit-qa #). The seed is the ONLY key
/// to on-chain ownership and lives in OPFS, so a fresh identity minted in a
/// volatile context is lost the moment the tab closes unless the user backs
/// it up (the QR `?adopt=1` flow) or switches to a normal window.
///
/// We ASK the browser to make storage durable (`navigator.storage.persist()`).
/// A granted (`true`) result is a definitive "safe". But a NON-granted result is
/// **NOT** a volatility signal: many normal browsers (Edge/Silk/most mobile
/// engines) decline to mark storage durable by default while still persisting
/// data across tab close / reload / device restart — treating that as volatile
/// false-flagged real users as "incognito". The ONLY signal we now warn on is a
/// genuinely tiny `estimate()` quota, which is a reliable private-window tell
/// (incognito jars report a few MB; a normal origin gets hundreds of MB to GBs).
/// Conservative everywhere else: any error / missing API / merely-undurable but
/// adequately-sized storage → NOT volatile. Best-effort only — never blocks.
pub(crate) async fn storage_is_volatile() -> bool {
    use wasm_bindgen_futures::JsFuture;
    let Some(win) = web_sys::window() else { return false };
    let storage = win.navigator().storage();

    // 1. Request durable storage (idempotent). GRANTED → definitively safe.
    //    A `false`/empty result is NOT treated as volatile (see the doc note):
    //    most non-incognito browsers simply don't grant durability on request.
    if let Ok(promise) = storage.persist() {
        if let Ok(val) = JsFuture::from(promise).await {
            if val.as_bool() == Some(true) {
                return false; // storage is durable — safe
            }
        }
    }
    // 2. Already-persistent check (some engines expose only this). TRUE → safe.
    if let Ok(promise) = storage.persisted() {
        if let Ok(val) = JsFuture::from(promise).await {
            if val.as_bool() == Some(true) {
                return false;
            }
        }
    }
    // 3. The one volatility signal we trust: a tiny quota. Real incognito jars
    //    report only a few MB; a normal origin reports hundreds of MB+. Use a
    //    low threshold (~32MB) so durable-but-undurable-flagged normal browsers
    //    (Edge/Silk/mobile), which report large quotas, are never false-warned.
    if let Ok(promise) = storage.estimate() {
        if let Ok(val) = JsFuture::from(promise).await {
            if let Some(quota) = js_sys::Reflect::get(&val, &"quota".into())
                .ok()
                .and_then(|q| q.as_f64())
            {
                return quota > 0.0 && quota < 32_000_000.0;
            }
        }
    }
    // No API / no signal — don't false-alarm a normal browser.
    false
}

fn restore_from_phrase(phrase: &str) -> Result<MasterWallet, String> {
    let mnemonic = wallet::mnemonic_from_phrase(phrase)?;
    let signer = wallet::signer_from_mnemonic(&mnemonic);
    let address = wallet::address(&signer);
    Ok(MasterWallet {
        mnemonic,
        signer,
        address,
    })
}
