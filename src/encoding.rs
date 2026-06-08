//! Pure hex / address / amount encoding helpers — native-testable, no DOM, no
//! state, no async.
//!
//! These were inlined in `app::events` (a 3.6k-line file), padding the event
//! dispatcher with codec logic AND hiding them from `cargo test` (the `app`
//! module only compiles under `browser-app` + wasm32). Hoisting them to the
//! crate root — alongside `raster` and `compose` — both shrinks `events` and
//! makes them real, native-run unit tests. Step 1 of breaking up the app
//! monolith; behavior is unchanged, which the proof-of-spec gate confirms.

/// Shorten an address to `0xABCD…WXYZ` for display. Returns the input unchanged
/// if it's too short to abbreviate.
pub fn short_addr(addr: &str) -> String {
    let stripped = addr.trim_start_matches("0x");
    if stripped.len() < 8 {
        return addr.to_string();
    }
    format!("0x{}…{}", &stripped[..4], &stripped[stripped.len() - 4..])
}

/// Whether `s` is a syntactically valid 20-byte hex address (with/without `0x`).
pub fn is_address_hex(s: &str) -> bool {
    let stripped = s.trim_start_matches("0x").trim_start_matches("0X");
    stripped.len() == 40 && stripped.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Parse a human-typed amount like `1.5` or `0.000001` into 18-decimal token
/// wei. Returns None on garbage input. Accepts up to 18 fractional digits;
/// truncates anything finer.
pub fn parse_token_amount(raw: &str) -> Option<u128> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let (whole_s, frac_s) = match raw.split_once('.') {
        Some((w, f)) => (w, f),
        None => (raw, ""),
    };
    let whole: u128 = if whole_s.is_empty() {
        0
    } else {
        whole_s.parse().ok()?
    };
    if frac_s.bytes().any(|b| !b.is_ascii_digit()) {
        return None;
    }
    let mut frac: u128 = 0;
    let mut scale: u128 = 1_000_000_000_000_000_000;
    for ch in frac_s.chars().take(18) {
        let d = ch.to_digit(10)? as u128;
        scale /= 10;
        frac = frac.checked_add(d.checked_mul(scale)?)?;
    }
    let whole_wei = whole.checked_mul(1_000_000_000_000_000_000)?;
    whole_wei.checked_add(frac)
}

/// How a user-supplied transfer recipient should be resolved: a raw 20-byte
/// hex address is used as-is; anything else is treated as a subdomain name to
/// look up on-chain. Empty input is rejected up front.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recipient {
    /// A syntactically valid `0x…` 40-hex address — use it directly.
    Address(String),
    /// A subdomain name (e.g. `alice`) — resolve to its on-chain owner address.
    Name(String),
}

/// Classify a transfer `recipient` argument WITHOUT any on-chain I/O: trim it,
/// reject empty, return `Address` for a 40-hex string (preserving the original
/// `0x…` form) else `Name` (lowercased — subdomain names are lowercase). Pure,
/// so it's unit-testable; the async owner lookup lives in the caller.
pub fn classify_recipient(raw: &str) -> Result<Recipient, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("recipient is empty".to_string());
    }
    if is_address_hex(trimmed) {
        Ok(Recipient::Address(trimmed.to_string()))
    } else {
        Ok(Recipient::Name(trimmed.to_lowercase()))
    }
}

/// Parse a 40-char hex string (with/without `0x`) into 20 address bytes.
pub fn parse_address(hex: &str) -> Result<[u8; 20], String> {
    let stripped = hex.trim_start_matches("0x").trim_start_matches("0X");
    if stripped.len() != 40 {
        return Err(format!("address must be 40 hex chars, got {}", stripped.len()));
    }
    let mut out = [0u8; 20];
    let bytes = stripped.as_bytes();
    for i in 0..20 {
        let hi = hex_nibble(bytes[i * 2])?;
        let lo = hex_nibble(bytes[i * 2 + 1])?;
        out[i] = (hi << 4) | lo;
    }
    Ok(out)
}

fn hex_nibble(b: u8) -> Result<u8, String> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(format!("non-hex byte {b}")),
    }
}

/// Encode bytes as a `0x`-prefixed lowercase hex string.
pub fn bytes_to_hex_str(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(2 + bytes.len() * 2);
    s.push_str("0x");
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Shorten a tx hash to `ABCDEF…WXYZ` for display.
pub fn tx_short_hash(tx_hash: &str) -> String {
    let stripped = tx_hash.trim_start_matches("0x");
    if stripped.len() < 12 {
        return tx_hash.to_string();
    }
    format!("{}…{}", &stripped[..6], &stripped[stripped.len() - 4..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_addr_abbreviates_and_passes_through_short() {
        assert_eq!(short_addr("0x1234567890abcdef"), "0x1234…cdef");
        assert_eq!(short_addr("0xabcd"), "0xabcd"); // too short to abbreviate
    }

    #[test]
    fn is_address_hex_checks_length_and_charset() {
        assert!(is_address_hex(&format!("0x{}", "a".repeat(40))));
        assert!(is_address_hex(&"F".repeat(40)));
        assert!(!is_address_hex("0x1234")); // too short
        assert!(!is_address_hex(&"g".repeat(40))); // non-hex
    }

    #[test]
    fn parse_token_amount_handles_whole_and_fractional() {
        assert_eq!(parse_token_amount("1"), Some(1_000_000_000_000_000_000));
        assert_eq!(parse_token_amount("1.5"), Some(1_500_000_000_000_000_000));
        assert_eq!(parse_token_amount("0.000001"), Some(1_000_000_000_000));
        assert_eq!(parse_token_amount(""), None);
        assert_eq!(parse_token_amount("abc"), None);
        assert_eq!(parse_token_amount("1.2x"), None);
    }

    #[test]
    fn parse_address_roundtrips_with_bytes_to_hex() {
        let addr = "0x00112233445566778899aabbccddeeff00112233";
        let bytes = parse_address(addr).unwrap();
        assert_eq!(bytes[0], 0x00);
        assert_eq!(bytes[19], 0x33);
        assert_eq!(bytes_to_hex_str(&bytes), addr);
        assert!(parse_address("0x1234").is_err()); // wrong length
        assert!(parse_address(&"z".repeat(40)).is_err()); // non-hex
    }

    #[test]
    fn tx_short_hash_abbreviates_and_passes_through_short() {
        assert_eq!(tx_short_hash("0xabcdef1234567890"), "abcdef…7890");
        assert_eq!(tx_short_hash("0xabcd"), "0xabcd");
    }

    #[test]
    fn classify_recipient_distinguishes_address_from_name() {
        // 40-hex (with and without 0x) → Address, original form preserved.
        let addr = format!("0x{}", "a".repeat(40));
        assert_eq!(
            classify_recipient(&addr),
            Ok(Recipient::Address(addr.clone()))
        );
        let bare = "B".repeat(40);
        assert_eq!(
            classify_recipient(&format!("  {bare}  ")),
            Ok(Recipient::Address(bare.clone()))
        );
        // A subdomain name → Name, lowercased + trimmed.
        assert_eq!(
            classify_recipient("  Alice "),
            Ok(Recipient::Name("alice".to_string()))
        );
        // Wrong-length hex is NOT an address — treated as a (doomed) name.
        assert_eq!(
            classify_recipient("0x1234"),
            Ok(Recipient::Name("0x1234".to_string()))
        );
        // Empty / whitespace-only is rejected.
        assert!(classify_recipient("").is_err());
        assert!(classify_recipient("   ").is_err());
    }
}
