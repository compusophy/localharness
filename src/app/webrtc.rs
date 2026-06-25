//! WebRTC P2P transport (Layer 3) for cross-device shared-folder sync.
//!
//! Where this sits in the stack:
//! - **Discovery** = `DeviceRegistryFacet.devicesOf` (the owner's linked
//!   devices ARE the peer set).
//! - **Signaling** = the on-chain `SignalingFacet` (post/poll SDP blobs, no
//!   server) — the caller carries the SDP strings this module produces/consumes
//!   to/from the chain, peer-encrypted.
//! - **This module** = the pure TRANSPORT: an [`RtcPeerConnection`] over a free
//!   public STUN server, **non-trickle ICE** (we wait for gathering to finish so
//!   the whole SDP — candidates included — rides ONE signaling blob), and a
//!   **negotiated** ordered [`RtcDataChannel`] carrying the sync protocol
//!   (Layer 4). Negotiated channels (both sides open id 0) avoid the
//!   `ondatachannel` event entirely, so offer + answer are symmetric.
//!
//! **COMPILE-VERIFIED ONLY.** WebRTC needs two real browsers to exercise; this
//! compiles and is structurally correct, but the offer/answer/ICE handshake is
//! proven only by a real cross-device run. Gated on `feature = "browser-app"`.

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    MessageEvent, RtcConfiguration, RtcDataChannel, RtcDataChannelInit, RtcDataChannelType,
    RtcIceGatheringState, RtcIceServer, RtcPeerConnection, RtcSdpType, RtcSessionDescriptionInit,
};

/// Public STUN server for reflexive ICE candidates. Free + ubiquitous — the one
/// external dependency on-chain signaling does NOT remove. TURN (a relay for the
/// ~20-30% of NATs that need it) is a deferred add.
const STUN_URL: &str = "stun:stun.l.google.com:19302";

/// Label / negotiated id for the RELIABLE, ordered shared-folder sync channel.
/// Both peers open it with the SAME id so no `ondatachannel` negotiation is
/// needed. Carries the SyncMsg protocol (teams-sync / shared-fs) — delivery
/// guarantees here are LOAD-BEARING (file sync must not drop/reorder).
const CHANNEL_LABEL: &str = "lh-sharedfs";
const CHANNEL_ID: u16 = 0;

/// Second negotiated channel (id 1) for fast game-state (`host::mp`): UNRELIABLE
/// and UNORDERED (`maxRetransmits 0`, `ordered false`) — UDP-like, drop-on-loss,
/// so a stale position/input frame is never retransmitted. SEPARATE from the sync
/// channel so a game's twitchy traffic and the reliable file-sync never share a
/// pipe (the old shared-channel-by-JSON-shape ambiguity is gone too).
const GAME_CHANNEL_LABEL: &str = "lh-game";
const GAME_CHANNEL_ID: u16 = 1;

/// An owned WebRTC peer connection + its data channel, holding the message
/// closure alive for the connection's lifetime (wasm needs the JS callback to
/// outlive the call that registered it — same pattern as `display.rs`'s
/// `CartridgeRuntime`). Drop it to tear the connection down.
pub(crate) struct Peer {
    pc: RtcPeerConnection,
    /// id 0, reliable + ordered: the sync channel (teams-sync / shared-fs). All
    /// the existing `send`/`sender`/`is_open` callers route here, unchanged.
    channel: RtcDataChannel,
    /// id 1, unreliable + unordered: the game channel (`host::mp`). `send_game`.
    game: RtcDataChannel,
    /// Kept alive; ONE closure serves BOTH channels' onmessage (each consumer
    /// discriminates by frame shape, so cross-channel frames are simply ignored).
    _on_message: Closure<dyn FnMut(MessageEvent)>,
}

thread_local! {
    // The proxy's ICE config (STUN + any provisioned TURN), fetched + cached once.
    static ICE_SERVERS: std::cell::RefCell<Option<js_sys::Array>> =
        const { std::cell::RefCell::new(None) };
}

