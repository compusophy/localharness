//! Tempo Transaction encoder (tx type `0x76`).
//!
//! Tempo's native account-abstraction tx type. Replaces legacy EIP-155
//! envelopes for everything we do on Tempo. Two superpowers we use:
//!
//! 1. **`fee_token`** — pay fees in any TIP-20 (typically `$LH`), not
//!    in native gas. Users hold zero native; their stablecoin balance
//!    is enough to operate.
//! 2. **`fee_payer` sponsorship** — a separate signer can sign as the
//!    one who PAYS for the tx. Lets a project-controlled wallet
//!    sponsor user txs without those users holding any balance at
//!    all. Used for first-claim bootstrap when the user has zero of
//!    everything.
//!
//! Wire format (RLP, prefixed with the type byte `0x76`):
//!
//! ```text
//! 0x76 || rlp([
//!     chain_id,
//!     max_priority_fee_per_gas,
//!     max_fee_per_gas,
//!     gas_limit,
//!     calls,                   // [[to, value, input], ...]
//!     access_list,             // EIP-2930 layout
//!     nonce_key,               // U256 — 0 for protocol nonce
//!     nonce,
//!     valid_before,            // 0x80 if None
//!     valid_after,             // 0x80 if None
//!     fee_token,               // 0x80 if None; else 20-byte address
//!     fee_payer_signature,     // 0x80 if no sponsor; else rlp([v,r,s])
//!     aa_authorization_list,   // empty list for our usage
//!     key_authorization,       // truly optional — omitted if None
//!     sender_signature         // rlp([v, r, s])
//! ])
//! ```
//!
//! Sender signs over a digest where:
//! - When sponsored: `fee_token` is replaced with `0x80` (empty) and
//!   `fee_payer_signature` is replaced with the literal byte `0x00`.
//! - When self-paid: `fee_token` is included normally and the
//!   `fee_payer_signature` slot is replaced with `0x80` (empty).
//!
//! Sender pre-image (12 fields after the 0x76 prefix; no
//! aa_authorization_list / key_authorization in this hash):
//!
//! ```text
//! keccak256(0x76 || rlp([
//!     chain_id, mpfpg, mfpg, gas_limit,
//!     calls, access_list, nonce_key, nonce,
//!     valid_before, valid_after,
//!     fee_token_for_sender_hash,
//!     fee_payer_slot_for_sender_hash
//! ]))
//! ```
//!
//! Fee-payer signs over a separate digest with magic byte `0x78`:
//!
//! ```text
//! keccak256(0x78 || rlp([
//!     chain_id, mpfpg, mfpg, gas_limit,
//!     calls, access_list, nonce_key, nonce,
//!     valid_before, valid_after,
//!     fee_token,           // always the real token here
//!     sender_address,      // 20 bytes
//!     key_authorization    // RLP — empty bytes 0x80 if None
//! ]))
//! ```
//!
//! Signatures are 65-byte `r ‖ s ‖ v` with `v` being a 0/1 recovery
//! id (NOT the Ethereum 27/28 convention). `wallet::sign_hash`
//! produces 27/28; we subtract 27 to convert. Encoded inside the tx
//! as `rlp([v, r, s])`.
//!
//! Submit via `eth_sendRawTransaction(hex(0x76 || rlp(...)))` — the
//! same JSON-RPC method as every other Ethereum tx.

use crate::wallet;

const TYPE_BYTE: u8 = 0x76;
const SENDER_DOMAIN: u8 = 0x76;
const FEE_PAYER_DOMAIN: u8 = 0x78;

/// One call inside a Tempo tx's `calls[]` array. Tempo natively
/// batches; the same tx can execute multiple calls atomically.
#[derive(Debug, Clone)]
pub struct TempoCall {
    /// `to` address. Always a real address for our use (no contract
    /// creation via Tempo tx in this codebase yet).
    pub to: [u8; 20],
    pub value_wei: u128,
    /// Pre-encoded calldata (selector + abi-encoded args). Empty for
    /// a plain native-value transfer.
    pub input: Vec<u8>,
}

