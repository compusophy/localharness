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
//! peers apart. The signaling envelope therefore carries the sender's ephemeral
//! address itself (authenticated — see below).
//!
//! **SDP sealing (sealed + SIGNED, hard-cut v2):** each peer announces its
//! ephemeral COMPRESSED PUBKEY in the presence roster; the SDP offer/answer is
//! ECIES-sealed to the recipient's ephemeral pubkey
//! (`encryption::ecies_seal`/`ecies_open` — confidentiality: an on-chain
//! observer never sees ICE candidates/topology) and then wrapped in a
//! SENDER-SIGNED envelope ([`crate::signaling_seal`] — authenticity: the
//! sender's ephemeral key signs over `(sender, recipient, sealed)`, so a
//! third party who seals a malicious SDP to our public roster pubkey and
//! claims a legit peer's address fails recovery, and a blob can't be replayed
//! into another inbox). Receipt order: verify the envelope (sender must be
//! the roster peer we're handshaking with) BEFORE decrypting; anything that
//! fails either step is silently skipped. Pre-v2 plaintext-prefix blobs are
//! REJECTED (hard cut — no production users; see `signaling_seal` docs).
//! A peer that announced no pubkey is skipped (we can't seal to it).
//!
//! ## Roster trust
//! `SignalingFacet.announce` is now OWNER-SIGNED for the DEVICES topic: the
//! announcement carries `sig` over `keccak256(topic || ephemeral || pubkey)`,
//! and the facet recovers it vs `owner` AND requires
//! `topic == keccak256("localharness.devices" || owner)` (recomputed on-chain).
//! Since device-linking shares ONE seed across the user's devices, only the seed
//! holder can produce a valid signature — so an attacker who derives the public
//! topic but lacks the seed **cannot** put a self-chosen pubkey on the roster,
//! which closes the MITM where the attacker received the SDP offer sealed to
//! THEIR key and pulled the shared folder. The roster returned by `peersOf` for
//! the devices topic is therefore TRUSTWORTHY (every entry was signed by the
//! owner). High-s is rejected (EIP-2). (Team topics are not live-used; their
//! announce currently requires only self-consistency `sig == ephemeral` —
//! full member-gating vs `TeamFacet.isMember` is a follow-up.)
//!
//! Defence-in-depth still enforced client-side (cheap, harmless now that the
//! roster is gated):
//!   - **self-consistency**: `address(announced_pubkey) == announced_ephemeral`
//!     — reject a roster entry whose pubkey doesn't hash to the address it was
//!     announced under (a forged/mismatched seal target).
//!   - **freshness**: skip entries older than [`PRESENCE_TTL_SECS`]. Stale
//!     ephemerals from prior sessions are offline; connecting to them wastes a
//!     ~60s poll AND a sponsored on-chain offer tx each (real funds), and
//!     widens the window in which a long-lived forged entry is honoured.
//!
//! **COMPILE-VERIFIED ONLY.** The whole flow only proves out across two real
//! browsers with `SignalingFacet` cut into the diamond; the inbox isn't cleared
//! between passes. Gated on `feature = "browser-app"`.

use std::cell::RefCell;

use crate::registry;

use super::sharedfs_sync::SharedFsSync;

thread_local! {
    /// Live sessions, kept alive past the connect call — the data channel's
    /// retained closures drive the sync (same lifetime pattern as `display.rs`).
    static ACTIVE: RefCell<Vec<SharedFsSync>> = const { RefCell::new(Vec::new()) };
}

/// How recent a roster `announce` must be to be treated as ONLINE. Devices
/// re-announce at the start of every sync pass, so a peer that genuinely wants
/// to connect has a `ts` within seconds of now. Entries older than this are
/// dead sessions left in the roster (the facet never auto-expires them) — we
/// skip them so we don't burn a sponsored offer tx + a ~60s poll per ghost, and
/// so a long-stale forged entry can't be honoured indefinitely. 10 min covers
/// chain/wall-clock skew on the testnet with margin.
const PRESENCE_TTL_SECS: u64 = 600;

/// Current wall-clock time in seconds (UTC), comparable to a chain
/// `block.timestamp`. Used to age out stale roster presence.
fn now_secs() -> u64 {
    (js_sys::Date::now() / 1000.0) as u64
}

fn hex20(a: &[u8; 20]) -> String {
    crate::encoding::bytes_to_hex_str(a)
}

/// Parse a `0x…` 40-hex-char address into 20 bytes.
fn addr20(hex: &str) -> Option<[u8; 20]> {
    crate::encoding::parse_address(hex.trim()).ok()
}

/// One-shot: sync the shared folder with the owner's OTHER online devices.
/// Returns how many peers it connected to (0 = nobody else online). Best-effort.
pub(crate) async fn sync_my_devices() -> Result<usize, String> {
    let (master, _) = super::chat::credit_signer().await.ok_or("no identity")?;
    let owner = super::chat::credit_address_existing()
        .await
        .ok_or("no identity")?;
    let owner_addr = addr20(&owner).ok_or("bad owner address")?;
    let topic = registry::devices_topic(&owner);
    sync_topic(&master, &owner_addr, &topic).await
}

