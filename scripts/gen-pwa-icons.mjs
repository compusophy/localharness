// Generate the PWA icons + favicon (web/icons/*) — zero runtime dependencies.
//
//   node scripts/gen-pwa-icons.mjs
//
// The glyph is the REAL "lh" from IBM Plex Mono SemiBold (weight 600 — the
// brand wordmark weight), not a hand-drawn lookalike. The true font outlines
// for `l` + `h` were extracted once with fontTools and flattened to polygons
// (see OUTLINE below, em=1000, y-up); this script rasterizes them with an
// anti-aliasing supersample. Re-extract only if the wordmark font/weight
// changes:
//   curl -sL -o /tmp/m.ttf https://github.com/google/fonts/raw/main/ofl/ibmplexmono/IBMPlexMono-SemiBold.ttf
//   # then a fontTools RecordingPen over glyphs 'l','h' (advance 600 each),
//   # flatten qCurveTo at ~24 steps, emit {em,bbox,advance_total,contours}.
//
// Emits monochrome (black field, white glyph) RGBA PNGs + a crisp vector
// favicon. Checked-in outputs are canonical; re-run only if the glyph changes.
//   icon-192.png           192x192, manifest "any" + apple-touch
//   icon-512.png           512x512, manifest "any"
//   icon-512-maskable.png  512x512, "maskable" (glyph in the ~80% safe zone)
//   favicon-32.png         32x32,   browser-tab fallback
//   favicon.svg            vector,  browser-tab (resolution-independent)