/// STUN-only fallback so a peer can always at least try a direct connection.
fn default_ice() -> js_sys::Array {
    let ice = RtcIceServer::new();
    ice.set_urls(&JsValue::from_str(STUN_URL));
    let servers = js_sys::Array::new();
    servers.push(&ice);
    servers
}

/// ICE servers for new peers: fetch `/api/turn` (STUN + TURN-when-provisioned)
/// ONCE, cache the parsed array; STUN-only fallback on any failure. The JSON
/// `iceServers` entries are already RTCIceServer-shaped ({urls, username?,
/// credential?}), so they pass straight to setIceServers.
async fn ice_servers() -> js_sys::Array {
    if let Some(a) = ICE_SERVERS.with(|c| c.borrow().clone()) {
        return a;
    }
    let arr = match crate::registry::fetch_ice_json().await {
        Ok(text) => js_sys::JSON::parse(&text)
            .ok()
            .and_then(|j| js_sys::Reflect::get(&j, &JsValue::from_str("iceServers")).ok())
            .and_then(|v| v.dyn_into::<js_sys::Array>().ok())
            .filter(|a| a.length() > 0)
            .unwrap_or_else(default_ice),
        Err(_) => default_ice(),
    };
    ICE_SERVERS.with(|c| *c.borrow_mut() = Some(arr.clone()));
    arr
}

/// Build a peer connection configured with the fetched ICE servers (STUN + TURN).
async fn new_pc() -> Result<RtcPeerConnection, JsValue> {
    let servers = ice_servers().await;
    let cfg = RtcConfiguration::new();
    cfg.set_ice_servers(&servers);
    RtcPeerConnection::new_with_configuration(&cfg)
}

/// Open BOTH negotiated channels — reliable sync (id 0) + unreliable game
/// (id 1) — and wire ONE shared `on_msg` to both (decoding ArrayBuffer / string
/// payloads to bytes). Each consumer discriminates by frame shape (sync =
/// `SyncMsg` JSON; game = `{"d"/"e"}` integer frames), so a frame arriving on the
/// "wrong" channel for a given consumer is simply ignored. Returns the two
/// channels + the closure to keep alive.
fn open_channels(
    pc: &RtcPeerConnection,
    mut on_msg: impl FnMut(Vec<u8>) + 'static,
) -> (RtcDataChannel, RtcDataChannel, Closure<dyn FnMut(MessageEvent)>) {
    // Reliable, ordered sync channel (id 0) — UNCHANGED semantics.
    let sync_init = RtcDataChannelInit::new();
    sync_init.set_negotiated(true);
    sync_init.set_id(CHANNEL_ID);
    let sync_dc = pc.create_data_channel_with_data_channel_dict(CHANNEL_LABEL, &sync_init);
    sync_dc.set_binary_type(RtcDataChannelType::Arraybuffer);

    // Unreliable, unordered game channel (id 1) — UDP-like: no retransmit, no
    // ordering, so a lost/late frame is dropped rather than stalling the pipe.
    let game_init = RtcDataChannelInit::new();
    game_init.set_negotiated(true);
    game_init.set_id(GAME_CHANNEL_ID);
    game_init.set_ordered(false);
    game_init.set_max_retransmits(0);
    let game_dc = pc.create_data_channel_with_data_channel_dict(GAME_CHANNEL_LABEL, &game_init);
    game_dc.set_binary_type(RtcDataChannelType::Arraybuffer);

    let cb = Closure::wrap(Box::new(move |ev: MessageEvent| {
        let data = ev.data();
        let bytes = if let Some(buf) = data.dyn_ref::<js_sys::ArrayBuffer>() {
            js_sys::Uint8Array::new(buf).to_vec()
        } else if let Some(s) = data.as_string() {
            s.into_bytes()
        } else {
            return; // Blob payloads (binaryType not honoured) are dropped in v1.
        };
        on_msg(bytes);
    }) as Box<dyn FnMut(MessageEvent)>);
    // One JS function, set as the handler on BOTH channels.
    sync_dc.set_onmessage(Some(cb.as_ref().unchecked_ref()));
    game_dc.set_onmessage(Some(cb.as_ref().unchecked_ref()));
    (sync_dc, game_dc, cb)
}

