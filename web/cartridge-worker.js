// cartridge-worker.js — runs an UNTRUSTED wasm cartridge OFF the main thread.
//
// WHY THIS EXISTS (the brick fix):
//   A cartridge's `frame(t)` is synchronous wasm. If it loops long/unbounded it
//   blocks whatever thread runs it. On the main thread that froze the WHOLE app
//   (chat included) — and because the cartridge is persisted as the subdomain's
//   public face, every reload re-ran it and re-hung => "subdomain requires
//   reset". You CANNOT preempt synchronous wasm from JS. So we run the cartridge
//   in THIS Web Worker: a hung frame only blocks the worker, the main thread
//   stays live, and the main thread's watchdog can `worker.terminate()` a worker
//   that stops posting frames. Containment is the whole point.
//
// WHAT THIS FILE IS:
//   A faithful JS re-implementation of the `host_display` ABI (clear / set_pixel
//   / fill_rect / draw_char / draw_number / draw_line / fill_triangle / present /
//   width / height / pointer_* / state_*), plus host_net (WebSocket — available
//   in workers), host_audio (forwarded to the main thread, which owns the
//   AudioContext), and the ambient host_log / host_time / host_abort modules. It
//   draws into a Uint32Array(FB_W*FB_H) framebuffer and, once per frame, posts
//   that framebuffer's ArrayBuffer (TRANSFERABLE, zero-copy) to the main thread
//   to blit.
//
//   ⚠️ DIVERGENCE RISK: the draw ops + 5x7 font + viewport/clip math here are a
//   HAND PORT of `src/raster.rs` (Rust). If you change the Rust raster, change
//   this too. `scripts/test-worker-host-parity.mjs` renders a known cartridge
//   through THIS host and asserts the pixel output matches the Rust host's
//   reference (scripts/render-cartridge.js), so the two can't silently drift —
//   run it (and `cargo test -p localharness raster`) when you touch either side.
//
// MESSAGE PROTOCOL (see display.rs for the main-thread counterpart):
//   main -> worker:
//     { type: 'load',  wasm: ArrayBuffer }   instantiate + start the frame loop
//     { type: 'input', x, y, down }          latest pointer (poll model)
//     { type: 'stop' }                        stop the loop (worker stays alive)
//   worker -> main:
//     { type: 'frame', fb: ArrayBuffer, w, h }  framebuffer for this tick (xfer)
//     { type: 'audio', op, args: [...] }        play a tone/noise on the main AudioContext
//     { type: 'error', detail }                 instantiate / fatal error
//     { type: 'log',   level, msg }             console passthrough

'use strict';

// Logical framebuffer resolution. MUST match FB_W/FB_H in src/app/display.rs.
const FB_W = 256;
const FB_H = 144;

// ---- shared mutable state for this cartridge run -----------------------------
let running = false;
let memory = null;            // the cartridge's WebAssembly.Memory (host_net strings)
let frameFn = null;           // exported frame(t) or render()
let isAnimated = false;       // true => frame(t) loop; false => one-shot render()
let startMs = 0;

// 32-bit RGBA framebuffer. We keep ONE backing ArrayBuffer and transfer it each
// frame, then allocate a fresh one (a transferred buffer is detached, so we must
// re-create). FB is little-endian: byte order R,G,B,A => packed 0xAABBGGRR.
let fbBytes = new Uint8ClampedArray(FB_W * FB_H * 4);
let fb32 = new Uint32Array(fbBytes.buffer);

// Input cells (poll model — the cartridge reads them each frame).
const ptr = { x: 0, y: 0, down: 0 };

// 64-slot integer register file (rustlite has no globals). Zeroed per load.
const state = new Int32Array(64);

// ---- raster core (PORT of src/raster.rs) ------------------------------------
// Identity viewport only (the single-cartridge path uses Viewport::full). The
// worker runs ONE cartridge fullscreen, so ox=oy=0, w=FB_W, h=FB_H — composition
// (offset viewports) stays on the main thread for now. Pack to 0xAABBGGRR so a
// single Uint32 write sets a pixel (matches ImageData's little-endian layout).
function packRgb(rgb) {
  // rgb is 0xRRGGBB (cartridge ABI). Alpha is always opaque.
  const r = (rgb >>> 16) & 0xff;
  const g = (rgb >>> 8) & 0xff;
  const b = rgb & 0xff;
  return (0xff << 24) | (b << 16) | (g << 8) | r; // 0xAABBGGRR
}

