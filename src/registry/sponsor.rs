//! THE one home of the sponsored-write `fee_payer` resolution (2026-07 SubmitCtx
//! refactor): every `*_sponsored` facet wrapper resolves its fee_payer + fee_token
//! HERE instead of threading them through ~75 signatures. On **testnet** the
//! committed low-budget key below signs `fee_payer` and pays AlphaUSD fees. On
//! **mainnet** the resolved key is an UNUSED PLACEHOLDER — `tx::fee_payer_sig_for`
//! routes the fee_payer half to the server relay ([`super::sponsor_relay`]); no
//! build embeds a mainnet money key. Callers needing a CUSTOM sponsor (live
//! examples, e2e harnesses) use the explicit primitives
//! [`super::submit_tempo_sponsored`] / [`super::create_sponsored`] directly.

use k256::ecdsa::SigningKey;

/// Committed TESTNET sponsor key (Tempo Moderato) — derives
/// `0x0AFf88Ad13eF24caC5BeFD0F9Dc3A05DF79a922C`, a dedicated low-budget wallet
/// holding only the AlphaUSD needed to pay user fees (loss capped at its balance
/// if extracted; refill via `tempo_fundAddress`). The PROD wasm+mainnet bundle
/// ships a DUMMY instead so no real key is extractable from it — the value is
/// unused there (the relay signs fee_payer on mainnet).
#[cfg(not(all(target_arch = "wasm32", feature = "mainnet")))]
const SPONSOR_PRIVATE_KEY_HEX: &str =
    "0x046a830b5203d1d2c0a205a1432746e4381d0874711b2de7f575a973644b9d43";
#[cfg(all(target_arch = "wasm32", feature = "mainnet"))]
const SPONSOR_PRIVATE_KEY_HEX: &str =
    "0x0000000000000000000000000000000000000000000000000000000000000001";

/// The default `fee_payer` `SigningKey` for sponsored Tempo txs. Cheap to call
/// repeatedly — k256 keys clone cheaply. Testnet: pays fees directly. Mainnet:
/// an unused placeholder (the keyless relay signs the fee_payer half).
pub fn fee_payer() -> Result<SigningKey, String> {
    crate::wallet::from_private_key_hex(SPONSOR_PRIVATE_KEY_HEX)
        .map_err(|e| format!("sponsor key invalid: {e}"))
}

#[cfg(all(test, not(all(target_arch = "wasm32", feature = "mainnet"))))]
mod tests {
    /// The committed sponsor key must parse and derive the documented address —
    /// a typo here silently breaks EVERY sponsored testnet write.
    #[test]
    fn sponsor_key_derives_documented_address() {
        let key = super::fee_payer().expect("committed sponsor key parses");
        let addr = super::super::address_to_hex(&crate::wallet::address(&key));
        assert_eq!(addr, "0x0aff88ad13ef24cac5befd0f9dc3a05df79a922c");
    }
}
