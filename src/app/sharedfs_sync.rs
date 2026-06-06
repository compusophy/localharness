//! Cross-device shared-folder SYNC protocol (Layer 4).
//!
//! Sits on top of [`super::webrtc::Peer`] (Layer 3 transport) and the
//! [`super::shared_fs`] apex store (Layer 2). Once a WebRTC data channel is
//! open between two of the owner's devices, this reconciles their shared
//! folders.
//!
//! **v1 = UNION sync by file NAME.** Both peers announce their file list on
//! connect; each requests the names it lacks; the holder replies with the
//! plaintext, which the receiver re-seals under ITS OWN master key (the bytes
//! on the wire are plaintext, but the channel itself is DTLS-encrypted and only
//! ever runs between the owner's own devices). Same-name / different-content
//! conflict resolution (content hashing + last-write-wins) is deferred to v2.
//!
//! **COMPILE-VERIFIED ONLY** — exercised only by a real two-device run. The
//! orchestration that drives offer→`SignalingFacet`→answer and discovers peers
//! via `DeviceRegistry` (Layer 5) is the remaining wiring. Gated on
//! `feature = "browser-app"`.

use std::cell::RefCell;
use std::rc::Rc;

use serde::{Deserialize, Serialize};
use wasm_bindgen::JsValue;
use web_sys::RtcDataChannel;

use super::shared_fs;
use super::webrtc::Peer;

/// A cloneable send handle, filled once the WebRTC channel exists so the inbound
/// `on_msg` callback can reply without borrowing the whole [`Peer`].
type Tx = Rc<RefCell<Option<RtcDataChannel>>>;

/// Wire messages over the data channel.
#[derive(Serialize, Deserialize)]
enum SyncMsg {
    /// "Here are the files I have." Sent by BOTH peers on connect → bidirectional
    /// union.
    Manifest(Vec<String>),
    /// "Send me this file."
    Want(String),
    /// "Here is the file you asked for." `data` is the decrypted plaintext; the
    /// receiver re-seals it under its own master key on write.
    File { name: String, data: Vec<u8> },
}

fn send_msg(tx: &Tx, msg: &SyncMsg) {
    let Ok(bytes) = serde_json::to_vec(msg) else {
        return;
    };
    if let Some(ch) = tx.borrow().as_ref() {
        let _ = ch.send_with_u8_array(&bytes);
    }
}

/// React to one inbound message. The async apex-store work runs on a detached
/// task so the sync callback itself stays synchronous.
fn handle_message(bytes: Vec<u8>, tx: Tx) {
    let Ok(msg) = serde_json::from_slice::<SyncMsg>(&bytes) else {
        return;
    };
    wasm_bindgen_futures::spawn_local(async move {
        match msg {
            SyncMsg::Manifest(remote) => {
                let local: Vec<String> = shared_fs::apex_list()
                    .await
                    .into_iter()
                    .map(|e| e.name)
                    .collect();
                for name in remote {
                    if !local.iter().any(|n| n == &name) {
                        send_msg(&tx, &SyncMsg::Want(name));
                    }
                }
            }
            SyncMsg::Want(name) => {
                if let Ok(Some(data)) = shared_fs::apex_read(&name).await {
                    send_msg(&tx, &SyncMsg::File { name, data });
                }
            }
            SyncMsg::File { name, data } => {
                let _ = shared_fs::apex_write(&name, &data).await;
            }
        }
    });
}

/// A live cross-device sync session: a [`Peer`] wired to the shared-folder
/// reconcile protocol. Drop to disconnect.
pub(crate) struct SharedFsSync {
    peer: Peer,
    tx: Tx,
}

impl SharedFsSync {
    /// OFFERER side. Returns the session + the offer SDP to post via the
    /// `SignalingFacet`. After the peer's answer arrives, call
    /// [`SharedFsSync::accept_answer`].
    pub(crate) async fn offer() -> Result<(Self, String), JsValue> {
        let tx: Tx = Rc::new(RefCell::new(None));
        let tx_cb = tx.clone();
        let (peer, sdp) = Peer::offer(move |bytes| handle_message(bytes, tx_cb.clone())).await?;
        *tx.borrow_mut() = Some(peer.sender());
        Ok((Self { peer, tx }, sdp))
    }

    /// ANSWERER side, given the offerer's SDP. Returns the session + answer SDP
    /// to post back via the `SignalingFacet`.
    pub(crate) async fn answer(offer_sdp: &str) -> Result<(Self, String), JsValue> {
        let tx: Tx = Rc::new(RefCell::new(None));
        let tx_cb = tx.clone();
        let (peer, sdp) =
            Peer::answer(offer_sdp, move |bytes| handle_message(bytes, tx_cb.clone())).await?;
        *tx.borrow_mut() = Some(peer.sender());
        Ok((Self { peer, tx }, sdp))
    }

    /// OFFERER, step 2: apply the peer's answer to complete the handshake.
    pub(crate) async fn accept_answer(&self, answer_sdp: &str) -> Result<(), JsValue> {
        self.peer.accept_answer(answer_sdp).await
    }

    /// Kick the reconcile by announcing our manifest. Call once the channel is
    /// open (poll [`SharedFsSync::is_open`]); both peers doing so yields the
    /// bidirectional union sync. Best-effort.
    pub(crate) async fn start(&self) {
        let names: Vec<String> = shared_fs::apex_list()
            .await
            .into_iter()
            .map(|e| e.name)
            .collect();
        send_msg(&self.tx, &SyncMsg::Manifest(names));
    }

    /// True once the data channel is open and sync can flow.
    pub(crate) fn is_open(&self) -> bool {
        self.peer.is_open()
    }
}