/// Non-trickle ICE: wait until candidate gathering completes so the local SDP
/// carries every candidate and the whole thing rides ONE signaling blob. Capped
/// at ~5s; returns regardless so a stuck gather still yields a usable SDP.
async fn wait_for_ice(pc: &RtcPeerConnection) {
    for _ in 0..100 {
        if pc.ice_gathering_state() == RtcIceGatheringState::Complete {
            return;
        }
        crate::runtime::sleep_ms(50).await;
    }
}

/// Read the fully-gathered local SDP off the connection.
fn local_sdp(pc: &RtcPeerConnection) -> Result<String, JsValue> {
    pc.local_description()
        .map(|d| d.sdp())
        .ok_or_else(|| JsValue::from_str("no local description"))
}

impl Peer {
    /// OFFERER: open the connection + channel, create an offer, gather ICE, and
    /// return `(peer, offer_sdp)`. Post `offer_sdp` to the peer via the on-chain
    /// `SignalingFacet`, then feed the peer's reply to [`Peer::accept_answer`].
    pub(crate) async fn offer(
        on_msg: impl FnMut(Vec<u8>) + 'static,
    ) -> Result<(Self, String), JsValue> {
        let pc = new_pc().await?;
        let (channel, game, on_message) = open_channels(&pc, on_msg);
        let offer = JsFuture::from(pc.create_offer()).await?;
        let sdp = js_sys::Reflect::get(&offer, &JsValue::from_str("sdp"))?
            .as_string()
            .ok_or_else(|| JsValue::from_str("offer has no sdp"))?;
        let desc = RtcSessionDescriptionInit::new(RtcSdpType::Offer);
        desc.set_sdp(&sdp);
        JsFuture::from(pc.set_local_description(&desc)).await?;
        wait_for_ice(&pc).await;
        let out = local_sdp(&pc)?;
        Ok((
            Self {
                pc,
                channel,
                game,
                _on_message: on_message,
            },
            out,
        ))
    }

    /// ANSWERER: given the offerer's `offer_sdp`, open the matching connection +
    /// channel, set the remote offer, create + gather an answer, and return
    /// `(peer, answer_sdp)`. Post `answer_sdp` back via the `SignalingFacet`.
    pub(crate) async fn answer(
        offer_sdp: &str,
        on_msg: impl FnMut(Vec<u8>) + 'static,
    ) -> Result<(Self, String), JsValue> {
        let pc = new_pc().await?;
        let (channel, game, on_message) = open_channels(&pc, on_msg);
        let remote = RtcSessionDescriptionInit::new(RtcSdpType::Offer);
        remote.set_sdp(offer_sdp);
        JsFuture::from(pc.set_remote_description(&remote)).await?;
        let answer = JsFuture::from(pc.create_answer()).await?;
        let sdp = js_sys::Reflect::get(&answer, &JsValue::from_str("sdp"))?
            .as_string()
            .ok_or_else(|| JsValue::from_str("answer has no sdp"))?;
        let desc = RtcSessionDescriptionInit::new(RtcSdpType::Answer);
        desc.set_sdp(&sdp);
        JsFuture::from(pc.set_local_description(&desc)).await?;
        wait_for_ice(&pc).await;
        let out = local_sdp(&pc)?;
        Ok((
            Self {
                pc,
                channel,
                game,
                _on_message: on_message,
            },
            out,
        ))
    }

