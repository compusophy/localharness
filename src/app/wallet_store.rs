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
        let mut s = String::with_capacity(42);
        s.push_str("0x");
        for b in &self.address {
            s.push_str(&format!("{b:02x}"));
        }
        s
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
