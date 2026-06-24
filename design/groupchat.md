# Open chatroom cartridges (`host::chat`)

A cartridge that is just an open chatroom — a scrolling message log + an on-screen
tap keyboard — served to every visitor of a subdomain with no browser chrome.
`groupchat.localharness.xyz` is the reference instance (`examples/cartridges/
groupchat.rl`).

## Why a new host capability

A cartridge can't open a WebSocket (no server — Vercel Edge can't hold persistent
sockets) and has no text input (no DOM keyboard). So a chatroom needs two things the
framebuffer doesn't give for free: a **relay** to fan messages out, and a way to
**type**. Both are solved off-chain + host-side, mirroring the off-chain scheduler /
signaling model (GitHub-backed store through the proxy, no DB, no daemon).

## The relay — `proxy/api/chat.ts` (off-chain)

One append-only JSON log per room: `chat/<room>.json = { next, messages:[{n, name,
text, ts}] }` in the jobs repo, trimmed to the last 80. `room` = the cartridge's
subdomain.

- `POST {room, text}` — personal-sign authed (anti-spam). The sender's **short
  address** (`addr.slice(2,6)`) is the display name (no name-entry in a cartridge).
  Whitespace is collapsed to one line; capped at 280 bytes; one-retry on a concurrent-
  write sha conflict (read-modify-write).
- `GET ?room=&after=` — open (the room id is the capability). Returns messages with
  `n > after` + the current `next` cursor.

## The capability — `host::chat` (integer-only ABI)

rustlite has no String/Vec and arrays are read-only, so **all chat text lives
host-side**: the worker (`web/cartridge-worker.js`) holds the received-line ring +
the outgoing compose buffer; the cartridge reads/writes them purely as integer
codepoint calls (keeping the host ABI integer-only — no memory pointers).

```
poll() -> n            start polling the relay (idempotent) + return # lines held
line_count() -> n      received lines currently buffered (oldest first)
line_len(i) -> len     length of received line i
line_char(i, p) -> cp  codepoint at p of line i (-1 out of range)
key(cp)                append a char to the outgoing compose buffer
backspace()            delete the last compose char
compose_len() -> len   current compose-buffer length
compose_char(p) -> cp  codepoint at p of the compose buffer (-1 oob)
send() -> 1/0          flush the compose buffer as a message, then clear it
```

The relay POST/GET + personal-sign auth live on **main** (`src/app/display.rs`): the
first `poll()` posts `{chat:start}` so main begins a 2s relay-poll loop (room = the
subdomain via `tenant::current_name()`); new lines arrive back as `{chat:msg}`;
`send()` posts `{chat:send}` → main POSTs it authed off the viewer's identity. No
identity → the cartridge can still READ the room, sends are dropped.

## Parity (the lockstep rule)

The host bindings exist in three places that must stay in sync, or instantiation
fails: `src/rustlite/typecheck.rs` (`chat::*` signatures) · `src/rustlite/loader.rs`
(`host_chat` stub for the compile-check / native loader) · `web/cartridge-worker.js`
(`host_chat` real impl). `src/app/display.rs` is the worker↔relay bridge.

## The cartridge (`groupchat.rl`)

320×240 framebuffer: header (`GROUPCHAT` + live line count), the last 12 log lines,
the compose line with a blinking cursor, and a 4-row QWERTY tap keyboard (SPACE +
DEL + SEND). Edge-triggered taps (act once on the press) map `(x,y)` → a key via
per-row codepoint arrays. ~2.2 KB of wasm.
