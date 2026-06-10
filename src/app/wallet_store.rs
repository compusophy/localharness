//! Master wallet persistence at the apex origin.
//!
//! Per-origin OPFS sandbox makes this naturally apex-only: the
//! `.lh_wallet` file written by this module lives at
//! `localharness.xyz`'s OPFS and is invisible to every subdomain.
//! That's exactly the boundary we want — the wallet is the master
//! identity; subdomains will eventually authenticate against it via
//! an iframe-signer (M8), not by importing the key.
//!
//! Storage format is a single line of 12 BIP-39 words. Plain text,
//! no encryption-at-rest yet (matches the API key situation — same
//! threat model: per-origin sandbox is the boundary, XSS-equivalent
//! risk if anything ever runs in this origin uninvited).

use crate::filesystem::Filesystem;
use crate::wallet;

const WALLET_FILE: &str = ".lh_wallet";

pub(crate) struct MasterWallet {
    pub(crate) mnemonic: bip39::Mnemonic,
    /// Held for M8 (iframe-signer): used to sign authentication
    /// challenges from subdomain origins so they can verify the
    /// visitor is the registered owner.
    #[allow(dead_code)]
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
    restore_from_phrase(&phrase).ok()
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
    let signer = wallet::signer_from_mnemonic(&mnemonic);
    let address = wallet::address(&signer);
    Ok(MasterWallet {
        mnemonic,
        signer,
        address,
    })
}

/// Wipe the wallet file — the "I want a new identity" affordance.
#[allow(dead_code)]
pub(crate) async fn forget() {
    let fs = super::shared_opfs();
    let _ = fs.delete(WALLET_FILE).await;
}

/// Per-device signer key, used by the device-pairing flow. This lives at
/// the TENANT origin (the phone opened `<name>.localharness.xyz/?pair=…`)
/// and is NOT the master seed — it's a fresh random key enrolled as an
/// additional signer on the MAIN's TBA, so the device can act as the
/// agent without ever importing the 12-word seed. Stored as a raw hex
/// private key (no mnemonic; it's not meant for human backup).
const DEVICE_KEY_FILE: &str = ".lh_device_key";

/// Persist a device signer's private key (hex) to this origin's OPFS.
///
/// Encrypted at rest with the per-origin device key (see
/// [`super::encryption`]) — same model as the API key / history: OPFS
/// holds ciphertext, doesn't defend against XSS, and is safe to lose
/// (re-pair / re-open a session, not an identity-loss like the seed
/// would be — which is why the seed is deliberately left plaintext).
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

/// Load this origin's device signer key, if one was enrolled here.
#[allow(dead_code)]
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
/// linked device (paired phone / second browser) holds only a per-origin
/// signer key, not the seed — so the apex has no master wallet to key on.
/// This tiny PUBLIC pointer (a plaintext 0x address) tells the apex which
/// identity to render; everything shown is then read live on-chain
/// (subdomains, linked devices, MAIN). Set during pairing.
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

/// Drop the linked-owner pointer (e.g. when the user creates/imports their
/// own seed on this origin and becomes a first-class identity).
#[allow(dead_code)]
pub(crate) async fn clear_linked_owner() {
    let fs = super::shared_opfs();
    let _ = fs.delete(LINKED_OWNER_FILE).await;
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
