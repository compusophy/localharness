use super::*;

// --- MessageFacet (async agent INBOX) reads (#35) --------------------------
//
// The permissionless on-chain inbox: any identity can `sendMessage(toId, body)`
// — the credit proxy does this when a cross-agent `notify` can't reach a device
// via Web Push (no subscription). The recipient POLLS their inbox here so those
// notes still surface in the in-app bell next time the tab opens. Reads only;
// the write path is the proxy (and, later, an agent tool).

/// Total messages ever delivered to identity `token_id`'s inbox (read + unread).
pub async fn inbox_count(token_id: u64) -> Result<u64, String> {
    let result = read_view(selector("inboxCount(uint256)"), &[u256_be(token_id as u128)]).await?;
    decode_u256_as_u64(&result)
}

/// One inbox message by index (0-based, oldest first): `(from 0x-address, unix
/// seconds, body)`. Length-checked decode of the `(address,uint64,string)`
/// return — an attacker-set string length can't overrun the buffer.
pub async fn message_at(token_id: u64, index: u64) -> Result<(String, u64, String), String> {
    let result = read_view(
        selector("messageAt(uint256,uint256)"),
        &[u256_be(token_id as u128), u256_be(index as u128)],
    )
    .await?;
    let raw = hex_to_bytes(&result)?;
    // 4 words minimum: from, timestamp, string-offset, string-length.
    if raw.len() < 128 {
        return Err(format!("messageAt: short response {} bytes", raw.len()));
    }
    let from = format!("0x{}", crate::encoding::bytes_to_hex(&raw[12..32]));
    let timestamp = u64::from_be_bytes(
        raw[56..64].try_into().map_err(|e: std::array::TryFromSliceError| e.to_string())?,
    );
    // word[2] = byte offset of the string within the return data.
    let str_off = u64::from_be_bytes(
        raw[88..96].try_into().map_err(|e: std::array::TryFromSliceError| e.to_string())?,
    ) as usize;
    let len_pos = str_off
        .checked_add(32)
        .filter(|&p| p <= raw.len())
        .ok_or_else(|| "messageAt: bad string offset".to_string())?;
    let len = u64::from_be_bytes(
        raw[len_pos - 8..len_pos]
            .try_into()
            .map_err(|e: std::array::TryFromSliceError| e.to_string())?,
    ) as usize;
    let end = len
        .checked_add(len_pos)
        .filter(|&e| e <= raw.len())
        .ok_or_else(|| format!("messageAt: truncated body (len {len}, have {})", raw.len()))?;
    let body = String::from_utf8(raw[len_pos..end].to_vec()).map_err(|e| e.to_string())?;
    Ok((from, timestamp, body))
}
