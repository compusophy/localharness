#!/usr/bin/env bash
# Compile the relay TS modules to a throwaway CommonJS build and run the
# sponsor-relay tests (wire-format parity vs the Rust golden vectors + the
# handler-level cap checks). Run from the proxy dir: ./test/run.sh
set -euo pipefail
cd "$(dirname "$0")/.."

rm -rf .ttest
./node_modules/.bin/tsc api/sponsor.ts api/_tempo.ts api/_chain.ts api/_ratelimit.ts \
  --ignoreConfig --outDir .ttest --target es2022 --module commonjs \
  --moduleResolution node --skipLibCheck --types node --ignoreDeprecations 6.0
echo '{"type":"commonjs"}' > .ttest/package.json

node test/tempo-feepayer.mjs
node test/sponsor-handler.mjs
echo "sponsor-relay tests passed"
