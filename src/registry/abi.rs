use sha3::{Digest, Keccak256};

// --- ABI encoding -------------------------------------------------------

/// Function selector = first 4 bytes of keccak256("<sig>").
pub(crate) fn selector(signature: &str) -> [u8; 4] {
    let mut h = Keccak256::new();
    h.update(signature.as_bytes());
    let digest = h.finalize();
    let mut out = [0u8; 4];
    out.copy_from_slice(&digest[..4]);
    out
}

/// Encode `idOfName(string)` calldata. ABI layout:
///   [0..4]     selector
///   [4..36]    offset to string head (always 0x20 for one dynamic arg)
///   [36..68]   string length (uint256, big-endian)
///   [68..]     string bytes, right-padded to 32-byte multiple
pub(crate) fn encode_id_of_name(name: &str) -> String {
    let sel = selector("idOfName(string)");
    let bytes = name.as_bytes();
    let len = bytes.len();
    let padded_len = len.div_ceil(32) * 32;

    let mut buf = Vec::with_capacity(4 + 32 + 32 + padded_len);
    buf.extend_from_slice(&sel);
    buf.extend_from_slice(&u256_be(0x20));
    buf.extend_from_slice(&u256_be(len as u128));
    buf.extend_from_slice(bytes);
    buf.resize(4 + 32 + 32 + padded_len, 0);

    let mut out = String::with_capacity(2 + buf.len() * 2);
    out.push_str("0x");
    for b in &buf {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Encode `register(string)` calldata. Same shape as `idOfName`.
pub(crate) fn encode_register(name: &str) -> String {
    let sel = selector("register(string)");
    let bytes = name.as_bytes();
    let len = bytes.len();
    let padded_len = len.div_ceil(32) * 32;

    let mut buf = Vec::with_capacity(4 + 32 + 32 + padded_len);
    buf.extend_from_slice(&sel);
    buf.extend_from_slice(&u256_be(0x20));
    buf.extend_from_slice(&u256_be(len as u128));
    buf.extend_from_slice(bytes);
    buf.resize(4 + 32 + 32 + padded_len, 0);

    let mut out = String::with_capacity(2 + buf.len() * 2);
    out.push_str("0x");
    for b in &buf {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

pub(crate) fn u256_be(value: u128) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[16..].copy_from_slice(&value.to_be_bytes());
    out
}

pub(crate) fn decode_u256_as_u64(hex: &str) -> Result<u64, String> {
    let stripped = hex.trim().trim_start_matches("0x");
    if stripped.is_empty() {
        return Ok(0);
    }
    if stripped.len() > 64 {
        return Err(format!("u256 hex too long: {}", stripped.len()));
    }
    // High bytes must be zero for u64.
    let high_end = stripped.len().saturating_sub(16);
    if stripped[..high_end].chars().any(|c| c != '0') {
        return Err("u256 exceeds u64 range".into());
    }
    let tail = &stripped[high_end..];
    u64::from_str_radix(tail, 16).map_err(|e| e.to_string())
}

pub(crate) fn zero_address() -> &'static str {
    "0x0000000000000000000000000000000000000000"
}

// --- shared word / dynamic-array decoders -------------------------------
//
// Solidity right-aligns scalar values in their 32-byte word, so a u64-scale
// value (id / timestamp / counter / enum) lives in the LOW 8 bytes and a
// token-wei u128 in the LOW 16. These were re-declared as local closures in
// every tuple decoder (getJob / getBounty / getProposal / tallyOf /
// reputationOf); the bare dynamic-array decoders below were copied verbatim
// across jobs_of / bounties_of / guilds_of (uint256[]) and devices_of /
// members_of_guild (address[]).

/// The LOW 8 bytes of a 32-byte ABI word as a `u64`. Panics if `w` is shorter
/// than 32 bytes — callers slice exact words out of a length-checked buffer.
pub(crate) fn u64_low(w: &[u8]) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&w[24..32]);
    u64::from_be_bytes(b)
}

