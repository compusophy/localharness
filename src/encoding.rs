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
///
/// The all-zero address is rejected: a `$LH` `transfer` to `0x0` burns the
/// funds irrecoverably (the chain may or may not revert), and it's never a
/// legitimate payee — far more likely a typo or an empty-input slip-through.
/// Catching it here protects every funding path (`send`/`send_lh`/`mcp-call`)
/// at the single pure choke point they all share.
pub fn classify_recipient(raw: &str) -> Result<Recipient, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("recipient is empty".to_string());
    }
    if is_address_hex(trimmed) {
        if is_zero_address(trimmed) {
            return Err("refusing to send to the zero address (0x0) — funds would be burned".to_string());
        }
        Ok(Recipient::Address(trimmed.to_string()))
    } else {
        Ok(Recipient::Name(trimmed.to_lowercase()))
    }
}

/// Whether a 40-hex address string (with/without `0x`) is the all-zero address.
/// Assumes the caller already validated it as 40-hex via [`is_address_hex`].
fn is_zero_address(s: &str) -> bool {
    let stripped = s.trim_start_matches("0x").trim_start_matches("0X");
    stripped.bytes().all(|b| b == b'0')
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

/// Encode bytes as bare lowercase hex — no `0x` prefix. The prefixed flavor
/// is [`bytes_to_hex_str`]; calldata/RLP assembly wants the bare form.
pub fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Decode a hex string (optional `0x`/`0X` prefix, surrounding whitespace
/// trimmed) into bytes. Rejects an odd nibble count and non-hex characters;
/// empty input (`""` / `0x`) decodes to no bytes.
pub fn hex_to_bytes(hex: &str) -> Result<Vec<u8>, String> {
    let trimmed = hex.trim().trim_start_matches("0x").trim_start_matches("0X");
    if trimmed.len() % 2 != 0 {
        return Err("hex odd length".into());
    }
    let bytes = trimmed.as_bytes();
    let mut out = Vec::with_capacity(trimmed.len() / 2);
    let mut i = 0;
    while i < bytes.len() {
        let hi = hex_nibble(bytes[i])?;
        let lo = hex_nibble(bytes[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Ok(out)
}

/// Like [`hex_to_bytes`], but an odd nibble count is left-padded with one `0`
/// instead of rejected — for refs/quantities where `0xabc` means `0x0abc`.
pub fn hex_to_bytes_padded(hex: &str) -> Result<Vec<u8>, String> {
    let trimmed = hex.trim().trim_start_matches("0x").trim_start_matches("0X");
    if trimmed.len() % 2 == 1 {
        hex_to_bytes(&format!("0{trimmed}"))
    } else {
        hex_to_bytes(trimmed)
    }
}

/// Parse a hex quantity (optional `0x` prefix) into a `u128`. Empty input is
/// zero — JSON-RPC returns `0x` for zero-valued quantities.
pub fn parse_hex_quantity(hex: &str) -> Result<u128, String> {
    let trimmed = hex.trim().trim_start_matches("0x");
    if trimmed.is_empty() {
        return Ok(0);
    }
    u128::from_str_radix(trimmed, 16).map_err(|e| e.to_string())
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

    /// Hostile / boundary inputs to the decimal→wei parser. A WRONG wei here
    /// moves the wrong amount of real `$LH`, and the release profile has
    /// `overflow-checks` OFF (`panic = "abort"`, no `overflow-checks = true`),
    /// so any unchecked multiply/add would WRAP silently to a bogus amount
    /// instead of panicking. These pin that every overflow path returns `None`.
    #[test]
    fn parse_token_amount_hostile_inputs() {
        // --- Overflow: whole part too large for u128 even before *1e18. ---
        // u128::MAX = 340282366920938463463374607431768211455.
        assert_eq!(parse_token_amount("340282366920938463463374607431768211455"), None);
        // A huge whole that PARSES as u128 but overflows the *1e18 scale → None,
        // NOT a wrapped-around small amount.
        assert_eq!(parse_token_amount("340282366920938463464"), None);
        // The largest whole that still fits after *1e18 round-trips exactly.
        assert_eq!(
            parse_token_amount("340282366920938463463"),
            Some(340_282_366_920_938_463_463_u128 * 1_000_000_000_000_000_000)
        );
        // A many-digit garbage number doesn't panic — just None.
        assert_eq!(parse_token_amount(&"9".repeat(60)), None);

        // --- Excess fractional precision: truncated, never rounded/overflowed. ---
        // 18 frac digits = exactly 1 wei.
        assert_eq!(parse_token_amount("0.000000000000000001"), Some(1));
        // 19+ frac digits: the sub-wei tail is dropped (truncated to 0), no panic.
        assert_eq!(parse_token_amount("0.0000000000000000009"), Some(0));
        assert_eq!(parse_token_amount(&format!("1.{}", "9".repeat(40))), {
            // 1.999…9 (40 nines) → 1 whole + .999999999999999999 (18 nines).
            Some(1_999_999_999_999_999_999)
        });

        // --- Malformed shapes. ---
        assert_eq!(parse_token_amount("1.2.3"), None); // two dots
        assert_eq!(parse_token_amount("1e5"), None); // scientific notation
        assert_eq!(parse_token_amount("-1"), None); // negative
        assert_eq!(parse_token_amount("0x10"), None); // hex
        assert_eq!(parse_token_amount("  1.5  "), Some(1_500_000_000_000_000_000)); // trims
        assert_eq!(parse_token_amount(" 1 2 "), None); // internal space
        // Zero / dot-only parse to 0 — callers gate on `> 0`, so this is safe,
        // but pin it so a future "treat 0 as error" change is a conscious choice.
        assert_eq!(parse_token_amount("0"), Some(0));
        assert_eq!(parse_token_amount("."), Some(0));
        assert_eq!(parse_token_amount("0.0"), Some(0));
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
    fn hex_codec_roundtrips_and_rejects() {
        // Bare vs prefixed encoders agree modulo the prefix.
        assert_eq!(bytes_to_hex(&[0x00, 0xab, 0xff]), "00abff");
        assert_eq!(bytes_to_hex_str(&[0x00, 0xab, 0xff]), "0x00abff");
        assert_eq!(bytes_to_hex(&[]), "");

        // Strict decoder: optional prefix, trims, case-insensitive.
        assert_eq!(hex_to_bytes("0x00abff").unwrap(), vec![0x00, 0xab, 0xff]);
        assert_eq!(hex_to_bytes("  00ABFF  ").unwrap(), vec![0x00, 0xab, 0xff]);
        assert_eq!(hex_to_bytes("0X00abff").unwrap(), vec![0x00, 0xab, 0xff]);
        assert_eq!(hex_to_bytes("").unwrap(), Vec::<u8>::new());
        assert_eq!(hex_to_bytes("0x").unwrap(), Vec::<u8>::new());
        assert!(hex_to_bytes("abc").is_err()); // odd
        assert!(hex_to_bytes("zz").is_err()); // non-hex

        // Padded decoder: odd nibble count left-pads instead of rejecting.
        assert_eq!(hex_to_bytes_padded("abc").unwrap(), vec![0x0a, 0xbc]);
        assert_eq!(hex_to_bytes_padded("0xabc").unwrap(), vec![0x0a, 0xbc]);
        assert_eq!(hex_to_bytes_padded("00abff").unwrap(), vec![0x00, 0xab, 0xff]);
        assert!(hex_to_bytes_padded("zz").is_err());

        // Round-trip.
        let data: Vec<u8> = (0..=255).collect();
        assert_eq!(hex_to_bytes(&bytes_to_hex(&data)).unwrap(), data);
    }

    #[test]
    fn parse_hex_quantity_handles_rpc_shapes() {
        assert_eq!(parse_hex_quantity("0x0"), Ok(0));
        assert_eq!(parse_hex_quantity("0x"), Ok(0)); // RPC zero
        assert_eq!(parse_hex_quantity(""), Ok(0));
        assert_eq!(parse_hex_quantity("0xff"), Ok(255));
        assert_eq!(parse_hex_quantity(" 0xDE "), Ok(222));
        assert!(parse_hex_quantity("0xzz").is_err());
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

    /// Hostile recipient inputs — these decide WHERE real `$LH` goes.
    #[test]
    fn classify_recipient_hostile_inputs() {
        // The all-zero address (with and without 0x, mixed-case 0X) is REFUSED
        // — a transfer there burns funds.
        assert!(classify_recipient(&format!("0x{}", "0".repeat(40))).is_err());
        assert!(classify_recipient(&"0".repeat(40)).is_err());
        assert!(classify_recipient(&format!("0X{}", "0".repeat(40))).is_err());
        // A near-zero address (one nonzero nibble) is a legitimate Address.
        let almost = format!("0x{}1", "0".repeat(39));
        assert_eq!(
            classify_recipient(&almost),
            Ok(Recipient::Address(almost.clone()))
        );

        // Mixed-case checksummed address is preserved verbatim (downstream
        // hex decode is case-insensitive) — NOT lowercased into a name.
        let checksum = "0xAbC0000000000000000000000000000000000123";
        assert_eq!(
            classify_recipient(checksum),
            Ok(Recipient::Address(checksum.to_string()))
        );

        // Off-by-one hex lengths are NOT addresses → treated as (doomed) names,
        // so the on-chain name lookup errors rather than sending to a malformed
        // address.
        assert!(matches!(
            classify_recipient(&format!("0x{}", "a".repeat(39))),
            Ok(Recipient::Name(_))
        ));
        assert!(matches!(
            classify_recipient(&format!("0x{}", "a".repeat(41))),
            Ok(Recipient::Name(_))
        ));

        // A 40-char ALL-HEX name collides with the address form (inherent
        // ambiguity, documented): it classifies as an Address. A real
        // subdomain name is unlikely to be exactly 40 hex chars, but pin the
        // behavior so a future change is deliberate.
        let hexname = "deadbeef".repeat(5); // 40 hex chars, no 0x
        assert!(matches!(
            classify_recipient(&hexname),
            Ok(Recipient::Address(_))
        ));

        // A non-hex name is lowercased.
        assert_eq!(
            classify_recipient("Solidity-Bob"),
            Ok(Recipient::Name("solidity-bob".to_string()))
        );
    }

    /// The address-ONLY acceptance contract for `$LH`-moving surfaces that
    /// funnel recipients through `classify_recipient` (`send_lh`, guild
    /// spends, the CLI's `tba exec`): a caller that accepts the recipient
    /// ONLY when classification yields an `Address` must see empty input,
    /// the funds-burning zero address, and anything that classifies as a
    /// `Name` (wrong-length / non-hex) rejected before any sponsored tx.
    /// This pins that filter so the zero-address guard can't regress.
    #[test]
    fn act_panel_address_only_filter() {
        // Helper mirroring an address-only caller's
        // `let Ok(Address(_)) = ... else return`.
        fn accepts(raw: &str) -> bool {
            matches!(classify_recipient(raw), Ok(Recipient::Address(_)))
        }
        // Accept: a real 40-hex address (with/without 0x).
        assert!(accepts(&format!("0x{}", "a".repeat(40))));
        assert!(accepts(&"B".repeat(40)));
        // Reject: zero address (funds would be burned).
        assert!(!accepts(&format!("0x{}", "0".repeat(40))));
        assert!(!accepts(&"0".repeat(40)));
        // Reject: empty / whitespace.
        assert!(!accepts(""));
        assert!(!accepts("   "));
        // Reject: a name (an address-only caller can't resolve names; it must
        // error out rather than send to a name-shaped string).
        assert!(!accepts("alice"));
        // Reject: off-by-one hex length (classifies as a doomed Name, not an
        // Address) — would otherwise have passed the old `is_address_hex`-only
        // check as false and been rejected, but pin it here too.
        assert!(!accepts(&format!("0x{}", "a".repeat(41))));
    }
}
