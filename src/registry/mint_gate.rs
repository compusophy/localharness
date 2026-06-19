use k256::ecdsa::SigningKey;

use super::*;

// --- Fiat on-ramp mint gate (MintGateFacet on the diamond) -----------
//
// The Stripe → Tempo on-ramp's money valve. The credit proxy signs an
// EIP-712 `FiatMint` with the dedicated fiat-issuer key on a verified
// `checkout.session.completed`; `mintFromFiat` mints `$LH` into the diamond
// escrow + credits the buyer a LOCKED balance. Domain/typehash MUST match
// `contracts/src/facets/MintGateFacet.sol`. These helpers give the CLI / native
// tests the same surface the TS proxy implements (`proxy/api/_stripe.ts`).

/// EIP-712 domain separator for the mint gate (name "localharness-mintgate",
/// version "1", `CHAIN_ID()`, diamond). Matches `fiatMintDomainSeparator()`.
pub fn fiat_mint_domain_separator() -> Result<[u8; 32], String> {
    let diamond = parse_eth_address(REGISTRY_ADDRESS())?;
    let mut dom = Vec::with_capacity(160);
    dom.extend_from_slice(&keccak32(
        b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)",
    ));
    dom.extend_from_slice(&keccak32(b"localharness-mintgate"));
    dom.extend_from_slice(&keccak32(b"1"));
    dom.extend_from_slice(&u256_be(CHAIN_ID() as u128));
    dom.extend_from_slice(&addr_word(&diamond));
    Ok(keccak32(&dom))
}

/// EIP-712 digest of a `FiatMint` (what the fiat-issuer key signs).
pub fn fiat_mint_digest(
    to: &[u8; 20],
    amount_wei: u128,
    receipt_id: &[u8; 32],
    valid_before: u64,
) -> Result<[u8; 32], String> {
    let mut st = Vec::with_capacity(160);
    st.extend_from_slice(&keccak32(
        b"FiatMint(address to,uint256 amount,bytes32 receiptId,uint256 validBefore)",
    ));
    st.extend_from_slice(&addr_word(to));
    st.extend_from_slice(&u256_be(amount_wei));
    st.extend_from_slice(receipt_id);
    st.extend_from_slice(&u256_be(valid_before as u128));
    let struct_hash = keccak32(&st);

    let mut pre = Vec::with_capacity(66);
    pre.extend_from_slice(&[0x19, 0x01]);
    pre.extend_from_slice(&fiat_mint_domain_separator()?);
    pre.extend_from_slice(&struct_hash);
    Ok(keccak32(&pre))
}

/// Sign a `FiatMint` with the fiat-issuer EOA key (k256 emits low-s, which the
/// facet requires). The returned 65-byte sig goes into `mintFromFiat`.
pub fn sign_fiat_mint(
    signer: &SigningKey,
    to: &[u8; 20],
    amount_wei: u128,
    receipt_id: &[u8; 32],
    valid_before: u64,
) -> Result<[u8; 65], String> {
    let digest = fiat_mint_digest(to, amount_wei, receipt_id, valid_before)?;
    Ok(crate::wallet::sign_hash(signer, &digest))
}

pub(crate) fn encode_mint_from_fiat(
    to: &[u8; 20],
    amount_wei: u128,
    receipt_id: &[u8; 32],
    valid_before: u64,
    signature: &[u8; 65],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 32 * 6 + 96);
    out.extend_from_slice(&selector(
        "mintFromFiat(address,uint256,bytes32,uint256,bytes)",
    ));
    out.extend_from_slice(&addr_word(to));
    out.extend_from_slice(&u256_be(amount_wei));
    out.extend_from_slice(receipt_id);
    out.extend_from_slice(&u256_be(valid_before as u128));
    out.extend_from_slice(&u256_be(5 * 32)); // offset to the `bytes` arg
    out.extend_from_slice(&u256_be(signature.len() as u128)); // 65
    out.extend_from_slice(signature);
    out.resize(out.len() + 31, 0); // pad 65 -> 96 (32-byte multiple)
    out
}

