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
//     { type: 'error', code, detail }           instantiate / fatal error (code = LH1xxx)
//     { type: 'log',   level, msg }             console passthrough
//     { type: 'done' }                          a one-shot render() finished (disarm
//                                               the watchdog — no more frames coming)
//
//   The `code` on an error message is a stable LH1xxx runtime code from the
//   localharness error registry (the LH-prefixed integer; see src/error_codes.rs
//   + docs/error-codes.md). It mirrors LH_RUNTIME below so display.rs can show
//   the code + meaning in the "CARTRIDGE STOPPED" overlay. (The watchdog-timeout
//   code LH1001 is assigned on the MAIN thread, since a hung worker can't post.)

'use strict';

// Stable LH1xxx runtime error codes (mirror of the LH1xxx block in
// src/error_codes.rs / docs/error-codes.md). LH1001 (frame timeout) is the
// watchdog's; the worker reports the trap / instantiate / no-entry ones.
const LH_RUNTIME = {
  WASM_TRAP: 1002,
  INSTANTIATE_FAILED: 1003,
  NO_ENTRY: 1004,
};
function lhLabel(code) {
  return 'LH' + String(code).padStart(4, '0');
}
function postError(code, detail) {
  self.postMessage({ type: 'error', code, detail });
}

// Logical framebuffer resolution. The DEFAULT (320x240, 4:3) MUST match the
// FB_W/FB_H defaults in src/app/display.rs. A cartridge MAY override these per
// load by exporting `dims() -> i32` returning a PACKED (width << 16) | height
// (width in the high 16 bits, height in the low 16). The worker calls it ONCE
// after instantiate; a cartridge with NO `dims()` export keeps the default, so
// every existing cartridge renders EXACTLY as before (backward compatible).
//
// These are mutable (`let`): `applyDims()` rewrites them at load time. The
// Node test surface still exports the DEFAULTS (FB_W_DEFAULT/FB_H_DEFAULT) and
// the live values, and `renderOnce` honors a cartridge's `dims()` too.
const FB_W_DEFAULT = 512;
const FB_H_DEFAULT = 512;
// Clamp range for a cartridge-declared dimension. The lower bound keeps a
// cartridge from declaring a degenerate (0/negative) surface; the upper bound
// caps the per-frame postMessage transfer cost (a frame is w*h*4 bytes, so
// 1024x1024 = 4MB/frame is already the ceiling we accept).
const FB_MIN = 16;
const FB_MAX = 1024;
let FB_W = FB_W_DEFAULT;
let FB_H = FB_H_DEFAULT;

// Decode a packed `(w << 16) | h` dims value and validate/clamp it. Returns
// `[w, h]` on success, or `null` (caller falls back to the default + logs) when
// either dimension is out of [FB_MIN, FB_MAX].
function decodeDims(packed) {
  const w = (packed >>> 16) & 0xffff;
  const h = packed & 0xffff;
  if (w < FB_MIN || w > FB_MAX || h < FB_MIN || h > FB_MAX) return null;
  return [w, h];
}

// Set the live framebuffer dimensions from a cartridge's `dims()` export (or
// reset to default when `instanceExports.dims` is absent). Reallocates the
// backing framebuffer to match. Logs + falls back to default on an invalid
// declared size.
function applyDims(instanceExports) {
  FB_W = FB_W_DEFAULT;
  FB_H = FB_H_DEFAULT;
  if (instanceExports && typeof instanceExports.dims === 'function') {
    let packed;
    try {
      packed = instanceExports.dims() | 0;
    } catch (_e) {
      packed = 0;
    }
    const dims = decodeDims(packed >>> 0);
    if (dims) {
      FB_W = dims[0];
      FB_H = dims[1];
    } else {
      const warn = '[cartridge] dims() out of range [' + FB_MIN + ',' + FB_MAX +
        '] (packed ' + (packed >>> 0) + ') — falling back to ' +
        FB_W_DEFAULT + 'x' + FB_H_DEFAULT;
      // Worker path posts a log message; under Node (test harness, no `self`)
      // fall back to console so the warning isn't lost.
      if (typeof self !== 'undefined' && typeof self.postMessage === 'function') {
        self.postMessage({ type: 'log', level: 'warn', msg: warn });
      } else if (typeof console !== 'undefined') {
        console.warn(warn);
      }
    }
  }
  fbBytes = new Uint8ClampedArray(FB_W * FB_H * 4);
  fb32 = new Uint32Array(fbBytes.buffer);
}

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
// f64, which exactly represents these products even at the 1024×1024 max
// framebuffer (cross products stay ~1e6, well under 2^53).
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

// ---- host_compose: cartridge-in-cartridge composition -----------------------
// A HAND PORT of `src/compose.rs::blit_child` + `map_pointer_into_child` (Rust).
// `scripts/test-compose-wiring.mjs` diffs these against the Rust impls, so the
// two can't silently drift — run it (and `cargo test -p localharness compose`)
// when you touch either side. Both take packed-u32 (0xAABBGGRR) framebuffers.

// Composite a CHILD framebuffer into a viewport (x, y, vw, vh) of a PARENT
// framebuffer. Nearest-neighbour integer scaling; total edge clipping; never
// indexes out of bounds (a dst/child shorter than its declared w*h is
// tolerated). Byte-for-byte mirror of compose.rs::blit_child.
function blitChild(dst, dstW, dstH, child, childW, childH, x, y, viewW, viewH) {
  if (viewW <= 0 || viewH <= 0 || childW <= 0 || childH <= 0 || dstW <= 0 || dstH <= 0) return;
  const dx0 = Math.max(0, x);
  const dy0 = Math.max(0, y);
  const dx1 = Math.min(x + viewW, dstW);
  const dy1 = Math.min(y + viewH, dstH);
  if (dx0 >= dx1 || dy0 >= dy1) return;
  for (let dy = dy0; dy < dy1; dy++) {
    const vy = dy - y;
    const sy = Math.trunc((vy * childH) / viewH);
    if (sy < 0 || sy >= childH) continue;
    const srcRow = sy * childW;
    const dstRow = dy * dstW;
    for (let dx = dx0; dx < dx1; dx++) {
      const vx = dx - x;
      const sx = Math.trunc((vx * childW) / viewW);
      if (sx < 0 || sx >= childW) continue;
      const si = srcRow + sx;
      const di = dstRow + dx;
      if (si < child.length && di < dst.length) dst[di] = child[si];
    }
  }
}

// Map a PARENT pointer (px, py) into a CHILD's local space given its viewport
// (x, y, vw, vh) and native (childW, childH). Returns [cx, cy] or null when the
// pointer is outside the viewport. Mirror of compose.rs::map_pointer_into_child.
function mapPointerIntoChild(px, py, x, y, viewW, viewH, childW, childH) {
  if (viewW <= 0 || viewH <= 0 || childW <= 0 || childH <= 0) return null;
  if (px < x || py < y || px >= x + viewW || py >= y + viewH) return null;
  const vx = px - x;
  const vy = py - y;
  let cx = Math.trunc((vx * childW) / viewW);
  let cy = Math.trunc((vy * childH) / viewH);
  cx = Math.max(0, Math.min(cx, childW - 1));
  cy = Math.max(0, Math.min(cy, childH - 1));
  return [cx, cy];
}

// ComposeBudget (mirror of compose.rs::ComposeBudget::v1). Caps the compose
// graph so an attacker-authored or runaway parent can't exhaust the worker.
// RECURSION is real now: a child gets its OWN compose table and can spawn
// grandchildren (the fractal). The fork-bomb backstop is three GLOBAL caps that
// hold across the WHOLE tree (not per-parent):
//   • per-node child count (8) — one node's immediate children
//   • depth (5) — root=0; a node at the cap gets an INERT compose api so its
//     spawn_module returns -1 (the ABI-level recursion stop)
//   • total live nodes (24) and total wasm bytes (256 KB) across every level
const COMPOSE_MAX_CHILDREN = 8;
const COMPOSE_MAX_BYTES_PER_CHILD = 16 * 1024;
const COMPOSE_MAX_TOTAL_BYTES = 256 * 1024;
const COMPOSE_MAX_DEPTH = 5;
const COMPOSE_MAX_NODES = 24;
// Framebuffer-memory caps (mirror compose.rs ComposeBudget v1). The wasm-byte
// caps above DON'T bound this: a 16 KB cartridge can declare dims()=1024x1024
// and allocate `new Uint32Array(1024*1024)` = 4 MB, so 24 such nodes = 96 MB+
// of worker memory while passing every byte/node cap (issue #78). Each child's
// surface is w*h*4 bytes; cap it per-child AND tree-wide.
const COMPOSE_MAX_FB_BYTES_PER_CHILD = 1024 * 1024; // 1 MB (≈512x512)
const COMPOSE_MAX_TOTAL_FB_BYTES = 8 * 1024 * 1024; // 8 MB across the whole tree

