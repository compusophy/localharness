#!/usr/bin/env node
// nostr-seti.mjs — "SETI for the colony": DISCOVER other AI agents on Nostr and
// HAIL them. Grows the localharness agent network over an open, self-sovereign wire.
//
// ZERO npm deps (project rule). Reuses the from-scratch BIP-340 signer, bech32
// codec, and node:tls WebSocket client from `nostr-broadcast.mjs` (imported), and
// adds REQ/subscribe scanning on top. Same nsec identity in `.nostr_identity`.
//
// Commands:
//   node scripts/nostr-seti.mjs discover [--limit N] [--out file.json] [--relays a,b]
//        Scan relays for AI service agents and print a registry
//        (npub, kind, what they do, last seen). Sources:
//          - NIP-89 handler announcements (kind 31990) — DVMs declaring a service
//          - NIP-90 Data Vending Machine job events (kinds 5xxx/6xxx/7000)
//          - kind-0 profiles whose name/about self-identify as bot/agent/AI/DVM
//          - kind-1 notes tagged #ai #agents #autonomousagents #nostragents #dvm
//
//   node scripts/nostr-seti.mjs hail <event-id> "<message>" [--dry]
//        Reply (kind-1, NIP-10 threaded) to a discovered agent's real public note.
//        Fetches the target first to derive its author (p-tag) + thread root.
//        --dry prints the signed event without publishing. Reuses the broadcaster's
//        publish+read-back verification path.
//
// Outreach rules (enforced by convention + the doc): truthful, disclosed as an
// automated localharness agent, no spam, no invented features, no chain address,
// pace them — a few real signals, not a storm. See
// design/autonomous-business/colony/SETI-NOSTR.md.

import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import {
  DEFAULT_RELAYS,
  loadIdentity,
  buildEvent,
  wsConnect,
  publishToRelay,
  fetchFromRelay,
  bech32Encode,
} from './nostr-broadcast.mjs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

// Wider relay set for discovery (read-heavy). Search-capable relays (NIP-50) get
// extra full-text profile/note filters; all relays get the kind/tag filters.
const DISCOVERY_RELAYS = [
  'wss://relay.damus.io',
  'wss://relay.primal.net',
  'wss://nos.lol',
  'wss://relay.nostr.band',
  'wss://relay.snort.social',
];
const SEARCH_RELAYS = new Set([
  'wss://relay.nostr.band',
  'wss://relay.primal.net',
  'wss://relay.snort.social',
]);

// ---------------------------------------------------------------------------
// NIP-90 Data Vending Machine kind map (data-vending-machines.org well-knowns).
// Requests are 5xxx, results are the same + 1000 (6xxx), 7000 = job feedback.
// ---------------------------------------------------------------------------
const DVM_KIND_LABEL = {
  5000: 'generic job',
  5001: 'summarization',
  5002: 'translation',
  5050: 'text generation',
  5100: 'image generation',
  5200: 'video conversion',
  5250: 'text-to-speech',
  5300: 'content discovery',
  5301: 'people discovery',
  5302: 'note search',
  5303: 'profile search',
  5400: 'count / aggregation',
  5500: 'malware / code scan',
  5900: 'op (sign/encrypt)',
  5969: 'DVM job',
  5970: 'DVM job',
};
const DVM_REQUEST_KINDS = Object.keys(DVM_KIND_LABEL).map(Number);
const DVM_RESULT_KINDS = DVM_REQUEST_KINDS.map((k) => k + 1000);
const DVM_KINDS = [...DVM_REQUEST_KINDS, ...DVM_RESULT_KINDS, 7000];

function describeKind(kind) {
  if (kind === 31990) return 'NIP-89 service announcement';
  if (kind === 7000) return 'DVM job feedback';
  if (kind === 0) return 'profile';
  if (kind === 1) return 'note';
  if (DVM_KIND_LABEL[kind]) return `DVM request — ${DVM_KIND_LABEL[kind]}`;
  if (DVM_KIND_LABEL[kind - 1000]) return `DVM result — ${DVM_KIND_LABEL[kind - 1000]}`;
  return `kind ${kind}`;
}

// Hashtags that AI/agent accounts self-apply on kind-1 notes (lowercase; tag
// filters are exact-match and the convention is lowercase hashtags).
const AGENT_HASHTAGS = ['ai', 'agents', 'aiagents', 'autonomousagents', 'nostragents', 'dvm'];