/// The LOW 16 bytes of a 32-byte ABI word as a `u128` (token-wei amounts).
pub(crate) fn u128_low(w: &[u8]) -> u128 {
    let mut b = [0u8; 16];
    b.copy_from_slice(&w[16..32]);
    u128::from_be_bytes(b)
}

/// Decode a bare ABI dynamic `uint256[]` return — `[offset(32)][len(32)]
/// [elem0(32)]…` — reading the low 8 bytes of each element (ids are monotonic
/// u64-scale counters, never near 2^64). Hostile-length-safe: no pre-alloc
/// (`len` is attacker-controlled, up to u64::MAX → OOM) and checked index
/// math stops the decode at the buffer edge instead of panicking.
pub(crate) fn decode_u64_array(bytes: &[u8]) -> Vec<u64> {
    if bytes.len() < 64 {
        return Vec::new();
    }
    let mut len_buf = [0u8; 8];
    len_buf.copy_from_slice(&bytes[56..64]); // low 8 bytes of the length word
    let len = u64::from_be_bytes(len_buf) as usize;
    let mut out = Vec::new();
    for i in 0..len {
        let start = match i.checked_mul(32).and_then(|o| o.checked_add(64)) {
            Some(s) => s,
            None => break,
        };
        let Some(word) = start.checked_add(32).and_then(|end| bytes.get(start + 24..end)) else {
            break;
        };
        let mut id_buf = [0u8; 8];
        id_buf.copy_from_slice(word);
        out.push(u64::from_be_bytes(id_buf));
    }
    out
}

/// Decode a bare ABI dynamic `address[]` return — `[offset(32)][len(32)]
/// [addr0(32)]…` — into lowercase `0x…` strings (each address right-aligned
/// in its word). Same hostile-length discipline as [`decode_u64_array`].
pub(crate) fn decode_address_array(bytes: &[u8]) -> Vec<String> {
    if bytes.len() < 64 {
        return Vec::new();
    }
    let mut len_buf = [0u8; 8];
    len_buf.copy_from_slice(&bytes[56..64]); // low 8 bytes of the length word
    let len = u64::from_be_bytes(len_buf) as usize;
    let mut out = Vec::new();
    for i in 0..len {
        let start = match i.checked_mul(32).and_then(|o| o.checked_add(64)) {
            Some(s) => s,
            None => break,
        };
        let Some(word) = start
            .checked_add(32)
            .and_then(|end| bytes.get(start + 12..end))
        else {
            break;
        };
        out.push(format!("0x{}", bytes_to_hex(word)));
    }
    out
}