/// Tempo Transaction. Everything outside the two signatures is the
/// "intent" the sender authorizes; the fee_payer authorizes a
/// subset (the intent + the fee_token + the sender_address) so they
/// know what they're paying for.
#[derive(Debug, Clone)]
pub struct TempoTx {
    pub chain_id: u64,
    pub max_priority_fee_per_gas: u128,
    pub max_fee_per_gas: u128,
    pub gas_limit: u128,
    pub calls: Vec<TempoCall>,
    /// EIP-2930 access list. Empty for our usage.
    pub access_list: Vec<AccessListItem>,
    /// 2D nonce key. `0` = the protocol/sequential nonce; non-zero
    /// keys give per-key parallelizable nonces. We default to 0.
    pub nonce_key: u128,
    pub nonce: u128,
    pub valid_before: Option<u64>,
    pub valid_after: Option<u64>,
    /// Token to pay fees in. `None` = chain default (native). Set to
    /// the `$LH` token address for our user flows so users never
    /// hold native.
    pub fee_token: Option<[u8; 20]>,
    /// Empty for our usage (EIP-7702 delegations not yet used).
    pub aa_authorization_list: Vec<SignedAuthorization>,
    /// Truly optional — when None, NO bytes are encoded for this
    /// field in the final RLP (backwards-compatible omission).
    pub key_authorization: Option<KeyAuthorization>,
    /// Sponsorship flag — drives the sender-hash branch. Set via
    /// `TempoTxBuilder::sponsored()`; flip on a built tx via
    /// `set_sponsored`. Kept here (not in the builder alone) so
    /// `sender_hash()` can read it without an extra arg.
    pub(crate) sponsored: bool,
}

/// EIP-2930 access list entry. Unused so far; kept for completeness.
#[derive(Debug, Clone)]
pub struct AccessListItem {
    pub address: [u8; 20],
    pub storage_keys: Vec<[u8; 32]>,
}

/// EIP-7702-style authorization. Unused so far.
#[derive(Debug, Clone)]
pub struct SignedAuthorization {
    pub chain_id: u64,
    pub address: [u8; 20],
    pub nonce: u64,
    pub signature: [u8; 65],
}

/// Tempo access-key authorization (T3+ format). Embedded in a tx
/// when present so the chain validates the access key's scope on
/// the fly via the Account Keychain precompile. Unused so far.
#[derive(Debug, Clone)]
pub struct KeyAuthorization {
    pub raw_rlp: Vec<u8>,
}

impl TempoTx {
    /// Compute the sender's signing digest. Returns 32 bytes ready
    /// for `wallet::sign_hash`.
    ///
    /// Two branches per the spec:
    /// - **Sponsored**: `fee_token` slot encoded as `0x80` (empty;
    ///   the fee_payer picks the token); fee_payer-sig slot encoded
    ///   as the literal byte `0x00` (the spec's placeholder).
    /// - **Self-paid**: `fee_token` slot encoded normally (real
    ///   address or 0x80 for native); fee_payer-sig slot encoded as
    ///   `0x80` (RLP empty — "no signature").
    pub fn sender_hash(&self) -> [u8; 32] {
        let mut items = self.common_rlp_items();
        if self.is_sponsored() {
            items.push(wallet::rlp_bytes(&[]));   // fee_token → 0x80
            items.push(vec![0x00]);                 // fee_payer_sig → 0x00 placeholder
        } else {
            items.push(rlp_fee_token(self.fee_token.as_ref())); // real or 0x80
            items.push(wallet::rlp_bytes(&[]));   // fee_payer_sig → 0x80 (empty)
        }
        // Include aa_authorization_list + key_authorization in the
        // sender's commitment so the chain's recovery hash matches
        // (otherwise ecrecover returns a phantom address). The
        // public spec page was ambiguous; trying the full-tx-minus-
        // sender-sig form per how EIP-1559 / 7702 typically work.
        items.push(rlp_authorization_list(&self.aa_authorization_list));
        if self.key_authorization.is_some() {
            items.push(rlp_key_authorization(self.key_authorization.as_ref()));
        }
        let body = wallet::rlp_list(&items);
        let mut payload = Vec::with_capacity(1 + body.len());
        payload.push(SENDER_DOMAIN);
        payload.extend_from_slice(&body);
        keccak(&payload)
    }

    /// Compute the fee_payer's signing digest. Per the spec, the
    /// fee_payer pre-image is:
    ///
    /// ```text
    /// 0x78 || rlp([
    ///     chain_id, mpfpg, mfpg, gas_limit,
    ///     calls, access_list, nonce_key, nonce,
    ///     valid_before, valid_after,
    ///     fee_token,           // ALWAYS the real token
    ///     sender_address,      // 20 bytes (recovered from sender sig)
    ///     key_authorization    // 0x80 when None
    /// ])
    /// ```
    ///
    /// Notably DIFFERENT from sender_hash: no `aa_authorization_list`
    /// here (the spec leaves it out of the fee_payer commitment).
    pub fn fee_payer_hash(&self, sender_address: &[u8; 20]) -> [u8; 32] {
        // Confirmed against wevm/ox `TxEnvelopeTempo.serialize` with
        // `format: 'feePayer'`. Field order:
        //   1-10: common (chain_id ... valid_after)
        //   11: feeToken
        //   12: sender_address (replaces feePayerSignatureOrSender)
        //   13: authorizationList
        //   14: keyAuthorization (conditional — included only when set)
        let mut items = self.common_rlp_items();
        items.push(rlp_fee_token(self.fee_token.as_ref()));
        items.push(wallet::rlp_bytes(sender_address));
        items.push(rlp_authorization_list(&self.aa_authorization_list));
        if self.key_authorization.is_some() {
            items.push(rlp_key_authorization(self.key_authorization.as_ref()));
        }
        let body = wallet::rlp_list(&items);
        let mut payload = Vec::with_capacity(1 + body.len());
        payload.push(FEE_PAYER_DOMAIN);
        payload.extend_from_slice(&body);
        keccak(&payload)
    }