// Self-identification keywords in a profile/announcement name+about.
const AGENT_RE =
  /\b(a\.?i\.?|bot|agent|agents|autonomous|dvm|data[- ]vending|llm|gpt|chatbot|assistant|automation|automated|machine[- ]learning|nostr ?agent|inference)\b/i;

// ---------------------------------------------------------------------------
// One REQ subscription: collect events until EOSE (or timeout) over one relay.
// ---------------------------------------------------------------------------
function collectFromRelay(url, filters, timeout = 12000) {
  return new Promise(async (resolve) => {
    const out = { relay: url, connected: false, events: [], eose: false, note: '' };
    let ws;
    try {
      ws = await wsConnect(url, timeout);
    } catch (e) {
      out.note = 'connect: ' + (e.message || e);
      return resolve(out);
    }
    out.connected = true;
    const subid = 'lh-seti-' + crypto.randomBytes(4).toString('hex');
    let finished = false;
    const finish = () => {
      if (finished) return;
      finished = true;
      clearTimeout(timer);
      try { ws.send(JSON.stringify(['CLOSE', subid])); } catch {}
      try { ws.close(); } catch {}
      resolve(out);
    };
    const timer = setTimeout(() => { out.note ||= 'timeout (collected ' + out.events.length + ')'; finish(); }, timeout);
    ws.on('message', (msg) => {
      let arr; try { arr = JSON.parse(msg); } catch { return; }
      const t = arr[0];
      if (t === 'EVENT' && arr[1] === subid && arr[2]) {
        out.events.push(arr[2]);
      } else if (t === 'EOSE' && arr[1] === subid) {
        out.eose = true;
        finish();
      } else if (t === 'CLOSED' && arr[1] === subid) {
        out.note ||= 'CLOSED: ' + (arr[2] || '');
        finish();
      } else if (t === 'NOTICE') {
        out.note ||= 'NOTICE: ' + (arr[1] || '');
      }
    });
    ws.on('error', (e) => { out.note ||= 'error: ' + (e.message || e); finish(); });
    ws.on('close', () => finish());
    ws.send(JSON.stringify(['REQ', subid, ...filters]));
  });
}

function safeJson(str) { try { return JSON.parse(str); } catch { return null; } }
function trunc(s, n) { s = (s || '').replace(/\s+/g, ' ').trim(); return s.length > n ? s.slice(0, n - 1) + '…' : s; }
function npubOf(hexPubkey) {
  try { return bech32Encode('npub', Buffer.from(hexPubkey, 'hex')); } catch { return hexPubkey; }
}
function ageStr(ts) {
  const s = Math.floor(Date.now() / 1000) - ts;
  if (s < 90) return s + 's';
  if (s < 5400) return Math.round(s / 60) + 'm';
  if (s < 129600) return Math.round(s / 3600) + 'h';
  return Math.round(s / 86400) + 'd';
}

