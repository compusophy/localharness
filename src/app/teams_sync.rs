//! Layer-5 orchestration for the agent-teams P2P collaboration layer.
//!
//! Ties the on-chain signaling (`registry::announce`/`peers_of`/`post_signal`/
//! `inbox_of`) to the WebRTC transport ([`SharedFsSync`]) to actually CONNECT
//! two of the owner's devices (or team members) and sync their shared folder.
//! Per "sync now":
//!   1. mint an EPHEMERAL signaling identity for this session (its address is
//!      this device's inbox; addresses also assign the offer/answer roles)
//!   2. `announce` it under the topic (own devices, or a team)
//!   3. discover the other online peers via `peersOf`
//!   4. for each, run the offer/answer handshake over the on-chain inbox — the
//!      lower ephemeral address offers, the higher answers — then open the
//!      data channel and start the union sync
//!
//! **Correlation:** `postSignal` records `from = msg.sender`, which is the MASTER
//! (sponsored) — and own-device peers share one master, so `from` can't tell
//! peers apart. The signaling blob therefore carries the sender's ephemeral
//! address itself: `"<eph_hex>\n<sdp>"`.
//!
//! **COMPILE-VERIFIED ONLY.** The whole flow only proves out across two real
//! browsers with `SignalingFacet` cut into the diamond. v1 limitations (noted):
//! the SDP rides the chain UNSEALED (DTLS still protects the data channel;
//! sealing to the peer pubkey is a privacy hardening), and the inbox isn't
//! cleared between passes. Gated on `feature = "browser-app"`.

use std::cell::RefCell;

use crate::registry;

use super::sharedfs_sync::SharedFsSync;

thread_local! {
    /// Live sessions, kept alive past the connect call — the data channel's
    /// retained closures drive the sync (same lifetime pattern as `display.rs`).
    static ACTIVE: RefCell<Vec<SharedFsSync>> = const { RefCell::new(Vec::new()) };
}

const HEXD: &[u8; 16] = b"0123456789abcdef";

fn hex20(a: &[u8; 20]) -> String {
    let mut s = String::with_capacity(42);
    s.push_str("0x");
    for b in a {
        s.push(HEXD[(b >> 4) as usize] as char);
        s.push(HEXD[(b & 0xf) as usize] as char);
    }
    s
}

/// Parse a `0x…` 40-hex-char address into 20 bytes.
fn addr20(hex: &str) -> Option<[u8; 20]> {
    let h = hex.trim().trim_start_matches("0x");
    if h.len() != 40 {
        return None;
    }
    let mut out = [0u8; 20];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(h.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

fn make_blob(sender_eph_hex: &str, sdp: &str) -> Vec<u8> {
    format!("{sender_eph_hex}\n{sdp}").into_bytes()
}

/// `(sender_eph_hex, sdp)` from a `"<eph>\n<sdp>"` blob.
fn parse_blob(bytes: &[u8]) -> Option<(String, String)> {
    let s = String::from_utf8(bytes.to_vec()).ok()?;
    let (eph, sdp) = s.split_once('\n')?;
    Some((eph.to_string(), sdp.to_string()))
}

/// One-shot: sync the shared folder with the owner's OTHER online devices.
/// Returns how many peers it connected to (0 = nobody else online). Best-effort.
pub(crate) async fn sync_my_devices() -> Result<usize, String> {
    let (master, _) = super::chat::credit_signer().await.ok_or("no identity")?;
    let owner = super::chat::credit_address_existing()
        .await
        .ok_or("no identity")?;
    let fee_payer = super::sponsor::signer().map_err(|_| "no sponsor")?;
    let topic = registry::devices_topic(&owner);
    sync_topic(&master, &fee_payer, &topic).await
}

/// Announce under `topic`, discover peers, connect + sync each.
async fn sync_topic(
    master: &k256::ecdsa::SigningKey,
    fee_payer: &k256::ecdsa::SigningKey,
    topic: &[u8; 32],
) -> Result<usize, String> {
    // Ephemeral signaling identity (its address is our inbox key).
    let eph = crate::wallet::generate();
    let eph_addr = addr20(&eph.address_hex()).ok_or("bad ephemeral address")?;
    let me = hex20(&eph_addr);
    registry::announce_sponsored(
        master,
        fee_payer,
        topic,
        &eph_addr,
        &[], // v1: empty pubkey (SDP posted unsealed — sealing deferred)
        registry::ALPHA_USD_ADDRESS,
    )
    .await?;

    let peers = registry::peers_of(topic).await?;
    let mut connected = 0usize;
    for (peer_hex, _ts, _pubkey) in peers {
        if peer_hex.eq_ignore_ascii_case(&me) {
            continue; // ourselves
        }
        let Some(peer_addr) = addr20(&peer_hex) else {
            continue;
        };
        if connect_and_sync(master, fee_payer, &eph_addr, &me, &peer_addr, &peer_hex)
            .await
            .is_ok()
        {
            connected += 1;
        }
    }
    Ok(connected)
}

/// The offer/answer handshake over the on-chain inbox + open the sync channel.
/// Lower ephemeral address offers; higher answers (so exactly one side offers).
async fn connect_and_sync(
    master: &k256::ecdsa::SigningKey,
    fee_payer: &k256::ecdsa::SigningKey,
    eph_addr: &[u8; 20],
    me_hex: &str,
    peer_addr: &[u8; 20],
    peer_hex: &str,
) -> Result<(), String> {
    let session = if me_hex < peer_hex {
        // OFFERER: create the offer, post it to the peer, await the answer.
        let (s, offer) = SharedFsSync::offer().await.map_err(|_| "offer failed")?;
        registry::post_signal_sponsored(
            master,
            fee_payer,
            peer_addr,
            &make_blob(me_hex, &offer),
            registry::ALPHA_USD_ADDRESS,
        )
        .await?;
        let answer = poll_inbox_from(eph_addr, peer_hex).await.ok_or("no answer")?;
        s.accept_answer(&answer).await.map_err(|_| "bad answer")?;
        s
    } else {
        // ANSWERER: await the offer, answer it, post the answer back.
        let offer = poll_inbox_from(eph_addr, peer_hex).await.ok_or("no offer")?;
        let (s, answer) = SharedFsSync::answer(&offer).await.map_err(|_| "answer failed")?;
        registry::post_signal_sponsored(
            master,
            fee_payer,
            peer_addr,
            &make_blob(me_hex, &answer),
            registry::ALPHA_USD_ADDRESS,
        )
        .await?;
        s
    };

    // Wait (≤10s) for the channel, then kick the union sync; keep it alive.
    for _ in 0..100 {
        if session.is_open() {
            break;
        }
        registry::sleep_ms(100).await;
    }
    session.start().await;
    ACTIVE.with(|a| a.borrow_mut().push(session));
    Ok(())
}

/// Poll our ephemeral inbox until a blob whose EMBEDDED sender == `from_hex`
/// arrives; return its SDP. Capped (~60s) so a missing peer can't hang forever.
async fn poll_inbox_from(eph_addr: &[u8; 20], from_hex: &str) -> Option<String> {
    for _ in 0..60 {
        if let Ok(signals) = registry::inbox_of(eph_addr, 0).await {
            for (_from_master, _ts, blob) in signals {
                if let Some((sender_eph, sdp)) = parse_blob(&blob) {
                    if sender_eph.eq_ignore_ascii_case(from_hex) {
                        return Some(sdp);
                    }
                }
            }
        }
        registry::sleep_ms(1000).await;
    }
    None
}