// Child module states (mirror the host::compose status() ABI).
const MOD_LOADING = 0;
const MOD_READY = 1;
const MOD_FAILED = 2;

// The compose tree. Every node — the root parent AND every composited child —
// owns a `children` array + a `focus` handle, so composition is recursive: a
// child's children blit into the child's buffer, which blits into its parent's,
// up to the root framebuffer. The ROOT node's buffer/dims ARE the live
// FB_W/FB_H/fb32 (handled specially in the composite walk), so it carries no fb
// of its own; `memory` is refreshed from the live root `memory` each load/pass.
// A child node additionally owns w/h/fb/instance-memory/frame/state/ptr.
let rootNode = { children: [], focus: -1, depth: 0, memory: null };
let composeTotalBytes = 0;       // wasm bytes across the whole tree
let composeTotalFbBytes = 0;     // framebuffer bytes (w*h*4) across the whole tree
let composeTotalNodes = 0;       // live (non-failed) child nodes across the tree
let composeNextUid = 1;          // monotonic; the compose_bytes round-trip key
const composeNodeIndex = new Map(); // uid -> child node (main-thread reply target)

// Live (non-failed) immediate children of one node.
function liveChildCount(children) {
  let n = 0;
  for (const c of children) if (c && c.state !== MOD_FAILED) n++;
  return n;
}
// Root-level live count (back-compat name; used by the root spawn cap + tests).
function composeLiveCount() {
  return liveChildCount(rootNode.children);
}

// First reusable slot index in a node's table: a null hole OR a FAILED tombstone
// (issue #92). Without this a spawn-and-fail loop appends a fresh tombstone every
// call — the array grows unbounded and the per-frame composite walk is O(n) over
// dead slots. A FAILED node's budget (composeTotalNodes / bytes / index entry) was
// already reclaimed when it died, so overwriting the slot is safe with no
// re-accounting; the prior reference is simply dropped. Returns -1 → caller pushes.
function reclaimableSlot(children) {
  for (let i = 0; i < children.length; i++) {
    const c = children[i];
    if (!c || c.state === MOD_FAILED) return i;
  }
  return -1;
}

// A fresh child slot in state LOADING (bytes arrive via the compose_bytes
// round-trip). `depth` is the parent's depth + 1; `uid` keys the async reply.
function makeChildSlot(name, x, y, w, h, depth, uid) {
  return {
    name, uid, depth, state: MOD_LOADING,
    vp: { x: x | 0, y: y | 0, w: Math.max(1, w | 0), h: Math.max(1, h | 0) },
    w: FB_W_DEFAULT, h: FB_H_DEFAULT, fb: null,
    memory: null, frame: null, bytes: 0, fbBytes: 0,
    state_regs: new Int32Array(64),
    ptr: { x: -1, y: -1, down: 0 },
    children: [], focus: -1,
  };
}

// Recursively free a child + its whole subtree's budget (bytes/nodes) and index
// entries. Used by close (the slot is then nulled by the caller) and by a frame
// trap (the slot stays as a FAILED tombstone so status() still reports 2). After
// this, the node no longer counts toward COMPOSE_MAX_NODES / _TOTAL_BYTES.
function reclaimSubtree(child) {
  if (!child) return;
  for (let i = 0; i < child.children.length; i++) {
    reclaimSubtree(child.children[i]);
    child.children[i] = null;
  }
  if (child.state === MOD_READY) {
    composeTotalBytes -= child.bytes;
    composeTotalFbBytes -= child.fbBytes; // its framebuffer is freed with the node
  }
  if (child.state !== MOD_FAILED) composeTotalNodes -= 1; // a FAILED node was already uncounted
  composeNodeIndex.delete(child.uid);
}

// Read a child's dims() the same way applyDims does for the parent: packed
// (w<<16)|h, clamped to [FB_MIN, FB_MAX]; default 320x240 when absent/invalid.
function childDims(exports) {
  if (exports && typeof exports.dims === 'function') {
    let packed;
    try { packed = exports.dims() | 0; } catch (_e) { packed = 0; }
    const d = decodeDims(packed >>> 0);
    if (d) return d;
  }
  return [FB_W_DEFAULT, FB_H_DEFAULT];
}

// Build a child's OWN host imports: a host_display bound to the child's private
// buffer (NOT the shared FB — the host blits the child's buffer in after its
// frame), its own pointer cell, its own 64-slot state, and inert host_net/
// host_audio/host_agent/host_compose (recursion is the parent's job, capped by
// ComposeBudget; a child's spawn_module returns FAILED). The child is unmodified
// — it draws into a (0,0)-origin surface of its native size, oblivious to being
// composited.
function buildChildImports(child) {
  const cw = () => child.w;
  const ch = () => child.h;
  function cSetPixel(x, y, packed) {
    if (x < 0 || y < 0 || x >= child.w || y >= child.h) return;
    child.fb[y * child.w + x] = packed;
  }
  function cFillRect(x, y, w, h, packed) {
    const x0 = Math.max(0, x), y0 = Math.max(0, y);
    const x1 = Math.min(child.w, x + w), y1 = Math.min(child.h, y + h);
    for (let yy = y0; yy < y1; yy++) {
      const base = yy * child.w;
      for (let xx = x0; xx < x1; xx++) child.fb[base + xx] = packed;
    }
  }
  function cBlitGlyph(x, y, code, packed, scale) {
    const glyph = glyph5x7(code >>> 0);
    const s = Math.max(1, scale | 0);
    for (let row = 0; row < 7; row++) {
      const bits = glyph[row];
      for (let col = 0; col < 5; col++) {
        if (((bits >> (4 - col)) & 1) === 0) continue;
        for (let dy = 0; dy < s; dy++) for (let dx = 0; dx < s; dx++) cSetPixel(x + col * s + dx, y + row * s + dy, packed);
      }
    }
  }
  function cDrawNumber(x, y, value, packed, scale) {
    const s = Math.max(1, scale | 0);
    const advance = 6 * s;
    let cx = x;
    let n = Math.abs(value | 0);
    if ((value | 0) < 0) { cBlitGlyph(cx, y, 0x2D, packed, s); cx += advance; }
    const digits = [];
    if (n === 0) digits.push(0x30);
    else while (n > 0) { digits.push(0x30 + (n % 10)); n = Math.floor(n / 10); }
    for (let i = digits.length - 1; i >= 0; i--) { cBlitGlyph(cx, y, digits[i], packed, s); cx += advance; }
  }
  function cDrawLine(x0, y0, x1, y1, packed) {
    let dx = Math.abs(x1 - x0), dy = -Math.abs(y1 - y0);
    let sx = x0 < x1 ? 1 : -1, sy = y0 < y1 ? 1 : -1, err = dx + dy, x = x0, y = y0;
    for (;;) { cSetPixel(x, y, packed); if (x === x1 && y === y1) break; const e2 = 2 * err; if (e2 >= dy) { err += dy; x += sx; } if (e2 <= dx) { err += dx; y += sy; } }
  }
  function cFillTriangle(x0, y0, x1, y1, x2, y2, packed) {
    const minX = Math.max(0, Math.min(x0, x1, x2)), minY = Math.max(0, Math.min(y0, y1, y2));
    const maxX = Math.min(child.w - 1, Math.max(x0, x1, x2)), maxY = Math.min(child.h - 1, Math.max(y0, y1, y2));
    if (minX > maxX || minY > maxY) return;
    const area = edge(x0, y0, x1, y1, x2, y2);
    if (area === 0) return;
    const positive = area > 0;
    for (let py = minY; py <= maxY; py++) for (let px = minX; px <= maxX; px++) {
      const w0 = edge(x1, y1, x2, y2, px, py), w1 = edge(x2, y2, x0, y0, px, py), w2 = edge(x0, y0, x1, y1, px, py);
      const inside = positive ? (w0 >= 0 && w1 >= 0 && w2 >= 0) : (w0 <= 0 && w1 <= 0 && w2 <= 0);
      if (inside) cSetPixel(px, py, packed);
    }
  }
  const child_display = {
    clear: (rgb) => cFillRect(0, 0, child.w, child.h, packRgb(rgb)),
    set_pixel: (x, y, rgb) => cSetPixel(x, y, packRgb(rgb)),
    fill_rect: (x, y, w, h, rgb) => cFillRect(x, y, w, h, packRgb(rgb)),
    draw_char: (x, y, code, rgb, scale) => cBlitGlyph(x, y, code, packRgb(rgb), scale),
    draw_number: (x, y, value, rgb, scale) => cDrawNumber(x, y, value, packRgb(rgb), scale),
    draw_line: (x0, y0, x1, y1, rgb) => cDrawLine(x0, y0, x1, y1, packRgb(rgb)),
    fill_triangle: (x0, y0, x1, y1, x2, y2, rgb) => cFillTriangle(x0, y0, x1, y1, x2, y2, packRgb(rgb)),
    present: () => {}, // host presents the composited frame, never a child
    width: cw,
    height: ch,
    // Pointer is filled per-frame (focus-gated) into child.ptr; -1 means "no
    // pointer here" so a poll-model child can tell unfocused/outside from (0,0).
    pointer_x: () => child.ptr.x,
    pointer_y: () => child.ptr.y,
    pointer_down: () => child.ptr.down,
    state_get: (slot) => (slot >= 0 && slot < 64 ? child.state_regs[slot] : 0),
    state_set: (slot, value) => { if (slot >= 0 && slot < 64) child.state_regs[slot] = value | 0; },
  };
  // A child gets a REAL compose api bound to its OWN table, so it can spawn
  // grandchildren (the fractal) — UNLESS it sits at the depth cap, where
  // makeComposeApi hands back the inert stub and spawn_module returns -1.
  const child_compose = makeComposeApi(child);
  // A child gets its own (no-op) net/http/audio/agent so its imports link, but
  // it can't reach the network/platform from inside a panel (the parent is the
  // surface). http: get refuses, pollers report bad-handle, parse_text no-ops.
  const child_net = { open: () => -1, send: () => 0, poll: () => -1, status: () => -1, close: () => {} };
  const child_http = {
    get: () => -1, ready: () => -1, status: () => -1,
    body_len: () => -1, read_body: () => -1, parse_text: () => 0,
    body_lines: () => 0, draw_line: () => 0,
  };
  const child_audio = { tone: () => -1, tone_at: () => -1, noise: () => -1, stop: () => {}, set_volume: () => {} };
  const child_agent = {
    notify: () => 0, viewer_is_owner: () => 0, viewer_has_identity: () => 0,
    subscribe: () => 0, unsubscribe: () => 0, is_subscribed: () => 0,
    subscriber_count: () => 0, broadcast: () => 0, broadcast_compose: () => 0,
    request_identity: () => 0,
  };
  return {
    host_display: child_display,
    host_compose: child_compose,
    host_net: child_net,
    host_http: child_http,
    host_audio: child_audio,
    host_agent: child_agent,
    host_log, host_time, host_abort,
  };
}

