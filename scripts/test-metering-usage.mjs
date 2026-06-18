// Runnable proof for proxy/api/_usage.ts (Option A metering foundation).
//
// Node 20 can't run .ts directly, so this is a faithful PORT of _usage.ts's pure
// logic (the repo's .mjs parity-test pattern — cf. test-compose-wiring.mjs),
// asserted against INDEPENDENTLY hand-computed expected values. The assertions
// are the real check: a shared bug in both copies would fail a hand-computed
// assertion. Keep this in lockstep with _usage.ts. Run: node scripts/test-metering-usage.mjs

// ---- port of _usage.ts ----
function perMillion(d) { return BigInt(Math.round(d * 1e12)); }
const RATE_USD = {
  'gemini-3.5-flash': [1.5, 9.0, 0.15],
  'gemini-2.5-flash': [0.3, 2.5, 0.03],
  'claude-haiku-4-5-20251001': [1.0, 5.0, 0.1],
  'claude-sonnet-4-6': [3.0, 15.0, 0.3],
  'claude-opus-4-8': [5.0, 25.0, 0.5],
  'gpt-5-nano': [0.2, 1.25, 0.02],
  'gpt-5-mini': [0.75, 4.5, 0.075],
  'gpt-5.1': [2.5, 15.0, 0.25],
  'gpt-5-pro': [30.0, 180.0, 3.0],
};
const DEFAULT_USD = { gemini: [1.5, 9.0, 0.15], anthropic: [3.0, 15.0, 0.3], openai: [2.5, 15.0, 0.25] };
function rateFor(provider, model) {
  const usd = RATE_USD[model] ?? DEFAULT_USD[provider];
  return { input: perMillion(usd[0]), output: perMillion(usd[1]), cached: perMillion(usd[2]) };
}
function usageCostWei(provider, model, usage, marginBps) {
  const r = rateFor(provider, model);
  const input = BigInt(Math.max(0, Math.floor(usage.inputTokens)));
  const output = BigInt(Math.max(0, Math.floor(usage.outputTokens)));
  const cached = BigInt(Math.max(0, Math.min(Math.floor(usage.cachedInputTokens), Number(input))));
  const billedInput = input - cached;
  const raw = billedInput * r.input + cached * r.cached + output * r.output;
  return (raw * marginBps) / 10000n;
}
function sseJsonFrames(sse) {
  const out = [];
  for (const line of sse.split('\n')) {
    const t = line.trim();
    if (!t.startsWith('data:')) continue;
    const payload = t.slice(5).trim();
    if (!payload || payload === '[DONE]') continue;
    try { const o = JSON.parse(payload); if (o && typeof o === 'object') out.push(o); } catch { /* skip */ }
  }
  return out;
}
function num(v) { return typeof v === 'number' && Number.isFinite(v) ? v : 0; }
function extractUsage(provider, sse) {
  const frames = sseJsonFrames(sse);
  if (provider === 'gemini') {
    let last = null;
    for (const f of frames) if (f.usageMetadata && typeof f.usageMetadata === 'object') last = f;
    if (!last) return null;
    const u = last.usageMetadata;
    return { inputTokens: num(u.promptTokenCount), outputTokens: num(u.candidatesTokenCount), cachedInputTokens: num(u.cachedContentTokenCount) };
  }
  if (provider === 'anthropic') {
    let input = 0, cached = 0, output = 0, sawUsage = false;
    for (const f of frames) {
      if (f.type === 'message_start') {
        const u = (f.message ?? {}).usage ?? {};
        input = num(u.input_tokens); cached = num(u.cache_read_input_tokens); sawUsage = true;
      } else if (f.type === 'message_delta') {
        const u = f.usage ?? {};
        if (typeof u.output_tokens === 'number') { output = num(u.output_tokens); sawUsage = true; }
      }
    }
    if (!sawUsage) return null;
    return { inputTokens: input, outputTokens: output, cachedInputTokens: cached };
  }
  let usageFrame = null;
  for (const f of frames) if (f.usage && typeof f.usage === 'object') usageFrame = f;
  if (!usageFrame) return null;
  const u = usageFrame.usage;
  const d = u.prompt_tokens_details ?? {};
  return { inputTokens: num(u.prompt_tokens), outputTokens: num(u.completion_tokens), cachedInputTokens: num(d.cached_tokens) };
}
// port of meteredAmountWei: max(floorCost, usageCostWei(extractUsage(sse))), floor on no-usage.
function meteredAmountWei(provider, model, sse, floorCost, marginBps) {
  const usage = extractUsage(provider, sse);
  if (!usage) return floorCost;
  const metered = usageCostWei(provider, model, usage, marginBps);
  return metered > floorCost ? metered : floorCost;
}