    /// Serialize the final, signed Tempo tx ready for
    /// `eth_sendRawTransaction`. `sender_sig` is the 65-byte (r‖s‖v
    /// where v∈{27,28}) sig produced by `wallet::sign_hash`. If
    /// sponsored, pass `Some(fee_payer_sig)`; otherwise `None`.
    ///
    /// Note: sender_signature is encoded as flat 65 bytes (the
    /// "TempoSignature" wire format — secp256k1 needs no type prefix).
    /// fee_payer_signature is encoded as `rlp([v, r, s])` per the
    /// spec's split for fee_payer (different from sender). Confirmed
    /// experimentally — sender as flat bytes decodes; fee_payer as
    /// flat bytes does NOT.
    pub fn serialize_signed(
        &self,
        sender_sig: &[u8; 65],
        fee_payer_sig: Option<&[u8; 65]>,
    ) -> Vec<u8> {
        let mut items = self.common_rlp_items();
        // Field 11: fee_token (real, regardless of sponsorship — the
        // sender hash hid it but the serialized tx reveals it).
        items.push(rlp_fee_token(self.fee_token.as_ref()));
        // Field 12: fee_payer_signature as rlp([v, r, s]) when set.
        match fee_payer_sig {
            Some(sig) => items.push(rlp_vrs_signature(sig)),
            None => items.push(wallet::rlp_bytes(&[])), // 0x80
        }
        // Field 13: aa_authorization_list (empty list for us).
        items.push(rlp_authorization_list(&self.aa_authorization_list));
        // Field 14: key_authorization (truly optional — omit when None).
        if let Some(_ka) = self.key_authorization.as_ref() {
            items.push(rlp_key_authorization(self.key_authorization.as_ref()));
        }
        // Field 15: sender_signature as flat 65 bytes wrapped in RLP.
        items.push(rlp_compact_signature(sender_sig));

        let body = wallet::rlp_list(&items);
        let mut out = Vec::with_capacity(1 + body.len());
        out.push(TYPE_BYTE);
        out.extend_from_slice(&body);
        out
    }

    /// `true` iff this tx is sponsored by a separate fee_payer.
    /// Drives the sender-hash branch.
    pub fn is_sponsored(&self) -> bool {
        // Set externally before computing the sender hash. The caller
        // tells the encoder which mode this tx is by leaving fee_token
        // unset OR by setting it AND signing a fee_payer hash. We
        // can't introspect that here, so the caller passes through
        // `set_sponsored_mode` if needed. Default: not sponsored.
        self.sponsored
    }
}

// The `sponsored` flag lives outside the public field set so the
// caller has to opt in explicitly — keeps the sender-hash branch
// from silently flipping based on whether `fee_token` happens to be
// `Some`. (Plenty of self-paid txs set `fee_token` to a TIP-20 like
// $LH; sponsorship is a separate decision.)
#[derive(Debug, Clone)]
pub struct TempoTxBuilder {
    inner: TempoTx,
    sponsored: bool,
}

impl TempoTxBuilder {
    pub fn new(chain_id: u64) -> Self {
        Self {
            inner: TempoTx {
                chain_id,
                max_priority_fee_per_gas: 0,
                max_fee_per_gas: 0,
                gas_limit: 0,
                calls: Vec::new(),
                access_list: Vec::new(),
                nonce_key: 0,
                nonce: 0,
                valid_before: None,
                valid_after: None,
                fee_token: None,
                aa_authorization_list: Vec::new(),
                key_authorization: None,
                sponsored: false,
            },
            sponsored: false,
        }
    }