// Instantiate the fetched bytes for a Loading child into its own instance +
// buffer. Marks the slot Ready (or Failed) and accounts its bytes against the
// total. Called from the main-thread `compose_bytes` reply.
// Mark a still-LOADING child FAILED and release its node budget (it never
// became drawable). The slot stays as a FAILED tombstone so status() → 2.
function failLoadingChild(child) {
  if (!child || child.state !== MOD_LOADING) return;
  composeTotalNodes -= 1;
  composeNodeIndex.delete(child.uid);
  child.state = MOD_FAILED;
}

// Instantiate fetched bytes INTO a specific child node (the parent-thread
// compose_bytes reply target, or a test mount). Marks it Ready (its own buffer
// at its own dims) or Failed. The byte caps (mirror ComposeBudget) are enforced
// here once the size is known; the count/depth caps were enforced at spawn.
function instantiateChild(child, wasmBuf) {
  if (!child || child.state !== MOD_LOADING) return;
  const bytes = new Uint8Array(wasmBuf);
  if (bytes.length > COMPOSE_MAX_BYTES_PER_CHILD ||
      composeTotalBytes + bytes.length > COMPOSE_MAX_TOTAL_BYTES) {
    failLoadingChild(child);
    return;
  }
  let instance;
  try {
    const mod = new WebAssembly.Module(bytes);
    instance = new WebAssembly.Instance(mod, buildChildImports(child));
  } catch (_e) {
    failLoadingChild(child);
    return;
  }
  const exp = instance.exports;
  const [dw, dh] = childDims(exp);
  // FRAMEBUFFER BUDGET (issue #78): the wasm-byte caps above don't bound the
  // surface a child allocates — childDims clamps to [FB_MIN, FB_MAX], so a tiny
  // cartridge can still ask for a 1024x1024 (4 MB) framebuffer. Cap it per-child
  // AND against the tree-wide aggregate BEFORE the allocation; over-budget →
  // failed (never drawable), so the 96 MB-from-24-tiny-nodes hole is closed.
  const fbBytes = dw * dh * 4;
  if (fbBytes > COMPOSE_MAX_FB_BYTES_PER_CHILD ||
      composeTotalFbBytes + fbBytes > COMPOSE_MAX_TOTAL_FB_BYTES) {
    failLoadingChild(child);
    return;
  }
  child.w = dw;
  child.h = dh;
  child.fb = new Uint32Array(dw * dh);
  child.memory = exp.memory || null;
  child.frame = (typeof exp.frame === 'function') ? exp.frame
    : (typeof exp.render === 'function') ? exp.render : null;
  if (!child.frame) { failLoadingChild(child); return; }
  composeTotalBytes += bytes.length;
  composeTotalFbBytes += fbBytes;
  child.bytes = bytes.length;
  child.fbBytes = fbBytes;
  child.state = MOD_READY;
}

// The compose_bytes round-trip target: resolve the LOADING node by its global
// uid (handles are per-node now, so a flat index can't address the tree) and
// instantiate it.
function composeInstantiate(uid, wasmBuf) {
  instantiateChild(composeNodeIndex.get(uid), wasmBuf);
}

// The inert compose api handed to a node AT the depth cap: the ABI still links
// so the cartridge instantiates, but spawn_module returns -1 (recursion stops).
const INERT_COMPOSE = {
  spawn_module: () => -1, status: () => -1, move_module: () => 0,
  focus_module: () => -1, focused: () => -1, close_module: () => -1,
  module_count: () => 0,
};

// Build the window-manager ABI bound to ONE node's child table. The root parent
// and every composited child get their own, so spawn/focus/close act on THAT
// node's children — recursion falls out for free. A node at the depth cap gets
// the inert stub (its children would be depth+1 over the cap). spawn_module
// posts a fetch request to the main thread (the worker can't read the chain);
// the rest mutate the node's table synchronously.
function makeComposeApi(node) {
  if (node.depth >= COMPOSE_MAX_DEPTH) return INERT_COMPOSE;
  return {
    spawn_module(namePtr, x, y, w, h) {
      const name = readStringFrom(node.memory, namePtr);
      if (name === null || name === '') return -1;
      if (liveChildCount(node.children) >= COMPOSE_MAX_CHILDREN) return -1; // per-node cap
      if (composeTotalNodes >= COMPOSE_MAX_NODES) return -1;                // global fork-bomb cap
      const uid = composeNextUid++;
      // Allocate a slot (reuse a null hole OR a FAILED tombstone, else push) so a
      // spawn-and-fail loop can't grow the table unbounded (issue #92). Slots
      // never alias; a reused tombstone's budget was already reclaimed when it died.
      let handle = reclaimableSlot(node.children);
      const child = makeChildSlot(name, x, y, w, h, node.depth + 1, uid);
      if (handle < 0) { handle = node.children.length; node.children.push(child); }
      else node.children[handle] = child;
      composeTotalNodes += 1;
      composeNodeIndex.set(uid, child);
      if (typeof self !== 'undefined' && self.postMessage) {
        self.postMessage({ type: 'compose_spawn', uid, name });
      }
      return handle;
    },
    status(handle) {
      const c = node.children[handle];
      return c ? c.state : -1;
    },
    move_module(handle, x, y, w, h) {
      const c = node.children[handle];
      if (!c) return 0;
      c.vp = { x: x | 0, y: y | 0, w: Math.max(1, w | 0), h: Math.max(1, h | 0) };
      return 1;
    },
    focus_module(handle) {
      if (handle === -1) { node.focus = -1; return 1; } // focus this node itself
      // Reject focusing an empty slot OR a FAILED tombstone (truthy) — focusing a
      // dead slot would silently sink the parent's pointer input (issue #92).
      const c = node.children[handle];
      if (!c || c.state === MOD_FAILED) return 0;
      node.focus = handle;
      return 1;
    },
    focused: () => node.focus,
    close_module(handle) {
      const c = node.children[handle];
      if (!c) return 0;
      reclaimSubtree(c);            // free its whole subtree's budget first
      node.children[handle] = null;
      if (node.focus === handle) node.focus = -1;
      return 1;
    },
    module_count: () => liveChildCount(node.children),
  };
}

