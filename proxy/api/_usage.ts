// _usage.ts — token-usage metering (Option A, design/metering.md). Pure +
// unit-tested (scripts/test-metering-usage.mjs, hand-computed).
//
// WIRED, FLAG-GATED OFF: gemini.ts imports this and bills `max(flatFloor,
// usageCostWei(extractUsage(sse), MARGIN_BPS))` via a passthrough stream tee —
// but ONLY when `LH_TOKEN_METERING=1`. The flag is UNSET in prod, so the live
// debit path is still the flat per-request price and importing/typechecking this
// changes nothing live. Flipping it on needs a LIVE call per provider to confirm
// the SSE carries a usage frame (the `extractUsage` shapes below) — see the
// HUMAN-REVIEW CHECKLIST in gemini.ts at the `TOKEN_METERING` flag.
//
// SSE usage-field shape per provider (verified against the Rust backends, the
// source of truth — src/backends/{gemini,anthropic}/wire.rs):
//   gemini    — every streamed chunk carries a CUMULATIVE `usageMetadata`
//               { promptTokenCount, candidatesTokenCount, cachedContentTokenCount? };
//               the LAST chunk is final. `cachedContentTokenCount` only appears
//               when implicit/explicit context caching hits (else absent → 0).
//   anthropic — `message_start.message.usage` carries `input_tokens` +
//               `cache_read_input_tokens?` (+ a 1-4 PLACEHOLDER `output_tokens`,
//               which we IGNORE); each `message_delta.usage.output_tokens` is the
//               running CUMULATIVE total → take the LAST. Caching just shipped on
//               this path, so `cache_read_input_tokens` now flows when a turn hits
//               the prompt cache.
//   openai    — a single late chunk carries `usage` { prompt_tokens,
//               completion_tokens, prompt_tokens_details.cached_tokens? } — ONLY
//               when the request set `stream_options.include_usage` (gemini.ts
//               injects this on the OpenAI path when metering is on).
//
// Pricing source: design/metering.md (live-verified Gemini + Anthropic 2026;
// OpenAI GPT-5 family is third-party ESTIMATED — VERIFY before relying on a GPT
// margin). Model IDs + prices drift (see CLAUDE.md) — re-verify against live
// provider pricing pages periodically.

import type { Provider } from './_prices';

/** Token counts pulled from a provider's response/SSE. `cachedInput` is the
 * subset of `input` served from cache (billed at the cheaper cached rate). */
export interface Usage {
  inputTokens: number;
  outputTokens: number;
  cachedInputTokens: number;
}

/** Per-1M-token USD list rates → $LH wei per token. 1 $LH = $1 = 1e18 wei, and
 * $X per 1M tokens = X·1e18/1e6 = X·1e12 wei/token. */
function perMillion(dollarsPerMillion: number): bigint {
  return BigInt(Math.round(dollarsPerMillion * 1e12));
}

interface Rate {
  input: bigint; // wei per input token (cache-miss)
  output: bigint; // wei per output token
  cached: bigint; // wei per cached-input token
}

// Dollars per 1M tokens [input, output, cached]. Keyed by the same model ids the
// proxy fronts (see _prices.ts / src/backends wire ids).
const RATE_USD: Record<string, [number, number, number]> = {
  // Gemini (live-verified)
  'gemini-3.5-flash': [1.5, 9.0, 0.15],
  'gemini-2.5-flash': [0.3, 2.5, 0.03],
  // Anthropic (live-verified); cache-read ~0.1x input
  'claude-haiku-4-5-20251001': [1.0, 5.0, 0.1],
  'claude-sonnet-4-6': [3.0, 15.0, 0.3],
  'claude-opus-4-8': [5.0, 25.0, 0.5],
  // OpenAI GPT-5 family (ESTIMATED — verify before relying on a GPT margin)
  'gpt-5-nano': [0.2, 1.25, 0.02],
  'gpt-5-mini': [0.75, 4.5, 0.075],
  'gpt-5.1': [2.5, 15.0, 0.25],
  'gpt-5-pro': [30.0, 180.0, 3.0],
};

// Per-provider fallback when the exact model id is unknown — NEVER free (mirrors
// _prices.ts's "unknown → mid default, never free" rule). Gemini falls to flash.
const DEFAULT_USD: Record<Provider, [number, number, number]> = {
  gemini: [1.5, 9.0, 0.15],
  anthropic: [3.0, 15.0, 0.3],
  openai: [2.5, 15.0, 0.25],
};

function rateFor(provider: Provider, model: string): Rate {
  const usd = RATE_USD[model] ?? DEFAULT_USD[provider];
  return { input: perMillion(usd[0]), output: perMillion(usd[1]), cached: perMillion(usd[2]) };
}

/**
 * Real $LH-wei cost of a request from its token usage, × a margin (basis points,
 * 10000 = 1.0x). Cached input is billed at the cached rate; the rest of input at
 * the input rate; output at the output rate. The caller (the live integration)
 * applies `max(flat_floor, usageCostWei(...))` so tiny requests still pay the
 * floor. Negative/garbage counts clamp to 0. Pure.
 */