/// Submit `mintFromFiat` (sponsored) — anyone may submit; the signature is the
/// authorization, so `submitter` need only sign the Tempo tx (fees on
/// `fee_payer`). Used by the CLI / native parity path; the live proxy submits
/// the equivalent viem tx with its own key.
#[allow(clippy::too_many_arguments)]
pub async fn mint_from_fiat_sponsored(
    submitter: &SigningKey,
    fee_payer: &SigningKey,
    to: &[u8; 20],
    amount_wei: u128,
    receipt_id: &[u8; 32],
    valid_before: u64,
    signature: &[u8; 65],
    fee_token: &str,
) -> Result<String, String> {
    // EIP-712 verify + mint-to-escrow (cold balance/supply SSTOREs) + creditOf
    // bump + lock SSTOREs; like redeem, comfortably more than 600k cold. 2M
    // gives headroom; ~275k is Tempo sponsorship overhead.
    sponsored_diamond_call(
        submitter,
        fee_payer,
        encode_mint_from_fiat(to, amount_wei, receipt_id, valid_before, signature),
        fee_token,
        2_000_000,
    )
    .await
}

// --- Read views ------------------------------------------------------

/// `circulatingSupply()` — `$LH` held OUTSIDE the diamond escrow (totalSupply −
/// diamond balance): the figure the off-chain reconciliation alarm compares to
/// settled USD (`circulating ≤ usd_held / peg`).
pub async fn circulating_supply() -> Result<u128, String> {
    let result = read_view(selector("circulatingSupply()"), &[]).await?;
    decode_u256_as_u128(&result)
}

/// `fiatLockedOf(user)` → (locked wei still in escrow, unix unlock time).
pub async fn fiat_locked_of(account_hex: &str) -> Result<(u128, u64), String> {
    let account = parse_eth_address(account_hex)?;
    let result = read_view(selector("fiatLockedOf(address)"), &[addr_word(&account)]).await?;
    let (w0, w1) = two_words(&result);
    Ok((decode_u256_as_u128(&w0)?, decode_u256_as_u64(&w1)?))
}

/// `receiptUsed(receiptId)` — true once a receipt has minted (one-shot).
pub async fn receipt_used(receipt_id: &[u8; 32]) -> Result<bool, String> {
    let result = read_view(selector("receiptUsed(bytes32)"), &[*receipt_id]).await?;
    Ok(decode_u256_as_u64(&result).map(|v| v != 0).unwrap_or(false))
}

/// `fiatMintWindow()` → (capWei, windowSecs, windowStart, mintedInWindow). The
/// diamond's fiat-specific rolling cap (a sub-ceiling under the token-wide cap).
pub async fn fiat_mint_window() -> Result<(u128, u64, u64, u128), String> {
    let result = read_view(selector("fiatMintWindow()"), &[]).await?;
    let s = result.trim().trim_start_matches("0x");
    let word = |i: usize| format!("0x{}", s.get(i * 64..i * 64 + 64).unwrap_or(""));
    Ok((
        decode_u256_as_u128(&word(0))?,
        decode_u256_as_u64(&word(1))?,
        decode_u256_as_u64(&word(2))?,
        decode_u256_as_u128(&word(3))?,
    ))
}

/// The token-wide global mint window on `LocalharnessCredits` (the C1 backstop):
/// (capWei, windowSecs, windowStart, mintedInWindow). Reads the token directly,
/// not the diamond. `0` cap = uncapped.
pub async fn token_mint_window() -> Result<(u128, u64, u64, u128), String> {
    let cap = eth_call(LOCALHARNESS_TOKEN_ADDRESS(), &encode_call_hex(selector("mintWindowCapWei()"), &[])).await?;
    let secs = eth_call(LOCALHARNESS_TOKEN_ADDRESS(), &encode_call_hex(selector("mintWindowSecs()"), &[])).await?;
    let start = eth_call(LOCALHARNESS_TOKEN_ADDRESS(), &encode_call_hex(selector("mintWindowStart()"), &[])).await?;
    let minted = eth_call(LOCALHARNESS_TOKEN_ADDRESS(), &encode_call_hex(selector("mintedInWindow()"), &[])).await?;
    Ok((
        decode_u256_as_u128(&cap)?,
        decode_u256_as_u64(&secs)?,
        decode_u256_as_u64(&start)?,
        decode_u256_as_u128(&minted)?,
    ))
}

