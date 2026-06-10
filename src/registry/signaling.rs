use k256::ecdsa::SigningKey;

use super::*;

// ─── Agent-teams P2P signaling (SignalingFacet) ────────────────────────────
// The on-chain seam for the WebRTC collaboration layer: a peer announces an
// EPHEMERAL signaling key under a TOPIC, discovers others via `peersOf`, then
// exchanges SDP offers/answers through `postSignal`/`inboxOf` (blobs sealed to
// the recipient's ephemeral pubkey). Topics:
//   - own devices: keccak256("localharness.devices" || owner_addr)
//   - agent team:  keccak256("localharness.team"   || team_id)
// `Presence` and `Signal` share the ABI shape `(address, uint64, bytes)`, so one
// decoder serves both reads.

/// Signaling topic for an owner's OWN devices.
pub fn devices_topic(owner_addr: &str) -> [u8; 32] {
    let mut pre = b"localharness.devices".to_vec();
    if let Ok(a) = parse_eth_address(owner_addr) {
        pre.extend_from_slice(&a);
    }
    keccak_key(&pre)
}

/// Signaling topic for an agent team.
pub fn team_topic(team_id: u64) -> [u8; 32] {
    let mut pre = b"localharness.team".to_vec();
    pre.extend_from_slice(&u256_be(team_id as u128));
    keccak_key(&pre)
}

/// 32-byte ABI word for an address (left-padded).
pub(crate) fn address_word(addr: &[u8; 20]) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[12..32].copy_from_slice(addr);
    w
}

/// ABI-encode a trailing dynamic `bytes` (length word + padded data) onto `d`.
pub(crate) fn push_abi_bytes(d: &mut Vec<u8>, bytes: &[u8]) {
    d.extend_from_slice(&u256_be(bytes.len() as u128));
    d.extend_from_slice(bytes);
    let pad = (32 - (bytes.len() % 32)) % 32;
    d.extend(std::iter::repeat(0u8).take(pad));
}

/// The 32-byte digest the OWNER signs to authorize an `announce`:
/// `keccak256(topic || ephemeral || pubkey)` — `abi.encodePacked(bytes32,
/// address, bytes)` on-chain. MUST match `SignalingFacet.announce`'s digest
/// byte-for-byte (topic[32] ‖ ephemeral_addr[20] ‖ raw pubkey).
pub fn announce_digest(topic: &[u8; 32], ephemeral: &[u8; 20], pubkey: &[u8]) -> [u8; 32] {
    let mut pre = Vec::with_capacity(32 + 20 + pubkey.len());
    pre.extend_from_slice(topic);
    pre.extend_from_slice(ephemeral);
    pre.extend_from_slice(pubkey);
    keccak32(&pre)
}

pub(crate) fn encode_announce(
    topic: &[u8; 32],
    owner: &[u8; 20],
    ephemeral: &[u8; 20],
    pubkey: &[u8],
    sig: &[u8; 65],
) -> Vec<u8> {
    // announce(bytes32 topic, address owner, address ephemeral, bytes pubkey,
    //          bytes sig). Head: topic, owner, ephemeral, off(pubkey), off(sig)
    // = 5 words. Two trailing dynamic `bytes` (pubkey then sig).
    let mut d = selector("announce(bytes32,address,address,bytes,bytes)").to_vec();
    d.extend_from_slice(topic);
    d.extend_from_slice(&address_word(owner));
    d.extend_from_slice(&address_word(ephemeral));
    // 5 head words = 0xa0 bytes before the first dynamic payload.
    d.extend_from_slice(&u256_be(0xa0)); // offset to `pubkey`
    // pubkey tail = len word + padded data; sig follows it.
    let pubkey_tail = 32 + ((pubkey.len() + 31) / 32) * 32;
    d.extend_from_slice(&u256_be((0xa0 + pubkey_tail) as u128)); // offset to `sig`
    push_abi_bytes(&mut d, pubkey);
    push_abi_bytes(&mut d, sig);
    d
}