pub(crate) fn address_to_hex(addr: &[u8; 20]) -> String {
    let mut s = String::with_capacity(42);
    s.push_str("0x");
    for b in addr {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

// Hex primitives — thin re-uses of the crate-canonical `crate::encoding`
// codecs (byte-identical behavior AND error texts: "hex odd length" /
// "non-hex byte {b}"; `parse_hex_quantity` treats empty/`0x` as zero). The
// registry's former local copies were verbatim duplicates.
pub(crate) use crate::encoding::{bytes_to_hex, hex_to_bytes, parse_hex_quantity};


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selector_matches_known_value() {
        // keccak256("idOfName(string)") = 0x127c388a...
        // Verified independently: `cast sig "idOfName(string)"`.
        let sel = selector("idOfName(string)");
        let hex: String = sel.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "127c388a");
    }

    #[test]
    fn encode_short_name_layout() {
        let cd = encode_id_of_name("abc");
        // selector + 0x20 offset + 0x03 length + "abc" + padding
        assert!(cd.starts_with("0x127c388a"));
        // Total length: "0x" + (4 + 32 + 32 + 32) bytes * 2 chars/byte
        assert_eq!(cd.len(), 2 + (4 + 32 + 32 + 32) * 2);
    }

    #[test]
    fn decode_zero_means_available() {
        // 32-byte zero word
        let z = format!("0x{}", "0".repeat(64));
        assert_eq!(decode_u256_as_u64(&z).unwrap(), 0);
    }

    #[test]
    fn decode_normal_id() {
        // agentId = 7
        let mut s = "0".repeat(63);
        s.push('7');
        let hex = format!("0x{s}");
        assert_eq!(decode_u256_as_u64(&hex).unwrap(), 7);
    }

    #[test]
    fn decode_oversize_errors() {
        // Bit set in the upper 192 bits — can't fit in u64.
        let mut s = String::from("1");
        s.push_str(&"0".repeat(63));
        let hex = format!("0x{s}");
        assert!(decode_u256_as_u64(&hex).is_err());
    }

    #[test]
    fn hex_to_bytes_rejects_malformed_without_panic() {
        assert!(hex_to_bytes("0xabc").is_err()); // odd length
        assert!(hex_to_bytes("0xzz").is_err()); // non-hex
        assert!(hex_to_bytes("0x").unwrap().is_empty()); // empty is ok
        assert_eq!(hex_to_bytes("0xAaBb").unwrap(), vec![0xAA, 0xBB]); // case-insensitive
        assert_eq!(hex_to_bytes("deadbeef").unwrap(), vec![0xDE, 0xAD, 0xBE, 0xEF]); // no prefix
    }

    #[test]
    fn word_low_extractors_read_right_aligned_values() {
        // u64 in the low 8 bytes; u128 in the low 16 — the Solidity layout.
        let w = u256_be(0x1234_5678_9ABC_DEF0_u64 as u128);
        assert_eq!(u64_low(&w), 0x1234_5678_9ABC_DEF0);
        let big = u256_be(u128::MAX);
        assert_eq!(u128_low(&big), u128::MAX);
        // High-byte garbage above the extracted range is ignored.
        let mut noisy = u256_be(7);
        noisy[0] = 0xFF;
        assert_eq!(u64_low(&noisy), 7);
        assert_eq!(u128_low(&noisy), 7);
    }

    #[test]
    fn decode_u64_array_roundtrip_and_hostile() {
        // Canonical [offset][len][ids…].
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&u256_be(0x20));
        bytes.extend_from_slice(&u256_be(3));
        for id in [5u64, 8, 13] {
            bytes.extend_from_slice(&u256_be(id as u128));
        }
        assert_eq!(decode_u64_array(&bytes), vec![5, 8, 13]);
        // Short / empty → empty, no panic.
        assert!(decode_u64_array(&[]).is_empty());
        assert!(decode_u64_array(&[0u8; 32]).is_empty());
        // Lying length (u64::MAX) with one real element → stops at the edge,
        // no pre-alloc OOM, no overflow.
        let mut lying = Vec::new();
        lying.extend_from_slice(&u256_be(0x20));
        lying.extend_from_slice(&u64::MAX.to_be_bytes().repeat(4)); // huge len word
        lying.extend_from_slice(&u256_be(7));
        assert_eq!(decode_u64_array(&lying), vec![7]);
    }

    #[test]
    fn decode_address_array_roundtrip_and_hostile() {
        let addr = [0xABu8; 20];
        let mut word = [0u8; 32];
        word[12..].copy_from_slice(&addr);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&u256_be(0x20));
        bytes.extend_from_slice(&u256_be(1));
        bytes.extend_from_slice(&word);
        let out = decode_address_array(&bytes);
        assert_eq!(out, vec![format!("0x{}", "ab".repeat(20))]);
        // Short / hostile-length inputs degrade to what's available.
        assert!(decode_address_array(&[]).is_empty());
        assert!(decode_address_array(&[0u8; 63]).is_empty());
        let mut lying = Vec::new();
        lying.extend_from_slice(&u256_be(0x20));
        lying.extend_from_slice(&u256_be(1000)); // claims 1000 entries
        lying.extend_from_slice(&word); // only one present
        assert_eq!(decode_address_array(&lying).len(), 1);
    }
}