// ---- assertions (hand-computed) ----
let fails = 0;
function eq(label, got, want) {
  const g = JSON.stringify(got, (_, v) => (typeof v === 'bigint' ? v.toString() : v));
  const w = JSON.stringify(want, (_, v) => (typeof v === 'bigint' ? v.toString() : v));
  if (g !== w) { console.error(`FAIL ${label}\n  got  ${g}\n  want ${w}`); fails++; }
  else console.log(`ok   ${label}`);
}

// usageCostWei — hand-computed:
// flash 1000in/500out @1.3x = (1000*1.5e12 + 500*9e12)*1.3 = 6e15*1.3 = 7.8e15
eq('cost flash 1000/500 x1.3', usageCostWei('gemini', 'gemini-3.5-flash', { inputTokens: 1000, outputTokens: 500, cachedInputTokens: 0 }, 13000n), 7800000000000000n);
// flash 1000in(400 cached)/0out @1.0x = 600*1.5e12 + 400*1.5e11 = 9e14 + 6e13 = 9.6e14
eq('cost flash cached x1.0', usageCostWei('gemini', 'gemini-3.5-flash', { inputTokens: 1000, outputTokens: 0, cachedInputTokens: 400 }, 10000n), 960000000000000n);
// opus 2000in/1000out @1.3x = (2000*5e12 + 1000*2.5e13)*1.3 = 3.5e16*1.3 = 4.55e16
eq('cost opus 2000/1000 x1.3', usageCostWei('anthropic', 'claude-opus-4-8', { inputTokens: 2000, outputTokens: 1000, cachedInputTokens: 0 }, 13000n), 45500000000000000n);
// unknown anthropic → default $3/1M in: 1000*3e12 = 3e15
eq('cost unknown anthropic fallback', usageCostWei('anthropic', 'claude-future-9', { inputTokens: 1000, outputTokens: 0, cachedInputTokens: 0 }, 10000n), 3000000000000000n);
// clamp: negative/over-cached → 0/clamped (no throw, no negative)
eq('cost clamps junk to 0', usageCostWei('gemini', 'gemini-3.5-flash', { inputTokens: -5, outputTokens: -1, cachedInputTokens: 999 }, 10000n), 0n);

// --- cached-input DISCOUNT (the prompt-caching savings must flow into billing) ---
// opus rates: in=5e12, out=25e12, cached=5e11 wei/tok.
// (A) 10000in (8000 cached)/2000out @1.0x: billed = 2000*5e12 + 8000*5e11 + 2000*25e12
//     = 1e16 + 4e15 + 5e16 = 6.4e16.
eq('cost opus cached-discount x1.0', usageCostWei('anthropic', 'claude-opus-4-8', { inputTokens: 10000, outputTokens: 2000, cachedInputTokens: 8000 }, 10000n), 64000000000000000n);
// (B) SAME tokens, NO cache: 10000*5e12 + 2000*25e12 = 5e16 + 5e16 = 1e17. The
//     delta vs (A) is the discount = 8000*(5e12-5e11) = 3.6e16 — proving the
//     cheaper cached rate is applied to exactly the cached subset.
eq('cost opus no-cache (discount baseline)', usageCostWei('anthropic', 'claude-opus-4-8', { inputTokens: 10000, outputTokens: 2000, cachedInputTokens: 0 }, 10000n), 100000000000000000n);
const discount = usageCostWei('anthropic', 'claude-opus-4-8', { inputTokens: 10000, outputTokens: 2000, cachedInputTokens: 0 }, 10000n)
  - usageCostWei('anthropic', 'claude-opus-4-8', { inputTokens: 10000, outputTokens: 2000, cachedInputTokens: 8000 }, 10000n);