/// Split a 2-word ABI return into ("0x"+word0, "0x"+word1); missing words
/// decode to 0 downstream.
fn two_words(hex: &str) -> (String, String) {
    let s = hex.trim().trim_start_matches("0x");
    (
        format!("0x{}", s.get(0..64).unwrap_or("")),
        format!("0x{}", s.get(64..128).unwrap_or("")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `mintFromFiat(...)` calldata: the dynamic-`bytes` signature must sit at
    /// the right offset (5 words) and be length-prefixed with 65 then zero-padded
    /// to a 32-byte multiple — same shape as x402 `settle`.
    #[test]
    fn mint_from_fiat_calldata_layout() {
        let to = [0x11u8; 20];
        let receipt = [0x33u8; 32];
        let sig = [0x44u8; 65];
        let amount = 7_000u128;
        let cd = encode_mint_from_fiat(&to, amount, &receipt, 1_999_999_999, &sig);
        assert_eq!(&cd[0..4], &selector("mintFromFiat(address,uint256,bytes32,uint256,bytes)"));
        // 4 static words + offset + length + 65 sig + 31 pad.
        assert_eq!(cd.len(), 4 + 4 * 32 + 32 + 32 + 96);
        assert_eq!(&cd[4 + 12..4 + 32], &to); // word 0 (address, right-aligned)
        assert_eq!(u128::from_be_bytes(cd[4 + 48..4 + 64].try_into().unwrap()), amount); // word 1
        assert_eq!(&cd[4 + 2 * 32..4 + 3 * 32], &receipt); // word 2 (bytes32)
        // word 4 = offset = 5*32 = 160.
        assert_eq!(u64::from_be_bytes(cd[4 + 4 * 32 + 24..4 + 5 * 32].try_into().unwrap()), 5 * 32);
        // word 5 = length = 65.
        assert_eq!(u64::from_be_bytes(cd[4 + 5 * 32 + 24..4 + 6 * 32].try_into().unwrap()), 65);
        assert_eq!(&cd[4 + 6 * 32..4 + 6 * 32 + 65], &sig);
        assert_eq!(&cd[4 + 6 * 32 + 65..], &[0u8; 31]);
    }

    /// The fiat-issuer signature must recover to the issuer over the SAME digest
    /// the facet recomputes — cross-checks the Rust EIP-712 encoding. Chain-
    /// agnostic: the domain binds the ACTIVE chain's diamond (non-empty on every
    /// preset), and the sign→recover round-trip is self-consistent regardless of
    /// which chain is active, so no `mainnet`-feature gate is needed.
    #[test]
    fn fiat_mint_sign_recovers_issuer() {
        let w = crate::wallet::generate();
        let to = [0x22u8; 20];
        let receipt = [0xABu8; 32];
        let sig = sign_fiat_mint(&w.signer, &to, 1_000_000, &receipt, 9_999_999_999).unwrap();
        let digest = fiat_mint_digest(&to, 1_000_000, &receipt, 9_999_999_999).unwrap();
        let recovered = crate::wallet::recover_address(&sig, &digest).unwrap();
        assert_eq!(recovered, w.address);
    }

    #[test]
    fn two_words_splits_and_pads() {
        let (a, b) = two_words(&format!("0x{}{}", "0".repeat(63) + "a", "0".repeat(63) + "5"));
        assert_eq!(decode_u256_as_u128(&a).unwrap(), 10);
        assert_eq!(decode_u256_as_u64(&b).unwrap(), 5);
        // Short input → second word empty → decodes to 0, never panics.
        let (_, z) = two_words("0x00");
        assert_eq!(decode_u256_as_u64(&z).unwrap(), 0);
    }
}