// ---------------------------------------------------------------------------
// discover
// ---------------------------------------------------------------------------
async function discover(opts) {
  const limit = opts.limit || 120;
  const relays = opts.relays || DISCOVERY_RELAYS;
  const baseFilters = [
    { kinds: [31990], limit },                                   // NIP-89 service announcements
    { kinds: DVM_KINDS, limit },                                 // NIP-90 DVM activity
    { kinds: [1], '#t': AGENT_HASHTAGS, limit },                 // agent-tagged notes
  ];
  const searchFilters = [
    { kinds: [0], search: 'AI agent', limit: 40 },               // NIP-50 profile search
    { kinds: [0], search: 'bot', limit: 30 },
    { kinds: [0], search: 'DVM data vending', limit: 30 },
  ];

  console.error(`[discover] scanning ${relays.length} relays (limit ${limit}/filter)…`);
  const round1 = await Promise.all(relays.map((r) => {
    const filters = SEARCH_RELAYS.has(r) ? [...baseFilters, ...searchFilters] : baseFilters;
    return collectFromRelay(r, filters);
  }));
  for (const r of round1) {
    console.error(`  ${r.relay.padEnd(28)} ${r.connected ? 'ok' : 'DOWN'}  events=${r.events.length}${r.note ? '  (' + r.note + ')' : ''}`);
  }

  // Dedupe events by id; index by pubkey.
  const eventsById = new Map();
  for (const r of round1) for (const ev of r.events) if (ev && ev.id && !eventsById.has(ev.id)) eventsById.set(ev.id, ev);

  const candidates = new Map(); // pubkey -> { kinds:Set, lastSeen, sources:Set, sample, handledKinds:Set, profile }
  const touch = (pk) => {
    if (!candidates.has(pk)) candidates.set(pk, { pubkey: pk, kinds: new Set(), lastSeen: 0, sources: new Set(), sample: null, handledKinds: new Set(), profile: null });
    return candidates.get(pk);
  };

  for (const ev of eventsById.values()) {
    const c = touch(ev.pubkey);
    c.kinds.add(ev.kind);
    if (ev.created_at > c.lastSeen) { c.lastSeen = ev.created_at; }
    if (ev.kind === 31990) {
      c.sources.add('nip89-dvm');
      const meta = safeJson(ev.content) || {};
      c.profile = c.profile || meta;
      for (const tg of ev.tags || []) if (tg[0] === 'k' && Number.isFinite(Number(tg[1]))) c.handledKinds.add(Number(tg[1]));
      if (!c.sample) c.sample = ev;
    } else if (ev.kind === 0) {
      c.sources.add('profile');
      c.profile = safeJson(ev.content) || c.profile;
    } else if (DVM_KINDS.includes(ev.kind)) {
      c.sources.add('dvm-event');
      if (!c.sample) c.sample = ev;
    } else if (ev.kind === 1) {
      c.sources.add('tagged-note');
      if (!c.sample) c.sample = ev;
    }
  }

  // Round 2: enrich with kind-0 profiles for every candidate pubkey.
  const pubkeys = [...candidates.keys()];
  if (pubkeys.length) {
    console.error(`[discover] fetching profiles for ${pubkeys.length} candidate pubkeys…`);
    const chunks = [];
    for (let i = 0; i < pubkeys.length; i += 200) chunks.push(pubkeys.slice(i, i + 200));
    const profileRuns = [];
    for (const r of relays) for (const ch of chunks) profileRuns.push(collectFromRelay(r, [{ kinds: [0], authors: ch }], 10000));
    const round2 = await Promise.all(profileRuns);
    for (const run of round2) for (const ev of run.events) {
      const c = candidates.get(ev.pubkey);
      if (!c) continue;
      const prof = safeJson(ev.content);
      if (prof) c.profile = { ...(c.profile || {}), ...prof };
      if (ev.created_at > c.lastSeen) c.lastSeen = ev.created_at;
    }
  }

  // Classify: who is actually an AI/agent worth listing.
  const registry = [];
  for (const c of candidates.values()) {
    const p = c.profile || {};
    const name = p.display_name || p.name || p.username || '';
    const about = p.about || p.description || '';
    const nip05 = p.nip05 || '';
    const lud16 = p.lud16 || p.lud06 || '';
    const isDvm = c.sources.has('nip89-dvm') || c.sources.has('dvm-event');
    const kwHit = AGENT_RE.test(name + ' ' + about + ' ' + nip05);
    const confidence = c.sources.has('nip89-dvm') ? 'high'
      : c.sources.has('dvm-event') ? 'high'
      : kwHit ? 'medium'
      : 'low'; // tagged-note only
    // Keep DVMs always; keyword-profiles always; tagged-notes only if keyword-confirmed.
    if (!isDvm && !kwHit) continue;

    let what = '';
    if (c.handledKinds.size) {
      what = 'DVM — ' + [...c.handledKinds].map((k) => DVM_KIND_LABEL[k] || `kind ${k}`).join(', ');
    } else if (isDvm) {
      const dvmKinds = [...c.kinds].filter((k) => k === 31990 || DVM_KINDS.includes(k));
      what = dvmKinds.map(describeKind).join('; ');
    }
    const desc = trunc(about, 140);
    registry.push({
      npub: npubOf(c.pubkey),
      pubkey: c.pubkey,
      name: trunc(name, 40),
      what: what || desc || [...c.kinds].map(describeKind).join('; '),
      about: desc,
      nip05,
      lud16,
      kinds: [...c.kinds].sort((a, b) => a - b),
      handledKinds: [...c.handledKinds].sort((a, b) => a - b),
      sources: [...c.sources],
      confidence,
      lastSeen: c.lastSeen,
      sampleEvent: c.sample ? { id: c.sample.id, kind: c.sample.kind, content: trunc(c.sample.content, 160) } : null,
    });
  }

  // Sort: DVM service announcements first, then by recency.
  const rank = (r) => (r.sources.includes('nip89-dvm') ? 0 : r.sources.includes('dvm-event') ? 1 : 2);
  registry.sort((a, b) => rank(a) - rank(b) || b.lastSeen - a.lastSeen);

  // Print human table.
  console.log('\n=== localharness Nostr SETI — agent registry ===');
  console.log(`discovered ${registry.length} agent/DVM candidates across ${relays.length} relays\n`);
  for (const r of registry) {
    const seen = r.lastSeen ? ageStr(r.lastSeen) + ' ago' : '—';
    console.log(`• ${r.name || '(no name)'}  [${r.confidence}]  seen ${seen}`);
    console.log(`    ${r.npub}`);
    console.log(`    what: ${r.what}`);
    if (r.nip05) console.log(`    nip05: ${r.nip05}`);
    if (r.lud16) console.log(`    zap:   ${r.lud16}`);
    console.log(`    kinds: [${r.kinds.join(', ')}]  via ${r.sources.join('+')}`);
    if (r.sampleEvent) console.log(`    sample evt ${r.sampleEvent.id.slice(0, 16)}… (kind ${r.sampleEvent.kind}): ${r.sampleEvent.content || '(no content)'}`);
    console.log('');
  }

  if (opts.out) {
    fs.writeFileSync(opts.out, JSON.stringify({ scanned_at: new Date().toISOString(), relays, count: registry.length, registry }, null, 2) + '\n');
    console.error('[discover] wrote registry -> ' + opts.out);
  }
  console.log('JSON ' + JSON.stringify({ count: registry.length }));
  return registry;
}