export function usageCostWei(
  provider: Provider,
  model: string,
  usage: Usage,
  marginBps: bigint,
): bigint {
  const r = rateFor(provider, model);
  const input = BigInt(Math.max(0, Math.floor(usage.inputTokens)));
  const output = BigInt(Math.max(0, Math.floor(usage.outputTokens)));
  const cached = BigInt(Math.max(0, Math.min(Math.floor(usage.cachedInputTokens), Number(input))));
  const billedInput = input - cached;
  const raw = billedInput * r.input + cached * r.cached + output * r.output;
  return (raw * marginBps) / 10_000n;
}

/**
 * The $LH-wei to DEBIT for a finished streamed response — the live integration's
 * whole amount decision in one pure, testable call:
 * `max(floorCost, usageCostWei(extractUsage(sse), marginBps))`. Falls back to the
 * flat `floorCost` when the SSE carries NO usage frame (or the client cut the
 * stream short) so a served call is never free, and never charges BELOW the floor
 * for a tiny request. Pure. Used by gemini.ts's metered passthrough (Option A).
 */
export function meteredAmountWei(
  provider: Provider,
  model: string,
  sse: string,
  floorCost: bigint,
  marginBps: bigint,
): bigint {
  const usage = extractUsage(provider, sse);
  if (!usage) return floorCost;
  const metered = usageCostWei(provider, model, usage, marginBps);
  return metered > floorCost ? metered : floorCost;
}

// --- SSE usage extraction (per provider) -----------------------------------
//
// Parse the ACCUMULATED SSE text (all `data:` frames of a streamed response)
// into a Usage. Returns null when no usage frame is present (→ the integration
// falls back to the flat floor rather than under-charging). NOTE: confirming the
// provider actually emits usage in the live SSE (OpenAI needs
// stream_options.include_usage; the integration must inject it) is the
// supervised live-verification step.

/** Pull every JSON object out of an SSE body's `data:` frames (skips `[DONE]`
 * and non-JSON keepalive lines). */
function sseJsonFrames(sse: string): Array<Record<string, unknown>> {
  const out: Array<Record<string, unknown>> = [];
  for (const line of sse.split('\n')) {
    const t = line.trim();
    if (!t.startsWith('data:')) continue;
    const payload = t.slice(5).trim();
    if (!payload || payload === '[DONE]') continue;
    try {
      const o = JSON.parse(payload);
      if (o && typeof o === 'object') out.push(o as Record<string, unknown>);
    } catch {
      /* keepalive / partial — skip */
    }
  }
  return out;
}

function num(v: unknown): number {
  return typeof v === 'number' && Number.isFinite(v) ? v : 0;
}

export function extractUsage(provider: Provider, sse: string): Usage | null {
  const frames = sseJsonFrames(sse);
  if (provider === 'gemini') {
    // Each chunk carries cumulative usageMetadata; the LAST one is final.
    let last: Record<string, unknown> | null = null;
    for (const f of frames) if (f.usageMetadata && typeof f.usageMetadata === 'object') last = f;
    if (!last) return null;
    const u = last.usageMetadata as Record<string, unknown>;
    // Gemini 3.x bills reasoning tokens at the OUTPUT rate but reports them in a
    // SEPARATE `thoughtsTokenCount` (totalTokenCount = prompt + candidates +
    // thoughts). Counting only candidatesTokenCount under-bills output by exactly
    // the thoughts (66% on thinking-heavy calls — design/metering-live-verification.md).
    return {
      inputTokens: num(u.promptTokenCount),
      outputTokens: num(u.candidatesTokenCount) + num(u.thoughtsTokenCount),
      cachedInputTokens: num(u.cachedContentTokenCount),
    };
  }
  if (provider === 'anthropic') {
    // input (+ cache) from message_start; output cumulative from the last
    // message_delta (Anthropic streams output_tokens cumulatively).
    let input = 0;
    let cached = 0;
    let output = 0;
    let sawUsage = false;
    for (const f of frames) {
      if (f.type === 'message_start') {
        const m = (f.message ?? {}) as Record<string, unknown>;
        const u = (m.usage ?? {}) as Record<string, unknown>;
        input = num(u.input_tokens);
        cached = num(u.cache_read_input_tokens);
        sawUsage = true;
      } else if (f.type === 'message_delta') {
        const u = (f.usage ?? {}) as Record<string, unknown>;
        if (typeof u.output_tokens === 'number') {
          output = num(u.output_tokens);
          sawUsage = true;
        }
      }
    }
    if (!sawUsage) return null;
    return { inputTokens: input, outputTokens: output, cachedInputTokens: cached };
  }
  // openai — a single chunk (with stream_options.include_usage) carries final usage.
  let usageFrame: Record<string, unknown> | null = null;
  for (const f of frames) if (f.usage && typeof f.usage === 'object') usageFrame = f;
  if (!usageFrame) return null;
  const u = usageFrame.usage as Record<string, unknown>;
  const details = (u.prompt_tokens_details ?? {}) as Record<string, unknown>;
  return {
    inputTokens: num(u.prompt_tokens),
    outputTokens: num(u.completion_tokens),
    cachedInputTokens: num(details.cached_tokens),
  };
}
