//! host::mp multiplayer bridge (worker ↔ the proven webrtc.rs Peer).
//!
//! A multiplayer cartridge (worker) posts mp:host/mp:join → we connect over the
//! off-chain relay; mp:deltas/mp:events → we frame + send over the UNRELIABLE
//! game channel (id 1); incoming peer frames → mp:peer back to the worker mirror.
//!
//! Topology = HOST-AUTHORITATIVE STAR. The host (index 0) answers EACH joiner on
//! its own connection; a joiner connects ONLY to the host. There are no joiner↔
//! joiner links — a host-authoritative game writes the world to the host's slots
//! (peer 0) and joiners read it there. A frame's peer index is just the
//! connection it arrived on (the host assigns join order; a joiner always sees
//! the host as peer 0, itself as 1). N=1 reduces EXACTLY to the old 2-peer game
//! (host 0 ↔ joiner 1). Up to MP_MAX_PEERS total. The session (peer(s) + the
//! ephemeral relay-auth wallet) lives in a thread-local for the cartridge.

use std::cell::RefCell;

use js_sys::{Object, Reflect};
use wasm_bindgen::prelude::*;
use web_sys::Worker;

const MP_MAX_PEERS: i32 = 8; // mirrors web/cartridge-worker.js MP_PEERS

const MESH_FRESH_SECS: u64 = 40; // a slot not refreshed in this long is reclaimable
const MESH_BEAT_TICKS: u32 = 3; // heartbeat every 3rd loop tick (~12s at 4s/tick)

enum MpRole {
    /// Hub: (joinerId, assignedIndex, peer) per joiner. The index is the
    /// joiner's ROSTER position + 1 (the SAME value the joiner derives for its
    /// own `self_index`), stored so the relay matches by index even if a
    /// mid-roster joiner failed to connect (vector position ≠ index then).
    Host {
        peers: Vec<(String, i32, crate::app::webrtc::Peer)>,
    },
    /// Leaf: the single connection to the host (peer 0 from our view).
    Joiner {
        peer: crate::app::webrtc::Peer,
    },
    /// FULL MESH (no host): `peers[q]` is my direct connection to the peer at
    /// slot q (`(theirId, Peer)`), `connecting[q]` guards an in-flight
    /// handshake from being re-dialed. My own slot index is carried by the
    /// mesh loop, not stored here. A peer leaving just nulls its slot; nothing
    /// breaks. Backs `host::mp::auto`.
    Mesh {
        peers: Vec<Option<(String, crate::app::webrtc::Peer)>>,
        connecting: Vec<bool>,
    },
}
struct MpSession {
    role: MpRole,
    _gw: crate::wallet::GeneratedWallet, // keep the ephemeral signer alive
    #[allow(dead_code)]
    room: String,
}
thread_local! {
    static MP_SESSION: RefCell<Option<MpSession>> = const { RefCell::new(None) };
}

