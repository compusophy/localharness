use k256::ecdsa::SigningKey;
use sha3::{Digest, Keccak256};

use super::*;

// --- x402 payment authorization (settled in $LH via X402Facet) -------
//
// EIP-712 "exact"-scheme settlement for agent-to-agent payments. The
// payer signs a `PaymentAuthorization` (gasless); the payee submits
// `settle`. Domain/typehash MUST match `contracts/src/facets/X402Facet.sol`
// — the `x402_domain_matches_live_facet` test pins it to the deployed
// diamond.

pub(crate) fn keccak32(data: &[u8]) -> [u8; 32] {
    let mut h = Keccak256::new();
    h.update(data);
    let d = h.finalize();
    let mut o = [0u8; 32];
    o.copy_from_slice(&d);
    o
}

pub(crate) fn addr_word(a: &[u8; 20]) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[12..].copy_from_slice(a);
    w
}

/// EIP-712 domain separator for the x402 facet (name "localharness-x402",
/// version "1", `CHAIN_ID`, diamond). Matches `x402DomainSeparator()`.
pub fn x402_domain_separator() -> Result<[u8; 32], String> {
    let diamond = parse_eth_address(REGISTRY_ADDRESS)?;
    let mut dom = Vec::with_capacity(160);
    dom.extend_from_slice(&keccak32(
        b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)",
    ));
    dom.extend_from_slice(&keccak32(b"localharness-x402"));
    dom.extend_from_slice(&keccak32(b"1"));
    dom.extend_from_slice(&u256_be(CHAIN_ID as u128));
    dom.extend_from_slice(&addr_word(&diamond));
    Ok(keccak32(&dom))
}

/// EIP-712 digest of an x402 `PaymentAuthorization` (what the payer signs).
pub fn x402_digest(
    from: &[u8; 20],
    to: &[u8; 20],
    value_wei: u128,
    valid_after: u64,
    valid_before: u64,
    nonce: &[u8; 32],
) -> Result<[u8; 32], String> {
    let mut st = Vec::with_capacity(224);
    st.extend_from_slice(&keccak32(
        b"PaymentAuthorization(address from,address to,uint256 value,uint256 validAfter,uint256 validBefore,bytes32 nonce)",
    ));
    st.extend_from_slice(&addr_word(from));
    st.extend_from_slice(&addr_word(to));
    st.extend_from_slice(&u256_be(value_wei));
    st.extend_from_slice(&u256_be(valid_after as u128));
    st.extend_from_slice(&u256_be(valid_before as u128));
    st.extend_from_slice(nonce);
    let struct_hash = keccak32(&st);

    let mut pre = Vec::with_capacity(66);
    pre.extend_from_slice(&[0x19, 0x01]);
    pre.extend_from_slice(&x402_domain_separator()?);
    pre.extend_from_slice(&struct_hash);
    Ok(keccak32(&pre))
}

/// Sign an x402 authorization with an EOA key — the 65-byte sig that
/// goes in the `X-PAYMENT` payload. (k256 emits low-s, which the facet
/// requires.) Agents paying from a contract TBA sign via EIP-1271 paths
/// instead; this is the EOA fast path.
pub fn sign_x402(
    signer: &SigningKey,
    from: &[u8; 20],
    to: &[u8; 20],
    value_wei: u128,
    valid_after: u64,
    valid_before: u64,
    nonce: &[u8; 32],
) -> Result<[u8; 65], String> {
    let digest = x402_digest(from, to, value_wei, valid_after, valid_before, nonce)?;
    Ok(crate::wallet::sign_hash(signer, &digest))
}

pub(crate) fn encode_settle(
    from: &[u8; 20],
    to: &[u8; 20],
    value_wei: u128,
    valid_after: u64,
    valid_before: u64,
    nonce: &[u8; 32],
    signature: &[u8; 65],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 32 * 9 + 96);
    out.extend_from_slice(&selector(
        "settle(address,address,uint256,uint256,uint256,bytes32,bytes)",
    ));
    out.extend_from_slice(&addr_word(from));
    out.extend_from_slice(&addr_word(to));
    out.extend_from_slice(&u256_be(value_wei));
    out.extend_from_slice(&u256_be(valid_after as u128));
    out.extend_from_slice(&u256_be(valid_before as u128));
    out.extend_from_slice(nonce);
    out.extend_from_slice(&u256_be(7 * 32)); // offset to the `bytes` arg
    out.extend_from_slice(&u256_be(signature.len() as u128)); // 65
    out.extend_from_slice(signature);
    out.resize(out.len() + 31, 0); // pad 65 -> 96 (32-byte multiple)
    out
}