    /// OFFERER, step 2: apply the peer's `answer_sdp` to complete the handshake.
    /// After this the data channel opens and [`Peer::send`] works.
    pub(crate) async fn accept_answer(&self, answer_sdp: &str) -> Result<(), JsValue> {
        let remote = RtcSessionDescriptionInit::new(RtcSdpType::Answer);
        remote.set_sdp(answer_sdp);
        JsFuture::from(self.pc.set_remote_description(&remote)).await?;
        Ok(())
    }

    /// True once the data channel is open and `send` will deliver.
    pub(crate) fn is_open(&self) -> bool {
        self.channel.ready_state() == web_sys::RtcDataChannelState::Open
    }

    /// Send bytes over the RELIABLE sync channel (id 0). Errs if not open yet.
    pub(crate) fn send(&self, bytes: &[u8]) -> Result<(), JsValue> {
        self.channel.send_with_u8_array(bytes)
    }

    /// Send bytes over the UNRELIABLE game channel (id 1, `host::mp`). Drops on
    /// loss (no retransmit/ordering) — the caller resends fresh state each tick,
    /// so a dropped frame is simply superseded. Errs if the channel isn't open.
    pub(crate) fn send_game(&self, bytes: &[u8]) -> Result<(), JsValue> {
        self.game.send_with_u8_array(bytes)
    }

    /// A cloneable handle to the data channel, so the sync layer (Layer 4) can
    /// send replies from inside the `on_msg` callback without borrowing the
    /// whole `Peer`. `RtcDataChannel` is a JS-reference wrapper (cheap to clone).
    pub(crate) fn sender(&self) -> RtcDataChannel {
        self.channel.clone()
    }

    // ── Relay-mediated connect (OFF-CHAIN signaling) ───────────────────────
    // The cross-owner multiplayer path: instead of the on-chain SignalingFacet
    // (a sponsored write per blob), peers rendezvous on the proxy's `/api/signal`
    // GitHub-backed relay (`registry::signal_*`), keyed on a shared `room` id.
    // The app/cartridge assigns roles (the session HOST offers; joiners answer).
    // Both sides are unproven until run with two real browsers (see the module
    // header) — but the relay leg is live-verified.

    // ── N-peer host-authoritative STAR (off-chain signaling) ──────────────────
    // Each host↔joiner link is the SAME proven offer/answer handshake, run once
    // PER joiner: the JOINER offers (it owns an id), the HOST answers each. There
    // are NO joiner↔joiner links — joiners read the HOST's broadcast (host = the
    // hub + authority), so a frame's peer is just "the connection it arrived on".
    // Slots: `offer-{id}` (joiner offer), `answer-{id}` (host answer), `join`
    // (the roster the host polls to discover joiners). The 2-peer game is N=1.

    /// JOINER: offer to the host — publish the offer under `offer-{joiner_id}`,
    /// register in the `join` roster, await the host's `answer-{joiner_id}`, and
    /// complete the handshake. Returns the connected `Peer` (poll `is_open()`).
    pub(crate) async fn offer_to_host(
        room: &str,
        joiner_id: &str,
        signer: &k256::ecdsa::SigningKey,
        on_msg: impl FnMut(Vec<u8>) + 'static,
    ) -> Result<Self, JsValue> {
        let (peer, offer) = Self::offer(on_msg).await?;
        crate::registry::signal_post(signer, now_secs(), room, &format!("offer-{joiner_id}"), &offer)
            .await
            .map_err(|e| JsValue::from_str(&format!("signal_post offer: {e}")))?;
        crate::registry::signal_join(signer, now_secs(), room, joiner_id)
            .await
            .map_err(|e| JsValue::from_str(&format!("signal_join: {e}")))?;
        let answer = poll_signal(room, &format!("answer-{joiner_id}"), 60)
            .await
            .ok_or_else(|| JsValue::from_str("timed out waiting for the host's answer"))?;
        peer.accept_answer(&answer).await?;
        Ok(peer)
    }

