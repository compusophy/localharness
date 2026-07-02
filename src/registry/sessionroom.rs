//! SessionRoom driver (GitHub #22): create/manage member-gated, append-only
//! logs of ENCRYPTED key/value ops on the diamond's `SessionRoomFacet`. Op
//! sealing and CRDT folding are off-chain (`crate::kv_room` and
//! `crate::kv_reduce`); this module is only the chain I/O — sponsored writes plus
//! decoded reads. The `Op` ABI shape `(address, uint64, bytes)` matches
//! `Signal`, so reads reuse the shared `decode_addr_ts_bytes_array` decoder.

use super::*;
use k256::ecdsa::SigningKey;
use sha3::{Digest, Keccak256};

// ---- writes (sponsored) ------------------------------------------------------

pub(crate) fn encode_create_room() -> Vec<u8> {
    selector("createRoom()").to_vec()
}

pub(crate) fn encode_room_add_member(room_id: u64, member: &[u8; 20]) -> Vec<u8> {
    let mut d = selector("roomAddMember(uint256,address)").to_vec();
    d.extend_from_slice(&u256_be(room_id as u128));
    d.extend_from_slice(&addr_word(member));
    d
}

pub(crate) fn encode_append_op(room_id: u64, blob: &[u8]) -> Vec<u8> {
    let mut d = selector("appendOp(uint256,bytes)").to_vec();
    d.extend_from_slice(&u256_be(room_id as u128));
    d.extend_from_slice(&u256_be(0x40)); // offset to `blob` (2 head words in)
    push_dynamic_bytes(&mut d, blob);
    d
}

pub(crate) fn encode_clear_room(room_id: u64) -> Vec<u8> {
    let mut d = selector("clearRoom(uint256)").to_vec();
    d.extend_from_slice(&u256_be(room_id as u128));
    d
}

/// Create a room (sponsored). Caller becomes creator + first member. The new
/// room id is not returned by a sponsored tx — read it back with
/// [`room_id_created_by`] (filtered by the creator address, so it is unaffected
/// by other accounts creating rooms concurrently).
pub async fn create_room_sponsored(
    sender: &SigningKey,
) -> Result<String, String> {
    // `cast estimate createRoom()` on the live diamond = ~1.31M gas (cold
    // SSTOREs + diamond fallback routing), NOT the ~225k a bare foundry call
    // shows. Plus ~275k AA/sponsor overhead → 2M leaves headroom (CLAUDE.md:
    // cast estimate, never guess — a 1.2M cap out-of-gassed the inner call).
    sponsored_diamond_call(sender, encode_create_room(), 2_000_000).await
}

/// Enroll `member` as a writer (creator-only, sponsored).
pub async fn room_add_member_sponsored(
    sender: &SigningKey,
    room_id: u64,
    member: &[u8; 20],
) -> Result<String, String> {
    sponsored_diamond_call(
        sender,
        encode_room_add_member(room_id, member),
        1_500_000,
    )
    .await
}

/// Append a sealed op to the room log (sponsored). Length-scaled gas, matching
/// `post_signal_sponsored`.
pub async fn append_op_sponsored(
    sender: &SigningKey,
    room_id: u64,
    blob: &[u8],
) -> Result<String, String> {
    // Like createRoom, the diamond-routed write base is ~1.3M live; blob bytes
    // add cold-SSTORE cost on top (length-scaled), matching post_signal's shape.
    let gas = 2_000_000u128 + (blob.len() as u128) * 9_000;
    sponsored_diamond_call(sender, encode_append_op(room_id, blob), gas).await
}

/// Clear the room log + bump its epoch (creator-only, sponsored).
pub async fn clear_room_sponsored(
    sender: &SigningKey,
    room_id: u64,
) -> Result<String, String> {
    sponsored_diamond_call(sender, encode_clear_room(room_id), 1_500_000).await
}

// ---- reads (free) ------------------------------------------------------------

/// `roomId`'s ops from `from_index` onward. Each entry is
/// `(writer_hex, ts, blob)`; the caller opens blobs via `crate::kv_room::open_op`
/// (using `writer_hex` as the authenticated writer) and folds with
/// `crate::kv_reduce::reduce`.
pub async fn ops_of(room_id: u64, from_index: u64) -> Result<Vec<AddrTsBytes>, String> {
    let res = read_view(
        selector("opsOf(uint256,uint256)"),
        &[u256_be(room_id as u128), u256_be(from_index as u128)],
    )
    .await?;
    Ok(decode_addr_ts_bytes_array(&res))
}

pub async fn op_count(room_id: u64) -> Result<u64, String> {
    let res = read_view(selector("opCount(uint256)"), &[u256_be(room_id as u128)]).await?;
    Ok(read_word_u64(&res))
}

