// _prices.ts — the per-model $LH price table: the "meter" between the fiat
// on-ramp (USD -> $LH) and x402 spend ($LH -> inference). Single source of
// truth for the gate (gemini.ts) and the read-only GET /prices route
// (prices.ts). All values env-overridable; UNSET = today's defaults, so the
// proxy is byte-for-byte unchanged.

export type Provider = 'gemini' | 'anthropic' | 'openai';

function envWei(name: string, def: bigint): bigint {
  try {
    const v = process.env[name];
    return v ? BigInt(v) : def;
  } catch {
    return def;
  }
}

// `$LH` (18-decimal wei) per Gemini request — FLAT (byte-identical to the prior
// COST_PER_REQUEST_WEI), so Gemini pricing is unchanged. Default 0.01 $LH.
export const COST_PER_REQUEST_WEI = envWei('COST_PER_REQUEST_WEI', 10_000_000_000_000_000n);

// Anthropic is per-model; an UNKNOWN model falls to a mid price, NEVER free (so
// a caller can't request an unpriced model to dodge the meter).
const PRICE_ANTHROPIC: Record<string, bigint> = {
  'claude-haiku-4-5-20251001': envWei('PRICE_ANTHROPIC_HAIKU_WEI', 10_000_000_000_000_000n), // 0.01
  'claude-sonnet-4-6': envWei('PRICE_ANTHROPIC_SONNET_WEI', 50_000_000_000_000_000n), // 0.05
  'claude-opus-4-8': envWei('PRICE_ANTHROPIC_OPUS_WEI', 200_000_000_000_000_000n), // 0.20
};
const PRICE_ANTHROPIC_DEFAULT = envWei('PRICE_ANTHROPIC_DEFAULT_WEI', 50_000_000_000_000_000n);

// OpenAI mirrors the Anthropic tiers; same UNKNOWN -> mid default, never free.
const PRICE_OPENAI: Record<string, bigint> = {
  'gpt-5-nano': envWei('PRICE_OPENAI_NANO_WEI', 10_000_000_000_000_000n), // 0.01
  'gpt-5-mini': envWei('PRICE_OPENAI_MINI_WEI', 10_000_000_000_000_000n), // 0.01
  'gpt-5.1': envWei('PRICE_OPENAI_FLAGSHIP_WEI', 50_000_000_000_000_000n), // 0.05
  'gpt-5-pro': envWei('PRICE_OPENAI_PRO_WEI', 200_000_000_000_000_000n), // 0.20
};
const PRICE_OPENAI_DEFAULT = envWei('PRICE_OPENAI_DEFAULT_WEI', 50_000_000_000_000_000n);

// Hard per-request ceiling: a misconfigured price env (an extra zero) must never
// debit an absurd amount in one shot. Anything above is clamped DOWN to this.
export const MAX_COST_PER_REQUEST_WEI = envWei('MAX_COST_PER_REQUEST_WEI', 1_000_000_000_000_000_000n);

export function priceOf(provider: Provider, model: string): bigint {
  const raw =
    provider === 'gemini'
      ? COST_PER_REQUEST_WEI
      : provider === 'anthropic'
        ? (PRICE_ANTHROPIC[model] ?? PRICE_ANTHROPIC_DEFAULT)
        : (PRICE_OPENAI[model] ?? PRICE_OPENAI_DEFAULT);
  return raw > MAX_COST_PER_REQUEST_WEI ? MAX_COST_PER_REQUEST_WEI : raw;
}

/** The full advertised price table for GET /prices — wei as decimal strings.
 * `*` is the per-provider fallback an unknown model is charged. */
export function priceTable(): Array<{ provider: Provider; model: string; price_wei: string }> {
  const rows: Array<{ provider: Provider; model: string; price_wei: string }> = [
    { provider: 'gemini', model: '*', price_wei: priceOf('gemini', '').toString() },
  ];
  for (const m of Object.keys(PRICE_ANTHROPIC)) {
    rows.push({ provider: 'anthropic', model: m, price_wei: priceOf('anthropic', m).toString() });
  }
  rows.push({ provider: 'anthropic', model: '*', price_wei: PRICE_ANTHROPIC_DEFAULT.toString() });
  for (const m of Object.keys(PRICE_OPENAI)) {
    rows.push({ provider: 'openai', model: m, price_wei: priceOf('openai', m).toString() });
  }
  rows.push({ provider: 'openai', model: '*', price_wei: PRICE_OPENAI_DEFAULT.toString() });
  return rows;
}