/// Submit an x402 settlement (sponsored). `submitter` is the payee /
/// facilitator (signs the Tempo tx); fees paid by `fee_payer`. Moves
/// `value_wei` `$LH` from the signed authorization's payer to `to`.
/// The payer must have `approve`d the diamond for `$LH` once.
#[allow(clippy::too_many_arguments)]
pub async fn settle_x402_sponsored(
    submitter: &SigningKey,
    fee_payer: &SigningKey,
    from: &[u8; 20],
    to: &[u8; 20],
    value_wei: u128,
    valid_after: u64,
    valid_before: u64,
    nonce: &[u8; 32],
    signature: &[u8; 65],
    fee_token: &str,
) -> Result<String, String> {
    let diamond_addr = parse_eth_address(REGISTRY_ADDRESS)?;
    let call = crate::tempo_tx::TempoCall {
        to: diamond_addr,
        value_wei: 0,
        input: encode_settle(from, to, value_wei, valid_after, valid_before, nonce, signature),
    };
    submit_tempo_sponsored(submitter, fee_payer, vec![call], fee_token, 400_000).await
}

/// Read `authorizationState(from, nonce)` — true if that x402 nonce was
/// already settled (lets a payee detect replays before serving).
pub async fn x402_authorization_state(
    from_hex: &str,
    nonce: &[u8; 32],
) -> Result<bool, String> {
    let from = parse_eth_address(from_hex)?;
    let result = read_view(
        selector("authorizationState(address,bytes32)"),
        &[addr_word(&from), *nonce],
    )
    .await?;
    Ok(decode_u256_as_u64(&result).map(|v| v != 0).unwrap_or(false))
}

/// A fresh random 32-byte x402 nonce (CSPRNG via `getrandom`). Each
/// `PaymentAuthorization` needs a unique nonce — the on-chain `settle`
/// records it one-shot, so a replayed nonce reverts.
pub fn random_x402_nonce() -> [u8; 32] {
    use rand_core::RngCore;
    let mut n = [0u8; 32];
    rand_core::OsRng.fill_bytes(&mut n);
    n
}


#[cfg(test)]
mod x402_tests {
    use super::*;

    #[test]
    fn x402_domain_matches_live_facet() {
        // Pinned to the deployed X402Facet's `x402DomainSeparator()` on the
        // diamond — guards the Rust EIP-712 encoding against the contract.
        let expected =
            "54530933a67f96286ac528dbff39d00c0ea49f4c6bd0f034343a0c78927f0b7a";
        let got = x402_domain_separator().unwrap();
        assert_eq!(bytes_to_hex(&got), expected);
    }

    #[test]
    fn x402_sign_recovers_payer() {
        let w = crate::wallet::generate();
        let from = w.address;
        let to = [0x11u8; 20];
        let nonce = [0x22u8; 32];
        let sig = sign_x402(&w.signer, &from, &to, 1_000, 0, 9_999_999_999, &nonce).unwrap();
        let digest = x402_digest(&from, &to, 1_000, 0, 9_999_999_999, &nonce).unwrap();
        // EIP-712 digest is signed directly (no personal-sign prefix).
        let recovered = crate::wallet::recover_address(&sig, &digest).unwrap();
        assert_eq!(recovered, from);
    }

    /// x402 `settle(...)` calldata: the dynamic-`bytes` signature must be
    /// pointed at by the right offset (7 words) and length-prefixed with 65,
    /// then zero-padded to a 32-byte multiple. A wrong offset/length makes the
    /// facet read a bogus signature → reject (or worse, accept the wrong one).
    #[test]
    fn settle_calldata_layout() {
        let from = [0x11u8; 20];
        let to = [0x22u8; 20];
        let nonce = [0x33u8; 32];
        let sig = [0x44u8; 65];
        let value = 7_000u128;
        let cd = encode_settle(&from, &to, value, 1, 2, &nonce, &sig);
        assert_eq!(
            &cd[0..4],
            &selector("settle(address,address,uint256,uint256,uint256,bytes32,bytes)")
        );
        // 6 static words + offset word + length word + 65 sig + 31 pad = 96 tail.
        assert_eq!(cd.len(), 4 + 6 * 32 + 32 + 32 + 96);
        assert_eq!(&cd[4 + 12..4 + 32], &from); // word 0
        assert_eq!(&cd[4 + 44..4 + 64], &to); // word 1
        assert_eq!(u128::from_be_bytes(cd[4 + 80..4 + 96].try_into().unwrap()), value); // word 2
        assert_eq!(&cd[4 + 5 * 32..4 + 6 * 32], &nonce); // word 5
        // Word 6 = offset to the bytes arg = 7*32 = 224.
        assert_eq!(u64::from_be_bytes(cd[4 + 6 * 32 + 24..4 + 7 * 32].try_into().unwrap()), 7 * 32);
        // Word 7 = bytes length = 65.
        assert_eq!(u64::from_be_bytes(cd[4 + 7 * 32 + 24..4 + 8 * 32].try_into().unwrap()), 65);
        // The 65 signature bytes follow, then zero padding to a 32-multiple.
        assert_eq!(&cd[4 + 8 * 32..4 + 8 * 32 + 65], &sig);
        assert_eq!(&cd[4 + 8 * 32 + 65..], &[0u8; 31]);
    }
}