function setPixel(x, y, packed) {
  if (x < 0 || y < 0 || x >= FB_W || y >= FB_H) return;
  fb32[y * FB_W + x] = packed;
}

function fillRect(x, y, w, h, packed) {
  // Mirrors raster::fill_rect: clamp to [0, FB_W/H).
  const x0 = Math.max(0, x);
  const y0 = Math.max(0, y);
  const x1 = Math.min(FB_W, x + w);
  const y1 = Math.min(FB_H, y + h);
  for (let yy = y0; yy < y1; yy++) {
    let base = yy * FB_W;
    for (let xx = x0; xx < x1; xx++) fb32[base + xx] = packed;
  }
}

function drawLine(x0, y0, x1, y1, packed) {
  // Integer Bresenham — byte-for-byte with raster::draw_line.
  let dx = Math.abs(x1 - x0);
  let dy = -Math.abs(y1 - y0);
  let sx = x0 < x1 ? 1 : -1;
  let sy = y0 < y1 ? 1 : -1;
  let err = dx + dy;
  let x = x0, y = y0;
  for (;;) {
    setPixel(x, y, packed);
    if (x === x1 && y === y1) break;
    const e2 = 2 * err;
    if (e2 >= dy) { err += dy; x += sx; }
    if (e2 <= dx) { err += dx; y += sy; }
  }
}

// edge(): twice the signed area, the barycentric edge function. JS numbers are
// f64, which exactly represents these products at 256x144 (well under 2^53).
function edge(ax, ay, bx, by, cx, cy) {
  return (bx - ax) * (cy - ay) - (by - ay) * (cx - ax);
}

function fillTriangle(x0, y0, x1, y1, x2, y2, packed) {
  // Mirrors raster::fill_triangle (identity viewport: vp.w=FB_W, vp.h=FB_H).
  const minX = Math.max(0, Math.min(x0, x1, x2));
  const minY = Math.max(0, Math.min(y0, y1, y2));
  const maxX = Math.min(FB_W - 1, Math.max(x0, x1, x2));
  const maxY = Math.min(FB_H - 1, Math.max(y0, y1, y2));
  if (minX > maxX || minY > maxY) return;
  const area = edge(x0, y0, x1, y1, x2, y2);
  if (area === 0) return;
  const positive = area > 0;
  for (let py = minY; py <= maxY; py++) {
    for (let px = minX; px <= maxX; px++) {
      const w0 = edge(x1, y1, x2, y2, px, py);
      const w1 = edge(x2, y2, x0, y0, px, py);
      const w2 = edge(x0, y0, x1, y1, px, py);
      const inside = positive
        ? (w0 >= 0 && w1 >= 0 && w2 >= 0)
        : (w0 <= 0 && w1 <= 0 && w2 <= 0);
      if (inside) setPixel(px, py, packed);
    }
  }
}

