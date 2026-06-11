// Generate the PWA icons (web/icons/*.png) — zero dependencies.
//
//   node scripts/gen-pwa-icons.mjs
//
// Emits 8-bit GRAYSCALE PNGs (monochrome by construction): a black square
// with a white blocky "lh" glyph, IBM-Plex-Mono-ish proportions. The PNG
// encoder is hand-rolled on top of Node's built-in zlib (deflateSync) +
// a small CRC32 — no npm packages, so the icons are reproducible from a
// bare checkout. Three outputs:
//   icon-192.png           192x192, purpose "any"
//   icon-512.png           512x512, purpose "any"
//   icon-512-maskable.png  512x512, purpose "maskable" (glyph shrunk into
//                          the ~80% safe zone so launcher masks don't clip)
//
// Checked-in outputs are canonical; re-run only if the glyph changes.

import { deflateSync } from 'node:zlib';
import { writeFileSync, mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const OUT_DIR = join(ROOT, 'web', 'icons');

// ---- "lh" glyph bitmap (12 x 11) -------------------------------------------
// '#' = white pixel. Lowercase l + h, 2px strokes, monospace-flavored.
const GLYPH = [
  '##...##.....',
  '##...##.....',
  '##...##.....',
  '##...##.....',
  '##...#######',
  '##...#######',
  '##...##...##',
  '##...##...##',
  '##...##...##',
  '##...##...##',
  '##...##...##',
];
const GLYPH_W = GLYPH[0].length;
const GLYPH_H = GLYPH.length;

// ---- CRC32 (PNG chunk checksums) --------------------------------------------
const CRC_TABLE = new Uint32Array(256);
for (let n = 0; n < 256; n++) {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  CRC_TABLE[n] = c >>> 0;
}
function crc32(buf) {
  let c = 0xffffffff;
  for (const byte of buf) c = CRC_TABLE[(c ^ byte) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, 'ascii'), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
}

/** Encode an 8-bit TRUECOLOR-ALPHA (RGBA) PNG from a width*height
 * luminance Uint8Array. RGBA (color type 6) rather than grayscale: the
 * image is still monochrome by construction, but Chrome's WebAPK minting
 * service (the thing that turns an installed PWA into a real app-drawer
 * entry on Android) has choked on less-common color types — a Pixel
 * install landed with no launcher icon until the icons were re-encoded
 * as plain RGBA. Boring on purpose. */
function encodePng(width, height, pixels) {
  const SIG = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // color type 6 = truecolor with alpha
  // compression / filter / interlace = 0
  // raw image data: each scanline prefixed with filter byte 0 (None),
  // 4 bytes (RGBA) per pixel, alpha always opaque
  const stride = width * 4 + 1;
  const raw = Buffer.alloc(height * stride);
  for (let y = 0; y < height; y++) {
    raw[y * stride] = 0;
    for (let x = 0; x < width; x++) {
      const v = pixels[y * width + x];
      const o = y * stride + 1 + x * 4;
      raw[o] = v;
      raw[o + 1] = v;
      raw[o + 2] = v;
      raw[o + 3] = 255;
    }
  }
  return Buffer.concat([
    SIG,
    chunk('IHDR', ihdr),
    chunk('IDAT', deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

/**
 * Render the icon: black field, white glyph centered, scaled so the glyph
 * spans `coverage` of the icon width (maskable uses a smaller coverage to
 * stay inside the launcher-mask safe zone).
 */
function renderIcon(size, coverage) {
  const px = new Uint8Array(size * size); // 0 = black
  const scale = Math.max(1, Math.floor((size * coverage) / GLYPH_W));
  const gw = GLYPH_W * scale;
  const gh = GLYPH_H * scale;
  const ox = Math.floor((size - gw) / 2);
  const oy = Math.floor((size - gh) / 2);
  for (let gy = 0; gy < GLYPH_H; gy++) {
    for (let gx = 0; gx < GLYPH_W; gx++) {
      if (GLYPH[gy][gx] !== '#') continue;
      for (let dy = 0; dy < scale; dy++) {
        const row = (oy + gy * scale + dy) * size;
        px.fill(255, row + ox + gx * scale, row + ox + (gx + 1) * scale);
      }
    }
  }
  return encodePng(size, size, px);
}

mkdirSync(OUT_DIR, { recursive: true });
const outputs = [
  ['icon-192.png', renderIcon(192, 0.62)],
  ['icon-512.png', renderIcon(512, 0.62)],
  ['icon-512-maskable.png', renderIcon(512, 0.42)],
];
for (const [name, buf] of outputs) {
  writeFileSync(join(OUT_DIR, name), buf);
  console.log(`wrote web/icons/${name} (${buf.length} bytes)`);
}
