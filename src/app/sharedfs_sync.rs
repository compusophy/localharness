//! Cross-device shared-folder SYNC protocol (Layer 4).
//!
//! Sits on top of [`super::webrtc::Peer`] (Layer 3 transport) and the
//! [`super::shared_fs`] apex store (Layer 2). Once a WebRTC data channel is
//! open between two of the owner's devices, this reconciles their shared
//! folders.
//!
//! **v2 = CONVERGENT sync by file NAME + CONTENT HASH.** Both peers announce
//! their manifest on connect — now `(name, keccak256(content))` pairs, not bare
//! names. Each peer feeds the pair `(local_manifest, remote_manifest)` to the
//! PURE [`crate::sharedfs_reconcile::plan_pulls`] reconcile, which decides which
//! names to request and which local files to copy to a conflict name. The holder
//! replies with the plaintext, which the receiver re-seals under ITS OWN master
//! key (the bytes on the wire are plaintext, but the channel itself is
//! DTLS-encrypted and only ever runs between the owner's own devices).
//!
//! **The bug this version fixes:** v1 merged by filename ONLY, so two devices
//! holding the same name with DIFFERENT content diverged silently and never
//! healed. There is no timestamp/version on a shared file (only `name + size`),
//! so last-write-wins is impossible; instead the reconcile picks a deterministic
//! winner by the LEXICOGRAPHICALLY-GREATER content hash and preserves the loser
//! as a `name.conflict-<shorthash>` copy. Both devices compute the same hashes →
//! pick the same winner → derive the same conflict name → CONVERGE to a
//! byte-identical folder. The convergence/symmetry proof lives in
//! [`crate::sharedfs_reconcile`]'s native unit tests; this module is the wiring.
//!
//! **COMPILE-VERIFIED ONLY** — the 2-device END-TO-END is exercised only by a
//! real two-device run; the reconcile LOGIC (determinism + convergence) is what
//! the headless tests prove. The orchestration that drives
//! offer→`SignalingFacet`→answer and discovers peers via `DeviceRegistry`
//! (Layer 5) is in [`super::teams_sync`]. Gated on `feature = "browser-app"`.

use std::cell::RefCell;
use std::rc::Rc;

use serde::{Deserialize, Serialize};
use wasm_bindgen::JsValue;
use web_sys::RtcDataChannel;

use crate::sharedfs_reconcile::FileMeta;

use super::shared_fs;
use super::webrtc::Peer;

/// keccak256 of `bytes` — the per-file content hash that drives convergent
/// conflict resolution. Same primitive the rest of `src/app` uses (`sha3`,
/// pulled by the `wallet` feature that `browser-app` enables transitively).
fn content_hash(bytes: &[u8]) -> Vec<u8> {
    use sha3::{Digest, Keccak256};
    Keccak256::digest(bytes).to_vec()
}

/// Build THIS device's manifest: `(name, keccak256(plaintext))` for every file
/// in the shared folder. Reads + decrypts each file (the hash is over the
/// PLAINTEXT, so it is comparable across devices regardless of per-device seal
/// keys). A file that fails to decrypt is skipped (it isn't ours / is empty).
async fn local_manifest() -> Vec<FileMeta> {
    let mut out = Vec::new();
    for entry in shared_fs::apex_list().await {
        if let Ok(Some(plain)) = shared_fs::apex_read(&entry.name).await {
            out.push(FileMeta::new(entry.name, content_hash(&plain)));
        }
    }
    out
}

/// A cloneable send handle, filled once the WebRTC channel exists so the inbound
/// `on_msg` callback can reply without borrowing the whole [`Peer`].
type Tx = Rc<RefCell<Option<RtcDataChannel>>>;

/// Wire messages over the data channel.
#[derive(Serialize, Deserialize)]
enum SyncMsg {
    /// "Here are the files I have, with a content hash each." Sent by BOTH peers
    /// on connect → bidirectional CONVERGENT reconcile. Each entry is
    /// `(name, keccak256(plaintext))`; the hash lets the receiver detect a
    /// same-name/different-content conflict (which bare names cannot) and resolve
    /// it deterministically via [`crate::sharedfs_reconcile::plan_pulls`].
    Manifest(Vec<(String, Vec<u8>)>),
    /// "Send me this file." The name may be a plain file name OR a
    /// `name.conflict-<shorthash>` copy the reconcile asked for — in either case
    /// the holder serves whichever local file currently sits at that name (the
    /// peer materialised the conflict copy via its own plan before replying).
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
                // CONVERGENT reconcile. Build our content-hashed manifest, then
                // let the PURE planner decide what to pull and which local files
                // to copy to a conflict name. Both devices run the symmetric
                // plan over the same hashes → same final set (see
                // `crate::sharedfs_reconcile`).
                let local = local_manifest().await;
                let remote: Vec<FileMeta> = remote
                    .into_iter()
                    .map(|(name, hash)| FileMeta::new(name, hash))
                    .collect();
                let plan = crate::sharedfs_reconcile::plan_pulls(&local, &remote);

                // Materialise local conflict copies FIRST: preserve our losing
                // edit under its `name.conflict-<shorthash>` name so it survives
                // (and so we can serve it if the peer asks). We read the source's
                // current plaintext and re-seal under our own key at the copy.
                for (from, to) in &plan.rename_local {
                    if let Ok(Some(plain)) = shared_fs::apex_read(from).await {
                        let _ = shared_fs::apex_write(to, &plain).await;
                    }
                }

                // Request every name the reconcile says we lack (peer-only
                // files + the peer's conflict copies + winners that override a
                // local loser).
                for name in plan.want {
                    send_msg(&tx, &SyncMsg::Want(name));
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

    /// Kick the reconcile by announcing our content-hashed manifest. Call once
    /// the channel is open (poll [`SharedFsSync::is_open`]); both peers doing so
    /// yields the bidirectional CONVERGENT sync. Best-effort.
    pub(crate) async fn start(&self) {
        let manifest: Vec<(String, Vec<u8>)> = local_manifest()
            .await
            .into_iter()
            .map(|f| (f.name, f.hash))
            .collect();
        send_msg(&self.tx, &SyncMsg::Manifest(manifest));
    }

    /// True once the data channel is open and sync can flow.
    pub(crate) fn is_open(&self) -> bool {
        self.peer.is_open()
    }
}
