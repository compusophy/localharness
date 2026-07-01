#!/usr/bin/env bash
# Compile the proxy TS modules to a throwaway CommonJS build and run the
# server-side tests: the sponsor-relay wire-format parity (vs the Rust golden
# vectors) + cap checks, the MPP USDC.e -> $LH on-ramp lego (peg parity, MPP 402
# challenge, on-chain settlement verify, idempotent mint), and the SHARED auth-
# primitive parity (_auth.ts: personal-sign recovery + freshness + CORS rules).
# Run from the proxy dir: ./test/run.sh
set -euo pipefail
cd "$(dirname "$0")/.."

rm -rf .ttest
./node_modules/.bin/tsc api/sponsor.ts api/_tempo.ts api/_chain.ts api/_ratelimit.ts \
  api/mpp-onramp.ts api/_mpp.ts api/_stripe.ts api/_auth.ts api/_webpush.ts \
  --ignoreConfig --outDir .ttest --target es2022 --module commonjs \
  --moduleResolution node --skipLibCheck --types node --ignoreDeprecations 6.0 \
  --esModuleInterop
echo '{"type":"commonjs"}' > .ttest/package.json

node test/tempo-feepayer.mjs
node test/sponsor-handler.mjs
node test/mpp-onramp.mjs
node test/auth-parity.mjs
node test/webpush-dedupe.mjs

# gemini.ts capOutputTokens (telemetry #38): compile its import graph so the real
# exported function is testable, then assert the thinking-budget invariant.
./node_modules/.bin/tsc api/gemini.ts \
  --ignoreConfig --outDir .ttest --target es2022 --module commonjs \
  --moduleResolution node --skipLibCheck --types node --ignoreDeprecations 6.0 \
  --esModuleInterop
node test/cap-output.mjs
echo "proxy tests passed"