/// Announce under `topic`, discover peers, connect + sync each. `owner_addr` is
/// the seed holder whose key authorizes the (owner-gated) `announce` — for the
/// devices topic this MUST be the address `topic` was derived from, and
/// `master` MUST be its key (true on the owner's device: same seed wallet).
async fn sync_topic(
    master: &k256::ecdsa::SigningKey,
    owner_addr: &[u8; 20],
    topic: &[u8; 32],
) -> Result<usize, String> {
    // Ephemeral signaling identity (its address is our inbox key).
    let eph = crate::wallet::generate();
    let eph_addr = addr20(&eph.address_hex()).ok_or("bad ephemeral address")?;
    let me = hex20(&eph_addr);
    // Owner-signed announce: the facet recovers the sig over
    // keccak256(topic||ephemeral||pubkey) vs `owner_addr` and requires
    // `topic == devices_topic(owner_addr)`, so the roster is gated to the seed
    // holder. We sign with `master` (= the owner's seed key on this device).
    registry::announce_sponsored(
        master,
        master,
        // owner_key = the seed key (== master on the owner's device)
        owner_addr,
        topic,
        &eph_addr,
        &crate::wallet::pubkey_compressed(&eph.signer),
    )
    .await?;

    let peers = registry::peers_of(topic).await?;
    let now = now_secs();
    let mut connected = 0usize;
    for (peer_hex, ts, peer_pubkey) in peers {
        if peer_hex.eq_ignore_ascii_case(&me) {
            continue; // ourselves
        }
        let Some(peer_addr) = addr20(&peer_hex) else {
            continue;
        };
        // Roster gate (pure, native-tested in `signaling_seal`): the entry
        // must be FRESH (skip dead sessions — each costs a sponsored offer tx
        // + a ~60s poll — and refuse a long-stale forged entry) and
        // SELF-CONSISTENT (the announced pubkey hashes to the address it was
        // announced under, so the SDP seal target isn't a pubkey swapped in
        // under someone else's address). `ts` is a chain `block.timestamp`.
        if !crate::signaling_seal::roster_entry_valid(
            &peer_addr,
            ts,
            &peer_pubkey,
            now,
            PRESENCE_TTL_SECS,
        ) {
            continue;
        }
        if connect_and_sync(
            master,
            &eph.signer,
            &eph_addr,
            &me,
            &peer_addr,
            &peer_hex,
            &peer_pubkey,
        )
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
#[allow(clippy::too_many_arguments)]
async fn connect_and_sync(
    master: &k256::ecdsa::SigningKey,
    eph_signer: &k256::ecdsa::SigningKey,
    eph_addr: &[u8; 20],
    me_hex: &str,
    peer_addr: &[u8; 20],
    peer_hex: &str,
    peer_pubkey: &[u8],
) -> Result<(), String> {
    let session = if me_hex < peer_hex {
        // OFFERER: create the offer, seal it to the peer, sign the envelope
        // with OUR ephemeral key (the roster identity), post, await the answer.
        let (s, offer) = SharedFsSync::offer().await.map_err(|_| "offer failed")?;
        let sealed = super::encryption::ecies_seal(peer_pubkey, offer.as_bytes())
            .await
            .ok_or("seal offer failed")?;
        registry::post_signal_sponsored(
            master,
            peer_addr,
            &crate::signaling_seal::seal_envelope(eph_signer, peer_addr, &sealed),
        )
        .await?;
        let answer = poll_inbox_from(eph_signer, eph_addr, peer_addr)
            .await
            .ok_or("no answer")?;
        s.accept_answer(&answer).await.map_err(|_| "bad answer")?;
        s
    } else {
        // ANSWERER: await the offer, answer it, seal + sign the answer back.
        let offer = poll_inbox_from(eph_signer, eph_addr, peer_addr)
            .await
            .ok_or("no offer")?;
        let (s, answer) = SharedFsSync::answer(&offer).await.map_err(|_| "answer failed")?;
        let sealed = super::encryption::ecies_seal(peer_pubkey, answer.as_bytes())
            .await
            .ok_or("seal answer failed")?;
        registry::post_signal_sponsored(
            master,
            peer_addr,
            &crate::signaling_seal::seal_envelope(eph_signer, peer_addr, &sealed),
        )
        .await?;
        s
    };

    // Wait (≤10s) for the channel, then kick the union sync; keep it alive.
    for _ in 0..100 {
        if session.is_open() {
            break;
        }
        crate::runtime::sleep_ms(100).await;
    }
    session.start().await;
    ACTIVE.with(|a| a.borrow_mut().push(session));
    Ok(())
}

/// Poll our ephemeral inbox until an envelope arrives that VERIFIES as coming
/// from `from_addr` (the roster peer we're handshaking with) and addressed to
/// US (`signaling_seal::open_envelope`: magic + claimed-sender match +
/// signature recovery + recipient binding), then ECIES-open its sealed SDP
/// with our ephemeral key. Blobs that fail any check — forged sender, wrong
/// recipient (cross-inbox replay), tampered bytes, pre-v2 plaintext format —
/// are skipped silently. Capped (~60s) so a missing peer can't hang forever.
async fn poll_inbox_from(
    eph_signer: &k256::ecdsa::SigningKey,
    eph_addr: &[u8; 20],
    from_addr: &[u8; 20],
) -> Option<String> {
    for _ in 0..60 {
        if let Ok(signals) = registry::inbox_of(eph_addr, 0).await {
            for (_from_master, _ts, blob) in signals {
                if let Some(sealed) =
                    crate::signaling_seal::open_envelope(&blob, from_addr, eph_addr)
                {
                    if let Some(sdp) = super::encryption::ecies_open(eph_signer, &sealed).await {
                        if let Ok(s) = String::from_utf8(sdp) {
                            return Some(s);
                        }
                    }
                }
            }
        }
        crate::runtime::sleep_ms(1000).await;
    }
    None
}
