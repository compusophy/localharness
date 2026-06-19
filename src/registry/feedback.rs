use k256::ecdsa::SigningKey;

use super::*;

/// One harvested `FeedbackSubmitted` event from the registry diamond.
#[derive(Debug, Clone)]
pub struct FeedbackEntry {
    /// Submitter address (`0x…`, lowercase).
    pub sender: String,
    /// Unix seconds the contract stamped at submission.
    pub timestamp: u64,
    /// The feedback text.
    pub text: String,
}

/// ABI-encode `submitFeedback(string)`: selector + offset(0x20) + length +
/// the UTF-8 bytes padded to a 32-byte boundary.
pub fn encode_submit_feedback(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let padded = len.div_ceil(32) * 32;
    let mut buf = Vec::with_capacity(4 + 64 + padded);
    buf.extend_from_slice(&selector("submitFeedback(string)"));
    buf.extend_from_slice(&u256_be(0x20));
    buf.extend_from_slice(&u256_be(len as u128));
    buf.extend_from_slice(bytes);
    buf.resize(4 + 64 + padded, 0);
    buf
}

/// Submit on-chain feedback via `FeedbackFacet.submitFeedback`, sponsored.
/// Gas is LENGTH-SCALED: the facet stores the full string in cold SSTOREs
/// (~1.3M for a short note up to ~17M near the 2048-byte cap), so a flat cap
/// silently out-of-gasses long notes (see CLAUDE.md feedback-gas gotcha).
pub async fn submit_feedback_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    text: &str,
    fee_token: &str,
) -> Result<String, String> {
    let gas = 1_500_000u128 + (text.len() as u128) * 9_000;
    sponsored_diamond_call(sender, fee_payer, encode_submit_feedback(text), fee_token, gas).await
}

/// Read recent `FeedbackSubmitted(address indexed sender, uint256
/// timestamp, string text)` events from the diamond, newest first.
///
/// Tempo caps `eth_getLogs` to a 100k-block window, so we scan the most
/// recent ~99k blocks (same bound as `scripts/harvest-feedback.sh`).
/// The non-indexed `(timestamp, text)` payload is ABI-decoded from the
/// log `data`; `sender` comes from the indexed topic.
pub async fn list_feedback() -> Result<Vec<FeedbackEntry>, String> {
    use sha3::{Digest, Keccak256};
    let topic0 = format!(
        "0x{}",
        bytes_to_hex(&Keccak256::digest(b"FeedbackSubmitted(address,uint256,string)"))
    );

    let latest_hex = rpc("eth_blockNumber", serde_json::json!([])).await?;
    let latest = parse_hex_quantity(&latest_hex)? as u64;
    let from = latest.saturating_sub(99_000);
    let from_hex = format!("0x{from:x}");

    let logs = eth_get_logs(REGISTRY_ADDRESS(), vec![serde_json::json!(topic0)], &from_hex).await?;

    let mut out = Vec::new();
    for log in &logs {
        let sender = log
            .get("topics")
            .and_then(|t| t.as_array())
            .and_then(|t| t.get(1))
            .and_then(|t| t.as_str())
            .map(|t| format!("0x{}", &t.trim_start_matches("0x")[24..]).to_lowercase())
            .unwrap_or_default();
        let Some(data_hex) = log.get("data").and_then(|d| d.as_str()) else {
            continue;
        };
        let Ok(bytes) = hex_to_bytes(data_hex) else { continue };
        if let Some(entry) = decode_feedback_data(&bytes, sender) {
            out.push(entry);
        }
    }
    out.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    Ok(out)
}

/// Decode a `(uint256 timestamp, string text)` ABI payload. Layout:
/// word0 = timestamp, word1 = offset (0x40), word2 = string length,
/// then the UTF-8 bytes.
pub(crate) fn decode_feedback_data(bytes: &[u8], sender: String) -> Option<FeedbackEntry> {
    if bytes.len() < 96 {
        return None;
    }
    let mut ts = [0u8; 8];
    ts.copy_from_slice(&bytes[24..32]); // low 8 bytes of the uint256
    let timestamp = u64::from_be_bytes(ts);

    let mut len_buf = [0u8; 8];
    len_buf.copy_from_slice(&bytes[88..96]); // low 8 bytes of the length word
    let len = u64::from_be_bytes(len_buf) as usize;

    // `len` is attacker-controlled — `96 + len` could overflow, so add checked.
    let end = len.checked_add(96)?;
    let text_bytes = bytes.get(96..end)?;
    let text = String::from_utf8_lossy(text_bytes).into_owned();
    Some(FeedbackEntry { sender, timestamp, text })
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_submit_feedback_abi_layout() {
        let cd = encode_submit_feedback("hi");
        assert_eq!(&cd[0..4], &selector("submitFeedback(string)"));
        assert_eq!(&cd[4..36], &u256_be(0x20), "string offset");
        assert_eq!(&cd[36..68], &u256_be(2), "string length");
        assert_eq!(&cd[68..70], b"hi");
        assert_eq!(cd.len(), 4 + 64 + 32, "selector + offset + len + padded payload");
        // A 32-byte string takes exactly one more word (no over-pad).
        assert_eq!(encode_submit_feedback(&"x".repeat(32)).len(), 4 + 64 + 32);
        assert_eq!(encode_submit_feedback(&"x".repeat(33)).len(), 4 + 64 + 64);
    }

    #[test]
    fn decode_feedback_data_edge_cases() {
        // < 96 bytes → None. (FeedbackEntry has no PartialEq → use is_none.)
        assert!(decode_feedback_data(&[], "s".into()).is_none());
        assert!(decode_feedback_data(&[0u8; 95], "s".into()).is_none());
        // Huge length word (low 8 bytes = u64::MAX) → None, no `96 + len` overflow.
        let mut buf = vec![0u8; 96];
        buf[88..96].copy_from_slice(&u64::MAX.to_be_bytes());
        assert!(decode_feedback_data(&buf, "s".into()).is_none());
        // Well-formed: ts=9, text="ab".
        let body = String::from("")
            + &word_usize(9) // timestamp
            + &word_usize(0x40) // offset
            + &word_usize(2) // text len
            + "6162000000000000000000000000000000000000000000000000000000000000";
        let bytes = hex_to_bytes(&body).unwrap();
        let entry = decode_feedback_data(&bytes, "sender".into()).unwrap();
        assert_eq!(entry.timestamp, 9);
        assert_eq!(entry.text, "ab");
        assert_eq!(entry.sender, "sender");
    }
}
