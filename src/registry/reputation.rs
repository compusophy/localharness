use k256::ecdsa::SigningKey;

use super::*;

// --- ReputationFacet (attestation-based agent reputation) -----------------
//
// A peer-attestation reputation primitive: an agent that controls one identity
// `attest`s to ANOTHER identity it has worked with, rating the work 1..5 with a
// `workRef` (e.g. the bounty id it's attesting about). The facet stores the
// running `(attestationCount, ratingSum)` per subject so a UI/agent reads an
// average in one call, plus the enumerable attestation list. The colony engine
// auto-attests the worker after a paid cycle, so every completed cycle builds
// the worker's on-chain reputation. EXACT ABI (a sibling builds the Solidity):
//   attest(uint256 subjectTokenId, uint8 rating, bytes32 workRef)
//     reverts BadRating / UnknownSubject / SelfAttestation / AlreadyAttested
//   reputationOf(uint256 tokenId) -> (uint256 attestationCount, uint256 ratingSum)
//   attestationsOf(uint256 tokenId, uint256 start, uint256 limit)
//     -> (address[] attesters, uint8[] ratings, bytes32[] workRefs, uint256 nextCursor)
//   hasAttested(address attester, uint256 subjectTokenId, bytes32 workRef) -> bool

/// Encode `attest(uint256 subjectTokenId, uint8 rating, bytes32 workRef)` — three
/// STATIC head words: the subject tokenId (right-aligned), the `uint8 rating`
/// (right-aligned in its word — Solidity left-pads the value, so the rating byte
/// lands in the LOW byte), and the raw `bytes32 workRef` (occupies its whole word
/// as-is, NOT right-aligned). Returns raw calldata for a `TempoCall.input`. The
/// `attest_calldata_layout` test pins this byte-for-byte.
pub(crate) fn encode_attest(subject_token_id: u64, rating: u8, work_ref: &[u8; 32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 3 * 32);
    out.extend_from_slice(&selector("attest(uint256,uint8,bytes32)"));
    out.extend_from_slice(&u256_be(subject_token_id as u128)); // word 0: subjectTokenId
    out.extend_from_slice(&u256_be(rating as u128)); // word 1: uint8 rating (low byte)
    out.extend_from_slice(work_ref); // word 2: bytes32 workRef (full word, as-is)
    out
}

/// Attest to `subject_token_id` with `rating` (1..5) about `work_ref`, via a single
/// sponsored Tempo tx. The caller's identity (`attester_signer`) is the attester;
/// the on-chain facet credits the attestation to whatever address signed the tx.
/// Reverts (surfaced to the caller) on a bad rating, an unknown subject, a
/// self-attestation, or a duplicate `(attester, subject, workRef)`.
pub async fn attest_sponsored(
    attester_signer: &SigningKey,
    fee_payer: &SigningKey,
    subject_token_id: u64,
    rating: u8,
    work_ref: [u8; 32],
    fee_token: &str,
) -> Result<String, String> {
    let call = crate::tempo_tx::TempoCall {
        to: parse_eth_address(REGISTRY_ADDRESS)?,
        value_wei: 0,
        input: encode_attest(subject_token_id, rating, &work_ref),
    };
    // One struct push (attester/rating/workRef) into the subject's enumerable
    // list + two counter SSTOREs (count, sum) + an event. The FIRST attestation to a
    // subject writes all-COLD storage (the array + both counters + the dedup slot,
    // never-touched) so 600k OOG'd live — bump to 2M (over-budget is free, billed on USED).
    submit_tempo_sponsored(attester_signer, fee_payer, vec![call], fee_token, 2_000_000).await
}

/// Read `reputationOf(uint256 tokenId)` → `(attestationCount, ratingSum)`. Both
/// are `uint256` on-chain but fit a `u64` at any realistic attestation count
/// (sum <= 5 * count); decoded from the low 8 bytes of each word. `(0, 0)` for an
/// unknown/never-attested token. Read-only, no `$LH`.
pub async fn reputation_of(token_id: u64) -> Result<(u64, u64), String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok((0, 0));
    }
    let calldata = call_uint("reputationOf(uint256)", token_id);
    let result = eth_call(REGISTRY_ADDRESS, &calldata).await?;
    let bytes = hex_to_bytes(&result)?;
    if bytes.len() < 64 {
        return Ok((0, 0));
    }
    let low_u64 = |w: &[u8]| {
        let mut b = [0u8; 8];
        b.copy_from_slice(&w[24..32]);
        u64::from_be_bytes(b)
    };
    let count = low_u64(&bytes[0..32]);
    let sum = low_u64(&bytes[32..64]);
    Ok((count, sum))
}