/// Announce `ephemeral` + `pubkey` under `topic` (sponsored; tx caller =
/// `sender`/master). `owner` is the seed-holder whose key authorizes the
/// announcement; the digest `keccak256(topic||ephemeral||pubkey)` is signed
/// with `owner_key` (the same seed key — `sender` and `owner_key` are the same
/// wallet on the owner's device). The facet recovers the sig vs `owner` and
/// requires `topic == devices_topic(owner)`, so an attacker without the seed
/// can't populate the roster.
#[allow(clippy::too_many_arguments)]
pub async fn announce_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    owner_key: &SigningKey,
    owner: &[u8; 20],
    topic: &[u8; 32],
    ephemeral: &[u8; 20],
    pubkey: &[u8],
    fee_token: &str,
) -> Result<String, String> {
    let digest = announce_digest(topic, ephemeral, pubkey);
    let sig = crate::wallet::sign_hash(owner_key, &digest); // low-s r‖s‖v, v∈{27,28}
    let gas = 1_200_000u128 + (pubkey.len() as u128) * 9_000;
    sponsored_diamond_call(
        sender,
        fee_payer,
        encode_announce(topic, owner, ephemeral, pubkey, &sig),
        fee_token,
        gas,
    )
    .await
}

pub(crate) fn encode_post_signal(to: &[u8; 20], blob: &[u8]) -> Vec<u8> {
    let mut d = selector("postSignal(address,bytes)").to_vec();
    d.extend_from_slice(&address_word(to));
    d.extend_from_slice(&u256_be(0x40)); // offset to `blob` (2 head words in)
    push_abi_bytes(&mut d, blob);
    d
}

/// Post a signaling blob (an SDP offer/answer/ICE bundle, sealed to `to`) into
/// `to`'s inbox (sponsored).
pub async fn post_signal_sponsored(
    sender: &SigningKey,
    fee_payer: &SigningKey,
    to: &[u8; 20],
    blob: &[u8],
    fee_token: &str,
) -> Result<String, String> {
    let gas = 1_200_000u128 + (blob.len() as u128) * 9_000;
    sponsored_diamond_call(sender, fee_payer, encode_post_signal(to, blob), fee_token, gas).await
}

/// One discovered/received entry. `peersOf` → (ephemeral, ts, pubkey);
/// `inboxOf` → (from, ts, blob).
pub type AddrTsBytes = (String, u64, Vec<u8>);

/// Decode an ABI `(address, uint64, bytes)[]` return — the shared shape of
/// `Presence[]` (peersOf) and `Signal[]` (inboxOf). Bounds-checked: a malformed
/// word stops decoding rather than panicking.
pub(crate) fn decode_addr_ts_bytes_array(result_hex: &str) -> Vec<AddrTsBytes> {
    let raw = match hex_to_bytes(result_hex) {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };
    // `read_usize` reads the low 8 bytes of a 32-byte word — so any offset or
    // length is an attacker-controlled value up to u64::MAX. Every derived index
    // below uses checked arithmetic so a hostile word stops the decode (returns
    // what was parsed so far) instead of overflowing (panic in debug / wraparound
    // garbage in release) or slicing out of bounds.
    let read_usize = |off: usize| -> Option<usize> {
        let end = off.checked_add(32)?;
        let w = raw.get(off..end)?;
        Some(u64::from_be_bytes(w[24..32].try_into().ok()?) as usize)
    };
    let mut out = Vec::new();
    let arr_off = match read_usize(0) {
        Some(o) => o,
        None => return out,
    };
    let len = match read_usize(arr_off) {
        Some(l) => l,
        None => return out,
    };
    let heads = match arr_off.checked_add(32) {
        Some(h) => h, // element offsets are relative to here
        None => return out,
    };
    for i in 0..len {
        // head slot for element i = heads + i*32
        let head_slot = match i.checked_mul(32).and_then(|o| heads.checked_add(o)) {
            Some(s) => s,
            None => break,
        };
        let elem = match read_usize(head_slot) {
            Some(rel) => match heads.checked_add(rel) {
                Some(e) => e,
                None => break,
            },
            None => break,
        };
        let addr = match elem
            .checked_add(12)
            .zip(elem.checked_add(32))
            .and_then(|(a, b)| raw.get(a..b))
        {
            Some(a) => format!("0x{}", bytes_to_hex(a)),
            None => break,
        };
        let ts = match elem
            .checked_add(56)
            .zip(elem.checked_add(64))
            .and_then(|(a, b)| raw.get(a..b))
        {
            Some(t) => u64::from_be_bytes(t.try_into().unwrap_or_default()),
            None => break,
        };
        let boff = match elem.checked_add(64).and_then(read_usize) {
            // bytes offset is relative to the element
            Some(rel) => match elem.checked_add(rel) {
                Some(b) => b,
                None => break,
            },
            None => break,
        };
        let blen = match read_usize(boff) {
            Some(l) => l,
            None => break,
        };
        let bytes = boff
            .checked_add(32)
            .and_then(|start| start.checked_add(blen).map(|end| (start, end)))
            .and_then(|(start, end)| raw.get(start..end))
            .map(|s| s.to_vec())
            .unwrap_or_default();
        out.push((addr, ts, bytes));
    }
    out
}