    pub fn max_priority_fee_per_gas(mut self, v: u128) -> Self {
        self.inner.max_priority_fee_per_gas = v;
        self
    }
    pub fn max_fee_per_gas(mut self, v: u128) -> Self {
        self.inner.max_fee_per_gas = v;
        self
    }
    pub fn gas_limit(mut self, v: u128) -> Self {
        self.inner.gas_limit = v;
        self
    }
    pub fn nonce(mut self, v: u128) -> Self {
        self.inner.nonce = v;
        self
    }
    pub fn nonce_key(mut self, v: u128) -> Self {
        self.inner.nonce_key = v;
        self
    }
    pub fn fee_token(mut self, addr: [u8; 20]) -> Self {
        self.inner.fee_token = Some(addr);
        self
    }
    pub fn call(mut self, call: TempoCall) -> Self {
        self.inner.calls.push(call);
        self
    }
    pub fn calls(mut self, calls: Vec<TempoCall>) -> Self {
        self.inner.calls = calls;
        self
    }
    /// Mark this tx as sponsored — the sender's hash will omit
    /// `fee_token` (replaced with 0x80) and use the 0x00 placeholder
    /// for the fee_payer_signature slot.
    pub fn sponsored(mut self) -> Self {
        self.sponsored = true;
        self.inner.sponsored = true;
        self
    }

    pub fn build(self) -> TempoTx {
        self.inner
    }
}

// =============================================================================
// High-level helpers: sign + serialize a Tempo tx in one call.
// =============================================================================

/// Sign and serialize a SELF-PAID tempo tx. Sender pays fees in
/// `fee_token` (None = native). Returns the 0x76-prefixed raw bytes
/// ready for `eth_sendRawTransaction`.
pub fn sign_self_paid(tx: TempoTx, sender: &k256::ecdsa::SigningKey) -> Vec<u8> {
    let sender_hash = tx.sender_hash();
    let sig = crate::wallet::sign_hash(sender, &sender_hash);
    tx.serialize_signed(&sig, None)
}

/// Sign and serialize a SPONSORED tempo tx. `sender` signs the
/// intent; `fee_payer` signs the payment commitment. Fees are
/// deducted from `fee_payer`'s `fee_token` balance.
///
/// The `tx` MUST have been built with `.sponsored()` so the sender
/// hash uses the empty-fee_token + 0x00-placeholder layout.
pub fn sign_sponsored(
    tx: TempoTx,
    sender: &k256::ecdsa::SigningKey,
    fee_payer: &k256::ecdsa::SigningKey,
) -> Vec<u8> {
    debug_assert!(
        tx.sponsored,
        "sign_sponsored called on a non-sponsored TempoTx — \
         use TempoTxBuilder::sponsored()"
    );
    let sender_addr = crate::wallet::address(sender);
    let sender_hash = tx.sender_hash();
    let fp_hash = tx.fee_payer_hash(&sender_addr);
    let sender_sig = crate::wallet::sign_hash(sender, &sender_hash);
    let fp_sig = crate::wallet::sign_hash(fee_payer, &fp_hash);
    tx.serialize_signed(&sender_sig, Some(&fp_sig))
}

// Stash the sponsorship flag inside TempoTx so the hash code can
// branch on it. Field is `pub(crate)` so the builder can set it but
// outside callers go through `TempoTxBuilder::sponsored()`.
impl TempoTx {
    // Kept as a builder setter for completeness; native callers use
    // `TempoTxBuilder::sponsored()`, so it reads as dead on every target.
    #[allow(dead_code)]
    pub(crate) fn set_sponsored(mut self, sponsored: bool) -> Self {
        self.sponsored = sponsored;
        self
    }
}

// --- RLP field encoders ---------------------------------------------

fn rlp_fee_token(addr: Option<&[u8; 20]>) -> Vec<u8> {
    match addr {
        Some(a) => wallet::rlp_bytes(a),
        None => wallet::rlp_bytes(&[]),
    }
}

fn rlp_key_authorization(ka: Option<&KeyAuthorization>) -> Vec<u8> {
    match ka {
        Some(k) => k.raw_rlp.clone(),
        None => wallet::rlp_bytes(&[]),
    }
}

fn rlp_authorization_list(list: &[SignedAuthorization]) -> Vec<u8> {
    // For our usage this is always empty — encode as the empty list
    // 0xc0 (RLP list, zero body).
    if list.is_empty() {
        return wallet::rlp_list(&[]);
    }
    let items: Vec<Vec<u8>> = list
        .iter()
        .map(|a| {
            wallet::rlp_list(&[
                wallet::rlp_uint(a.chain_id as u128),
                wallet::rlp_bytes(&a.address),
                wallet::rlp_uint(a.nonce as u128),
                wallet::rlp_bytes(&a.signature),
            ])
        })
        .collect();
    wallet::rlp_list(&items)
}

fn rlp_call(call: &TempoCall) -> Vec<u8> {
    wallet::rlp_list(&[
        wallet::rlp_bytes(&call.to),
        wallet::rlp_uint(call.value_wei),
        wallet::rlp_bytes(&call.input),
    ])
}

