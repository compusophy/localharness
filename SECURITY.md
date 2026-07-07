# Security policy

localharness custodies seeds and moves real money (`$LH`, sponsored gas). Read
this before reporting, and read the trust model before assuming a bug: the
security boundary is deliberately the host device, and keys are deliberately
plaintext — see "Key custody and trust model" in
[localharness.xyz/llms.txt](https://localharness.xyz/llms.txt).

## Supported versions

Pre-1.0. Only the **latest published minor** (`0.x`) receives security fixes.
Older minors are not patched; upgrade.

## Reporting a vulnerability

Use **GitHub private vulnerability reporting** on
[compusophy/localharness](https://github.com/compusophy/localharness/security/advisories/new)
(enabled). Do not open a public issue with exploit details. If private
reporting is unavailable to you, open a plain issue that says only "security
report, need a private contact" — no details — and a maintainer will follow up.

There is **no bug bounty program** pre-1.0. Reports are triaged best-effort by
a solo maintainer; expect an acknowledgment within a few days, not an SLA.

## Scope

Security-sensitive surfaces (in scope):

- **Self-custody keys.** Identity seeds/keys are stored as plaintext by design
  (`~/.localharness/keys/<name>.localharness.key` on the CLI; `.lh_wallet` in
  browser OPFS, exempt from at-rest encryption because it IS the key root).
  "The key file is readable by local processes" is the documented trust model,
  not a vulnerability. A path that leaks a key **off the device** — over the
  wire, into logs, to another origin — is very much in scope.
- **The credit proxy** (`proxy/`): personal-sign auth, `$LH` metering, and the
  x402 settle path. Bugs that let a caller stream inference without being
  metered, or settle without a valid signature, are in scope.
- **The sponsor relay** (`proxy/api/sponsor.ts`): the mainnet fee_payer signs
  server-side behind a selector allowlist, per-address rate window, gas cap,
  and a float circuit-breaker. Any way to make the relay sign outside those
  gates — or to spend the sponsor's fee-token float on non-gas value — is the
  highest-severity class here.
- **x402 signatures** (X402Facet): replay past the one-shot nonce, ecrecover /
  EIP-1271 confusion, or bypassing the price-lock ceiling.
- **The diamond owner key** (EIP-173 `owner()` / `diamondCut`): anything that
  lets a non-owner cut facets or reach owner-only selectors.
- **wasm bundle integrity** (`web/`): the browser app is a static Vercel site;
  tampering paths (cache poisoning, CSP bypass, cross-origin postMessage abuse
  of the `?signer=1` / `?rpc=1` protocols) are in scope.

Out of scope:

- The **Moderato testnet** deployment and its embedded low-budget testnet
  sponsor key (loss capped at its balance, by design).
- The **QA fleet personas** (`scripts/test-fleet/`) and their throwaway keys.
- Local-attacker reads of plaintext key files (the documented custody model).
- Social engineering, physical access, denial of service by paying for it.

## Incident response runbook

Per key class, in the order an incident would demand:

- **Sponsor relay key** (mainnet fee_payer, proxy env only — never in a build):
  generate a new key, move the remaining fee-token float to it, set
  `LH_SPONSOR_KEY` in the proxy's Vercel env, redeploy the proxy
  (`cd proxy && vercel --prod`). The old key holds nothing but gas float, so
  exposure is capped at whatever float it still had. This rotation has been
  done once already (the bundle-exposed testnet-era key was retired).
- **Deployer / diamond owner key**: `transferOwnership` (EIP-173) on the
  diamond to a fresh key immediately; that key gates `diamondCut` and all
  owner-only facet methods, so it is the single most valuable key in the
  system. Then rotate any proxy env or script that referenced it.
- **User seeds**: there is no server-side revocation — the remedy is the
  NFT-transfer race documented in the trust model (llms.txt): transfer the
  identity NFT to a secure wallet, which revokes all enrolled device signers.
  It only works if the owner moves before the thief does.
- **Float drain in progress**: the relay's circuit-breaker
  (`LH_RELAY_MIN_FLOAT_WEI`) refuses to sign once the sponsor's fee-token
  balance drops below the floor, bounding a drain automatically. Response:
  let it trip (or set the floor high to halt sponsorship outright), rotate the
  sponsor key as above, then audit the selector allowlist and rate window in
  `proxy/api/sponsor.ts` before refunding the float.

Disclosure after a fix is coordinated with the reporter; pre-1.0 the changelog
entry is the advisory.