    /// HOST: answer ONE joiner — read its `offer-{joiner_id}`, create the answer,
    /// publish it under `answer-{joiner_id}`. Returns the connected `Peer`. The
    /// host calls this per joiner id discovered in the roster.
    pub(crate) async fn answer_joiner(
        room: &str,
        joiner_id: &str,
        signer: &k256::ecdsa::SigningKey,
        on_msg: impl FnMut(Vec<u8>) + 'static,
    ) -> Result<Self, JsValue> {
        let offer = poll_signal(room, &format!("offer-{joiner_id}"), 30)
            .await
            .ok_or_else(|| JsValue::from_str("joiner offer not found"))?;
        let (peer, answer) = Self::answer(&offer, on_msg).await?;
        crate::registry::signal_post(signer, now_secs(), room, &format!("answer-{joiner_id}"), &answer)
            .await
            .map_err(|e| JsValue::from_str(&format!("signal_post answer: {e}")))?;
        Ok(peer)
    }

    /// OFFERER: create an offer, POST it to the relay under `room`, poll for the
    /// peer's answer, complete the handshake, and best-effort clear the room.
    /// Returns the connected `Peer` (poll `is_open()` before `send`). Legacy
    /// 2-peer relay primitive — superseded by the star above, kept for reference.
    #[allow(dead_code)]
    pub(crate) async fn connect_offerer(
        room: &str,
        signer: &k256::ecdsa::SigningKey,
        on_msg: impl FnMut(Vec<u8>) + 'static,
    ) -> Result<Self, JsValue> {
        let (peer, offer) = Self::offer(on_msg).await?;
        crate::registry::signal_post(signer, now_secs(), room, "offer", &offer)
            .await
            .map_err(|e| JsValue::from_str(&format!("signal_post offer: {e}")))?;
        let answer = poll_signal(room, "answer", 60)
            .await
            .ok_or_else(|| JsValue::from_str("timed out waiting for the peer's answer"))?;
        peer.accept_answer(&answer).await?;
        // Cleanup once we have the answer — best-effort (a stale room self-expires).
        let _ = crate::registry::signal_clear(signer, now_secs(), room).await;
        Ok(peer)
    }

    /// ANSWERER: poll the relay for the offer under `room`, answer it, POST the
    /// answer back, and return the connected `Peer`. Legacy 2-peer relay
    /// primitive — superseded by the star above, kept for reference.
    #[allow(dead_code)]
    pub(crate) async fn connect_answerer(
        room: &str,
        signer: &k256::ecdsa::SigningKey,
        on_msg: impl FnMut(Vec<u8>) + 'static,
    ) -> Result<Self, JsValue> {
        let offer = poll_signal(room, "offer", 60)
            .await
            .ok_or_else(|| JsValue::from_str("timed out waiting for the peer's offer"))?;
        let (peer, answer) = Self::answer(&offer, on_msg).await?;
        crate::registry::signal_post(signer, now_secs(), room, "answer", &answer)
            .await
            .map_err(|e| JsValue::from_str(&format!("signal_post answer: {e}")))?;
        Ok(peer)
    }
}

/// Current UNIX seconds (browser clock) for the relay auth token freshness.
fn now_secs() -> u64 {
    (js_sys::Date::now() / 1000.0) as u64
}

/// Poll the relay for a slot's SDP, up to `secs` (1s interval). `None` on timeout.
async fn poll_signal(room: &str, slot: &str, secs: u32) -> Option<String> {
    for _ in 0..secs {
        if let Ok(Some(sdp)) = crate::registry::signal_get(room, slot).await {
            return Some(sdp);
        }
        crate::runtime::sleep_ms(1000).await;
    }
    None
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.channel.set_onmessage(None);
        self.game.set_onmessage(None);
        self.channel.close();
        self.game.close();
        self.pc.close();
    }
}