pub async fn room_epoch(room_id: u64) -> Result<u64, String> {
    let res = read_view(selector("roomEpoch(uint256)"), &[u256_be(room_id as u128)]).await?;
    Ok(read_word_u64(&res))
}

pub async fn room_creator(room_id: u64) -> Result<String, String> {
    let res = read_view(selector("roomCreator(uint256)"), &[u256_be(room_id as u128)]).await?;
    Ok(read_word_address(&res))
}

pub async fn room_is_member(room_id: u64, who: &[u8; 20]) -> Result<bool, String> {
    let res = read_view(
        selector("roomIsMember(uint256,address)"),
        &[u256_be(room_id as u128), addr_word(who)],
    )
    .await?;
    Ok(read_word_u64(&res) != 0)
}

/// The room's member addresses (lowercase `0x…` hex).
pub async fn room_members_of(room_id: u64) -> Result<Vec<String>, String> {
    let res = read_view(selector("roomMembersOf(uint256)"), &[u256_be(room_id as u128)]).await?;
    let bytes = hex_to_bytes(&res)?;
    // Bare dynamic `address[]` ABI return — the canonical `abi::decode_address_array`
    // (same decode as `devices_of` / `members_of_guild`).
    Ok(decode_address_array(&bytes))
}

/// The id of the most recent room created by `creator_hex`, read from
/// `RoomCreated(uint256 indexed roomId, address indexed creator)` logs (newest
/// scan window). Filtered by the creator topic, so a single caller's
/// create-then-read is race-free w.r.t. other accounts.
pub async fn room_id_created_by(creator_hex: &str) -> Result<Option<u64>, String> {
    let topic0 = format!(
        "0x{}",
        bytes_to_hex(&Keccak256::digest(b"RoomCreated(uint256,address)"))
    );
    let creator = creator_hex.trim_start_matches("0x").to_lowercase();
    let topic2 = format!("0x{:0>64}", creator);

    let latest_hex = rpc("eth_blockNumber", serde_json::json!([])).await?;
    let latest = parse_hex_quantity(&latest_hex)? as u64;
    let from = latest.saturating_sub(99_000);
    let from_hex = format!("0x{from:x}");

    // topics: [RoomCreated, <any roomId>, <this creator>]
    let topics = vec![
        serde_json::json!(topic0),
        serde_json::Value::Null,
        serde_json::json!(topic2),
    ];
    let logs = eth_get_logs(REGISTRY_ADDRESS(), topics, &from_hex).await?;

    // Return the CANONICAL (lowest = first-created) room for this creator, NOT
    // the most recent. An owner's shared volume must be STABLE: if a later
    // `createRoom` shifted the answer to a higher id, sibling subdomains (and the
    // CLI vs the browser tool) could diverge onto different rooms and split the
    // shared state. Lowest-id = the owner's first room = the one everyone agrees on.
    let mut best: Option<u64> = None;
    for log in &logs {
        if let Some(id) = log
            .get("topics")
            .and_then(|t| t.as_array())
            .and_then(|t| t.get(1))
            .and_then(|t| t.as_str())
            .and_then(|t| u64::from_str_radix(t.trim_start_matches("0x").trim_start_matches('0'), 16).ok())
        {
            best = Some(best.map_or(id, |b| b.min(id)));
        }
    }
    Ok(best)
}

// ---- local ABI decode helpers ------------------------------------------------

/// Low 8 bytes of the first 32-byte return word as a u64 (uint/bool reads).
fn read_word_u64(hex: &str) -> u64 {
    match hex_to_bytes(hex) {
        Ok(b) if b.len() >= 32 => u64::from_be_bytes(b[24..32].try_into().unwrap_or_default()),
        _ => 0,
    }
}

/// Last 20 bytes of the first return word as a lowercase `0x…` address.
fn read_word_address(hex: &str) -> String {
    match hex_to_bytes(hex) {
        Ok(b) if b.len() >= 32 => format!("0x{}", bytes_to_hex(&b[12..32])),
        _ => "0x0000000000000000000000000000000000000000".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_append_op_layout() {
        let d = encode_append_op(7, b"hi");
        // selector(4) + roomId(32) + offset(32) + len(32) + padded data(32)
        assert_eq!(d.len(), 4 + 32 + 32 + 32 + 32);
        assert_eq!(&d[..4], &selector("appendOp(uint256,bytes)"));
        // offset word == 0x40
        assert_eq!(d[4 + 32 + 31], 0x40);
        // length word == 2
        assert_eq!(d[4 + 64 + 31], 2);
    }

    #[test]
    fn read_word_helpers() {
        let hex = format!("0x{:0>64}", "2a"); // 42
        assert_eq!(read_word_u64(&hex), 42);
        let addr_hex = format!("0x{:0>24}{}", "", "a".repeat(40));
        assert_eq!(read_word_address(&addr_hex), format!("0x{}", "a".repeat(40)));
    }
}