eq('cached discount == 8000*(in-cached)', discount, 36000000000000000n);
// (C) FULLY cached input (every input token a cache hit) → only the cheap rate
//     applies to input. flash: 1000 cached/0out @1.0x = 1000*1.5e11 = 1.5e14.
eq('cost flash fully-cached x1.0', usageCostWei('gemini', 'gemini-3.5-flash', { inputTokens: 1000, outputTokens: 0, cachedInputTokens: 1000 }, 10000n), 150000000000000n);
// (D) cachedInput > input is clamped to input (cached can't exceed prompt); with
//     real output it doesn't zero the whole charge. flash 500in(clamped 500)/100out
//     @1.0x = 0*1.5e12 + 500*1.5e11 + 100*9e12 = 7.5e13 + 9e14 = 9.75e14.
eq('cost flash over-cached clamps to input', usageCostWei('gemini', 'gemini-3.5-flash', { inputTokens: 500, outputTokens: 100, cachedInputTokens: 900 }, 10000n), 975000000000000n);
// (E) margin applies AFTER the cached split: (A) at 1.3x = 6.4e16*1.3 = 8.32e16.
eq('cost opus cached-discount x1.3', usageCostWei('anthropic', 'claude-opus-4-8', { inputTokens: 10000, outputTokens: 2000, cachedInputTokens: 8000 }, 13000n), 83200000000000000n);

// extractUsage — gemini (last cumulative chunk wins)
const GEM = 'data: {"usageMetadata":{"promptTokenCount":1000,"candidatesTokenCount":200,"totalTokenCount":1200}}\n\ndata: {"usageMetadata":{"promptTokenCount":1000,"candidatesTokenCount":500,"cachedContentTokenCount":300,"totalTokenCount":1500}}\n';
eq('extract gemini', extractUsage('gemini', GEM), { inputTokens: 1000, outputTokens: 500, cachedInputTokens: 300 });
// anthropic — input from message_start, cumulative output from last message_delta
const ANT = 'event: message_start\ndata: {"type":"message_start","message":{"usage":{"input_tokens":1200,"cache_read_input_tokens":200}}}\n\nevent: message_delta\ndata: {"type":"message_delta","usage":{"output_tokens":50}}\n\nevent: message_delta\ndata: {"type":"message_delta","usage":{"output_tokens":420}}\n';
eq('extract anthropic', extractUsage('anthropic', ANT), { inputTokens: 1200, outputTokens: 420, cachedInputTokens: 200 });
// openai — usage chunk + [DONE]
const OAI = 'data: {"choices":[{"delta":{"content":"hi"}}]}\n\ndata: {"choices":[],"usage":{"prompt_tokens":800,"completion_tokens":150,"prompt_tokens_details":{"cached_tokens":100}}}\n\ndata: [DONE]\n';
eq('extract openai', extractUsage('openai', OAI), { inputTokens: 800, outputTokens: 150, cachedInputTokens: 100 });
// null when no usage frame present (→ integration falls back to the flat floor)
eq('extract null when absent', extractUsage('gemini', 'data: {"candidates":[]}\n'), null);