// 5x7 bitmap font — a HAND PORT of raster::glyph_5x7. Each entry is 7 rows; each
// row's low 5 bits are pixels (bit 4 = leftmost). Keyed by codepoint. Missing
// codes fall through to the hollow box, exactly like the Rust `_ =>` arm.
const GLYPHS = {
  0x20: [0, 0, 0, 0, 0, 0, 0],
  0x30: [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
  0x31: [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
  0x32: [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
  0x33: [0x1E, 0x01, 0x01, 0x0E, 0x01, 0x01, 0x1E],
  0x34: [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
  0x35: [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
  0x36: [0x0E, 0x10, 0x10, 0x1E, 0x11, 0x11, 0x0E],
  0x37: [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
  0x38: [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
  0x39: [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x01, 0x0E],
  0x21: [0x04, 0x04, 0x04, 0x04, 0x04, 0x00, 0x04],
  0x22: [0x0A, 0x0A, 0x0A, 0x00, 0x00, 0x00, 0x00],
  0x23: [0x0A, 0x0A, 0x1F, 0x0A, 0x1F, 0x0A, 0x0A],
  0x25: [0x18, 0x19, 0x02, 0x04, 0x08, 0x13, 0x03],
  0x26: [0x0C, 0x12, 0x14, 0x08, 0x15, 0x12, 0x0D],
  0x27: [0x04, 0x04, 0x08, 0x00, 0x00, 0x00, 0x00],
  0x28: [0x04, 0x08, 0x10, 0x10, 0x10, 0x08, 0x04],
  0x29: [0x04, 0x02, 0x01, 0x01, 0x01, 0x02, 0x04],
  0x2A: [0x00, 0x04, 0x15, 0x0E, 0x15, 0x04, 0x00],
  0x2B: [0x00, 0x04, 0x04, 0x1F, 0x04, 0x04, 0x00],
  0x2C: [0x00, 0x00, 0x00, 0x00, 0x06, 0x04, 0x08],
  0x2D: [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00],
  0x2E: [0x00, 0x00, 0x00, 0x00, 0x00, 0x06, 0x06],
  0x2F: [0x01, 0x01, 0x02, 0x04, 0x08, 0x10, 0x10],
  0x3A: [0x00, 0x06, 0x06, 0x00, 0x06, 0x06, 0x00],
  0x3B: [0x00, 0x06, 0x06, 0x00, 0x06, 0x04, 0x08],
  0x3C: [0x02, 0x04, 0x08, 0x10, 0x08, 0x04, 0x02],
  0x3D: [0x00, 0x00, 0x1F, 0x00, 0x1F, 0x00, 0x00],
  0x3E: [0x08, 0x04, 0x02, 0x01, 0x02, 0x04, 0x08],
  0x3F: [0x0E, 0x11, 0x01, 0x02, 0x04, 0x00, 0x04],
  0x40: [0x0E, 0x11, 0x17, 0x15, 0x17, 0x10, 0x0E],
  0x5B: [0x0E, 0x08, 0x08, 0x08, 0x08, 0x08, 0x0E],
  0x5D: [0x0E, 0x02, 0x02, 0x02, 0x02, 0x02, 0x0E],
  0x5F: [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1F],
  0x41: [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
  0x42: [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
  0x43: [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
  0x44: [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],
  0x45: [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
  0x46: [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
  0x47: [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0E],
  0x48: [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
  0x49: [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
  0x4A: [0x07, 0x02, 0x02, 0x02, 0x12, 0x12, 0x0C],
  0x4B: [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
  0x4C: [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
  0x4D: [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
  0x4E: [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11],
  0x4F: [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
  0x50: [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
  0x51: [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
  0x52: [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
  0x53: [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
  0x54: [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
  0x55: [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
  0x56: [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
  0x57: [0x11, 0x11, 0x11, 0x15, 0x15, 0x1B, 0x11],
  0x58: [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
  0x59: [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04],
  0x5A: [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
  0x61: [0x00, 0x00, 0x0E, 0x01, 0x0F, 0x11, 0x0F],
  0x62: [0x10, 0x10, 0x16, 0x19, 0x11, 0x11, 0x1E],
  0x63: [0x00, 0x00, 0x0E, 0x10, 0x10, 0x11, 0x0E],
  0x64: [0x01, 0x01, 0x0D, 0x13, 0x11, 0x11, 0x0F],
  0x65: [0x00, 0x00, 0x0E, 0x11, 0x1F, 0x10, 0x0E],
  0x66: [0x06, 0x09, 0x08, 0x1C, 0x08, 0x08, 0x08],
  0x67: [0x00, 0x0F, 0x11, 0x11, 0x0F, 0x01, 0x0E],
  0x68: [0x10, 0x10, 0x16, 0x19, 0x11, 0x11, 0x11],
  0x69: [0x04, 0x00, 0x0C, 0x04, 0x04, 0x04, 0x0E],
  0x6A: [0x02, 0x00, 0x06, 0x02, 0x02, 0x12, 0x0C],
  0x6B: [0x10, 0x10, 0x12, 0x14, 0x18, 0x14, 0x12],
  0x6C: [0x0C, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
  0x6D: [0x00, 0x00, 0x1A, 0x15, 0x15, 0x11, 0x11],
  0x6E: [0x00, 0x00, 0x16, 0x19, 0x11, 0x11, 0x11],
  0x6F: [0x00, 0x00, 0x0E, 0x11, 0x11, 0x11, 0x0E],
  0x70: [0x00, 0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10],
  0x71: [0x00, 0x0F, 0x11, 0x11, 0x0F, 0x01, 0x01],
  0x72: [0x00, 0x00, 0x16, 0x19, 0x10, 0x10, 0x10],
  0x73: [0x00, 0x00, 0x0F, 0x10, 0x0E, 0x01, 0x1E],
  0x74: [0x08, 0x08, 0x1C, 0x08, 0x08, 0x09, 0x06],
  0x75: [0x00, 0x00, 0x11, 0x11, 0x11, 0x13, 0x0D],
  0x76: [0x00, 0x00, 0x11, 0x11, 0x11, 0x0A, 0x04],
  0x77: [0x00, 0x00, 0x11, 0x11, 0x15, 0x15, 0x0A],
  0x78: [0x00, 0x00, 0x11, 0x0A, 0x04, 0x0A, 0x11],
  0x79: [0x00, 0x11, 0x11, 0x11, 0x0F, 0x01, 0x0E],
  0x7A: [0x00, 0x00, 0x1F, 0x02, 0x04, 0x08, 0x1F],
};
const GLYPH_BOX = [0x1F, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1F]; // unknown -> box

function glyph5x7(code) {
  const g = GLYPHS[code];
  return g !== undefined ? g : GLYPH_BOX;
}

function blitGlyph(x, y, code, packed, scale) {
  const glyph = glyph5x7(code >>> 0);
  const s = Math.max(1, scale | 0);
  for (let row = 0; row < 7; row++) {
    const bits = glyph[row];
    for (let col = 0; col < 5; col++) {
      if (((bits >> (4 - col)) & 1) === 0) continue;
      for (let dy = 0; dy < s; dy++) {
        for (let dx = 0; dx < s; dx++) {
          setPixel(x + col * s + dx, y + row * s + dy, packed);
        }
      }
    }
  }
}

function drawNumber(x, y, value, packed, scale) {
  // Mirrors raster::draw_number. `value` is i32; render base-10 with a leading
  // '-' for negatives. Use Math.abs over the i32 (no i64 needed — i32::MIN's
  // abs fits a JS number exactly).
  const s = Math.max(1, scale | 0);
  const advance = 6 * s;
  let cx = x;
  let n = Math.abs(value | 0);
  if ((value | 0) < 0) {
    blitGlyph(cx, y, 0x2D, packed, s); // '-'
    cx += advance;
  }
  const digits = [];
  if (n === 0) {
    digits.push(0x30);
  } else {
    while (n > 0) {
      digits.push(0x30 + (n % 10));
      n = Math.floor(n / 10);
    }
  }
  for (let i = digits.length - 1; i >= 0; i--) {
    blitGlyph(cx, y, digits[i], packed, s);
    cx += advance;
  }
}

// ---- host_net: WebSocket (works in a worker) --------------------------------
// Faithful port of display.rs::net — poll-model sockets, SSRF wss-only gate,
// MAX_SOCKETS / MAX_INBOX caps, length-prefixed strings over cartridge memory.
const MAX_INBOX = 256;
const MAX_SOCKETS = 8;
const sockets = []; // index = handle; closed slots become null

function urlIsAllowed(url) {
  // Port of display.rs::net::url_is_allowed — wss:// only, no loopback/LAN/IP.
  const m = url.split('://');
  if (m.length < 2 || m[0].toLowerCase() !== 'wss') return false;
  const rest = m.slice(1).join('://');
  const authority = rest.split(/[/?#]/)[0] || '';
  const at = authority.lastIndexOf('@');
  const hostport = at >= 0 ? authority.slice(at + 1) : authority;
  if (hostport.startsWith('[')) return false; // IPv6 literal
  const host = hostport.split(':')[0] || '';
  if (host === '') return false;
  const lower = host.toLowerCase();
  if (lower === 'localhost' || lower.endsWith('.localhost') || lower.endsWith('.local')) {
    return false;
  }
  const octets = lower.split('.');
  if (octets.length === 4 && octets.every((o) => o.length > 0 && /^[0-9]+$/.test(o))) {
    return false; // bare IPv4 literal
  }
  return lower.includes('.');
}

function memU8() {
  // Fresh view each call — memory.buffer detaches on grow.
  return new Uint8Array(memory.buffer);
}

function readString(p) {
  if (p < 0 || memory === null) return null;
  const a = memU8();
  const cap = a.length;
  if (p + 4 > cap) return null;
  const len = a[p] | (a[p + 1] << 8) | (a[p + 2] << 16) | (a[p + 3] << 24);
  if (len < 0 || len > 65536 || p + 4 + len > cap) return null;
  return new TextDecoder().decode(a.subarray(p + 4, p + 4 + len));
}

function writeString(outPtr, s, max) {
  if (outPtr < 0 || memory === null) return -1;
  const a = memU8();
  const cap = a.length;
  let bytes = new TextEncoder().encode(s);
  if (bytes.length > max) {
    let end = max;
    // Don't split a UTF-8 codepoint: back off continuation bytes (0b10xxxxxx).
    while (end > 0 && (bytes[end] & 0xc0) === 0x80) end--;
    bytes = bytes.subarray(0, end);
  }
  const len = bytes.length;
  if (outPtr + 4 + len > cap) return -1;
  a[outPtr] = len & 0xff;
  a[outPtr + 1] = (len >> 8) & 0xff;
  a[outPtr + 2] = (len >> 16) & 0xff;
  a[outPtr + 3] = (len >> 24) & 0xff;
  a.set(bytes, outPtr + 4);
  return len;
}

const host_net = {
  open(urlPtr) {
    const url = readString(urlPtr);
    if (url === null || !urlIsAllowed(url)) return -1;
    const live = sockets.filter((s) => s !== null).length;
    if (live >= MAX_SOCKETS) return -1;
    let ws;
    try {
      ws = new WebSocket(url);
    } catch (_e) {
      return -1;
    }
    ws.binaryType = 'arraybuffer';
    const inbox = [];
    ws.onmessage = (e) => {
      if (typeof e.data === 'string') {
        if (inbox.length >= MAX_INBOX) inbox.shift();
        inbox.push(e.data);
      }
    };
    const sock = { ws, inbox };
    let i = sockets.indexOf(null);
    if (i < 0) { i = sockets.length; sockets.push(sock); } else { sockets[i] = sock; }
    return i;
  },
  send(handle, p) {
    const msg = readString(p);
    if (msg === null) return 0;
    const s = sockets[handle];
    if (!s) return 0;
    try { s.ws.send(msg); return 1; } catch (_e) { return 0; }
  },
  poll(handle, outPtr, max) {
    const s = sockets[handle];
    if (!s) return -1;
    if (s.inbox.length === 0) return 0;
    const msg = s.inbox.shift();
    return writeString(outPtr, msg, Math.max(0, max));
  },
  status(handle) {
    const s = sockets[handle];
    return s ? s.ws.readyState : -1; // 0 CONNECTING 1 OPEN 2 CLOSING 3 CLOSED
  },
  close(handle) {
    const s = sockets[handle];
    if (s) { try { s.ws.close(); } catch (_e) {} sockets[handle] = null; }
  },
};

function closeAllSockets() {
  for (let i = 0; i < sockets.length; i++) {
    const s = sockets[i];
    if (s) { try { s.ws.close(); } catch (_e) {} }
    sockets[i] = null;
  }
  sockets.length = 0;
}

// ---- host_audio: forward to the main thread (AudioContext isn't in a worker) -
// The cartridge ABI returns a voice handle; from a worker we can't know the
// real handle synchronously, so we return a monotonic local handle and forward
// the op. `stop` forwards the handle as-is. This loses precise per-voice stop
// mapping across the boundary, but tone/noise/stop-all behave correctly, which
// is what cartridges use. (Documented divergence from the in-thread engine.)
let audioHandle = 0;
function postAudio(op, args) {
  self.postMessage({ type: 'audio', op, args });
}
const host_audio = {
  tone(freq, dur, wave) { postAudio('tone', [freq, dur, wave]); return audioHandle++; },
  tone_at(freq, dur, wave, delay) { postAudio('tone_at', [freq, dur, wave, delay]); return audioHandle++; },
  noise(dur) { postAudio('noise', [dur]); return audioHandle++; },
  stop(handle) { postAudio('stop', [handle]); },
  set_volume(pct) { postAudio('set_volume', [pct]); },
};

// ---- host_display ------------------------------------------------------------
const host_display = {
  clear: (rgb) => fillRect(0, 0, FB_W, FB_H, packRgb(rgb)),
  set_pixel: (x, y, rgb) => setPixel(x, y, packRgb(rgb)),
  fill_rect: (x, y, w, h, rgb) => fillRect(x, y, w, h, packRgb(rgb)),
  draw_char: (x, y, code, rgb, scale) => blitGlyph(x, y, code, packRgb(rgb), scale),
  draw_number: (x, y, value, rgb, scale) => drawNumber(x, y, value, packRgb(rgb), scale),
  draw_line: (x0, y0, x1, y1, rgb) => drawLine(x0, y0, x1, y1, packRgb(rgb)),
  fill_triangle: (x0, y0, x1, y1, x2, y2, rgb) => fillTriangle(x0, y0, x1, y1, x2, y2, packRgb(rgb)),
  // present is a NO-OP — the HOST (the frame loop below) presents once after
  // frame() returns, matching the present-ownership inversion in display.rs.
  present: () => {},
  width: () => FB_W,
  height: () => FB_H,
  pointer_x: () => ptr.x,
  pointer_y: () => ptr.y,
  pointer_down: () => ptr.down,
  state_get: (slot) => (slot >= 0 && slot < 64 ? state[slot] : 0),
  state_set: (slot, value) => { if (slot >= 0 && slot < 64) state[slot] = value | 0; },
};

// ---- ambient host modules (host_log / host_time / host_abort) ----------------
const host_log = {
  info: () => {}, warn: () => {}, error: () => {}, debug: () => {},
};
const host_time = {
  now_unix_ms: () => Date.now(),
  monotonic_ms: () => Date.now(),
};
const host_abort = {
  panic: () => { self.postMessage({ type: 'log', level: 'error', msg: '[cartridge] panic' }); },
  // fuel_remaining: a REAL per-frame decreasing budget now (was a dead no-op in
  // the Rust loader). The compiler emits no fuel checks today, so this only
  // helps cartridges that voluntarily poll it — the worker watchdog is the
  // actual hang defense. Reset to FUEL_PER_FRAME at the top of each frame.
  fuel_remaining: () => fuel,
  memory_bytes: () => (memory ? memory.buffer.byteLength : 0),
};

const FUEL_PER_FRAME = 1_000_000;
let fuel = FUEL_PER_FRAME;

function buildImports() {
  return { host_display, host_net, host_audio, host_log, host_time, host_abort };
}

// ---- present + frame loop ----------------------------------------------------
function present() {
  // Transfer the framebuffer's ArrayBuffer to the main thread (zero-copy), then
  // re-create our backing store (the transferred buffer is now detached).
  const buf = fbBytes.buffer;
  self.postMessage({ type: 'frame', fb: buf, w: FB_W, h: FB_H }, [buf]);
  fbBytes = new Uint8ClampedArray(FB_W * FB_H * 4);
  fb32 = new Uint32Array(fbBytes.buffer);
}

function tick() {
  if (!running) return;
  fuel = FUEL_PER_FRAME;
  const t = (Date.now() - startMs) | 0;
  try {
    frameFn(t);
  } catch (e) {
    // A wasm trap (unreachable / OOB) ends the run cleanly rather than spinning.
    running = false;
    self.postMessage({ type: 'error', detail: 'cartridge trapped: ' + (e && e.message ? e.message : String(e)) });
    return;
  }
  present();
  // Worker rAF is available in modern browsers, but to keep this dependency-free
  // and predictable we self-pace with a ~16ms timer. The main thread's watchdog
  // (not this cadence) is what detects a hung frame.
  if (running) setTimeout(tick, 16);
}

async function load(wasmBuf) {
  running = false;
  closeAllSockets();
  state.fill(0);
  ptr.x = 0; ptr.y = 0; ptr.down = 0;
  memory = null;
  fbBytes = new Uint8ClampedArray(FB_W * FB_H * 4);
  fb32 = new Uint32Array(fbBytes.buffer);

  let instance;
  try {
    const result = await WebAssembly.instantiate(wasmBuf, buildImports());
    instance = result.instance;
  } catch (e) {
    self.postMessage({ type: 'error', detail: 'instantiate failed: ' + (e && e.message ? e.message : String(e)) });
    return;
  }
  const exp = instance.exports;
  memory = exp.memory || null;

  if (typeof exp.frame === 'function') {
    frameFn = exp.frame;
    isAnimated = true;
  } else if (typeof exp.render === 'function') {
    frameFn = exp.render;
    isAnimated = false;
  } else {
    self.postMessage({ type: 'error', detail: 'cartridge exports neither frame nor render' });
    return;
  }

  running = true;
  startMs = Date.now();
  if (isAnimated) {
    tick();
  } else {
    // One-shot render(): draw once, present once, then idle (worker stays alive
    // so a re-load reuses it). Still inside the worker => a hung render() can't
    // freeze the main thread.
    fuel = FUEL_PER_FRAME;
    try {
      frameFn();
    } catch (e) {
      self.postMessage({ type: 'error', detail: 'cartridge trapped: ' + (e && e.message ? e.message : String(e)) });
      running = false;
      return;
    }
    present();
    running = false;
  }
}

// Wire the worker message handler ONLY when running as an actual Web Worker
// (DedicatedWorkerGlobalScope has `postMessage` on `self`). Under Node (the
// host-parity test imports this file via `require`), `self`/`postMessage` don't
// exist — skip the wiring and export the pure host surface instead so the test
// can render a cartridge through THIS host and diff it against the Rust
// reference. This keeps a single source of truth for the host re-implementation.
const IS_WORKER =
  typeof self !== 'undefined' && typeof self.postMessage === 'function';
if (IS_WORKER) {
  self.onmessage = (e) => {
    const msg = e.data;
    if (!msg || typeof msg.type !== 'string') return;
    switch (msg.type) {
      case 'load':
        load(msg.wasm);
        break;
      case 'input':
        ptr.x = msg.x | 0;
        ptr.y = msg.y | 0;
        ptr.down = msg.down | 0;
        break;
      case 'stop':
        running = false;
        closeAllSockets();
        break;
      default:
        break;
    }
  };
}

// Node-only test surface (the host-parity harness). NOT used by the worker.
if (typeof module !== 'undefined' && module.exports) {
  module.exports = {
    FB_W,
    FB_H,
    host_display, // the re-implemented draw ABI under test
    glyph5x7, // expose the font table for a byte-for-byte vs-Rust check
    packRgb,
    // Hand-instantiate a cartridge against this host (no Worker, no postMessage)
    // and return the presented framebuffer as a fresh Uint8ClampedArray. Drives
    // the present-after-frame model: frame(t) draws, then we snapshot.
    renderOnce(wasmBytes, t = 0) {
      fbBytes = new Uint8ClampedArray(FB_W * FB_H * 4);
      fb32 = new Uint32Array(fbBytes.buffer);
      state.fill(0);
      ptr.x = 0; ptr.y = 0; ptr.down = 0;
      const mod = new WebAssembly.Module(wasmBytes);
      const importObj = {};
      for (const imp of WebAssembly.Module.imports(mod)) {
        importObj[imp.module] = importObj[imp.module] || {};
      }
      Object.assign(importObj, buildImports());
      // Supply any imports the cartridge declares that our host doesn't (e.g. a
      // module-provided memory) so instantiation can't fail on a missing import.
      for (const imp of WebAssembly.Module.imports(mod)) {
        if (importObj[imp.module][imp.name] !== undefined) continue;
        if (imp.kind === 'memory') {
          importObj[imp.module][imp.name] = new WebAssembly.Memory({ initial: 1 });
        } else if (imp.kind === 'function') {
          importObj[imp.module][imp.name] = () => 0;
        }
      }
      const inst = new WebAssembly.Instance(mod, importObj);
      memory = inst.exports.memory || null;
      const fn = inst.exports.frame || inst.exports.render;
      fn(t);
      return fbBytes.slice();
    },
    // Drive the host_display ABI directly (no cartridge) so the parity test can
    // exercise set_pixel / draw_line / fill_triangle — ops `bitmask.rl` doesn't
    // use — and snapshot the result. `ops` is a list of [methodName, ...args].
    drawProbe(ops) {
      fbBytes = new Uint8ClampedArray(FB_W * FB_H * 4);
      fb32 = new Uint32Array(fbBytes.buffer);
      for (const [name, ...args] of ops) host_display[name](...args);
      return fbBytes.slice();
    },
  };
}
