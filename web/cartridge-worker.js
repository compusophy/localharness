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

// Logical framebuffer resolution. The DEFAULT (256x144, 16:9) MUST match the
// FB_W/FB_H defaults in src/app/display.rs. A cartridge MAY override these per
// load by exporting `dims() -> i32` returning a PACKED (width << 16) | height
// (width in the high 16 bits, height in the low 16). The worker calls it ONCE
// after instantiate; a cartridge with NO `dims()` export keeps the default, so
// every existing cartridge renders EXACTLY as before (backward compatible).
//
// These are mutable (`let`): `applyDims()` rewrites them at load time. The
// Node test surface still exports the DEFAULTS (FB_W_DEFAULT/FB_H_DEFAULT) and
// the live values, and `renderOnce` honors a cartridge's `dims()` too.
const FB_W_DEFAULT = 256;
const FB_H_DEFAULT = 144;
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
// graph so an attacker-authored or runaway parent can't exhaust the worker:
// 8 children, 16 KB each, 64 KB total. A child that itself spawns grandchildren
// counts against the same total/count — the aggregate is the recursion backstop.
const COMPOSE_MAX_CHILDREN = 8;
const COMPOSE_MAX_BYTES_PER_CHILD = 16 * 1024;
const COMPOSE_MAX_TOTAL_BYTES = 64 * 1024;

// Child module states (mirror the host::compose status() ABI).
const MOD_LOADING = 0;
const MOD_READY = 1;
const MOD_FAILED = 2;

// The live child table. handle = index; a closed slot becomes null (never
// aliased). Each entry owns its OWN buffer/instance/memory/state — isolation is
// per-instance. `focus` is the handle that receives pointer input (-1 = parent).
let composeChildren = [];
let composeFocus = -1;
let composeTotalBytes = 0;

function composeLiveCount() {
  let n = 0;
  for (const c of composeChildren) if (c && c.state !== MOD_FAILED) n++;
  return n;
}

// Read a child's dims() the same way applyDims does for the parent: packed
// (w<<16)|h, clamped to [FB_MIN, FB_MAX]; default 256x144 when absent/invalid.
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
  // Inert recursion stub: a child cannot itself spawn (depth-1 cap); the ABI
  // still resolves so a child importing host_compose instantiates.
  const child_compose = {
    spawn_module: () => -1,
    status: () => -1,
    move_module: () => 0,
    focus_module: () => -1,
    focused: () => -1,
    close_module: () => -1,
    module_count: () => 0,
  };
  // A child gets its own (no-op) net/audio/agent so its imports link, but it
  // can't reach the platform from inside a panel (the parent is the surface).
  const child_net = { open: () => -1, send: () => 0, poll: () => -1, status: () => -1, close: () => {} };
  const child_audio = { tone: () => -1, tone_at: () => -1, noise: () => -1, stop: () => {}, set_volume: () => {} };
  const child_agent = {
    notify: () => 0, viewer_is_owner: () => 0, viewer_has_identity: () => 0,
    subscribe: () => 0, unsubscribe: () => 0, is_subscribed: () => 0,
    subscriber_count: () => 0, broadcast: () => 0, request_identity: () => 0,
  };
  return {
    host_display: child_display,
    host_compose: child_compose,
    host_net: child_net,
    host_audio: child_audio,
    host_agent: child_agent,
    host_log, host_time, host_abort,
  };
}

// Instantiate the fetched bytes for a Loading child into its own instance +
// buffer. Marks the slot Ready (or Failed) and accounts its bytes against the
// total. Called from the main-thread `compose_bytes` reply.
function composeInstantiate(handle, wasmBuf) {
  const child = composeChildren[handle];
  if (!child || child.state === MOD_FAILED) return;
  const bytes = new Uint8Array(wasmBuf);
  // Per-child + aggregate byte caps (mirror ComposeBudget::admit). The count cap
  // is enforced at spawn; the byte caps are enforced here once the size is known.
  if (bytes.length > COMPOSE_MAX_BYTES_PER_CHILD ||
      composeTotalBytes + bytes.length > COMPOSE_MAX_TOTAL_BYTES) {
    child.state = MOD_FAILED;
    return;
  }
  let instance;
  try {
    const mod = new WebAssembly.Module(bytes);
    instance = new WebAssembly.Instance(mod, buildChildImports(child));
  } catch (_e) {
    child.state = MOD_FAILED;
    return;
  }
  const exp = instance.exports;
  const [dw, dh] = childDims(exp);
  child.w = dw;
  child.h = dh;
  child.fb = new Uint32Array(dw * dh);
  child.memory = exp.memory || null;
  child.frame = (typeof exp.frame === 'function') ? exp.frame
    : (typeof exp.render === 'function') ? exp.render : null;
  if (!child.frame) { child.state = MOD_FAILED; return; }
  composeTotalBytes += bytes.length;
  child.bytes = bytes.length;
  child.state = MOD_READY;
}

