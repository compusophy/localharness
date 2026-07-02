//! BRIDGE — one module per host capability a cartridge reaches through the
//! worker→main postMessage boundary. The worker can't sign tokens, read the
//! chain, hold an RtcPeerConnection, or own an AudioContext, so each
//! capability's main-thread half lives here; `super::worker`'s onmessage
//! router dispatches to these. Each bridge keeps its own thread_local state
//! MODULE-PRIVATE.

pub(super) mod audio;
pub(super) mod chat;
pub(super) mod compose;
pub(super) mod feed;
pub(super) mod http;
pub(super) mod mp;