/// An 8-hex-char joiner id from the ephemeral wallet address (first 4 bytes) —
/// matches the relay's JOINER_RE (`[a-z0-9]{8}`).
fn joiner_id_from(addr: &[u8; 20]) -> String {
    let mut s = String::with_capacity(8);
    for b in &addr[0..4] {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Connect this cartridge's session for room code `code`. HOST opens the room
/// and spawns a loop that answers each joiner as they appear; a JOINER offers
/// to the host. Incoming frames route to the worker mirror; status is reported
/// as peers connect. Spawned on `mp:host`/`mp:join`.
pub(crate) async fn mp_connect(worker: Worker, code: i32, is_host: bool) {
    mp_teardown(); // a fresh connect replaces any prior session
    let room = format!("mp-{code}");
    let gw = crate::wallet::generate();
    let signer = gw.signer.clone(); // clone for the host's background answer loop

    if is_host {
        // The host IS the hub — connected immediately (index 0); joiners attach
        // over time via the accept loop.
        MP_SESSION.with(|s| {
            *s.borrow_mut() = Some(MpSession {
                role: MpRole::Host { peers: Vec::new() },
                _gw: gw,
                room: room.clone(),
            });
        });
        // Hosting but no peer yet → connected=0 (a cartridge gates its game on
        // `connected()==1`, i.e. ≥1 other player present); peerCount=1 (just us).
        mp_post_status(&worker, 0, 0, 1);
        // open()/join(): host is OUTSIDE the roster → joiner idx = roster_pos+1.
        wasm_bindgen_futures::spawn_local(mp_host_accept_loop(worker, room, signer, 1, false));
        return;
    }

    // JOINER: offer to the host. The host is peer 0 from our view; we are 1.
    let joiner_id = joiner_id_from(&crate::wallet::address(&signer));
    let worker_for_msg = worker.clone();
    let on_msg = move |bytes: Vec<u8>| mp_dispatch_peer_frame(&worker_for_msg, 0, &bytes);
    match crate::app::webrtc::Peer::offer_to_host(&room, &joiner_id, &signer, on_msg).await {
        Ok(peer) => {
            // Wait for ICE to actually open the data channel (~15s cap).
            for _ in 0..150 {
                if peer.is_open() {
                    break;
                }
                crate::runtime::sleep_ms(100).await;
            }
            let connected = i32::from(peer.is_open());
            // Our network-slot index must match the index everyone ELSE files
            // us under — the host assigns by roster position, so derive it the
            // same way. Retry briefly: our own join write can lag this read.
            let mut self_index = 1;
            for _ in 0..4 {
                let roster =
                    crate::registry::signal_get_joiners(&room).await.unwrap_or_default();
                if let Some(pos) = roster.iter().position(|id| id == &joiner_id) {
                    self_index = pos as i32 + 1;
                    break;
                }
                crate::runtime::sleep_ms(300).await;
            }
            MP_SESSION.with(|s| {
                *s.borrow_mut() = Some(MpSession {
                    role: MpRole::Joiner { peer },
                    _gw: gw,
                    room: room.clone(),
                });
            });
            mp_post_status(&worker, connected, self_index, if connected == 1 { 2 } else { 1 });
        }
        Err(e) => {
            web_sys::console::warn_1(&JsValue::from_str(&format!("mp join failed: {e:?}")));
            mp_post_status(&worker, 0, -1, 0);
        }
    }
}

/// HOST: poll the relay roster; answer each NEW joiner on its own connection,
/// assign its peer index, and report the growing peer count. Exits when the
/// session is torn down or is no longer a host session.
///
/// `idx_base`/`skip_first` differ by entry path so the host and joiner always
/// derive the SAME index: open()/join() keeps the host OUTSIDE the roster
/// (idx = roster_pos+1, skip nothing); auto() has the host AT roster[0]
/// (idx = roster_pos, skip position 0 = the host itself).
async fn mp_host_accept_loop(
    worker: Worker,
    room: String,
    signer: k256::ecdsa::SigningKey,
    idx_base: i32,
    skip_first: bool,
) {
    loop {
        // Stop if the session vanished (teardown) or is no longer a host.
        let is_host = MP_SESSION
            .with(|s| matches!(s.borrow().as_ref().map(|x| &x.role), Some(MpRole::Host { .. })));
        if !is_host {
            return;
        }
        let joiners = crate::registry::signal_get_joiners(&room).await.unwrap_or_default();
        for (roster_pos, jid) in joiners.iter().enumerate() {
            if skip_first && roster_pos == 0 {
                continue; // roster[0] is the host (us) in auto mode
            }
            let idx = roster_pos as i32 + idx_base;
            // Skip joiners we already hold, and cap the total at MP_MAX_PEERS.
            let known = MP_SESSION.with(|s| {
                if let Some(MpSession { role: MpRole::Host { peers }, .. }) = s.borrow().as_ref() {
                    peers.iter().any(|(id, _, _)| id == jid)
                } else {
                    true
                }
            });
            if known || idx >= MP_MAX_PEERS {
                continue;
            }
            let worker_for_msg = worker.clone();
            let on_msg = move |bytes: Vec<u8>| mp_dispatch_peer_frame(&worker_for_msg, idx, &bytes);
            match crate::app::webrtc::Peer::answer_joiner(&room, jid, &signer, on_msg).await {
                Ok(peer) => {
                    let count = MP_SESSION.with(|s| {
                        if let Some(MpSession { role: MpRole::Host { peers }, .. }) =
                            s.borrow_mut().as_mut()
                        {
                            peers.push((jid.clone(), idx, peer));
                            peers.len() as i32 + 1
                        } else {
                            1
                        }
                    });
                    // connected once ≥1 peer is in (peerCount ≥ 2).
                    mp_post_status(&worker, i32::from(count >= 2), 0, count);
                }
                Err(e) => {
                    web_sys::console::warn_1(&JsValue::from_str(&format!(
                        "mp answer_joiner failed: {e:?}"
                    )));
                }
            }
        }
        crate::runtime::sleep_ms(2000).await;
    }
}

/// SINGLE SHARED ROOM (`mp:auto`) — FULL P2P MESH, NO HOST. Claim a slot in the
/// 8-slot membership blob, then connect DIRECTLY to every other live peer (the
/// lower slot offers, the higher answers). A heartbeat keeps my slot fresh; a
/// peer that leaves just stops heartbeating and its slot frees for reuse — the
/// room never breaks and no host is sticky.
pub(crate) async fn mp_connect_mesh(worker: Worker, code: i32) {
    mp_teardown();
    let room = format!("mp-{code}");
    let gw = crate::wallet::generate();
    let signer = gw.signer.clone();
    let addr_bytes = crate::wallet::address(&signer);
    let my_id = joiner_id_from(&addr_bytes);
    let my_addr = crate::encoding::bytes_to_hex_str(&addr_bytes);

    let my_slot = match mesh_claim_slot(&room, &signer, &my_id, &my_addr).await {
        Ok(s) => s,
        Err(e) => {
            web_sys::console::warn_1(&JsValue::from_str(&format!("mesh claim failed: {e}")));
            mp_post_status(&worker, 0, -1, 0);
            return;
        }
    };

    MP_SESSION.with(|s| {
        *s.borrow_mut() = Some(MpSession {
            role: MpRole::Mesh {
                peers: (0..MP_MAX_PEERS).map(|_| None).collect(),
                connecting: (0..MP_MAX_PEERS).map(|_| false).collect(),
            },
            _gw: gw,
            room: room.clone(),
        });
    });
    mp_post_status(&worker, 0, my_slot, 1); // self_index = my slot; no peers yet
    wasm_bindgen_futures::spawn_local(mesh_loop(worker, room, signer, my_id, my_addr, my_slot));
}

/// Claim the LOWEST free-or-stale slot for my (id, addr). Re-entry keeps my
/// existing slot. Returns my slot index, or an error if the arena is full.
async fn mesh_claim_slot(
    room: &str,
    signer: &k256::ecdsa::SigningKey,
    my_id: &str,
    my_addr: &str,
) -> Result<i32, String> {
    for _ in 0..6 {
        let ms = crate::registry::signal_get_slots(room).await?;
        if let Some(pos) = ms.slots.iter().position(|e| {
            e.as_ref().map(|x| x.addr.eq_ignore_ascii_case(my_addr)).unwrap_or(false)
        }) {
            return Ok(pos as i32); // already mine
        }
        let free = ms.slots.iter().position(|e| match e {
            None => true,
            Some(x) => ms.now.saturating_sub(x.ts) > MESH_FRESH_SECS,
        });
        let idx = free.ok_or_else(|| "arena full (8 players)".to_string())?;
        let mut next = ms.slots.clone();
        next[idx] = Some(crate::registry::SlotEntry {
            id: my_id.to_string(),
            addr: my_addr.to_string(),
            ts: ms.now,
        });
        let now = (js_sys::Date::now() / 1000.0) as u64;
        match crate::registry::signal_put_slots(signer, now, room, &next, idx, ms.sha.as_deref()).await {
            Ok(crate::registry::PutSlots::Written) => return Ok(idx as i32),
            Ok(crate::registry::PutSlots::Conflict) => crate::runtime::sleep_ms(250).await,
            Err(e) => return Err(e),
        }
    }
    Err("slot claim contention".to_string())
}

/// Mesh session loop: heartbeat my slot, discover live peers, and dial any I
/// don't hold. Exits when the session is gone/no-longer-mesh, or my slot was
/// reclaimed (my heartbeat lapsed and someone took it → I'm out).
async fn mesh_loop(
    worker: Worker,
    room: String,
    signer: k256::ecdsa::SigningKey,
    my_id: String,
    my_addr: String,
    my_slot: i32,
) {
    let mut tick: u32 = 0;
    loop {
        let is_mesh = MP_SESSION
            .with(|s| matches!(s.borrow().as_ref().map(|x| &x.role), Some(MpRole::Mesh { .. })));
        if !is_mesh {
            return;
        }
        let ms = match crate::registry::signal_get_slots(&room).await {
            Ok(m) => m,
            Err(_) => {
                crate::runtime::sleep_ms(4000).await;
                tick += 1;
                continue;
            }
        };
        // Liveness: if my slot no longer carries my address, I was reclaimed.
        let still_mine = ms
            .slots
            .get(my_slot as usize)
            .and_then(|e| e.as_ref())
            .map(|x| x.addr.eq_ignore_ascii_case(&my_addr))
            .unwrap_or(false);
        if !still_mine && tick > 0 {
            mp_teardown();
            mp_post_status(&worker, 0, -1, 0);
            return;
        }
        // Heartbeat (~every MESH_BEAT_TICKS ticks): refresh my slot ts via CAS.
        if tick % MESH_BEAT_TICKS == 0 {
            let mut next = ms.slots.clone();
            next[my_slot as usize] = Some(crate::registry::SlotEntry {
                id: my_id.clone(),
                addr: my_addr.clone(),
                ts: ms.now,
            });
            let now = (js_sys::Date::now() / 1000.0) as u64;
            let _ = crate::registry::signal_put_slots(
                &signer, now, &room, &next, my_slot as usize, ms.sha.as_deref(),
            )
            .await;
        }
        // Discover + connect every fresh peer I'm not holding / dialing.
        for q in 0..(MP_MAX_PEERS as usize) {
            if q as i32 == my_slot {
                continue;
            }
            let entry = ms.slots[q].as_ref();
            let fresh = entry
                .map(|x| ms.now.saturating_sub(x.ts) <= MESH_FRESH_SECS)
                .unwrap_or(false);
            if !fresh {
                continue;
            }
            let their_id = entry.map(|x| x.id.clone()).unwrap_or_default();
            let skip = MP_SESSION.with(|s| {
                if let Some(MpSession { role: MpRole::Mesh { peers, connecting, .. }, .. }) =
                    s.borrow().as_ref()
                {
                    connecting[q] || peers[q].as_ref().map(|(id, _)| id == &their_id).unwrap_or(false)
                } else {
                    true
                }
            });
            if skip {
                continue;
            }
            MP_SESSION.with(|s| {
                if let Some(MpSession { role: MpRole::Mesh { connecting, .. }, .. }) =
                    s.borrow_mut().as_mut()
                {
                    connecting[q] = true;
                }
            });
            wasm_bindgen_futures::spawn_local(mesh_connect_one(
                worker.clone(), room.clone(), signer.clone(), my_slot, q as i32, their_id,
            ));
        }
        tick += 1;
        crate::runtime::sleep_ms(4000).await;
    }
}

/// Connect to the peer at slot `q`: I OFFER if I'm the lower index, else ANSWER.
/// Store the Peer in `peers[q]` on success; always clear `connecting[q]`.
async fn mesh_connect_one(
    worker: Worker,
    room: String,
    signer: k256::ecdsa::SigningKey,
    my_slot: i32,
    q: i32,
    their_id: String,
) {
    let worker_for_msg = worker.clone();
    let on_msg = move |bytes: Vec<u8>| mp_dispatch_peer_frame(&worker_for_msg, q, &bytes);
    let result = if my_slot < q {
        crate::app::webrtc::Peer::mesh_offer(&room, my_slot, q, &signer, on_msg).await
    } else {
        crate::app::webrtc::Peer::mesh_answer(&room, q, my_slot, &signer, on_msg).await
    };
    MP_SESSION.with(|s| {
        if let Some(MpSession { role: MpRole::Mesh { peers, connecting, .. }, .. }) =
            s.borrow_mut().as_mut()
        {
            connecting[q as usize] = false;
            if let Ok(peer) = result {
                peers[q as usize] = Some((their_id, peer));
            }
        }
    });
    // Let ICE open the channel (~10s), then report aggregate status.
    for _ in 0..100 {
        let open = MP_SESSION.with(|s| {
            matches!(s.borrow().as_ref().map(|x| &x.role), Some(MpRole::Mesh { peers, .. })
                if peers.iter().flatten().any(|(_, p)| p.is_open()))
        });
        if open {
            break;
        }
        crate::runtime::sleep_ms(100).await;
    }
    let (connected, total) = MP_SESSION.with(|s| {
        if let Some(MpSession { role: MpRole::Mesh { peers, .. }, .. }) = s.borrow().as_ref() {
            let open = peers.iter().flatten().filter(|(_, p)| p.is_open()).count() as i32;
            (i32::from(open > 0), open + 1)
        } else {
            (0, 0)
        }
    });
    mp_post_status(&worker, connected, my_slot, total);
}

fn mp_post_status(worker: &Worker, connected: i32, self_index: i32, peer_count: i32) {
    let m = Object::new();
    let _ = Reflect::set(&m, &JsValue::from_str("type"), &JsValue::from_str("mp:status"));
    let _ = Reflect::set(&m, &JsValue::from_str("connected"), &JsValue::from_f64(connected as f64));
    let _ = Reflect::set(&m, &JsValue::from_str("selfIndex"), &JsValue::from_f64(self_index as f64));
    let _ = Reflect::set(&m, &JsValue::from_str("peerCount"), &JsValue::from_f64(peer_count as f64));
    let _ = worker.post_message(&m);
}

/// Read an i32[] off a worker message field (e.g. the flushed deltas/events).
pub(crate) fn mp_read_int_array(data: &JsValue, field: &str) -> Vec<i32> {
    Reflect::get(data, &JsValue::from_str(field))
        .ok()
        .map(|v| {
            js_sys::Array::from(&v)
                .iter()
                .map(|x| x.as_f64().unwrap_or(0.0) as i32)
                .collect()
        })
        .unwrap_or_default()
}

/// Send buffered deltas/events as a JSON frame (`{"d":[slot,val,...]}` or
/// `{"e":[val,...]}`) over the UNRELIABLE game channel: the HOST broadcasts to
/// every joiner; a JOINER sends to the host. No-op if no open peer.
pub(crate) fn mp_send(deltas: Option<Vec<i32>>, events: Option<Vec<i32>>) {
    let json = if let Some(d) = deltas {
        format!("{{\"d\":{}}}", mp_ints_json(&d))
    } else if let Some(ev) = events {
        format!("{{\"e\":{}}}", mp_ints_json(&ev))
    } else {
        return;
    };
    MP_SESSION.with(|s| {
        if let Some(sess) = s.borrow().as_ref() {
            match &sess.role {
                MpRole::Host { peers } => {
                    for (_, _, p) in peers {
                        if p.is_open() {
                            let _ = p.send_game(json.as_bytes());
                        }
                    }
                }
                MpRole::Joiner { peer } => {
                    if peer.is_open() {
                        let _ = peer.send_game(json.as_bytes());
                    }
                }
                MpRole::Mesh { peers, .. } => {
                    // direct to every connected peer — no host, no relay.
                    for slot in peers.iter().flatten() {
                        if slot.1.is_open() {
                            let _ = slot.1.send_game(json.as_bytes());
                        }
                    }
                }
            }
        }
    });
}

fn mp_ints_json(v: &[i32]) -> String {
    let mut s = String::from("[");
    for (i, n) in v.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&n.to_string());
    }
    s.push(']');
    s
}