// meteredAmountWei — floor fallback vs metered-above-floor (the live debit amount)
const FLOOR = 10000000000000000n; // 0.01 $LH flat floor (priceOf default)
// usage present + above floor → the metered amount (flash 1000/500 x1.3 = 7.8e15 < floor → floor!)
eq('amount small-usage hits floor', meteredAmountWei('gemini', 'gemini-3.5-flash', GEM, FLOOR, 13000n), 10000000000000000n);
// a big opus response blows past the floor → charge actual (2000in/1000out x1.3 = 4.55e16)
const BIGOPUS = 'event: message_start\ndata: {"type":"message_start","message":{"usage":{"input_tokens":2000}}}\n\nevent: message_delta\ndata: {"type":"message_delta","usage":{"output_tokens":1000}}\n';
eq('amount big-usage above floor', meteredAmountWei('anthropic', 'claude-opus-4-8', BIGOPUS, FLOOR, 13000n), 45500000000000000n);
// no usage frame in the SSE → fall back to the flat floor (never free)
eq('amount no-usage → floor', meteredAmountWei('gemini', 'gemini-3.5-flash', 'data: {"candidates":[]}\n', FLOOR, 13000n), FLOOR);
// END-TO-END cached discount: an anthropic SSE that reports cache_read_input_tokens
// must bill the cheaper cached rate via the SSE → extractUsage → usageCostWei
// path. opus 10000in (8000 cache_read)/2000out @1.3x = 6.4e16*1.3 = 8.32e16 (the
// same as the unit case (E) — confirms the cache field survives extraction).
const CACHED_SSE = 'event: message_start\ndata: {"type":"message_start","message":{"usage":{"input_tokens":10000,"cache_read_input_tokens":8000,"output_tokens":1}}}\n\nevent: message_delta\ndata: {"type":"message_delta","usage":{"output_tokens":2000}}\n';
eq('amount cached SSE bills cached rate', meteredAmountWei('anthropic', 'claude-opus-4-8', CACHED_SSE, FLOOR, 13000n), 83200000000000000n);
// the SAME SSE WITHOUT the cache field bills full input → strictly MORE (the
// discount is observable end-to-end, not just in the unit math).
const UNCACHED_SSE = 'event: message_start\ndata: {"type":"message_start","message":{"usage":{"input_tokens":10000,"output_tokens":1}}}\n\nevent: message_delta\ndata: {"type":"message_delta","usage":{"output_tokens":2000}}\n';
eq('amount uncached SSE bills full input', meteredAmountWei('anthropic', 'claude-opus-4-8', UNCACHED_SSE, FLOOR, 13000n), 130000000000000000n);

// --- passthrough plumbing: the metered TransformStream streams bytes VERBATIM
//     and computes the right debit on flush (mirrors gemini.ts meteredBody, sans
//     the on-chain debit — the amount is captured instead). ------------------
async function passthroughProof() {
  const provider = 'anthropic', model = 'claude-opus-4-8';
  const decoder = new TextDecoder();
  let acc = '', captured = null;
  const transform = new TransformStream({
    transform(chunk, controller) { controller.enqueue(chunk); acc += decoder.decode(chunk, { stream: true }); },
    flush() { captured = meteredAmountWei(provider, model, acc, FLOOR, 13000n); },
  });
  // Feed BIGOPUS split across arbitrary chunk boundaries (incl. mid-frame).
  const enc = new TextEncoder();
  const bytes = enc.encode(BIGOPUS);
  const src = new ReadableStream({
    start(c) { for (let i = 0; i < bytes.length; i += 7) c.enqueue(bytes.slice(i, i + 7)); c.close(); },
  });
  const outChunks = [];
  const reader = src.pipeThrough(transform).getReader();
  for (;;) { const { done, value } = await reader.read(); if (done) break; outChunks.push(value); }
  const out = new Uint8Array(outChunks.reduce((n, c) => n + c.length, 0));
  let off = 0; for (const c of outChunks) { out.set(c, off); off += c.length; }
  eq('passthrough bytes verbatim', new TextDecoder().decode(out), BIGOPUS);
  eq('passthrough flush amount', captured, 45500000000000000n);
}
await passthroughProof();

if (fails) { console.error(`\n${fails} assertion(s) FAILED`); process.exit(1); }
console.log('\nall metering-usage assertions passed');
