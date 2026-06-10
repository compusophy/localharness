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

/// Label / negotiated id for the single shared-folder sync channel. Both peers
/// open it with the SAME id so no `ondatachannel` negotiation is needed.
const CHANNEL_LABEL: &str = "lh-sharedfs";
const CHANNEL_ID: u16 = 0;

/// An owned WebRTC peer connection + its data channel, holding the message
/// closure alive for the connection's lifetime (wasm needs the JS callback to
/// outlive the call that registered it — same pattern as `display.rs`'s
/// `CartridgeRuntime`). Drop it to tear the connection down.
pub(crate) struct Peer {
    pc: RtcPeerConnection,
    channel: RtcDataChannel,
    /// Kept alive; never read directly.
    _on_message: Closure<dyn FnMut(MessageEvent)>,
}

/// Build a peer connection configured with the public STUN server.
fn new_pc() -> Result<RtcPeerConnection, JsValue> {
    let ice = RtcIceServer::new();
    ice.set_urls(&JsValue::from_str(STUN_URL));
    let servers = js_sys::Array::new();
    servers.push(&ice);
    let cfg = RtcConfiguration::new();
    cfg.set_ice_servers(&servers);
    RtcPeerConnection::new_with_configuration(&cfg)
}

/// Open the negotiated sync channel and wire its `onmessage` to `on_msg`
/// (decoding ArrayBuffer / string payloads to bytes). Returns the channel + the
/// closure to keep alive.
fn open_channel(
    pc: &RtcPeerConnection,
    mut on_msg: impl FnMut(Vec<u8>) + 'static,
) -> (RtcDataChannel, Closure<dyn FnMut(MessageEvent)>) {
    let init = RtcDataChannelInit::new();
    init.set_negotiated(true);
    init.set_id(CHANNEL_ID);
    let dc = pc.create_data_channel_with_data_channel_dict(CHANNEL_LABEL, &init);
    dc.set_binary_type(RtcDataChannelType::Arraybuffer);

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
    dc.set_onmessage(Some(cb.as_ref().unchecked_ref()));
    (dc, cb)
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
        let pc = new_pc()?;
        let (channel, on_message) = open_channel(&pc, on_msg);
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
        let pc = new_pc()?;
        let (channel, on_message) = open_channel(&pc, on_msg);
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

    /// Send bytes over the sync channel. Errs if the channel isn't open yet.
    pub(crate) fn send(&self, bytes: &[u8]) -> Result<(), JsValue> {
        self.channel.send_with_u8_array(bytes)
    }

    /// A cloneable handle to the data channel, so the sync layer (Layer 4) can
    /// send replies from inside the `on_msg` callback without borrowing the
    /// whole `Peer`. `RtcDataChannel` is a JS-reference wrapper (cheap to clone).
    pub(crate) fn sender(&self) -> RtcDataChannel {
        self.channel.clone()
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.channel.set_onmessage(None);
        self.channel.close();
        self.pc.close();
    }
}
