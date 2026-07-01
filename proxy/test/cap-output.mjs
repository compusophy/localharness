#!/usr/bin/env node
// Regression for capOutputTokens (telemetry #38). A browser request on
// claude-opus-4-8 with byok=false 400'd with "max_tokens must be greater than
// thinking.budget_tokens": the LH_MAX_OUTPUT_TOKENS cap (default 8192) dropped
// max_tokens to/below the client's thinking budget. capOutputTokens must keep
// Anthropic's invariant (max_tokens > budget_tokens) after capping.
import assert from 'node:assert';
import mod from '../.ttest/gemini.js';

const capOutputTokens = mod.capOutputTokens ?? mod.default?.capOutputTokens;
assert.strictEqual(typeof capOutputTokens, 'function', 'capOutputTokens is exported');

// Default cap = Number(process.env.LH_MAX_OUTPUT_TOKENS ?? '8192') = 8192.

// 1. Budget ABOVE the cap (High=16384) → budget clamped below the capped
//    max_tokens, thinking preserved. This is exactly frank's failing shape.
{
  const body = { max_tokens: 20000, thinking: { type: 'enabled', budget_tokens: 16384 } };
  const changed = capOutputTokens('anthropic', body);
  assert.strictEqual(changed, true, 'a capping change is reported');
  assert.strictEqual(body.max_tokens, 8192, 'max_tokens capped to 8192');
  assert.ok(body.thinking, 'thinking is preserved when it fits');
  assert.ok(
    body.thinking.budget_tokens < body.max_tokens,
    `budget ${body.thinking.budget_tokens} must be < max_tokens ${body.max_tokens}`,
  );
  assert.ok(body.thinking.budget_tokens >= 1024, 'budget stays at/above the 1024 minimum');
}

// 2. Budget EQUAL to the cap (Medium=8192) → still fixed (8192 is not > 8192).
{
  const body = { max_tokens: 20000, thinking: { type: 'enabled', budget_tokens: 8192 } };
  capOutputTokens('anthropic', body);
  assert.ok(body.thinking.budget_tokens < body.max_tokens, 'budget==cap is clamped below max');
}

// 3. Budget already below the cap → untouched.
{
  const body = { max_tokens: 20000, thinking: { type: 'enabled', budget_tokens: 4000 } };
  capOutputTokens('anthropic', body);
  assert.strictEqual(body.thinking.budget_tokens, 4000, 'a fitting budget is left alone');
  assert.strictEqual(body.max_tokens, 8192);
}

// 4. No thinking → just the plain max_tokens cap, no thinking key invented.
{
  const body = { max_tokens: 20000 };
  capOutputTokens('anthropic', body);
  assert.strictEqual(body.max_tokens, 8192);
  assert.strictEqual(body.thinking, undefined);
}

console.log('cap-output (telemetry #38) tests passed');