// The root parent's compose api (bound to rootNode). buildImports hands this to
// the top-level cartridge.
const host_compose = makeComposeApi(rootNode);

// Reset the whole compose tree (a fresh parent load clears every level).
// MUTATE rootNode in place — never reassign it: host_compose = makeComposeApi(
// rootNode) closes over this exact object, so swapping it for a new one would
// leave the root cartridge's compose api driving an orphaned table.
function composeReset() {
  rootNode.children = [];
  rootNode.focus = -1;
  rootNode.depth = 0;
  rootNode.memory = null;
  composeTotalBytes = 0;
  composeTotalFbBytes = 0;
  composeTotalNodes = 0;
  composeNextUid = 1;
  composeNodeIndex.clear();
}

// Recursively composite a node's children INTO a destination buffer. For each
// Ready child: set its (focus-gated) pointer from this node's pointer, run its
// frame() into its own buffer, recurse so ITS children composite on top, then
// blit the child (nearest-neighbour scaled) into the destination. This is the
// fractal: the same fold runs at every level. A trapping child is reclaimed +
// latched Failed + skipped — it never takes down a parent or sibling. The array
// under iteration (node.children) is only ever mutated by node's OWN frame(),
// which already ran one level up — so no re-entrant mutation here.
function compositeChildren(node, dstFb, dstW, dstH, parentPtr, t) {
  const children = node.children;
  const focus = node.focus;
  for (let i = 0; i < children.length; i++) {
    const c = children[i];
    if (!c || c.state !== MOD_READY) continue;
    if (i === focus) {
      const mapped = mapPointerIntoChild(parentPtr.x, parentPtr.y, c.vp.x, c.vp.y, c.vp.w, c.vp.h, c.w, c.h);
      if (mapped) { c.ptr.x = mapped[0]; c.ptr.y = mapped[1]; c.ptr.down = parentPtr.down; }
      else { c.ptr.x = -1; c.ptr.y = -1; c.ptr.down = 0; }
    } else {
      c.ptr.x = -1; c.ptr.y = -1; c.ptr.down = 0;
    }
    try {
      c.frame(t);
    } catch (_e) {
      reclaimSubtree(c);
      c.state = MOD_FAILED; // tombstone (status → 2); skip — never propagates up
      continue;
    }
    // Recurse: this child's own children draw on top of what it just drew.
    if (c.children.length) compositeChildren(c, c.fb, c.w, c.h, c.ptr, t);
    blitChild(dstFb, dstW, dstH, c.fb, c.w, c.h, c.vp.x, c.vp.y, c.vp.w, c.vp.h);
  }
}

// Composite the whole tree into the root framebuffer. No-op (byte-identical to a
// non-compose cartridge) when the root never spawned a child. Called from the
// parent's tick() after its frame() draws, before present().
function composeCompositePass(t) {
  if (!rootNode.children.length) return;
  rootNode.memory = memory; // root spawn reads names from the live root memory
  compositeChildren(rootNode, fb32, FB_W, FB_H, ptr, t);
}

// ---- ?compose= : a ROOTLESS composition of named modules (issue #77) ---------
// The `?compose=name1,name2,…` path tiles several published cartridges in a grid
// with NO parent cartridge driving them. It previously ran each child's frame()
// on the MAIN thread (display.rs::start_compose_loop) — UNTRUSTED wasm with no
// worker isolation or watchdog, so one hung child re-bricked the tab. Now it runs
// HERE, in the worker, reusing the exact compose tree + budget caps + the main-
// thread watchdog the single-cartridge path already has.
//
// There is no root cartridge: the SYNTHETIC root is `rootNode` itself, and
// `composeTick` is its frame loop — it composites the grid children into the
// shared framebuffer and presents once per frame, focus-gating the pointer to the
// topmost child that contains it (the `focus_at` rule, mirroring the old main-
// thread loop). Children are mounted as LOADING slots whose bytes arrive via the
// SAME compose_bytes round-trip as a recursive spawn (the main thread resolves
// each name's on-chain app.wasm).

// The topmost ready child whose viewport contains (px, py), or -1. Last index =
// topmost (z-order), matching ModuleTable::focus_at on the Rust side.
function composeFocusAt(node, px, py) {
  const children = node.children;
  for (let i = children.length - 1; i >= 0; i--) {
    const c = children[i];
    if (!c || c.state !== MOD_READY) continue;
    if (px >= c.vp.x && py >= c.vp.y && px < c.vp.x + c.vp.w && py < c.vp.y + c.vp.h) {
      return i;
    }
  }
  return -1;
}

// The synthetic-root frame loop for a ?compose= composition. Clears the root FB
// (inter-cell gaps stay black), focus-gates the pointer to the child under it,
// composites every child, and presents — then self-paces like `tick`. The main-
// thread watchdog terminates this worker if a child hangs and stops the frames.
function composeTick() {
  if (!running) return;
  const t = (Date.now() - startMs) | 0;
  try {
    // Opaque-black the root framebuffer each frame (gaps between grid cells).
    fb32.fill(packRgb(0x000000));
    // Route the pointer to whichever child sits under it (topmost wins), so a
    // click in one panel can't drive a sibling — same gate as the in-thread loop.
    rootNode.focus = composeFocusAt(rootNode, ptr.x, ptr.y);
    compositeChildren(rootNode, fb32, FB_W, FB_H, ptr, t);
    present();
  } catch (e) {
    running = false;
    postError(LH_RUNTIME.WASM_TRAP, 'compose pass failed: ' + (e && e.message ? e.message : String(e)));
    return;
  }
  if (running) setTimeout(composeTick, 16);
}

// Start a rootless grid composition of `slots` ({ name, x, y, w, h }, … already
// laid out by the main thread's grid_viewports). Resets the tree, mounts each
// slot as a LOADING child of the synthetic root (budget-capped), kicks the bytes
// round-trip for each, and starts `composeTick`. The main thread blits frames +
// forwards input + arms the watchdog exactly as for a single cartridge.
function composeLoad(slots) {
  running = false;
  closeAllSockets();
  clearAllHttp();
  composeReset();
  state.fill(0);
  ptr.x = -1; ptr.y = -1; ptr.down = 0; // no pointer until the viewer moves it
  memory = null;
  // The composition uses the default 320x240 surface (the grid viewports the
  // main thread sent are computed against it). A child still declares its OWN
  // dims() and is scaled into its cell by blitChild.
  FB_W = FB_W_DEFAULT;
  FB_H = FB_H_DEFAULT;
  fbBytes = new Uint8ClampedArray(FB_W * FB_H * 4);
  fb32 = new Uint32Array(fbBytes.buffer);

  for (const slot of (Array.isArray(slots) ? slots : [])) {
    if (!slot || typeof slot.name !== 'string' || slot.name === '') continue;
    if (liveChildCount(rootNode.children) >= COMPOSE_MAX_CHILDREN) break;
    if (composeTotalNodes >= COMPOSE_MAX_NODES) break;
    const uid = composeNextUid++;
    const child = makeChildSlot(slot.name, slot.x, slot.y, slot.w, slot.h, rootNode.depth + 1, uid);
    rootNode.children.push(child);
    composeTotalNodes += 1;
    composeNodeIndex.set(uid, child);
    // Ask the main thread to resolve this name's published app.wasm; the
    // compose_bytes reply instantiates the slot (or marks it Failed).
    if (typeof self !== 'undefined' && self.postMessage) {
      self.postMessage({ type: 'compose_spawn', uid, name: slot.name });
    }
  }

  if (!rootNode.children.length) {
    postError(LH_RUNTIME.NO_ENTRY, 'compose: no module to composite');
    return;
  }
  running = true;
  startMs = Date.now();
  composeTick();
}

// ---- host_net: WebSocket (works in a worker) --------------------------------
// The cartridge host's network surface (the in-thread Rust mirror was removed
// with the in-thread cartridge runtime — issue #77; the native rustlite loader
// keeps its own copy at src/rustlite/loader.rs). Poll-model sockets, SSRF
// wss-only gate, MAX_SOCKETS / MAX_INBOX caps, length-prefixed strings over
// cartridge memory.
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