/// An incoming peer frame off the game channel → post `mp:peer` (deltas/events)
/// to the worker's mirror. `peer_index` is the connection's mirror index: the
/// host passes each joiner's assigned index; a joiner passes 0 (the host).
///
/// STAR RELAY (for >2 players): the host re-broadcasts each joiner's frame to
/// the OTHER joiners so joiner↔joiner state is visible — tagged with the
/// origin index (`"p"`) so the receiver attributes it to that joiner, not the
/// host. A frame WITH a `"p"` tag is already relayed (never re-relay); an
/// untagged frame uses the connection index. Backward-compatible: a
/// host-authoritative game whose joiners send no body state just relays tiny
/// input frames everyone ignores.
fn mp_dispatch_peer_frame(worker: &Worker, peer_index: i32, bytes: &[u8]) {
    let text = match std::str::from_utf8(bytes) {
        Ok(t) => t,
        Err(_) => return,
    };
    let v: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return,
    };
    // Origin peer index. Only a JOINER may trust an attacker-supplied "p"
    // tag: a joiner's single connection is to the TRUSTED host, which relays
    // other joiners' frames stamped with their origin index. A HOST or MESH
    // receiver ALWAYS attributes a frame to the connection it arrived on
    // (`peer_index`) and ignores "p" — otherwise any peer could spoof another
    // peer's (or the host's) slot by sending {"d":[…],"p":<victim>}. Checking
    // `peer_index == 0` is insufficient: a mesh peer can legitimately occupy
    // slot 0, so the role itself is the gate.
    let trust_p_tag = MP_SESSION
        .with(|s| matches!(s.borrow().as_ref().map(|x| &x.role), Some(MpRole::Joiner { .. })));
    let origin = if trust_p_tag {
        v.get("p")
            .and_then(|x| x.as_i64())
            .map(|x| x as i32)
            .unwrap_or(peer_index)
    } else {
        peer_index
    };
    let m = Object::new();
    let _ = Reflect::set(&m, &JsValue::from_str("type"), &JsValue::from_str("mp:peer"));
    let _ = Reflect::set(&m, &JsValue::from_str("peer"), &JsValue::from_f64(origin as f64));
    if let Some(d) = v.get("d").and_then(|x| x.as_array()) {
        let arr = js_sys::Array::new();
        for n in d {
            arr.push(&JsValue::from_f64(n.as_i64().unwrap_or(0) as f64));
        }
        let _ = Reflect::set(&m, &JsValue::from_str("deltas"), &arr);
    }
    if let Some(ev) = v.get("e").and_then(|x| x.as_array()) {
        let arr = js_sys::Array::new();
        for n in ev {
            arr.push(&JsValue::from_f64(n.as_i64().unwrap_or(0) as f64));
        }
        let _ = Reflect::set(&m, &JsValue::from_str("events"), &arr);
    }
    let _ = worker.post_message(&m);
    // HOST: fan an UNTAGGED joiner frame out to the other joiners (tagged with
    // the origin index) so joiner↔joiner state is visible. No-op for a joiner
    // (peer_index 0) and for already-tagged relayed frames.
    if peer_index >= 1 && v.get("p").is_none() && v.is_object() {
        mp_relay_from_host(peer_index, &v);
    }
}

/// HOST-only: re-broadcast joiner `origin_idx`'s frame to every OTHER joiner,
/// stamped with `"p":origin_idx`. No-op unless this session is a host.
fn mp_relay_from_host(origin_idx: i32, v: &serde_json::Value) {
    MP_SESSION.with(|s| {
        if let Some(MpSession { role: MpRole::Host { peers }, .. }) = s.borrow().as_ref() {
            let mut tagged = v.clone();
            tagged["p"] = serde_json::json!(origin_idx);
            let bytes = tagged.to_string();
            // Send to every joiner EXCEPT the origin (matched by its stored
            // index, robust to a mid-roster join that never connected).
            for (_, pidx, p) in peers.iter() {
                if *pidx != origin_idx && p.is_open() {
                    let _ = p.send_game(bytes.as_bytes());
                }
            }
        }
    });
}

/// Drop the multiplayer session (Peer::drop closes the connection). Idempotent.
pub(crate) fn mp_teardown() {
    MP_SESSION.with(|s| *s.borrow_mut() = None);
}
