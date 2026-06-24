# design/multiplayer-cartridges — `host::net` for live multiplayer

> **STATUS: DESIGN (awaiting sign-off).** Transport is PROVEN (`webrtc.rs` +
> off-chain relay connected two browsers + exchanged a message,
> `project_webrtc_multiplayer`). This specs the cartridge-facing API + wire
> protocol BEFORE the multi-file binding build.

## Goal
A rustlite cartridge becomes multiplayer by calling `host::net::*`: open/join a
room, read+write a shared per-peer state vector, and send/receive discrete
events. Two players running the same cartridge see each other in real time, P2P
(data flows over the WebRTC data channel; the proxy only does matchmaking).

## The hard constraint: the host ABI is INTEGER-ONLY
Cartridge host calls pass/return `i32` only (loader.rs / cartridge-worker.js,
mirrored). So the API is built from integers — which fits multiplayer well
(positions, inputs, scores, events are ints). No strings/bytes cross the ABI.

## API — `host::net::*` (all `i32`)

| Fn | Meaning |
|----|---------|
| `open() -> i32` | Host a NEW room; returns a room CODE to show the other player (draw_number). Runtime: random room id → post an OFFER, await a joiner. |
| `join(room: i32)` | Join room `room` as a peer. Runtime: poll for the host's offer → answer. |
| `connected() -> i32` | 1 once the data channel is open, else 0. (Cartridge polls each frame.) |
| `self_index() -> i32` | This peer's index (host=0, joiner=1; -1 if not connected). |
| `peer_count() -> i32` | Participants connected (self + peers). 2 for a full v1 pair. |
| `set(slot: i32, value: i32)` | Write MY shared-state slot (`0..SLOTS`). Broadcast (coalesced per frame). |
| `get(peer: i32, slot: i32) -> i32` | Read peer `peer`'s slot (last value seen; 0 if unknown). |
| `send(value: i32)` | Broadcast a discrete EVENT to peers (queued on their side). |
| `event_count() -> i32` | How many received events are queued. |
| `event_next() -> i32` | Pop + return the oldest received event (0 if none). |

**Shared state** = continuous "where is everyone" (last-write-wins, broadcast on
change). **Events** = one-shot "something happened" (queued, consumed once). Most
games need both. `SLOTS` per peer = 32 (v1).

### Cartridge usage sketch (a 2-cursor demo)
```
fn frame(t: i32) {
    if host::net::connected() == 0 { /* draw "press O to host / J to join" */ return; }
    // publish my cursor
    host::net::set(0, host::display::pointer_x());
    host::net::set(1, host::display::pointer_y());
    host::display::clear(0);
    // draw every peer's cursor
    let n = host::net::peer_count();
    let i = 0;
    while i < n {
        host::display::fill_rect(host::net::get(i, 0), host::net::get(i, 1), 8, 8, 0xffffff);
        i = i + 1;
    }
    host::display::present();
}
```

## Wire protocol (over the data channel; compact binary frames)
- On connect, the HOST sends `[0x00][your_index:u8][peer_count:u8]` to assign the
  joiner its index (host=0, joiner=1).
- **State delta:** `[0x01][slot:u8][value:i32 LE]` (6 bytes). Receiver stores
  `value` under the SENDER's peer index + `slot`. Coalesced: at most the changed
  slots, once per frame.
- **Event:** `[0x02][value:i32 LE]` (5 bytes). Receiver enqueues it.

## Runtime data flow (recommended: reuse the proven `webrtc.rs` Peer on MAIN)
The cartridge runs in the WORKER (`cartridge-worker.js`); the proven `Peer`
(RtcPeerConnection) lives on MAIN. They bridge over the EXISTING worker↔main
postMessage channel (already carries draw-commands worker→main + pointer/tick
main→worker):

```
cartridge (worker)            cartridge-worker.js        display.rs (main)        Peer ↔ relay/peer
 host::net::set(s,v) ─────────▶ buffer dirty slot
 host::net::open()   ─────────▶ postMessage{net:open} ──▶ run relay signaling ──▶ Peer::connect_offerer
                       (per tick) postMessage{net:deltas} ▶ send frames ─────────▶ data channel
 host::net::get(p,s) ◀── mirror table ◀── postMessage{net:peerstate} ◀── on_msg ◀── peer frames
 host::net::event_next() ◀── mirror queue ◀────────────────┘
```
The WORKER mirrors the state table `[peer][slot]` + the event queue (updated from
main's postMessages) so `get`/`event_*` return SYNCHRONOUSLY. Outgoing `set`/`send`
buffer in the worker and flush to main once per tick (coalesced).

*Alternative considered — Peer IN the worker (JS reimpl of the handshake): no
bridge + lower latency, but duplicates the proven `webrtc.rs` in JS. Rejected for
v1 (reuse the proven Rust path; the bridge is incremental on the existing one).*

## Scope
- **v1 = 2-PEER**, host/joiner via `open()`/`join(code)` (explicit roles → no
  signaling race), 32 state slots + an event queue, same-NAT proven.
- **Deferred:** N-peer (host assigns indices; full-mesh signaling or host-relay),
  and **TURN** (cross-internet reach — STUN-only fails ~20-30% of NATs; a managed
  TURN service + a creds endpoint on the proxy). The API (`peer_count`,
  `self_index`, per-peer `get`) is already N-peer-shaped.

## Build surface (the multi-file binding — after sign-off)
1. `src/rustlite/typecheck.rs` — register the `net` module + the 9 fn signatures.
2. `src/rustlite/codegen.rs` — emit the `host::net::*` wasm imports.
3. `src/rustlite/loader.rs` — `host_net` closures (Rust host; for the native/main loader).
4. `web/cartridge-worker.js` — the JS hand-port: buffer `set`/`send`, mirror the
   state table + event queue, postMessage to/from main. PARITY with (3).
5. `src/app/display.rs` — the main bridge: on `net:open`/`net:join` run relay
   signaling + `Peer::connect_*`; relay data-channel frames ↔ worker; keep the net
   state table; expose connection status.
6. `examples/cartridges/multiplayer.rl` — the 2-cursor demo (proves round-trip).

## Open questions for sign-off
- **Room code UX**: `open()` returns a numeric code the host shows; the joiner
  types it (cartridge handles digit input). OK, or derive the room from the
  cartridge name (a single global room per cartridge)?
- **SLOTS = 32** per peer enough for v1?
- **Peer on MAIN + bridge** (recommended) vs **Peer in the worker** (no bridge,
  JS reimpl) — confirm the bridge approach.