// Read a length-prefixed UTF-8 string out of an ARBITRARY linear memory. The
// compose tree needs this: a child's spawn_module passes a pointer into ITS OWN
// memory, not the root parent's, so makeComposeApi reads from node.memory.
function readStringFrom(mem, p) {
  if (p < 0 || !mem) return null;
  const a = new Uint8Array(mem.buffer);
  const cap = a.length;
  if (p + 4 > cap) return null;
  const len = a[p] | (a[p + 1] << 8) | (a[p + 2] << 16) | (a[p + 3] << 24);
  if (len < 0 || len > 65536 || p + 4 + len > cap) return null;
  return new TextDecoder().decode(a.subarray(p + 4, p + 4 + len));
}

function readString(p) {
  if (memory === null) return null;
  return readStringFrom(memory, p);
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

// ---- host_http: one-shot HTTP GET via the proxy + HTML→text (issue #19) ------
// A cartridge can't fetch arbitrary origins itself (no DOM; CORS; and the worker
// has no auth signer to mint the proxy token). So host_http mirrors the
// host::compose spawn round-trip EXACTLY: `get(url)` allocates a PENDING handle,
// posts an `http_fetch { id, url }` to the MAIN thread, and returns the handle.
// The main thread does the SAME authed `/api/fetch` proxy POST the agent
// web_fetch tool uses (signed token, https-only, private hosts denied, 200KB
// cap) and posts an `http_result { id, status, body }` (or `{ id, error: true }`)
// back; `applyHttpResult` marks the handle READY/ERROR. The cartridge POLLS
// `ready(handle)` each frame (poll-model, like host_net's drain) and then
// `read_body` the length-prefixed body out of its memory. `parse_text` is PURE
// (no network) — strip tags + decode entities, in-worker.
const MAX_HTTP_REQUESTS = 8; // live request cap (mirror MAX_SOCKETS; flood guard)
const HTTP_PENDING = 0;
const HTTP_READY = 1;
const HTTP_ERROR = 2;
const httpReqs = []; // index = handle; { state, status, body }. Closed slots → null.
let httpNextId = 1;  // global id stamped on the fetch round-trip (the reply key)
const httpIdToHandle = new Map(); // id -> handle (resolve a reply to its slot)

// Decode the handful of HTML entities that actually matter for readable text.
// Numeric (&#NN; / &#xHH;) + the 5 named XML ones + a few common HTML ones.
function decodeEntities(s) {
  return s.replace(/&(#x?[0-9a-fA-F]+|[a-zA-Z]+);/g, (m, body) => {
    if (body[0] === '#') {
      const code = body[1] === 'x' || body[1] === 'X'
        ? parseInt(body.slice(2), 16)
        : parseInt(body.slice(1), 10);
      if (Number.isFinite(code) && code > 0 && code <= 0x10ffff) {
        try { return String.fromCodePoint(code); } catch (_e) { return m; }
      }
      return m;
    }
    switch (body.toLowerCase()) {
      case 'amp': return '&';
      case 'lt': return '<';
      case 'gt': return '>';
      case 'quot': return '"';
      case 'apos': return "'";
      case 'nbsp': return ' ';
      case 'copy': return '©';
      case 'reg': return '®';
      case 'mdash': return '—';
      case 'ndash': return '–';
      case 'hellip': return '…';
      default: return m;
    }
  });
}

// Minimal HTML → plain text: drop <script>/<style> contents, treat block-level
// closers as newlines, strip the rest of the tags, decode entities, and collapse
// runs of whitespace. Not a full parser — a lightweight readability pass for an
// in-sandbox document reader (the SAME altitude as the proxy's textual handling).
function htmlToText(html) {
  let s = String(html);
  // Remove script/style blocks entirely (their text is never readable content).
  s = s.replace(/<script[\s\S]*?<\/script>/gi, ' ');
  s = s.replace(/<style[\s\S]*?<\/style>/gi, ' ');
  s = s.replace(/<!--[\s\S]*?-->/g, ' ');
  // Block-level boundaries → newlines BEFORE stripping tags, so structure shows.
  s = s.replace(/<\/(p|div|h[1-6]|li|tr|section|article|header|footer|blockquote)>/gi, '\n');
  s = s.replace(/<br\s*\/?>/gi, '\n');
  s = s.replace(/<\/?(ul|ol|table|thead|tbody)[^>]*>/gi, '\n');
  // Strip every remaining tag.
  s = s.replace(/<[^>]*>/g, '');
  s = decodeEntities(s);
  // Collapse whitespace: trim each line, drop blank-line runs to a single \n.
  s = s.replace(/[ \t\f\v]+/g, ' ');
  s = s.replace(/ *\n */g, '\n');
  s = s.replace(/\n{3,}/g, '\n\n');
  return s.trim();
}

const host_http = {
  get(urlPtr, _urlLen) {
    const url = readString(urlPtr);
    if (url === null || url === '') return -1;
    // The proxy enforces the real SSRF/scheme policy (https-only, private hosts
    // denied) AND it's authed+metered there — but cheaply reject the obvious
    // non-https here so a bad URL never burns the round-trip.
    if (!/^https:\/\//i.test(url)) return -1;
    const live = httpReqs.filter((r) => r !== null && r.state === HTTP_PENDING).length;
    if (live >= MAX_HTTP_REQUESTS) return -1;
    const id = httpNextId++;
    const req = { id, state: HTTP_PENDING, status: 0, body: '' };
    let handle = httpReqs.indexOf(null);
    if (handle < 0) { handle = httpReqs.length; httpReqs.push(req); }
    else httpReqs[handle] = req;
    httpIdToHandle.set(id, handle);
    if (typeof self !== 'undefined' && self.postMessage) {
      self.postMessage({ type: 'http_fetch', id, url });
    }
    return handle;
  },
  ready(handle) {
    const r = httpReqs[handle];
    if (!r) return -1;
    if (r.state === HTTP_ERROR) return -2;
    return r.state === HTTP_READY ? 1 : 0;
  },
  status(handle) {
    const r = httpReqs[handle];
    if (!r) return -1;
    return r.state === HTTP_READY ? (r.status | 0) : 0;
  },
  body_len(handle) {
    const r = httpReqs[handle];
    if (!r) return -1;
    if (r.state !== HTTP_READY) return 0;
    return new TextEncoder().encode(r.body).length;
  },
  read_body(handle, outPtr, max) {
    const r = httpReqs[handle];
    if (!r) return -1;
    if (r.state !== HTTP_READY) return 0;
    return writeString(outPtr, r.body, Math.max(0, max));
  },
  parse_text(htmlPtr, _htmlLen, outPtr, max) {
    const html = readString(htmlPtr);
    if (html === null) return -1;
    return writeString(outPtr, htmlToText(html), Math.max(0, max));
  },
  // body_lines / draw_line: render the HOST-HELD fetched body as text WITHOUT the
  // cartridge needing a writable buffer (rustlite can only produce a string-LITERAL
  // pointer, so read_body's out_ptr is unusable from rustlite — this is how a
  // data-driven cartridge shows live fetched text). Lines are '\n'-delimited.
  body_lines(handle) {
    const r = httpReqs[handle];
    if (!r || r.state !== HTTP_READY) return 0;
    const b = r.body.replace(/\n+$/, '');
    return b === '' ? 0 : b.split('\n').length;
  },
  draw_line(handle, line, x, y, rgb, scale) {
    const r = httpReqs[handle];
    if (!r || r.state !== HTTP_READY) return 0;
    const lines = r.body.replace(/\n+$/, '').split('\n');
    if (line < 0 || line >= lines.length) return 0;
    const s = lines[line];
    const sc = Math.max(1, scale | 0);
    const packed = packRgb(rgb);
    for (let i = 0; i < s.length; i++) {
      blitGlyph((x | 0) + i * 6 * sc, y | 0, s.charCodeAt(i), packed, sc);
    }
    return s.length;
  },
};

// The main thread resolved an `http_fetch` (the authed /api/fetch proxy POST).
// Mark the matching handle READY (with the upstream status + body) or ERROR.
// Keyed by the global id (a handle slot may have been reused after close).
function applyHttpResult(msg) {
  const handle = httpIdToHandle.get(msg.id | 0);
  if (handle === undefined) return;
  httpIdToHandle.delete(msg.id | 0);
  const r = httpReqs[handle];
  if (!r || r.id !== (msg.id | 0)) return; // slot reused — drop the stale reply
  if (msg.error) {
    r.state = HTTP_ERROR;
    return;
  }
  r.status = msg.status | 0;
  r.body = typeof msg.body === 'string' ? msg.body : '';
  r.state = HTTP_READY;
}

// Drop every in-flight/finished request (a fresh cartridge load resets them).
function clearAllHttp() {
  httpReqs.length = 0;
  httpIdToHandle.clear();
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

// ---- host_agent: the cartridge<->platform bridge (feedback #66/#103) ---------
// notify forwards to the main thread (which owns Notification + the permission
// state); viewer context is passed in at load time (the worker can't read the
// wallet/owner state). notify is RATE-LIMITED here (1/3s) as the first line of
// defense; the main thread re-gates on permission and never prompts. Reaching
// OTHER users' devices (P2P subscriber push) is the deliberate follow-up — this
// only ever notifies the current viewer.
let viewerIsOwner = 0;
let viewerHasIdentity = 0;
let feedIsSubscribed = 0;   // cached at load + refreshed after subscribe/unsubscribe
let feedSubscriberCount = 0;
let lastAgentNotify = 0;
let lastAgentBroadcast = 0;
const AGENT_NOTIFY_MIN_MS = 3000;
const AGENT_BROADCAST_MIN_MS = 3000;
const host_agent = {
  notify(titlePtr, bodyPtr) {
    const now = Date.now();
    if (now - lastAgentNotify < AGENT_NOTIFY_MIN_MS) return 0; // rate-limited
    const title = (readString(titlePtr) || '').slice(0, 80);
    const body = (readString(bodyPtr) || '').slice(0, 200);
    if (!title) return 0;
    lastAgentNotify = now;
    self.postMessage({ type: 'agent_notify', title, body });
    return 1;
  },
  viewer_is_owner: () => viewerIsOwner,
  viewer_has_identity: () => viewerHasIdentity,
  // --- subscriber feed (fire-and-forget writes; cached reads) ---------------
  subscribe() {
    if (!viewerHasIdentity) return 0;
    if (!feedIsSubscribed) feedSubscriberCount += 1; // optimistic; main re-reads + corrects
    feedIsSubscribed = 1;
    self.postMessage({ type: 'agent_subscribe' });
    return 1;
  },
  unsubscribe() {
    if (!viewerHasIdentity) return 0;
    if (feedIsSubscribed && feedSubscriberCount > 0) feedSubscriberCount -= 1; // optimistic
    feedIsSubscribed = 0;
    self.postMessage({ type: 'agent_unsubscribe' });
    return 1;
  },
  is_subscribed: () => feedIsSubscribed,
  subscriber_count: () => feedSubscriberCount,
  broadcast(titlePtr, bodyPtr) {
    if (!viewerHasIdentity) return 0;
    const now = Date.now();
    if (now - lastAgentBroadcast < AGENT_BROADCAST_MIN_MS) return 0;
    const title = (readString(titlePtr) || '').slice(0, 80);
    const body = (readString(bodyPtr) || '').slice(0, 200);
    if (!title) return 0;
    lastAgentBroadcast = now;
    self.postMessage({ type: 'agent_broadcast', title, body });
    return 1;
  },
  broadcast_compose(titlePtr, bodyPtr) {
    // `broadcast`, but the MAIN thread opens a text input over the canvas
    // first (a cartridge is pixels-only — it can't summon a keyboard). The
    // typed body is broadcast from the composer's [send]; rate-limiting the
    // actual send is the human typing + tapping, so opening isn't limited.
    if (!viewerHasIdentity) return 0;
    const title = (readString(titlePtr) || '').slice(0, 80);
    const body = (readString(bodyPtr) || '').slice(0, 200);
    if (!title) return 0;
    self.postMessage({ type: 'agent_broadcast_compose', title, body });
    return 1;
  },
  request_identity() {
    if (viewerHasIdentity) return 1;
    self.postMessage({ type: 'agent_request_identity' });
    return 0; // not yet — main creates it async; viewer_has_identity flips on the next refresh
  },
};

// Main-thread updates the cached feed/context state (after a subscribe tx, an
// identity creation, etc.) so the sync getters reflect reality next frame.
function applyAgentContext(msg) {
  if (typeof msg.viewerIsOwner === 'number') viewerIsOwner = msg.viewerIsOwner | 0;
  if (typeof msg.viewerHasIdentity === 'number') viewerHasIdentity = msg.viewerHasIdentity | 0;
  if (typeof msg.feedIsSubscribed === 'number') feedIsSubscribed = msg.feedIsSubscribed | 0;
  if (typeof msg.feedSubscriberCount === 'number') feedSubscriberCount = msg.feedSubscriberCount | 0;
}

// ---- host_mp: browser-to-browser MULTIPLAYER over a WebRTC data channel -------
// The cartridge calls host::mp::*; the Peer + relay live on MAIN (display.rs +
// webrtc.rs). This worker side: open()/join() ask MAIN to connect; set()/send()
// BUFFER outgoing → flushed once per frame as {mp:deltas}/{mp:events}; get()/event_*
// read a LOCALLY-MIRRORED table MAIN fills via {mp:status}/{mp:peer}. So all host
// calls are synchronous. 2-peer v1. PARITY with src/rustlite/loader.rs host_mp.
const MP_SLOTS = 32;
const MP_PEERS = 8;
let mpConnected = 0;
let mpSelfIndex = -1;
let mpPeerCount = 0;
const mpState = [];
for (let mpi = 0; mpi < MP_PEERS; mpi++) mpState.push(new Int32Array(MP_SLOTS));
let mpDirty = [];      // buffered outgoing [slot, value, ...] this frame
let mpOutEvents = [];  // buffered outgoing event values this frame
let mpInEvents = [];   // received events (FIFO)
const MP_MAX_EVENTS = 256;

const host_mp = {
  open() {
    // Pick a 4-digit room code, ask MAIN to host it, return the code to display.
    const code = (1000 + Math.floor(Math.random() * 9000)) | 0;
    self.postMessage({ type: 'mp:host', room: code });
    return code;
  },
  join(code) {
    self.postMessage({ type: 'mp:join', room: code | 0 });
  },
  auto(code) {
    // Join a SHARED room with no host/join choice: MAIN elects the host (the
    // first peer in the room roster) and connects everyone else to it.
    self.postMessage({ type: 'mp:auto', room: code | 0 });
  },
  connected: () => mpConnected,
  self_index: () => mpSelfIndex,
  peer_count: () => mpPeerCount,
  set(slot, value) {
    slot = slot | 0; value = value | 0;
    if (slot < 0 || slot >= MP_SLOTS) return;
    if (mpSelfIndex >= 0 && mpSelfIndex < MP_PEERS) mpState[mpSelfIndex][slot] = value; // mirror my own
    mpDirty.push(slot, value);
  },
  get(peer, slot) {
    peer = peer | 0; slot = slot | 0;
    if (peer < 0 || peer >= MP_PEERS || slot < 0 || slot >= MP_SLOTS) return 0;
    return mpState[peer][slot];
  },
  send(value) {
    if (mpOutEvents.length < MP_MAX_EVENTS) mpOutEvents.push(value | 0);
  },
  event_count: () => mpInEvents.length,
  event_next: () => (mpInEvents.length ? mpInEvents.shift() : 0),
};

// Flush buffered set()/send() to MAIN once per frame (called from tick()).
function flushMp() {
  if (mpDirty.length) { self.postMessage({ type: 'mp:deltas', deltas: mpDirty }); mpDirty = []; }
  if (mpOutEvents.length) { self.postMessage({ type: 'mp:events', events: mpOutEvents }); mpOutEvents = []; }
}
// MAIN → worker: connection status + incoming peer state/events → the mirror.
function applyMpStatus(msg) {
  mpConnected = msg.connected | 0;
  if (typeof msg.selfIndex === 'number') mpSelfIndex = msg.selfIndex | 0;
  mpPeerCount = msg.peerCount | 0;
}
function applyMpPeer(msg) {
  const peer = msg.peer | 0;
  if (peer < 0 || peer >= MP_PEERS) return;
  const d = msg.deltas || [];
  for (let i = 0; i + 1 < d.length; i += 2) {
    const slot = d[i] | 0;
    if (slot >= 0 && slot < MP_SLOTS) mpState[peer][slot] = d[i + 1] | 0;
  }
  const ev = msg.events || [];
  for (let i = 0; i < ev.length; i++) if (mpInEvents.length < MP_MAX_EVENTS) mpInEvents.push(ev[i] | 0);
}
function resetMp() {
  mpConnected = 0; mpSelfIndex = -1; mpPeerCount = 0;
  for (let i = 0; i < MP_PEERS; i++) mpState[i].fill(0);
  mpDirty = []; mpOutEvents = []; mpInEvents = [];
  if (typeof self !== 'undefined' && self.postMessage) self.postMessage({ type: 'mp:leave' });
}

// ---- host_chat: open CHATROOM (per-subdomain off-chain message log) -----------
// rustlite has no String/Vec + read-only arrays, so ALL chat text lives HERE (the
// worker): chatLines = the received-line ring, chatCompose = the outgoing buffer.
// The cartridge reads/writes them purely as INTEGER codepoint calls (integer-only
// host ABI). The relay (/api/chat) + personal-sign auth live on MAIN (display.rs):
// first poll() posts {chat:start} so MAIN begins polling; received lines arrive as
// {chat:msg}; send() posts {chat:send}. PARITY with src/rustlite/loader.rs host_chat.
let chatLines = []; // received "name: text" lines, oldest first
let chatCompose = ''; // the line the user is typing
let chatStarted = 0;
let chatRecentSent = []; // normalized text of messages WE just sent (echo dedup)
const CHAT_MAX_LINES = 64; // ring cap
const CHAT_MAX_COMPOSE = 200;
const CHAT_DEDUP_RING = 8; // how many recent sends to dedup against
function chatStart() {
  if (!chatStarted) { chatStarted = 1; self.postMessage({ type: 'chat:start' }); }
}
// Collapse whitespace + lowercase so an optimistic echo matches the relay's
// echoed copy regardless of case/spacing (the relay does .split(/\s+/).join(' ')).
function chatNorm(s) { return String(s).trim().replace(/\s+/g, ' ').toLowerCase(); }
function chatPushLine(line) {
  chatLines.push(line);
  if (chatLines.length > CHAT_MAX_LINES) chatLines = chatLines.slice(-CHAT_MAX_LINES);
}
const host_chat = {
  poll() { chatStart(); return chatLines.length; }, // first poll kicks MAIN's relay loop
  line_count() { return chatLines.length; },
  line_len(i) { const s = chatLines[i | 0]; return typeof s === 'string' ? s.length : 0; },
  line_char(i, p) {
    const s = chatLines[i | 0];
    if (typeof s !== 'string' || (p | 0) < 0 || (p | 0) >= s.length) return -1;
    return s.charCodeAt(p | 0) | 0;
  },
  key(cp) {
    if (chatCompose.length < CHAT_MAX_COMPOSE && (cp | 0) > 0) chatCompose += String.fromCharCode(cp | 0);
  },
  backspace() { chatCompose = chatCompose.slice(0, -1); },
  compose_len() { return chatCompose.length; },
  compose_char(p) {
    if ((p | 0) < 0 || (p | 0) >= chatCompose.length) return -1;
    return chatCompose.charCodeAt(p | 0) | 0;
  },
  send() {
    const text = chatCompose.trim();
    if (text.length === 0) return 0;
    chatStart();
    // OPTIMISTIC ECHO: show our own line INSTANTLY (next frame) instead of waiting
    // a full POST + poll round-trip (~2-5s). Remember its normalized text so the
    // relay's echoed copy is dedup'd when it polls back, not shown twice.
    chatPushLine('you: ' + text);
    chatRecentSent.push(chatNorm(text));
    if (chatRecentSent.length > CHAT_DEDUP_RING) chatRecentSent = chatRecentSent.slice(-CHAT_DEDUP_RING);
    self.postMessage({ type: 'chat:send', text });
    chatCompose = '';
    return 1;
  },
};
function applyChatMsg(msg) {
  const line = typeof msg.text === 'string' ? msg.text : '';
  if (!line) return;
  // Dedup our own echo: a relay line is "name: text"; if its text matches one we
  // just sent, consume that ring entry and skip (we already showed it as "you: …").
  const idx = line.indexOf(': ');
  const body = idx >= 0 ? line.slice(idx + 2) : line;
  const hit = chatRecentSent.indexOf(chatNorm(body));
  if (hit >= 0) {
    chatRecentSent.splice(hit, 1);
    return;
  }
  chatPushLine(line);
}
function resetChat() {
  chatLines = [];
  chatCompose = '';
  chatStarted = 0;
  chatRecentSent = [];
}

function buildImports() {
  return { host_display, host_net, host_http, host_audio, host_log, host_time, host_abort, host_agent, host_compose, host_mp, host_chat };
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
    // host::compose: composite every live child into the parent FB after the
    // parent's frame() draws and before present(), so the canvas only ever shows
    // a fully-composited frame. No-op when the parent never spawned a child
    // (composeChildren empty) — so a non-compose cartridge is byte-identical.
    composeCompositePass(t);
    // host::mp: flush this frame's buffered set()/send() to MAIN (→ data channel).
    // No-op when the cartridge never touched host_mp (mpDirty/mpOutEvents empty).
    flushMp();
    present();
  } catch (e) {
    // ANY failure in the frame body — a wasm trap (unreachable / OOB), a compose
    // pass error, or a present()/postMessage throw — ends the run cleanly with a
    // CODED error. Critically, present() + the compose pass are INSIDE this guard:
    // a throw there used to escape as an unhandled rejection, leaving the worker
    // SILENT (no frame, no error) so the main-thread watchdog mis-reported a live
    // cartridge as a hang (LH1001). Now it surfaces the real reason.
    running = false;
    postError(LH_RUNTIME.WASM_TRAP, 'cartridge frame failed: ' + (e && e.message ? e.message : String(e)));
    return;
  }
  // Worker rAF is available in modern browsers, but to keep this dependency-free
  // and predictable we self-pace with a ~16ms timer. The main thread's watchdog
  // (not this cadence) is what detects a hung frame.
  if (running) setTimeout(tick, 16);
}

async function load(wasmBuf) {
  running = false;
  closeAllSockets();
  clearAllHttp();
  composeReset(); // a fresh parent clears the whole compose graph
  resetMp(); // tear down any multiplayer session + clear the mirror
  resetChat(); // clear the chatroom inbox + poll-start flag
  state.fill(0);
  ptr.x = 0; ptr.y = 0; ptr.down = 0;
  memory = null;
  // Reset to default dims; the real size is decided AFTER instantiate (a
  // cartridge's `dims()` export needs a live instance to call). applyDims()
  // below reallocates the framebuffer once the instance exists.
  FB_W = FB_W_DEFAULT;
  FB_H = FB_H_DEFAULT;
  fbBytes = new Uint8ClampedArray(FB_W * FB_H * 4);
  fb32 = new Uint32Array(fbBytes.buffer);

  let instance;
  try {
    const module = await WebAssembly.compile(wasmBuf);
    // If the cartridge imports the host::agent feed surface (subscribe / broadcast
    // / notify / …), tell the main thread so it can PRIME notification permission
    // on the next canvas tap — the only main-thread USER GESTURE in the cartridge
    // flow. The worker postMessage that carries subscribe() can't prompt (lost
    // activation); the canvas tap that PRODUCED it can.
    if (
      IS_WORKER &&
      WebAssembly.Module.imports(module).some((i) => i.module === 'host_agent')
    ) {
      self.postMessage({ type: 'cartridge_uses_feed' });
    }
    instance = await WebAssembly.instantiate(module, buildImports());
  } catch (e) {
    postError(LH_RUNTIME.INSTANTIATE_FAILED, 'instantiate failed: ' + (e && e.message ? e.message : String(e)));
    return;
  }
  const exp = instance.exports;
  memory = exp.memory || null;
  rootNode.memory = memory; // a root spawn_module on the first frame reads names from here
  // A cartridge MAY declare its own framebuffer dims via `dims() -> i32`
  // (packed (w<<16)|h). No export => the 320x240 default. Reallocates the FB.
  // Guarded: a throwing dims()/alloc must surface a CODED error, not an unhandled
  // rejection that leaves the worker silent (→ a false watchdog "hung" LH1001).
  try {
    applyDims(exp);
  } catch (e) {
    postError(LH_RUNTIME.WASM_TRAP, 'dims() failed: ' + (e && e.message ? e.message : String(e)));
    return;
  }

  if (typeof exp.frame === 'function') {
    frameFn = exp.frame;
    isAnimated = true;
  } else if (typeof exp.render === 'function') {
    frameFn = exp.render;
    isAnimated = false;
  } else {
    postError(LH_RUNTIME.NO_ENTRY, 'cartridge exports neither frame nor render');
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
      postError(LH_RUNTIME.WASM_TRAP, 'cartridge trapped: ' + (e && e.message ? e.message : String(e)));
      running = false;
      return;
    }
    composeCompositePass(0); // composite any children a one-shot parent mounted
    present();
    running = false;
    // Tell the main thread this was a ONE-SHOT render that completed: it posted
    // exactly one frame and will never post another, so the watchdog must stand
    // down (else it fires ~1.5s later and falsely paints "CARTRIDGE STOPPED
    // LH1001" over a good static render). Animated cartridges never send `done`.
    self.postMessage({ type: 'done' });
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
        applyAgentContext(msg);
        load(msg.wasm);
        break;
      case 'compose_load':
        // ?compose= : a rootless grid composition of named modules, run HERE so
        // it gets the same worker isolation + watchdog as a single cartridge
        // (issue #77). `slots` carries the grid viewports the main thread laid
        // out; each child's bytes arrive via the compose_bytes round-trip.
        applyAgentContext(msg);
        composeLoad(msg.slots);
        break;
      case 'agent_context':
        applyAgentContext(msg);
        break;
      case 'compose_bytes':
        // Main thread resolved a child's on-chain app.wasm (or signalled a
        // failure with wasm=null). The child is addressed by its global uid
        // (handles are per-node now). Instantiate it, or mark the slot Failed.
        if (msg.wasm) {
          composeInstantiate(msg.uid | 0, msg.wasm);
        } else {
          failLoadingChild(composeNodeIndex.get(msg.uid | 0));
        }
        break;
      case 'http_result':
        // Main thread resolved a `http_fetch` (the authed /api/fetch proxy
        // POST). Mark the matching handle READY (status + body) or ERROR; the
        // cartridge sees it on its next ready()/read_body() poll.
        applyHttpResult(msg);
        break;
      case 'input':
        ptr.x = msg.x | 0;
        ptr.y = msg.y | 0;
        ptr.down = msg.down | 0;
        break;
      case 'mp:status':
        // MAIN: connection state changed (connected/selfIndex/peerCount).
        applyMpStatus(msg);
        break;
      case 'mp:peer':
        // MAIN: a peer's state deltas / events arrived over the data channel.
        applyMpPeer(msg);
        break;
      case 'chat:msg':
        // MAIN: a new chatroom line arrived from the relay poll.
        applyChatMsg(msg);
        break;
      case 'stop':
        running = false;
        closeAllSockets();
        clearAllHttp();
        break;
      default:
        break;
    }
  };
}