/// The ephemeral peers announced under `topic` (peersOf). Callers filter stale
/// entries by the `ts` field.
pub async fn peers_of(topic: &[u8; 32]) -> Result<Vec<AddrTsBytes>, String> {
    let res = read_view(selector("peersOf(bytes32)"), &[*topic]).await?;
    Ok(decode_addr_ts_bytes_array(&res))
}

/// `peer`'s signaling inbox from `from_index` onward (inboxOf). The caller
/// tracks its own cursor.
pub async fn inbox_of(peer: &[u8; 20], from_index: u64) -> Result<Vec<AddrTsBytes>, String> {
    let res = read_view(
        selector("inboxOf(address,uint256)"),
        &[address_word(peer), u256_be(from_index as u128)],
    )
    .await?;
    Ok(decode_addr_ts_bytes_array(&res))
}

/// `peer`'s inbox length (a cheap cursor poll).
pub async fn inbox_length(peer: &[u8; 20]) -> Result<u64, String> {
    let res = read_view(selector("inboxLength(address)"), &[address_word(peer)]).await?;
    let raw = hex_to_bytes(&res)?;
    if raw.len() < 32 {
        return Ok(0);
    }
    Ok(u64::from_be_bytes(raw[24..32].try_into().map_err(|_| "bad len")?))
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_presence_signal_array() {
        // Hand-crafted ABI `(address, uint64, bytes)[]` with one element:
        // (0x11..11, ts=5, bytes=[0xAA, 0xBB]) — the Presence/Signal shape that
        // peersOf/inboxOf return. Verifies the nested-offset decode.
        let hex = String::from("0x")
            + "0000000000000000000000000000000000000000000000000000000000000020" // array offset
            + "0000000000000000000000000000000000000000000000000000000000000001" // len = 1
            + "0000000000000000000000000000000000000000000000000000000000000020" // head[0] offset
            + "0000000000000000000000001111111111111111111111111111111111111111" // address
            + "0000000000000000000000000000000000000000000000000000000000000005" // ts = 5
            + "0000000000000000000000000000000000000000000000000000000000000060" // bytes offset
            + "0000000000000000000000000000000000000000000000000000000000000002" // bytes len = 2
            + "aabb000000000000000000000000000000000000000000000000000000000000"; // bytes data
        let out = decode_addr_ts_bytes_array(&hex);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, "0x1111111111111111111111111111111111111111");
        assert_eq!(out[0].1, 5);
        assert_eq!(out[0].2, vec![0xAA, 0xBB]);
        // An empty array decodes to nothing (no panic).
        let empty = String::from("0x")
            + "0000000000000000000000000000000000000000000000000000000000000020"
            + "0000000000000000000000000000000000000000000000000000000000000000";
        assert!(decode_addr_ts_bytes_array(&empty).is_empty());
    }

    #[test]
    fn devices_topic_preimage_is_label_then_raw_address() {
        // MUST equal keccak256("localharness.devices" || owner_20bytes), the
        // SAME preimage SignalingFacet recomputes on-chain as
        // keccak256(abi.encodePacked("localharness.devices", owner)). Any drift
        // here silently breaks the owner-gated announce (topic != devicesTopic).
        let owner = "0x1111111111111111111111111111111111111111";
        let topic = devices_topic(owner);
        let mut pre = b"localharness.devices".to_vec();
        pre.extend_from_slice(&parse_eth_address(owner).unwrap());
        assert_eq!(topic, keccak_key(&pre));
        // The label is 20 ASCII bytes; the address is appended raw (20 bytes),
        // so the preimage is exactly 40 bytes.
        assert_eq!(pre.len(), 40);
    }

    #[test]
    fn announce_digest_is_packed_topic_ephemeral_pubkey() {
        // The owner signs keccak256(topic || ephemeral || pubkey) — matching
        // SignalingFacet's keccak256(abi.encodePacked(bytes32, address, bytes)).
        // topic[32] ‖ ephemeral_addr[20] ‖ raw pubkey, NO padding.
        let topic = [0xABu8; 32];
        let eph = [0x22u8; 20];
        let pubkey = vec![0x02u8; 33];
        let mut pre = Vec::new();
        pre.extend_from_slice(&topic);
        pre.extend_from_slice(&eph);
        pre.extend_from_slice(&pubkey);
        assert_eq!(pre.len(), 32 + 20 + 33);
        assert_eq!(announce_digest(&topic, &eph, &pubkey), keccak32(&pre));
    }

    #[test]
    fn announce_digest_signature_recovers_to_owner() {
        // Full round-trip: the sig the driver attaches MUST recover to the
        // OWNER over the announce digest — exactly what the facet's `_recover`
        // checks (`recover(...) == owner`). Proves the driver's signing path
        // and the facet's recovery path agree.
        let w = crate::wallet::generate();
        let owner = crate::wallet::address(&w.signer); // [u8;20]
        let topic = [0x11u8; 32];
        let eph = [0x99u8; 20];
        let pubkey = vec![0x03u8; 33];
        let digest = announce_digest(&topic, &eph, &pubkey);
        let sig = crate::wallet::sign_hash(&w.signer, &digest); // low-s r‖s‖v
        let recovered = crate::wallet::recover_address(&sig, &digest)
            .expect("sig recovers");
        assert_eq!(recovered, owner, "announce sig recovers to the owner");
    }

    #[test]
    fn encode_announce_5arg_layout() {
        // New owner-signed signature; pin the selector + the two-trailing-bytes
        // ABI layout so it matches announce(bytes32,address,address,bytes,bytes).
        let topic = [0x11u8; 32];
        let owner = [0x22u8; 20];
        let eph = [0x33u8; 20];
        let pubkey = vec![0x02u8; 33]; // compressed-pubkey-shaped (1 padded word past len)
        let sig = [0x44u8; 65];
        let cd = encode_announce(&topic, &owner, &eph, &pubkey, &sig);

        assert_eq!(
            &cd[..4],
            &selector("announce(bytes32,address,address,bytes,bytes)")
        );
        // Head: topic, owner, ephemeral, off(pubkey)=0xa0, off(sig)=0xa0+96=0x100.
        assert_eq!(&cd[4..36], &topic[..]);
        assert_eq!(&cd[36..68], &address_word(&owner)[..]);
        assert_eq!(&cd[68..100], &address_word(&eph)[..]);
        assert_eq!(&cd[100..132], &u256_be(0xa0)[..]); // pubkey offset
        assert_eq!(&cd[132..164], &u256_be(0x100)[..]); // sig offset (0xa0 + 0x60)
        // pubkey tail at 4 + 0xa0 = 0xa4: len word (33) then 64 padded bytes.
        let pk_off = 4 + 0xa0;
        assert_eq!(&cd[pk_off..pk_off + 32], &u256_be(33)[..]);
        assert_eq!(&cd[pk_off + 32..pk_off + 32 + 33], &pubkey[..]);
        // sig tail at 4 + 0x100 = 0x104: len word (65) then 96 padded bytes.
        let sig_off = 4 + 0x100;
        assert_eq!(&cd[sig_off..sig_off + 32], &u256_be(65)[..]);
        assert_eq!(&cd[sig_off + 32..sig_off + 32 + 65], &sig[..]);
        // Total: 4 sel + 5 head words + (32+64) pubkey + (32+96) sig.
        assert_eq!(cd.len(), 4 + 5 * 32 + (32 + 64) + (32 + 96));
    }

    #[test]
    fn addr_ts_bytes_array_empty_and_short_inputs() {
        // Totally empty RPC result ("0x").
        assert!(decode_addr_ts_bytes_array("0x").is_empty());
        // Not even one word.
        assert!(decode_addr_ts_bytes_array("0x00").is_empty());
        // Odd-length / non-hex never panics (hex_to_bytes errors → empty).
        assert!(decode_addr_ts_bytes_array("0xabc").is_empty());
        assert!(decode_addr_ts_bytes_array("0xzz").is_empty());
        assert!(decode_addr_ts_bytes_array("nonsense").is_empty());
        // Array offset points past the buffer → empty, no panic.
        let off_oob = format!("0x{}", word_usize(0x40)); // offset 64, only 32 bytes present
        assert!(decode_addr_ts_bytes_array(&off_oob).is_empty());
    }

    #[test]
    fn addr_ts_bytes_array_hostile_offsets_dont_overflow() {
        // Array offset = u64::MAX. `arr_off + 32` must NOT overflow.
        let huge_off = format!("0x{}", word_u64_max());
        assert!(decode_addr_ts_bytes_array(&huge_off).is_empty());

        // Valid array offset (0x20) + length = u64::MAX. The per-element head
        // read must stop at the buffer end, not loop u64::MAX times or overflow
        // `heads + i*32`.
        let huge_len = format!("0x{}{}", word_usize(0x20), word_u64_max());
        assert!(decode_addr_ts_bytes_array(&huge_len).is_empty());

        // One element whose head-offset word is u64::MAX → `heads + rel` overflow.
        let bad_head = String::from("0x")
            + &word_usize(0x20) // array offset
            + &word_usize(1) // len = 1
            + &word_u64_max(); // head[0] = u64::MAX (relative element offset)
        assert!(decode_addr_ts_bytes_array(&bad_head).is_empty());

        // One element whose inner bytes-offset is u64::MAX → `elem + rel` overflow.
        let bad_bytes_off = String::from("0x")
            + &word_usize(0x20) // array offset
            + &word_usize(1) // len = 1
            + &word_usize(0x20) // head[0] → element starts right after heads
            + &word_usize(0x1111) // address word
            + &word_usize(7) // ts
            + &word_u64_max(); // bytes offset = u64::MAX
        assert!(decode_addr_ts_bytes_array(&bad_bytes_off).is_empty());
    }

    #[test]
    fn addr_ts_bytes_array_multi_element_decodes() {
        // Two elements: (0x11..,1,[0xAA]) and (0x22..,2,[0xBB,0xCC]).
        // Each element is a `(address,uint64,bytes)` tuple, encoded as 5 words:
        // [addr][ts][bytes-rel-offset(0x60)][bytes-len][bytes-data].
        let elem0 = String::from("")
            + "0000000000000000000000001111111111111111111111111111111111111111" // addr
            + &word_usize(1) // ts
            + &word_usize(0x60) // bytes offset (relative to element)
            + &word_usize(1) // bytes len
            + "aa00000000000000000000000000000000000000000000000000000000000000"; // data
        let elem1 = String::from("")
            + "0000000000000000000000002222222222222222222222222222222222222222"
            + &word_usize(2)
            + &word_usize(0x60)
            + &word_usize(2)
            + "bbcc000000000000000000000000000000000000000000000000000000000000";
        // elem0 is 5 words = 0xA0 bytes. heads = arr_off(0x20)+0x20 = 0x40.
        // head[0] rel = 0x40 (2 head words), head[1] rel = 0x40 + 0xA0 = 0xE0.
        let hex = String::from("0x")
            + &word_usize(0x20) // array offset
            + &word_usize(2) // len = 2
            + &word_usize(0x40) // head[0]
            + &word_usize(0xE0) // head[1]
            + &elem0
            + &elem1;
        let out = decode_addr_ts_bytes_array(&hex);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].0, "0x1111111111111111111111111111111111111111");
        assert_eq!(out[0].1, 1);
        assert_eq!(out[0].2, vec![0xAA]);
        assert_eq!(out[1].0, "0x2222222222222222222222222222222222222222");
        assert_eq!(out[1].1, 2);
        assert_eq!(out[1].2, vec![0xBB, 0xCC]);
    }
}
