# Paymaster — our own, not Tempo's

Tracks the user ask "we need our own paymaster not using theirs."

## Why our own

Tempo's `tempo_fundAddress` is a public faucet — drips test ETH to any
EOA on request. Adequate for testnet, but:

1. **It's their infra.** Mainnet equivalent won't exist; depending on
   them for gas means depending on them for liveness.
2. **No policy.** Anyone can drip; nothing ties drips to our users
   or our token. We can't gate, throttle, or charge in $LH.
3. **No accounting.** A real paymaster integrates with the MAIN's
   economic substrate ($LH balance → gas credits → tx sponsored).
   The faucet has no such hook.

We want a contract we deploy + control, with policy we can iterate on,
denominated in $LH so the agent economy is self-contained.

## Goals (v1)

- A user with a MAIN can act on-chain without holding native TMP gas.
- The MAIN's $LH balance (held at the MAIN's TBA) pays for gas in
  proportion to the gas burned.
- The paymaster is a single contract — added as a facet to the
  diamond, or standalone with a configured allowlist.
- The mechanism survives a tempo_fundAddress shutdown: we have our
  own native-gas reserve OR we use Tempo's reserve via a hardened
  reimbursement loop, but the USER experience doesn't change.

## Goals (v2, not v1)

- ERC-4337 integration so any 4337 wallet (not just our MAIN TBAs)
  can use this paymaster.
- Session keys: time-limited / scope-limited delegated signers paid
  for by the paymaster.
- $LH↔gas price oracle so gas charges accurately reflect native
  costs.
- Pre-funded gas tickets (user pays $LH up front, gets a quota).

## Architecture options

### Option A — Trusted-Forwarder relayer (EIP-2771)

```
[user device] --signs meta-tx-->
[relayer EOA] --submits-->
[Paymaster.executeMeta(metaTx)] --validates + calls-->
[target] --reads _msgSender() = user via ERC2771Context-->
```

- Off-chain relayer (Vercel serverless / dedicated process) holds a
  hot EOA pre-funded from `tempo_fundAddress`. Relayer signs the
  outer tx and pays native gas.
- Paymaster contract validates the user's meta-tx signature, calls
  the target, and reimburses the relayer in $LH at the end of the
  call.
- Every contract that wants to support sponsorship has to use
  `_msgSender()` instead of `msg.sender`. Our facets currently use
  `msg.sender` directly; migration is mechanical but touches every
  facet.

Pros: works without 4337 / bundler infra. Conceptually simple.
Cons: meta-tx replay protection per nonce; the relayer is a piece
of off-chain infra to operate; the `_msgSender` retrofit is a
diamond-wide diff.

### Option B — 4337 EntryPoint + Paymaster

```
[user MAIN smart wallet] --signs UserOperation-->
[bundler off-chain] --submits via-->
[EntryPoint.handleOps([userOp])] --validates with-->
[Paymaster.validatePaymasterUserOp + postOp]
```

- Requires deploying an EntryPoint (or using a canonical one if Tempo
  has it) + writing a Paymaster that conforms to the 4337
  IPaymaster interface.
- Bundler is off-chain infra; we'd need one.
- The MAIN must be a 4337 smart wallet (matches the `MultiSignerAccount`
  direction from main-identity.md).

Pros: standard pattern, ecosystem support, scales well.
Cons: largest engineering surface; bundler + paymaster + smart
account all need to land together for v1 to be useful.

### Option C — On-chain reimbursement (no off-chain relayer)

The MAIN's TBA wraps every action it takes in a call to the
Paymaster, which:
1. Records gasleft() at entry.
2. Executes the user's intent via `target.call(data)`.
3. Computes gas used; debits the TBA's $LH balance at a fixed rate.
4. Transfers the equivalent native to a hot reserve.

This still requires the caller to have SOME native gas (to pay the
initial tx). It's a $LH-denominated post-pay model, not true gas
sponsorship — but it's CHEAPER than running a relayer + works
without 4337 infra.

Pros: no off-chain relayer; no 4337 dependency; doable now.
Cons: doesn't solve the "I have ZERO native" problem — the caller
still has to bootstrap with some native to send the first tx. The
$LH debit is for accounting / monetization, not gas sponsorship.

## Recommendation

**Ship Option C for v1, plan Option B for v2.**

C is the minimal step that's compatible with what we already have:
- Single facet (`PaymasterFacet`) cut into the existing diamond
- Acts as the always-on intermediary for MAIN TBA actions
- Charges $LH at a fixed rate per gas unit (adjustable by owner)
- Uses Tempo's faucet for the underlying native gas during testnet —
  the contract holds a small native reserve, draws from
  `tempo_fundAddress` periodically (or accepts native deposits from
  anyone)

C is honest about what it is: a $LH-denominated accounting layer,
not a sponsor. When Option B lands, it's an upgrade — the EntryPoint
becomes the integration point, and the existing PaymasterFacet's
accounting carries over.

## What lands in v1

```solidity
// contracts/src/facets/PaymasterFacet.sol
contract PaymasterFacet {
    event Sponsored(address indexed payer, uint256 gasUsed, uint256 lhCharged);

    // Owner-only config knobs:
    function setLhPerGas(uint256 wei) external onlyOwner;
    function setMinReserveWei(uint256 wei) external onlyOwner;

    // The one-shot path the MAIN TBA calls:
    function executeWithSponsorship(
        address target,
        uint256 value,
        bytes calldata data
    ) external returns (bytes memory result);
}
```

Storage in `LibPaymasterStorage`:
- `uint256 lhPerGas` — exchange rate; charge = gasUsed * lhPerGas
- `uint256 minReserveWei` — when the native reserve falls below this,
  the contract auto-drips from the chain's faucet (testnet-only)
- `mapping(address => uint256) lhBalance` — per-user pre-paid $LH if
  we want a deposit model later

## What lands in v2

- `EntryPoint` + bundler (run our own or use Pimlico-equivalent)
- `MultiSignerAccount` becomes 4337-compatible (validateUserOp +
  validateNonce)
- Paymaster surface implements `IPaymaster` for EntryPoint
- Off-chain bundler service (TS or Rust on Vercel/fly.io)

## Open questions

- **$LH↔native exchange rate.** Hard-code, oracle, or
  governance-tunable? Probably governance-tunable v1, oracle v2.
- **Reserve top-up.** Testnet: `tempo_fundAddress` periodically.
  Mainnet: needs a real source of native ETH for the contract.
- **Per-user quotas.** Free for the first N gas units / month for a
  MAIN? Necessary to bootstrap; necessary to bound abuse.

## Where this lives in the code

- `contracts/src/facets/PaymasterFacet.sol` — v1 facet (sketch
  above; not deployed yet).
- `contracts/src/libraries/LibPaymasterStorage.sol` — storage slot.
- `src/registry.rs::execute_with_sponsorship` — bundle helper that
  routes a tx through the paymaster instead of a plain
  `eth_sendRawTransaction`.

## Decision needed before code lands

I want pushback on Option C as the v1. It's cheaper but it's not
*true* sponsorship — the user still needs initial native gas. If
the goal is "user holds zero TMP and still operates", Option A or B
is the actual answer; C is just an accounting/monetization layer.

If true zero-native operation is the goal, the path is A → B over
two commits: build the meta-tx relayer (off-chain piece on Vercel),
then graduate to 4337 once `MultiSignerAccount` lands.