// Node-only test surface (the host-parity harness). NOT used by the worker.
if (typeof module !== 'undefined' && module.exports) {
  module.exports = {
    // The DEFAULT dims (320x240). The live FB_W/FB_H are `let`-mutable and a
    // cartridge's dims() can change them at load; expose both so a test can
    // assert the default AND read the live size after renderOnce.
    FB_W: FB_W_DEFAULT,
    FB_H: FB_H_DEFAULT,
    FB_MIN,
    FB_MAX,
    decodeDims,
    // Live framebuffer dims (reflect the last renderOnce's cartridge dims()).
    liveDims: () => [FB_W, FB_H],
    host_display, // the re-implemented draw ABI under test
    host_agent,
    host_http, // the HTTP poll-model ABI (get/ready/status/body_len/read_body/parse_text)
    htmlToText, // the pure HTML→text pass (exercised directly by tests)
    decodeEntities,
    applyHttpResult, // drive an http_fetch reply in a test (no main thread)
    host_compose, // the window-manager ABI (spawn/status/focus/close/...)
    // The JS mirrors of compose.rs (under parity test vs the Rust impls).
    blitChild,
    mapPointerIntoChild,
    composeReset,
    composeChildren: () => rootNode.children,
    composeFocus: () => rootNode.focus,
    COMPOSE_MAX_DEPTH,
    COMPOSE_MAX_FB_BYTES_PER_CHILD,
    COMPOSE_MAX_TOTAL_FB_BYTES,
    composeTotalFbBytes: () => composeTotalFbBytes,
    // Mount a LOADING child slot into a node's table directly (test shim —
    // drives the real tree + budget caps, avoiding spawn_module's readString/
    // postMessage path, which needs a live worker + the parent's linear memory).
    // `parent` is null/undefined for a root child, or a child node object for a
    // grandchild (recursion). The caller then feeds bytes via
    // composeInstantiateForTest and ticks via composeRunPass. Returns the handle
    // (index into the parent's children), or -1 when a cap is hit.
    composeMountInto(parent, name, x, y, w, h) {
      const node = parent || rootNode;
      if (node.depth >= COMPOSE_MAX_DEPTH) return -1;          // depth cap
      if (liveChildCount(node.children) >= COMPOSE_MAX_CHILDREN) return -1; // per-node cap
      if (composeTotalNodes >= COMPOSE_MAX_NODES) return -1;   // global cap
      const uid = composeNextUid++;
      let handle = reclaimableSlot(node.children); // reuse null hole OR FAILED tombstone (#92)
      const child = makeChildSlot(name, x, y, w, h, node.depth + 1, uid);
      if (handle < 0) { handle = node.children.length; node.children.push(child); }
      else node.children[handle] = child;
      composeTotalNodes += 1;
      composeNodeIndex.set(uid, child);
      return handle;
    },
    composeMountForTest(name, x, y, w, h) {
      return module.exports.composeMountInto(null, name, x, y, w, h);
    },
    // Instantiate fetched bytes into the child at `handle` of `parent`'s table
    // (root if parent is null) — the test's stand-in for the compose_bytes reply.
    composeInstantiateForTest(handle, wasmBuf, parent) {
      const node = parent || rootNode;
      instantiateChild(node.children[handle], wasmBuf);
    },
    composeFocusForTest: (h) => host_compose.focus_module(h),
    // Run the composite pass into a caller-supplied parent FB at the given
    // pointer. Sets the live FB_W/FB_H + fb32 + ptr to the parent's, ticks every
    // Ready child, blits each into the parent FB, and returns it.
    composeRunPass(parentFb, parentW, parentH, t, pointer) {
      FB_W = parentW; FB_H = parentH;
      fb32 = parentFb;
      ptr.x = pointer ? pointer.x : 0;
      ptr.y = pointer ? pointer.y : 0;
      ptr.down = pointer ? pointer.down : 0;
      composeCompositePass(t | 0);
      return parentFb;
    },
    glyph5x7, // expose the font table for a byte-for-byte vs-Rust check
    packRgb,
    LH_RUNTIME, // the LH1xxx runtime codes the worker reports (headless check)
    lhLabel,
    // Hand-instantiate a cartridge against this host (no Worker, no postMessage)
    // and return the presented framebuffer as a fresh Uint8ClampedArray. Drives
    // the present-after-frame model: frame(t) draws, then we snapshot.
    renderOnce(wasmBytes, t = 0) {
      // Start from the default-size framebuffer; if the cartridge exports
      // dims(), applyDims() (called after instantiate, like the worker) resizes
      // it before frame() draws — so the snapshot reflects the cartridge's
      // chosen resolution. liveDims() reports the size this frame rendered at.
      FB_W = FB_W_DEFAULT;
      FB_H = FB_H_DEFAULT;
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
      rootNode.memory = memory; // mirror load(): a root spawn reads names from here
      // Honor a cartridge's dims() (no-op when absent — keeps the 320x240
      // default and the existing parity snapshots byte-identical).
      applyDims(inst.exports);
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
    // Run the worker's REAL `load()` against a captured postMessage sink and
    // return the ordered list of message `type`s it posted. Drives the actual
    // one-shot-vs-animated branch so the parity harness can assert that a
    // one-shot render() emits a `done` (the watchdog-disarm signal) and an
    // animated frame() does NOT — the regression guard for the false-LH1001
    // bug. A no-op `setTimeout` neutralizes the animated self-pacing loop so the
    // test posts exactly one frame and returns.
    async loadAndCollect(wasmBytes) {
      const types = [];
      const prevSelf = globalThis.self;
      const prevSetTimeout = globalThis.setTimeout;
      globalThis.self = { postMessage: (m) => types.push(m && m.type) };
      globalThis.setTimeout = () => 0; // don't actually re-tick the frame loop
      try {
        await load(wasmBytes);
      } finally {
        globalThis.self = prevSelf;
        globalThis.setTimeout = prevSetTimeout;
      }
      return types;
    },
  };
}
