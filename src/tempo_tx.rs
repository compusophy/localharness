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

// Stash the sponsorship flag inside TempoTx so the hash code can
// branch on it. Field is `pub(crate)` so the builder can set it but
// outside callers go through `TempoTxBuilder::sponsored()`.
impl TempoTx {
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
            let inner = wallet::rlp_list(&[
                wallet::rlp_uint(a.chain_id as u128),
                wallet::rlp_bytes(&a.address),
                wallet::rlp_uint(a.nonce as u128),
                wallet::rlp_bytes(&a.signature),
            ]);
            inner
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
}