// host_compose: the parent's window-manager ABI. spawn_module posts a fetch
// request to the main thread (the worker can't do the on-chain registry read);
// the rest mutate the child table synchronously.
const host_compose = {
  spawn_module(namePtr, x, y, w, h) {
    const name = readString(namePtr);
    if (name === null || name === '') return -1;
    if (composeLiveCount() >= COMPOSE_MAX_CHILDREN) return -1; // count cap
    // Allocate a slot (reuse a null hole, else push). Slots never alias: a fresh
    // logical child each spawn even when reusing a freed index.
    let handle = composeChildren.indexOf(null);
    const child = {
      name, state: MOD_LOADING,
      vp: { x: x | 0, y: y | 0, w: Math.max(1, w | 0), h: Math.max(1, h | 0) },
      w: FB_W_DEFAULT, h: FB_H_DEFAULT, fb: null,
      memory: null, frame: null, bytes: 0,
      state_regs: new Int32Array(64),
      ptr: { x: -1, y: -1, down: 0 },
    };
    if (handle < 0) { handle = composeChildren.length; composeChildren.push(child); }
    else composeChildren[handle] = child;
    self.postMessage({ type: 'compose_spawn', handle, name });
    return handle;
  },
  status(handle) {
    const c = composeChildren[handle];
    return c ? c.state : -1;
  },
  move_module(handle, x, y, w, h) {
    const c = composeChildren[handle];
    if (!c) return 0;
    c.vp = { x: x | 0, y: y | 0, w: Math.max(1, w | 0), h: Math.max(1, h | 0) };
    return 1;
  },
  focus_module(handle) {
    if (handle === -1) { composeFocus = -1; return 1; } // focus the parent
    if (!composeChildren[handle]) return 0;
    composeFocus = handle;
    return 1;
  },
  focused: () => composeFocus,
  close_module(handle) {
    const c = composeChildren[handle];
    if (!c) return 0;
    if (c.state === MOD_READY) composeTotalBytes -= c.bytes;
    composeChildren[handle] = null;
    if (composeFocus === handle) composeFocus = -1;
    return 1;
  },
  module_count: () => composeLiveCount(),
};

// Reset the compose table (a fresh parent load clears the whole graph).
function composeReset() {
  composeChildren = [];
  composeFocus = -1;
  composeTotalBytes = 0;
}

// Tick every Ready child into its own buffer, then blit it into the parent FB at
// the child's viewport (nearest-neighbour scale). Pointer routes only into the
// focused child (focus-gated). A trapping child is latched Failed + skipped — it
// can't take down the parent or a sibling. Called from the parent's tick() after
// the parent's frame() draws, before present().
function composeCompositePass(t) {
  for (let i = 0; i < composeChildren.length; i++) {
    const c = composeChildren[i];
    if (!c) continue;
    if (c.state !== MOD_READY) continue; // Loading/Failed draw nothing in v1
    // Focus-gated pointer: only the focused child feels the pointer, and only
    // over its own rect; everyone else reads "no pointer" (-1 / 0).
    if (i === composeFocus) {
      const mapped = mapPointerIntoChild(ptr.x, ptr.y, c.vp.x, c.vp.y, c.vp.w, c.vp.h, c.w, c.h);
      if (mapped) { c.ptr.x = mapped[0]; c.ptr.y = mapped[1]; c.ptr.down = ptr.down; }
      else { c.ptr.x = -1; c.ptr.y = -1; c.ptr.down = 0; }
    } else {
      c.ptr.x = -1; c.ptr.y = -1; c.ptr.down = 0;
    }
    try {
      c.frame(t);
    } catch (_e) {
      c.state = MOD_FAILED; // latch + skip; never propagates to the parent
      composeTotalBytes -= c.bytes;
      continue;
    }
    blitChild(fb32, FB_W, FB_H, c.fb, c.w, c.h, c.vp.x, c.vp.y, c.vp.w, c.vp.h);
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

function buildImports() {
  return { host_display, host_net, host_audio, host_log, host_time, host_abort, host_agent, host_compose };
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
  composeReset(); // a fresh parent clears the whole compose graph
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
  // A cartridge MAY declare its own framebuffer dims via `dims() -> i32`
  // (packed (w<<16)|h). No export => the 256x144 default. Reallocates the FB.
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
      case 'agent_context':
        applyAgentContext(msg);
        break;
      case 'compose_bytes':
        // Main thread resolved a child's on-chain app.wasm (or signalled a
        // failure with wasm=null). Instantiate it into its slot, or mark Failed.
        if (msg.wasm) {
          composeInstantiate(msg.handle | 0, msg.wasm);
        } else {
          const c = composeChildren[msg.handle | 0];
          if (c) c.state = MOD_FAILED;
        }
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
    // The DEFAULT dims (256x144). The live FB_W/FB_H are `let`-mutable and a
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
    host_compose, // the window-manager ABI (spawn/status/focus/close/...)
    // The JS mirrors of compose.rs (under parity test vs the Rust impls).
    blitChild,
    mapPointerIntoChild,
    composeReset,
    composeInstantiate,
    composeChildren: () => composeChildren,
    composeFocus: () => composeFocus,
    // Allocate a LOADING child slot directly (test shim — drives the real child
    // table + ComposeBudget count cap, avoiding spawn_module's readString/
    // postMessage path, which needs a live worker + a parent's linear memory).
    // The caller then feeds the child's bytes via composeInstantiate (simulating
    // the main-thread compose_bytes reply) and ticks via composeRunPass.
    // Returns the handle, or -1 when the child-count cap is hit.
    composeMountForTest(name, x, y, w, h) {
      if (composeLiveCount() >= COMPOSE_MAX_CHILDREN) return -1; // count cap
      let handle = composeChildren.indexOf(null);
      const child = {
        name, state: MOD_LOADING,
        vp: { x: x | 0, y: y | 0, w: Math.max(1, w | 0), h: Math.max(1, h | 0) },
        w: FB_W_DEFAULT, h: FB_H_DEFAULT, fb: null,
        memory: null, frame: null, bytes: 0,
        state_regs: new Int32Array(64),
        ptr: { x: -1, y: -1, down: 0 },
      };
      if (handle < 0) { handle = composeChildren.length; composeChildren.push(child); }
      else composeChildren[handle] = child;
      return handle;
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
      // Honor a cartridge's dims() (no-op when absent — keeps the 256x144
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