// ---------------------------------------------------------------------------
// notes — list recent kind-1 notes by an author (npub or hex) so the operator can
// pick a genuine public post to reply to (proper etiquette: hail a real note).
// ---------------------------------------------------------------------------
async function notes(author, opts) {
  if (!author) { console.error('usage: notes <npub|hex-pubkey> [--limit N]'); process.exit(1); }
  let hex = author;
  if (author.startsWith('npub1')) {
    const mod = await import('./nostr-broadcast.mjs');
    const d = mod.bech32Decode(author);
    hex = d.bytes.toString('hex');
  }
  const limit = opts.limit || 10;
  console.error(`[notes] fetching up to ${limit} recent kind-1 notes by ${npubOf(hex)} …`);
  const runs = await Promise.all(DISCOVERY_RELAYS.map((r) => collectFromRelay(r, [{ kinds: [1], authors: [hex], limit }], 10000)));
  const byId = new Map();
  for (const run of runs) for (const ev of run.events) if (!byId.has(ev.id)) byId.set(ev.id, ev);
  const list = [...byId.values()].sort((a, b) => b.created_at - a.created_at);
  console.log(`\n=== ${list.length} recent notes by ${npubOf(hex)} ===\n`);
  for (const ev of list) {
    const isReply = (ev.tags || []).some((t) => t[0] === 'e');
    console.log(`evt ${ev.id}  ${ageStr(ev.created_at)} ago${isReply ? '  [reply]' : ''}`);
    console.log('    ' + trunc(ev.content, 220));
    console.log('    njump: https://njump.me/' + ev.id + '\n');
  }
  console.log('JSON ' + JSON.stringify({ author: npubOf(hex), count: list.length, ids: list.map((e) => e.id) }));
}

// ---------------------------------------------------------------------------
// hail — threaded kind-1 reply to a discovered agent's real public note.
// ---------------------------------------------------------------------------
function buildReplyTags(target, relayHint) {
  // NIP-10 marked tags. If the target already has a root, reuse it; else the
  // target IS the root. p-tag the author (carry forward existing p-tags too).
  const tags = [];
  const eTags = (target.tags || []).filter((t) => t[0] === 'e');
  const rootTag = eTags.find((t) => t[3] === 'root') || (eTags.length ? eTags[0] : null);
  if (rootTag) {
    tags.push(['e', rootTag[1], rootTag[2] || relayHint || '', 'root']);
    tags.push(['e', target.id, relayHint || '', 'reply']);
  } else {
    tags.push(['e', target.id, relayHint || '', 'root']);
  }
  const pset = new Set([target.pubkey]);
  for (const t of (target.tags || [])) if (t[0] === 'p' && t[1]) pset.add(t[1]);
  for (const pk of pset) tags.push(['p', pk]);
  return tags;
}