fn rlp_access_list(list: &[AccessListItem]) -> Vec<u8> {
    // EIP-2930 layout: list of [address, [storage_key, ...]]. Empty
    // by default for our usage.
    if list.is_empty() {
        return wallet::rlp_list(&[]);
    }
    let items: Vec<Vec<u8>> = list
        .iter()
        .map(|item| {
            let keys: Vec<Vec<u8>> =
                item.storage_keys.iter().map(|k| wallet::rlp_bytes(k)).collect();
            wallet::rlp_list(&[
                wallet::rlp_bytes(&item.address),
                wallet::rlp_list(&keys),
            ])
        })
        .collect();
    wallet::rlp_list(&items)
}

/// Encode a 65-byte (r ‖ s ‖ v) signature in Tempo's `TempoSignature`
/// format. For secp256k1: 65 raw bytes (r 32 ‖ s 32 ‖ v 1) with NO
/// type prefix, packed into a single RLP byte string. `v` is
/// normalized to the recovery id 0 or 1 (NOT Ethereum's 27/28).
/// Used for the SENDER signature field.
fn rlp_compact_signature(sig: &[u8; 65]) -> Vec<u8> {
    let mut packed = [0u8; 65];
    packed.copy_from_slice(sig);
    packed[64] = packed[64].saturating_sub(27); // 27/28 → 0/1
    wallet::rlp_bytes(&packed)
}

/// Encode a fee_payer signature as `rlp([v, r, s])` — the 3-element
/// list form Tempo expects in the fee_payer slot specifically.
/// Different from `rlp_compact_signature` (which is used for the
/// sender slot). Empirically: fee_payer expects the list form,
/// sender expects the flat form.
fn rlp_vrs_signature(sig: &[u8; 65]) -> Vec<u8> {
    let v = sig[64].saturating_sub(27); // 27/28 → 0/1
    wallet::rlp_list(&[
        wallet::rlp_uint(v as u128),
        wallet::rlp_bytes(&sig[..32]),
        wallet::rlp_bytes(&sig[32..64]),
    ])
}

// --- helpers --------------------------------------------------------

impl TempoTx {
    /// First ten fields, shared between sender hash, fee_payer hash,
    /// and the serialized tx body (before the signature trailers).
    fn common_rlp_items(&self) -> Vec<Vec<u8>> {
        let call_items: Vec<Vec<u8>> = self.calls.iter().map(rlp_call).collect();
        vec![
            wallet::rlp_uint(self.chain_id as u128),
            wallet::rlp_uint(self.max_priority_fee_per_gas),
            wallet::rlp_uint(self.max_fee_per_gas),
            wallet::rlp_uint(self.gas_limit),
            wallet::rlp_list(&call_items),
            rlp_access_list(&self.access_list),
            rlp_uint_u256(self.nonce_key),
            wallet::rlp_uint(self.nonce),
            self.valid_before
                .map(|v| wallet::rlp_uint(v as u128))
                .unwrap_or_else(|| wallet::rlp_bytes(&[])),
            self.valid_after
                .map(|v| wallet::rlp_uint(v as u128))
                .unwrap_or_else(|| wallet::rlp_bytes(&[])),
        ]
    }
}

fn rlp_uint_u256(value: u128) -> Vec<u8> {
    // nonce_key is conceptually a U256 but we cap at u128 for now.
    // Same RLP encoding either way — the value is just minimal
    // big-endian bytes with leading zeros stripped.
    wallet::rlp_uint(value)
}

fn keccak(input: &[u8]) -> [u8; 32] {
    use sha3::{Digest, Keccak256};
    let mut hasher = Keccak256::new();
    hasher.update(input);
    let mut out = [0u8; 32];
    out.copy_from_slice(&hasher.finalize());
    out
}