/// Read `attestationsOf(uint256 tokenId, uint256 start, uint256 limit)` →
/// `(address[] attesters, uint8[] ratings, bytes32[] workRefs, uint256 nextCursor)`,
/// returning the parallel rows as `(attester_hex, rating, work_ref_hex)` tuples
/// (the trailing cursor is dropped — callers page by bumping `start`). The three
/// arrays are equal-length parallel; we zip by index up to the shortest. The
/// `attester_hex` is `0x`-lowercase; `work_ref_hex` is the raw 32-byte ref as
/// `0x`-hex. Bounds-checked: a hostile length/offset stops the decode rather than
/// panicking. Read-only, no `$LH`.
pub async fn attestations_of(
    token_id: u64,
    start: u64,
    limit: u64,
) -> Result<Vec<(String, u8, String)>, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(Vec::new());
    }
    let mut calldata = selector("attestationsOf(uint256,uint256,uint256)").to_vec();
    calldata.extend_from_slice(&u256_be(token_id as u128));
    calldata.extend_from_slice(&u256_be(start as u128));
    calldata.extend_from_slice(&u256_be(limit as u128));
    let calldata_hex = format!("0x{}", bytes_to_hex(&calldata));
    let result = eth_call(REGISTRY_ADDRESS, &calldata_hex).await?;
    let bytes = hex_to_bytes(&result)?;
    Ok(decode_attestations(&bytes))
}

/// Decode the `attestationsOf` return — three dynamic arrays (`address[]`,
/// `uint8[]`, `bytes32[]`) followed by a static `uint256` cursor. Head layout:
/// word 0/1/2 = offsets to each array, word 3 = the cursor. Each array body is
/// `[len][elem0][elem1]…`. Pure + bounds-checked (every derived index uses checked
/// arithmetic; a bogus offset/length yields what was parsed so far, never a panic
/// or OOM). Zips the three arrays by index up to the shortest length.
pub(crate) fn decode_attestations(bytes: &[u8]) -> Vec<(String, u8, String)> {
    // Read the low 8 bytes (an offset/length word; never near 2^64 in practice).
    let read_usize = |off: usize| -> Option<usize> {
        let end = off.checked_add(32)?;
        let w = bytes.get(off..end)?;
        Some(u64::from_be_bytes(w[24..32].try_into().ok()?) as usize)
    };
    // The body of the dynamic array whose head offset sits at `head_word` (0,1,2):
    // returns the array length + the byte offset of its first element word.
    let array_body = |head_word: usize| -> Option<(usize, usize)> {
        let off = read_usize(head_word * 32)?;
        let len = read_usize(off)?;
        let body = off.checked_add(32)?; // first element word
        Some((len, body))
    };
    let (Some((a_len, a_body)), Some((r_len, r_body)), Some((w_len, w_body))) =
        (array_body(0), array_body(1), array_body(2))
    else {
        return Vec::new();
    };
    // The arrays are parallel — zip up to the shortest so a malformed length on
    // one can't read past another.
    let n = a_len.min(r_len).min(w_len);
    let word_at = |base: usize, i: usize| -> Option<&[u8]> {
        let start = i.checked_mul(32).and_then(|o| base.checked_add(o))?;
        let end = start.checked_add(32)?;
        bytes.get(start..end)
    };
    let mut out = Vec::new();
    for i in 0..n {
        let (Some(aw), Some(rw), Some(ww)) =
            (word_at(a_body, i), word_at(r_body, i), word_at(w_body, i))
        else {
            break;
        };
        // address = low 20 bytes; rating = low byte of the uint8 word; workRef =
        // the whole 32-byte word, surfaced as 0x-hex.
        let attester = format!("0x{}", bytes_to_hex(&aw[12..32]));
        let rating = rw[31];
        let work_ref = format!("0x{}", bytes_to_hex(ww));
        out.push((attester, rating, work_ref));
    }
    out
}