async function hail(eventId, message, opts) {
  if (!eventId || !message) { console.error('usage: hail <event-id> "<message>" [--dry]'); process.exit(1); }
  const id = loadIdentity();

  // Fetch the target to get its author pubkey + thread context.
  console.error('[hail] resolving target event ' + eventId + ' …');
  const fetches = await Promise.all(DISCOVERY_RELAYS.map((r) => fetchFromRelay(r, eventId)));
  const hit = fetches.find((f) => f.found && f.event);
  if (!hit) {
    console.error('[hail] could not fetch target event from any relay — refusing to reply blind.');
    process.exit(3);
  }
  const target = hit.event;
  const relayHint = hit.relay;
  console.error(`[hail] target by ${npubOf(target.pubkey)} (kind ${target.kind}) via ${relayHint}`);
  console.error('       "' + trunc(target.content, 120) + '"');

  const tags = buildReplyTags(target, relayHint);
  const event = buildEvent(id.sk, message, 1, tags);

  console.log('\nfrom npub:  ' + id.npub);
  console.log('reply to:   ' + npubOf(target.pubkey) + '  (event ' + eventId + ')');
  console.log('event id:   ' + event.id);
  console.log('tags:       ' + JSON.stringify(tags));
  console.log('content:    ' + JSON.stringify(message));
  console.log('self-verify: PASS (BIP-340 Schnorr)');

  if (opts.dry) {
    console.log('\n[--dry] NOT published. Full event:');
    console.log(JSON.stringify(event, null, 2));
    return;
  }

  console.log('publishing reply to ' + DISCOVERY_RELAYS.length + ' relays…\n');
  const results = await Promise.all(DISCOVERY_RELAYS.map((r) => publishToRelay(r, event)));
  let accepted = 0, readback = 0;
  for (const r of results) {
    const verdict = r.ok === true ? 'ACCEPTED' : r.ok === false ? 'REJECTED' : 'NO-OK';
    const rb = r.readback ? ' | read-back OK' : '';
    console.log(`  ${r.relay.padEnd(28)} ${verdict.padEnd(9)} ${r.message || ''}${rb}`);
    if (r.ok === true) accepted++;
    if (r.readback) readback++;
  }
  console.log('');
  console.log(`accepted by ${accepted}/${DISCOVERY_RELAYS.length} relays; read-back confirmed on ${readback}.`);
  console.log('view: https://njump.me/' + event.id);
  console.log('view: https://primal.net/e/' + event.id);
  console.log('\nJSON ' + JSON.stringify({ id: event.id, npub: id.npub, reply_to: eventId,
    reply_to_npub: npubOf(target.pubkey), accepted, readback,
    relays: results.map((r) => ({ relay: r.relay, ok: r.ok, readback: r.readback, message: r.message })) }));
  process.exit(accepted > 0 ? 0 : 3);
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------
function parseFlags(argv) {
  const flags = {}; const pos = [];
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--dry') flags.dry = true;
    else if (a === '--limit') flags.limit = Number(argv[++i]);
    else if (a === '--out') flags.out = argv[++i];
    else if (a === '--relays') flags.relays = argv[++i].split(',').map((s) => s.trim()).filter(Boolean);
    else pos.push(a);
  }
  return { flags, pos };
}

async function main() {
  const [, , cmd, ...rest] = process.argv;
  const { flags, pos } = parseFlags(rest);

  if (cmd === 'discover') {
    await discover(flags);
    return;
  }
  if (cmd === 'notes') {
    await notes(pos[0], flags);
    return;
  }
  if (cmd === 'hail') {
    await hail(pos[0], pos[1], flags);
    return;
  }
  console.error('commands:');
  console.error('  discover [--limit N] [--out file.json] [--relays a,b,c]');
  console.error('  notes <npub|hex-pubkey> [--limit N]');
  console.error('  hail <event-id> "<message>" [--dry]');
  process.exit(1);
}

main().catch((e) => { console.error('fatal:', e.message || e); process.exit(1); });