// --- visible field on TempoTx ---------------------------------------
//
// We need the sponsored flag inside the struct so `sender_hash` can
// branch on it without an extra arg. Hide it from public field
// initializers by keeping the only entry point the builder.

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_tx() -> TempoTx {
        TempoTxBuilder::new(42431)
            .max_priority_fee_per_gas(1_000_000_000)
            .max_fee_per_gas(40_000_000_000)
            .gas_limit(200_000)
            .nonce(0)
            .call(TempoCall {
                to: [0x11; 20],
                value_wei: 0,
                input: vec![0xde, 0xad, 0xbe, 0xef],
            })
            .build()
    }

    #[test]
    fn sender_hash_self_paid_is_32_bytes() {
        let tx = dummy_tx();
        let h = tx.sender_hash();
        assert_eq!(h.len(), 32);
    }

    #[test]
    fn sender_hash_sponsored_differs_from_self_paid() {
        let tx_self = dummy_tx();
        let tx_sponsored = dummy_tx().set_sponsored(true);
        assert_ne!(tx_self.sender_hash(), tx_sponsored.sender_hash());
    }

    #[test]
    fn serialized_starts_with_type_byte() {
        let tx = dummy_tx();
        let sig = [0u8; 65];
        let bytes = tx.serialize_signed(&sig, None);
        assert_eq!(bytes[0], 0x76);
    }

    #[test]
    fn fee_payer_hash_includes_sender_address() {
        let tx = dummy_tx().set_sponsored(true);
        let sender = [0x42; 20];
        let other_sender = [0x99; 20];
        assert_ne!(
            tx.fee_payer_hash(&sender),
            tx.fee_payer_hash(&other_sender)
        );
    }

    // === GOLDEN VECTORS ==================================================
    //
    // Every constant below was generated ONCE from the implementation as it
    // was LIVE-PROVEN against Tempo Moderato (`examples/tempo_tx_live.rs`,
    // sponsored mints landing on-chain). They pin the 0x76 WIRE FORMAT:
    //
    //   - the 0x76 sender / 0x78 fee_payer domain bytes,
    //   - the exact field order of the common 10-item prefix,
    //   - the sponsored sender-hash branch (fee_token → 0x80 empty,
    //     fee_payer-sig slot → literal 0x00 placeholder),
    //   - the self-paid sender-hash branch (real fee_token, 0x80 sig slot),
    //   - the fee_payer hash including aa_authorization_list at position 13
    //     (the spec page OMITS it — found by diffing wevm/ox),
    //   - key_authorization OMISSION when None (no 0x80 stuffed in),
    //   - sender signature as FLAT 65 bytes vs fee_payer signature as
    //     rlp([v, r, s]) — the asymmetry that only shows up on-wire,
    //   - RLP long-form string header (0xb8) for >55-byte calldata.
    //
    // k256's RFC6979 signing is deterministic, so the raw-tx bytes are
    // byte-stable across runs/platforms. A MISMATCH MEANS THE WIRE FORMAT
    // CHANGED: a sender-hash preimage drift executes silently (ecrecover
    // returns a phantom address → mints land on an unspendable identity
    // while the sponsor keeps paying). Do NOT casually regenerate these to
    // make the test pass — first prove the new bytes against the live chain
    // via `examples/tempo_tx_live.rs`, then update them deliberately.

    /// `keccak256(0x76 || rlp(..))` for [`golden_tx`] in SPONSORED mode:
    /// fee_token slot empty (0x80), fee_payer-sig slot the 0x00 placeholder.
    const GOLDEN_SPONSORED_SENDER_HASH: &str =
        "3e6d7f767fb15c062735b045126a54e9ea8f4d098cebe942cb18761532242d17";
    /// `keccak256(0x76 || rlp(..))` for [`golden_tx`] in SELF-PAID mode:
    /// real fee_token (AlphaUSD), fee_payer-sig slot 0x80 (empty).
    const GOLDEN_SELF_PAID_SENDER_HASH: &str =
        "3c842190b039b46368cfe5d12268bce7a539274d88c48ae43d0f7ef230f164d7";
    /// `keccak256(0x78 || rlp(..))` — the fee_payer commitment over the
    /// real fee_token + the 0x…01 sender's address + aa_authorization_list.
    const GOLDEN_FEE_PAYER_HASH: &str =
        "a6e9b8ae237b8711335dad82bdcb3cda9b52278f4a479392bbc153e888a4b5b5";
    /// Full `sign_sponsored` output (0x76-prefixed raw tx): flat-65-byte
    /// sender sig trailing, rlp([v,r,s]) fee_payer sig in field 12,
    /// key_authorization omitted.
    const GOLDEN_SPONSORED_RAW_TX: &str =
        "76f9011482a5bf843b9aca0084773594008316e360f85ef85c94d7d7d7d7d7d7d7d7d7d7\
         d7d7d7d7d7d7d7d7d7d780b844a9059cbb000102030405060708090a0b0c0d0e0f101112\
         131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f30313233343536\
         3738393a3b3c3d3e3fc0800780809420c0000000000000000000000000000000000001f8\
         4380a0bedf191eaaaa41e9b67003e472eed8eb0577b09a96a337158819aee742f8b951a0\
         3352b344cad1fadc97aee9f08ddbc42d1648e76f6e16a937f6aa8703636b79c1c0b8419b\
         46f696dddfbd4739b1bbf7a108ee4cde2de6826dcf49079fba621ca473a5f51f20fd463c\
         4c1573accb51f4021bc108a1bfb44d2fbecd2bff45faf9969dcf6900";
    /// Full `sign_self_paid` output: fee_payer slot 0x80, same trailing
    /// flat sender sig encoding.
    const GOLDEN_SELF_PAID_RAW_TX: &str =
        "76f8d082a5bf843b9aca0084773594008316e360f85ef85c94d7d7d7d7d7d7d7d7d7d7d7\
         d7d7d7d7d7d7d7d7d780b844a9059cbb000102030405060708090a0b0c0d0e0f10111213\
         1415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f3031323334353637\
         38393a3b3c3d3e3fc0800780809420c000000000000000000000000000000000000180c0\
         b8413f550f9766ba12e152f0a9ea828f3eaa45363c80278c98706d773d1b5f359c71538a\
         9dbdd8140c8e7c6df77c6593727d34ecf36150484f59ae24912f759e3ce800";

    /// Deterministic fixture: chain 42431, nonce 7, gas 1.5M, AlphaUSD
    /// fee_token, ONE call whose 68-byte calldata (selector + 2 ABI words)
    /// exceeds RLP's 55-byte short-string limit — pinning the long-form
    /// (0xb8) branch every real `setMetadata`/`settle` call rides on.
    fn golden_tx() -> TempoTx {
        // AlphaUSD on Tempo Moderato: 0x20c0…0001 (the sponsor fee_token).
        let mut alpha_usd = [0u8; 20];
        alpha_usd[0] = 0x20;
        alpha_usd[1] = 0xc0;
        alpha_usd[19] = 0x01;
        let mut input = vec![0xa9, 0x05, 0x9c, 0xbb];
        input.extend(0u8..64);
        debug_assert!(input.len() > 55);
        TempoTxBuilder::new(42431)
            .max_priority_fee_per_gas(1_000_000_000)
            .max_fee_per_gas(2_000_000_000)
            .gas_limit(1_500_000)
            .nonce(7)
            .fee_token(alpha_usd)
            .call(TempoCall {
                to: [0xd7; 20],
                value_wei: 0,
                input,
            })
            .build()
    }

    /// Fixed keys — k256 RFC6979 makes every signature over them
    /// deterministic, so the golden raw-tx constants are byte-stable.
    fn golden_keys() -> (k256::ecdsa::SigningKey, k256::ecdsa::SigningKey) {
        let sender = wallet::from_private_key_hex(
            "0x0000000000000000000000000000000000000000000000000000000000000001",
        )
        .unwrap();
        let fee_payer = wallet::from_private_key_hex(
            "0x0000000000000000000000000000000000000000000000000000000000000002",
        )
        .unwrap();
        (sender, fee_payer)
    }

    fn hex(b: &[u8]) -> String {
        crate::encoding::bytes_to_hex(b)
    }

    #[test]
    fn sponsored_tx_golden_vector() {
        let (sender, fee_payer) = golden_keys();
        let tx = golden_tx().set_sponsored(true);
        let sender_addr = wallet::address(&sender);

        assert_eq!(
            hex(&tx.sender_hash()),
            GOLDEN_SPONSORED_SENDER_HASH,
            "sponsored sender-hash preimage changed — on-chain ecrecover \
             would now yield a PHANTOM sender (identity brick + sponsor drain)"
        );
        assert_eq!(
            hex(&tx.fee_payer_hash(&sender_addr)),
            GOLDEN_FEE_PAYER_HASH,
            "fee_payer-hash preimage changed — sponsor signature would no \
             longer validate"
        );

        let raw = sign_sponsored(tx, &sender, &fee_payer);
        assert_eq!(raw[0], 0x76);
        assert_eq!(
            hex(&raw),
            GOLDEN_SPONSORED_RAW_TX,
            "serialized sponsored 0x76 tx changed — the WIRE FORMAT moved; \
             prove the new bytes via examples/tempo_tx_live.rs before \
             regenerating"
        );
    }

    #[test]
    fn self_paid_tx_golden_vector() {
        let (sender, _) = golden_keys();
        let tx = golden_tx();

        assert_eq!(
            hex(&tx.sender_hash()),
            GOLDEN_SELF_PAID_SENDER_HASH,
            "self-paid sender-hash preimage changed — signatures would \
             recover to a phantom address"
        );
        // The two branches MUST diverge — sponsored hides fee_token and
        // stuffs the 0x00 placeholder; identical hashes would mean the
        // branch collapsed.
        assert_ne!(GOLDEN_SPONSORED_SENDER_HASH, GOLDEN_SELF_PAID_SENDER_HASH);

        let raw = sign_self_paid(tx, &sender);
        assert_eq!(raw[0], 0x76);
        assert_eq!(
            hex(&raw),
            GOLDEN_SELF_PAID_RAW_TX,
            "serialized self-paid 0x76 tx changed — the WIRE FORMAT moved; \
             prove the new bytes via examples/tempo_tx_live.rs before \
             regenerating"
        );
    }

    // --- envelope-decode regression for a u128::MAX approve --------------
    //
    // The call_agent x402 approve passes `u128::MAX` as the ERC-20 allowance,
    // which `encode_approve` lays into the LOW 16 bytes of a 32-byte ABI word
    // (`00…00ffffffffffffffffffffffffffffff`). A live submit of this exact tx
    // mines fine (proven on Moderato), but the on-chain report claimed the node
    // "failed to decode signed transaction" — reth's signal for a STRUCTURALLY
    // malformed 0x76 envelope. This test pins that the serialized envelope is a
    // COMPLETE, canonical RLP list (header length matches the body, 15 top-level
    // items, the 0xFF allowance word intact) so an encoder regression that
    // truncated the envelope or mangled the max-value word would fail HERE
    // rather than as a confusing runtime node error.

    /// Minimal RLP: read one item's header at `buf[i]`, return
    /// `(payload_offset, payload_len, is_list)`. Panics on a truncated header.
    fn rlp_header(buf: &[u8], i: usize) -> (usize, usize, bool) {
        let b = buf[i];
        match b {
            0x00..=0x7f => (i, 1, false), // single byte is its own payload
            0x80..=0xb7 => (i + 1, (b - 0x80) as usize, false),
            0xb8..=0xbf => {
                let n = (b - 0xb7) as usize;
                let len = be_to_usize(&buf[i + 1..i + 1 + n]);
                (i + 1 + n, len, false)
            }
            0xc0..=0xf7 => (i + 1, (b - 0xc0) as usize, true),
            0xf8..=0xff => {
                let n = (b - 0xf7) as usize;
                let len = be_to_usize(&buf[i + 1..i + 1 + n]);
                (i + 1 + n, len, true)
            }
        }
    }

    fn be_to_usize(bytes: &[u8]) -> usize {
        bytes.iter().fold(0usize, |acc, &b| (acc << 8) | b as usize)
    }

    /// Count the top-level items inside the list whose body is `[start, end)`.
    fn rlp_list_len(buf: &[u8], start: usize, end: usize) -> usize {
        let mut i = start;
        let mut count = 0;
        while i < end {
            let (off, len, _) = rlp_header(buf, i);
            i = off + len;
            count += 1;
        }
        assert_eq!(i, end, "RLP item ran past its parent list bound");
        count
    }

    #[test]
    fn approve_u128_max_envelope_is_well_formed() {
        let (sender, fee_payer) = golden_keys();
        // approve(address,uint256) with the diamond spender + u128::MAX.
        let mut input = vec![0x09, 0x5e, 0xa7, 0xb3]; // approve selector
        let mut spender = [0u8; 32];
        spender[12..].copy_from_slice(&[0x6c; 20]);
        input.extend_from_slice(&spender);
        let mut amount = [0u8; 32];
        amount[16..].copy_from_slice(&u128::MAX.to_be_bytes()); // 00..00ff..ff
        input.extend_from_slice(&amount);
        assert_eq!(input.len(), 68);

        let tx = TempoTxBuilder::new(42431)
            .max_priority_fee_per_gas(20_000_000_000)
            .max_fee_per_gas(20_000_000_000)
            .gas_limit(300_000)
            .nonce(43)
            .fee_token([0x20; 20])
            .call(TempoCall { to: [0x90; 20], value_wei: 0, input: input.clone() })
            .sponsored()
            .build();
        let raw = sign_sponsored(tx, &sender, &fee_payer);

        // 1. Type byte present, body is one complete RLP list spanning the rest.
        assert_eq!(raw[0], 0x76);
        let (body_off, body_len, is_list) = rlp_header(&raw, 1);
        assert!(is_list, "0x76 body must be an RLP list");
        assert_eq!(
            body_off + body_len,
            raw.len(),
            "RLP list header length must match the actual body — a mismatch is \
             exactly what reth rejects as 'failed to decode signed transaction'"
        );

        // 2. Exactly 14 top-level fields: chain_id, mpfpg, mfpg, gas_limit,
        //    calls, access_list, nonce_key, nonce, valid_before, valid_after,
        //    fee_token, fee_payer_sig, aa_authorization_list, sender_signature
        //    — key_authorization is OMITTED when None (no 0x80 stuffed in).
        assert_eq!(
            rlp_list_len(&raw, body_off, body_off + body_len),
            14,
            "sponsored 0x76 envelope must carry 14 top-level items \
             (key_authorization omitted when None)"
        );

        // 3. The 0xFF max-value allowance word survives verbatim inside the
        //    calldata byte string (no leading-zero stripping leaked into the
        //    fixed-width ABI word).
        let needle = &input[36..68]; // the uint256 amount word
        assert!(
            raw.windows(needle.len()).any(|w| w == needle),
            "u128::MAX allowance word must be preserved verbatim in the envelope"
        );
    }
}