import { deflateSync } from 'node:zlib';
import { writeFileSync, mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const OUT_DIR = join(ROOT, 'web', 'icons');

// ---- true IBM Plex Mono SemiBold "lh" outline (font units, em=1000, y-up) ---
// Flattened contours: [l-stem, h]. No counters, so a single even-odd fill paints
// both. bbox lets the renderer normalize; coverage frames the glyph in the icon.
const OUTLINE = {"em":1000,"bbox":[72,0,1130,740],"advance_total":1200,"contours":[[[72,101],[236,101],[236,639],[72,639],[72,740],[364,740],[364,101],[529,101],[529,0],[72,0]],[[675,740],[803,740],[803,425],[808,425],[809.4,428.5],[811,431.9],[812.5,435.3],[814.2,438.7],[815.9,442],[817.7,445.3],[819.5,448.5],[821.4,451.7],[823.4,454.9],[825.4,458],[827.5,461.1],[829.6,464.1],[831.8,467.1],[834.1,470.1],[836.5,473],[838.9,475.9],[841.4,478.7],[843.9,481.5],[846.5,484.3],[849.2,487],[851.9,489.7],[854.7,492.3],[857.6,494.9],[860.5,497.5],[863.5,500],[866.6,502.4],[869.8,504.6],[873.1,506.8],[876.5,508.9],[880,510.8],[883.5,512.7],[887.2,514.4],[891,516.1],[894.9,517.6],[898.8,519.1],[902.9,520.4],[907,521.6],[911.3,522.7],[915.6,523.7],[920.1,524.6],[924.6,525.4],[929.2,526.1],[933.9,526.7],[938.8,527.2],[943.7,527.5],[948.7,527.8],[953.8,527.9],[959,528],[965.3,527.9],[971.5,527.6],[977.6,527.2],[983.5,526.6],[989.4,525.8],[995.2,524.8],[1000.9,523.7],[1006.5,522.4],[1012,520.9],[1017.4,519.2],[1022.7,517.4],[1027.9,515.4],[1033,513.2],[1038,510.8],[1042.9,508.3],[1047.7,505.6],[1052.4,502.7],[1057,499.6],[1061.5,496.3],[1065.9,492.9],[1070.2,489.3],[1074.4,485.6],[1078.5,481.6],[1082.5,477.5],[1086.4,473.2],[1090.1,468.8],[1093.6,464.2],[1097,459.5],[1100.2,454.6],[1103.3,449.5],[1106.2,444.3],[1108.9,439],[1111.4,433.5],[1113.8,427.9],[1116.1,422.1],[1118.1,416.1],[1120,410],[1121.8,403.8],[1123.3,397.4],[1124.7,390.8],[1126,384.1],[1127,377.3],[1127.9,370.3],[1128.7,363.1],[1129.3,355.8],[1129.7,348.4],[1129.9,340.8],[1130,333],[1130,0],[1002,0],[1002,315],[1001.8,324.1],[1001.3,332.9],[1000.5,341.2],[999.3,349.2],[997.8,356.8],[995.9,364],[993.7,370.8],[991.2,377.2],[988.4,383.2],[985.2,388.9],[981.6,394.1],[977.8,399],[973.5,403.5],[969,407.6],[964.1,411.2],[958.9,414.6],[953.3,417.5],[947.4,420],[941.2,422.1],[934.6,423.9],[927.7,425.2],[920.5,426.2],[912.9,426.8],[905,427],[903.3,427],[901.7,427],[900,426.9],[898.4,426.9],[896.8,426.8],[895.1,426.7],[893.5,426.6],[891.9,426.4],[890.3,426.3],[888.7,426.1],[887.1,425.9],[885.5,425.8],[883.9,425.5],[882.3,425.3],[880.8,425],[879.2,424.8],[877.7,424.5],[876.1,424.2],[874.6,423.9],[873.1,423.5],[871.5,423.2],[870,422.8],[868.5,422.4],[867,422],[865.5,421.6],[864,421.1],[862.6,420.7],[861.1,420.2],[859.7,419.7],[858.2,419.2],[856.8,418.6],[855.4,418.1],[854,417.5],[852.6,416.9],[851.2,416.3],[849.9,415.6],[848.5,415],[847.2,414.3],[845.9,413.6],[844.6,412.9],[843.3,412.2],[842,411.4],[840.7,410.6],[839.4,409.8],[838.2,409],[836.9,408.2],[835.7,407.4],[834.5,406.5],[833.3,405.6],[832.1,404.7],[831,403.8],[829.8,402.9],[828.7,401.9],[827.6,401],[826.6,400],[825.5,399],[824.5,398],[823.5,397],[822.5,395.9],[821.5,394.9],[820.6,393.8],[819.6,392.7],[818.7,391.6],[817.8,390.5],[817,389.4],[816.1,388.2],[815.3,387.1],[814.5,385.9],[813.7,384.7],[813,383.5],[812.2,382.2],[811.5,381],[810.8,379.7],[810.1,378.5],[809.5,377.2],[808.9,375.8],[808.3,374.5],[807.8,373.1],[807.3,371.7],[806.8,370.3],[806.3,368.9],[805.9,367.5],[805.5,366],[805.1,364.5],[804.8,363],[804.5,361.5],[804.2,359.9],[803.9,358.3],[803.7,356.7],[803.5,355.1],[803.4,353.5],[803.2,351.8],[803.1,350.2],[803.1,348.5],[803,346.7],[803,345],[803,0],[675,0]]]};

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

/** Encode an 8-bit TRUECOLOR-ALPHA (RGBA) PNG from a width*height luminance
 * Uint8Array. RGBA (color type 6) rather than grayscale: the image is still
 * monochrome by construction, but Chrome's WebAPK minting service (turns an
 * installed PWA into a real app-drawer entry on Android) has choked on
 * less-common color types — a Pixel install landed with no launcher icon until
 * the icons were re-encoded as plain RGBA. Boring on purpose. */
function encodePng(width, height, pixels) {
  const SIG = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // color type 6 = truecolor with alpha
  const stride = width * 4 + 1;
  const raw = Buffer.alloc(height * stride);
  for (let y = 0; y < height; y++) {
    raw[y * stride] = 0; // filter byte 0 (None)
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

/** Map the font-unit outline into a square of side `size`, framed so the glyph
 * width spans `coverage` of it, returning pixel-space polygons (y-down). */
function placeGlyph(size, coverage) {
  const [minx, miny, maxx, maxy] = OUTLINE.bbox;
  const gw = maxx - minx, gh = maxy - miny;
  const scale = (coverage * size) / gw;
  const ox = (size - gw * scale) / 2;
  const oy = (size - gh * scale) / 2;
  // font y-up → pixel y-down: py = oy + (maxy - y) * scale
  return OUTLINE.contours.map((c) =>
    c.map(([x, y]) => [ox + (x - minx) * scale, oy + (maxy - y) * scale]),
  );
}

/** Even-odd scanline fill of closed polygons into a 0/1 mask (W*H). */
function fillMask(mask, W, H, polys) {
  for (let y = 0; y < H; y++) {
    const yc = y + 0.5;
    const xs = [];
    for (const poly of polys) {
      for (let i = 0; i < poly.length; i++) {
        const [x1, y1] = poly[i];
        const [x2, y2] = poly[(i + 1) % poly.length];
        if (y1 === y2) continue;
        if ((yc >= y1 && yc < y2) || (yc >= y2 && yc < y1)) {
          xs.push(x1 + ((yc - y1) / (y2 - y1)) * (x2 - x1));
        }
      }
    }
    xs.sort((a, b) => a - b);
    for (let i = 0; i + 1 < xs.length; i += 2) {
      let xa = Math.max(0, Math.round(xs[i]));
      let xb = Math.min(W, Math.round(xs[i + 1]));
      for (let x = xa; x < xb; x++) mask[y * W + x] = 1;
    }
  }
}

const SS = 8; // supersample factor → anti-aliased glyph edges

/** Render a `size`x`size` icon: black field, white glyph, AA via supersample. */
function renderIcon(size, coverage) {
  const S = size * SS;
  const hi = new Uint8Array(S * S);
  fillMask(hi, S, S, placeGlyph(S, coverage));
  const out = new Uint8Array(size * size); // 0 = black
  const norm = 255 / (SS * SS);
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      let acc = 0;
      for (let dy = 0; dy < SS; dy++) {
        const row = (y * SS + dy) * S + x * SS;
        for (let dx = 0; dx < SS; dx++) acc += hi[row + dx];
      }
      out[y * size + x] = Math.round(acc * norm);
    }
  }
  return encodePng(size, size, out);
}

/** Crisp resolution-independent favicon: the true outline as an SVG path,
 * white-on-black, framed to match the PNG `coverage`. */
function renderSvg(coverage) {
  const [minx, miny, maxx, maxy] = OUTLINE.bbox;
  const gw = maxx - minx, gh = maxy - miny;
  const side = gw / coverage;
  const padx = (side - gw) / 2, pady = (side - gh) / 2;
  const d = OUTLINE.contours
    .map((c) =>
      c
        .map(([x, y], i) => {
          const px = (padx + (x - minx)).toFixed(1);
          const py = (pady + (maxy - y)).toFixed(1); // y-up → y-down
          return `${i ? 'L' : 'M'}${px} ${py}`;
        })
        .join('') + 'Z',
    )
    .join('');
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${side.toFixed(1)} ${side.toFixed(1)}" width="64" height="64"><rect width="100%" height="100%" fill="#000"/><path d="${d}" fill="#fff"/></svg>\n`;
}

mkdirSync(OUT_DIR, { recursive: true });
const outputs = [
  ['icon-192.png', renderIcon(192, 0.62)],
  ['icon-512.png', renderIcon(512, 0.62)],
  ['icon-512-maskable.png', renderIcon(512, 0.42)],
  ['favicon-32.png', renderIcon(32, 0.62)],
  ['favicon.svg', Buffer.from(renderSvg(0.62), 'utf8')],
];
for (const [name, buf] of outputs) {
  writeFileSync(join(OUT_DIR, name), buf);
  console.log(`wrote web/icons/${name} (${buf.length} bytes)`);
}
