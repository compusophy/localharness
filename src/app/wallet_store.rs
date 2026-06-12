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
/// the [`crate::filesystem::EncryptedFilesystem`] wrapper. Idempotent.
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
    let fs = super::shared_opfs();
    let (mnemonic, signer) = wallet::generate_with_mnemonic();
    fs.write_atomic(WALLET_FILE, mnemonic.to_string().as_bytes())
        .await
        .map_err(|e| format!("wallet save: {e}"))?;
    install_at_rest(&mnemonic);
    let address = wallet::address(&signer);
    Ok(MasterWallet {
        mnemonic,
        signer,
        address,
    })
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