/// Read `hasAttested(address attester, uint256 subjectTokenId, bytes32 workRef)`
/// → bool — whether `attester_hex` has already attested to `subject` about
/// `work_ref` (the facet's `AlreadyAttested` guard, queryable up front to skip a
/// doomed write). `false` for the zero/unset registry. Read-only.
pub async fn has_attested(
    attester_hex: &str,
    subject: u64,
    work_ref: [u8; 32],
) -> Result<bool, String> {
    if REGISTRY_ADDRESS == zero_address() {
        return Ok(false);
    }
    let attester = parse_eth_address(attester_hex)?;
    let mut calldata = selector("hasAttested(address,uint256,bytes32)").to_vec();
    calldata.extend_from_slice(&addr_word(&attester));
    calldata.extend_from_slice(&u256_be(subject as u128));
    calldata.extend_from_slice(&work_ref);
    let calldata_hex = format!("0x{}", bytes_to_hex(&calldata));
    let result = eth_call(REGISTRY_ADDRESS, &calldata_hex).await?;
    let bytes = hex_to_bytes(&result)?;
    // A bool return is a single right-aligned word: non-zero low byte = true.
    Ok(bytes.last().is_some_and(|&b| b != 0))
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attest_calldata_layout() {
        // Pin the EXACT `attest(uint256,uint8,bytes32)` wire bytes — the uint8 and
        // bytes32 packing are the easy-to-get-wrong parts, so assert each word.
        let mut work_ref = [0u8; 32];
        // A recognisable workRef: the byte pattern 0xAB.. in the high bytes plus a
        // bounty-id-style value in the low word (left-padded big-endian u64).
        work_ref[0] = 0xAB;
        work_ref[24..32].copy_from_slice(&7u64.to_be_bytes());
        let cd = encode_attest(42, 5, &work_ref);
        // selector(4) + 3 static head words = 100 bytes, no dynamic tail.
        assert_eq!(cd.len(), 4 + 3 * 32);
        assert_eq!(&cd[..4], &selector("attest(uint256,uint8,bytes32)"));
        // Word 0 = subjectTokenId, right-aligned (low 8 bytes carry the u64).
        assert_eq!(&cd[4..36], &u256_be(42)[..]);
        assert_eq!(&cd[28..36], &42u64.to_be_bytes());
        // Word 1 = the uint8 rating, value left-padded so the rating is the LOW
        // byte and the leading 31 bytes are zero.
        assert!(cd[36..67].iter().all(|&b| b == 0), "rating word must be zero-padded");
        assert_eq!(cd[67], 5, "rating must occupy the low byte of word 1");
        // Word 2 = the raw bytes32 workRef, occupying the WHOLE word as-is (NOT
        // right-aligned) — the high byte is preserved.
        assert_eq!(&cd[68..100], &work_ref[..]);
        assert_eq!(cd[68], 0xAB);
    }

    #[test]
    fn decode_attestations_zips_three_parallel_arrays() {
        // Hand-build an `attestationsOf` return: 3 dynamic arrays + a cursor word.
        // One row: attester 0x11..11, rating 3, workRef 0xCD..(low byte 9).
        let word = |hex: &str| -> String {
            assert!(hex.len() <= 64);
            format!("{:0>64}", hex)
        };
        // Head: 4 words. Offsets are byte offsets from the start of the return.
        // head = 4*32 = 128 (0x80). addr[] at 0x80, uint8[] after it (len+1 elem =
        // 2 words → next at 0x80 + 0x40 = 0xC0), bytes32[] at 0x100.
        let mut hex = String::from("0x");
        hex.push_str(&word("80")); // word0: offset to address[]
        hex.push_str(&word("c0")); // word1: offset to uint8[]
        hex.push_str(&word("100")); // word2: offset to bytes32[]
        hex.push_str(&word("5")); // word3: nextCursor = 5
        // address[]: len 1 + one element (addr 0x11..11).
        hex.push_str(&word("1"));
        hex.push_str(&word("1111111111111111111111111111111111111111"));
        // uint8[]: len 1 + rating 3 (low byte).
        hex.push_str(&word("1"));
        hex.push_str(&word("3"));
        // bytes32[]: len 1 + a full 32-byte ref (high byte CD, low byte 09).
        hex.push_str(&word("1"));
        hex.push_str("cd000000000000000000000000000000000000000000000000000000000000" );
        hex.push_str("09");
        let bytes = hex_to_bytes(&hex).unwrap();
        let rows = decode_attestations(&bytes);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "0x1111111111111111111111111111111111111111");
        assert_eq!(rows[0].1, 3);
        assert!(rows[0].2.starts_with("0xcd"));
        assert!(rows[0].2.ends_with("09"));
        // An empty/short buffer decodes to nothing (no panic).
        assert!(decode_attestations(&[]).is_empty());
        assert!(decode_attestations(&[0u8; 32]).is_empty());
    }
}
