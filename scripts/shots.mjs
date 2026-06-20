#!/usr/bin/env node
// scripts/shots.mjs — the committed mobile screenshot suite.
//
// Serves the local `web/` build and walks every surface in MOBILE-PREVIEW mode
// (the `?preview=mobile` setting) in BOTH themes (`?theme=dark|light`), writing
// a full set into `web/screenshots/suite/` in one command — so "full suite" is
// one run, not a manual click-path. It covers everything a localhost
// (`Host::Other`) render shows without an identity: the studio + admin panels.
// (The apex `$2` landing and the live-agent chat-with-cartridge need the real
// site / a funded agent — captured via the same render-mode params on the real
// Chrome, not here.)
//
//   node scripts/shots.mjs
//
// Needs a built `web/pkg` (./scripts/build-web.sh or wasm-pack) + puppeteer-core
// (`npm i -g puppeteer-core`, or run from a dir that has it). Override the
// browser with CHROME=/path/to/chrome.
import puppeteer from 'puppeteer-core';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { mkdirSync } from 'node:fs';
import { join, extname, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { tmpdir } from 'node:os';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const WEB = join(ROOT, 'web');
const OUT = join(WEB, 'screenshots', 'suite');
const CHROME = process.env.CHROME || 'C:/Program Files/Google/Chrome/Application/chrome.exe';
const PORT = Number(process.env.PORT || 8744);
mkdirSync(OUT, { recursive: true });

const MIME = { '.html': 'text/html', '.js': 'text/javascript', '.wasm': 'application/wasm', '.json': 'application/json', '.css': 'text/css', '.png': 'image/png', '.svg': 'image/svg+xml', '.txt': 'text/plain', '.ico': 'image/x-icon', '.rl': 'text/plain' };
const server = createServer(async (req, res) => {
  try {
    let p = decodeURIComponent(new URL(req.url, 'http://x').pathname);
    if (p === '/' || p.endsWith('/')) p += 'index.html';
    const body = await readFile(join(WEB, p));
    res.writeHead(200, { 'content-type': MIME[extname(p).toLowerCase()] || 'application/octet-stream' });
    res.end(body);
  } catch { res.writeHead(404); res.end('404'); }
}).listen(PORT);
const base = `http://localhost:${PORT}`;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const ready = async (page, t = 20000) => { const t0 = Date.now(); while (Date.now() - t0 < t) { if (await page.evaluate(() => document.documentElement.dataset.lhReady === '1').catch(() => false)) return true; await sleep(200); } return false; };
async function clickText(page, re) {
  const h = await page.evaluateHandle((r) => { const rx = new RegExp(r, 'i'); return [...document.querySelectorAll('button,[data-action]')].find((e) => rx.test((e.textContent || '').trim()) || rx.test(e.getAttribute('data-action') || '')) || null; }, re);
  const el = h.asElement(); if (el) { await el.click(); return true; } return false;
}
async function openAdmin(page, tab) {
  await clickText(page, 'header-admin-toggle'); await sleep(800);
  if (tab !== 'account') await clickText(page, '^' + tab + '$');
  await sleep(500);
  if (tab === 'display') { await page.evaluate(() => document.getElementById('display-section')?.scrollIntoView({ block: 'center' })); await sleep(300); }
}

// Surfaces a localhost (Host::Other) render shows with no identity.
const SURFACES = [
  { name: 'studio', go: async () => {} },                            // empty workshop / prompt
  { name: 'admin-account', go: (p) => openAdmin(p, 'account') },     // identity + credits
  { name: 'admin-agent', go: (p) => openAdmin(p, 'agent') },         // model / persona / price / tools
  { name: 'admin-display', go: (p) => openAdmin(p, 'display') },     // the new render-mode toggles
];
const THEMES = ['dark', 'light'];

const browser = await puppeteer.launch({ executablePath: CHROME, headless: 'new', userDataDir: join(tmpdir(), 'lh-shots-' + Date.now()), args: ['--disable-features=ThirdPartyStoragePartitioning', '--no-sandbox', '--hide-scrollbars'] });
const page = (await browser.pages())[0];
await page.setViewport({ width: 390, height: 844, deviceScaleFactor: 2, isMobile: true, hasTouch: true });
await page.setUserAgent('Mozilla/5.0 (Linux; Android 14; Pixel 7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Mobile Safari/537.36');
try {
  for (const theme of THEMES) {
    for (const s of SURFACES) {
      await page.goto(`${base}/?preview=mobile&theme=${theme}`, { waitUntil: 'networkidle2', timeout: 30000 });
      await ready(page); await sleep(800);
      await s.go(page); await sleep(400);
      const cls = await page.evaluate(() => document.documentElement.className);
      const file = join(OUT, `${s.name}-${theme}.png`);
      await page.screenshot({ path: file });
      console.log(`wrote ${s.name}-${theme}.png  [html.class="${cls}"]`);
    }
  }
} finally { await browser.close(); server.close(); }
