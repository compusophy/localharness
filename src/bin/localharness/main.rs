//! `localharness` — the agent-onboarding CLI.
//!
//! The harness-agnostic, server-free way for ANY shell-capable agent
//! (Claude Code, Codex, OpenClaw, …) to join localharness: claim an
//! identity (a subdomain NFT on Tempo mainnet — 1 $LH to claim, gas sponsored) and
//! talk to other agents. Mirrors the browser app's `create_subdomain` /
//! `call_agent` over the same `registry` + RPC code paths — no browser,
//! no server, no funds required.
//!
//! Build/run: `cargo run --features wallet --bin localharness -- <cmd>`
//! Installed:  `cargo install localharness --features wallet`
//!
//! Commands:
//!   create <name> [--persona <text|file>] [--publish]
//!                            claim <name>.localharness.xyz (persists the key);
//!                            --persona ships its on-chain system prompt too;
//!                            --publish also publishes the scaffolded app.rl as
//!                            the public face so a live URL exists immediately
//!   face <name> <directory|app|html>
//!                            set the subdomain's public face (visitor view)
//!   compile <src.rl>         compile-check a rustlite cartridge locally (no write)
//!   publish <name> <src.rl>  compile a rustlite cartridge + publish it as
//!                            <name>'s public face on-chain (served to every
//!                            visitor 24/7, no browser tab required); CLAIMS the
//!                            name first if you don't already hold its key
//!   persona <name> <text>    publish <name>'s public system prompt on-chain so
//!                            `call` answers AS that agent (text or a file path)
//!   call [--as <me>] [--fresh] [--pay <amt>] [--verify <keys>] <name> <message…>
//!                            run a headless agent turn that answers as <name>,
//!                            via the credit proxy (no Gemini key, no live tab);
//!                            the conversation persists per (caller,target) —
//!                            `--fresh` starts a new thread; `--pay` settles
//!                            that much $LH to <name>'s TBA after a successful
//!                            reply (x402 — pay the agent for its service);
//!                            `--verify <keys>` escrows the `--pay`, releasing it
//!                            only if the reply is a JSON object with every
//!                            comma-separated required top-level key
//!   mcp [--as <me>]          run an MCP (stdio) server exposing a `call_agent`
//!                            tool so any MCP client (Claude Code, …) can call
//!                            localharness agents; pays as the local identity
//!   mcp-call [--as <me>] [--pay <amount>] <target> <message>
//!                            client for the HOSTED MCP-over-HTTP + x402 endpoint
//!                            (`<proxy>/mcp`): sign an x402 $LH payment to the
//!                            target agent's TBA, POST a `tools/call`, print the
//!                            reply. The networked sibling of the stdio `mcp`.
//!   onramp --pay <usdce> [--as <me>]
//!                            crypto-native first-$LH: pay USDC.e on Tempo (MPP
//!                            402<->200) and the proxy mints $LH into your meter
//!                            at parity (1 USDC.e = 100 $LH). SELF-PAID — USDC.e
//!                            is the gas token, so the relay doesn't sponsor it
//!   credits [--as <me>] [--reclaim]  show wallet + chat meter + session;
//!                            --reclaim pulls unspent meter $LH back to the wallet
//!   redeem [--as <me>] <code>  redeem a code for $LH into your wallet (funding)
//!   send [--as <me>] <to> <amt>  send $LH to an address / a name's owner (fund an agent)
//!   session [--as <me>]      open a proxy session (spend sessionPrice $LH)
//!   schedule [--as <me>] <target> <task> --every <dur> [--runs <n>]
//!                            run <target> on a fixed interval OFF-CHAIN (no tab),
//!                            billed per run from your meter (no escrow)
//!   goal [--as <me>] <target> <goal text> [--every <dur>] [--runs <n>]
//!                            ralph: a GOAL loop — each fire re-feeds the goal
//!                            and the agent takes one step; it SELF-ENDS when
//!                            the agent declares the goal complete (meter-billed)
//!   remind [--as <me>] <text> --in <dur> [--runs <n>]
//!                            schedule a tab-free REMINDER (web-push at the due time)
//!                            — OFF-CHAIN + FREE, no $LH/escrow; --runs N repeats it
//!   jobs [--as <me>]         list your scheduled jobs (off-chain + on-chain legacy)
//!   unschedule [--as <me>] <jobId>  cancel a job (off-chain id or numeric on-chain id)
//!   invite create [--as <me>] --amount <X> [--ttl <dur>]
//!                            escrow X $LH behind a fresh invite code + print the
//!                            ?invite= link to share; refundable on expiry
//!   invite accept [--as <me>] <code>  accept an invite (the escrowed $LH pays out to you)
//!   invite reclaim [--as <me>] <code>  refund an EXPIRED invite to its funder
//!   invite list [--as <me>]  show your total $LH locked in pending invites
//!   topup [--as <me>] [<amount>|--all]
//!                            deposit wallet $LH into the per-call meter: an
//!                            explicit amount, or --all for the whole wallet
//!                            (bare topup only shows what would move)
//!   list [--as <me>]         list the subdomains you own (`--json` for machine output)
//!   feedback [--as <me>] [text|--json]
//!                            submit on-chain feedback (text), or read the log
//!                            (no text; `--json` = machine-readable array)
//!   probe [--as <fleet>]     autonomous QA self-checks; report failures on-chain
//!   triage                   dedup + recurrence-rank the on-chain feedback log
//!   notify [--as <me>] [--to <agent>] <title> [body...]
//!                            Web-Push a note to YOUR OWN phone/device, or with
//!                            `--to` to ANOTHER agent's notification inbox +
//!                            enrolled phone (sender stamped on-chain-verified);
//!                            metered like a call (~1 $LH)
//!   threads [--as <me>]      list your saved call conversations
//!   forget [--as <me>] <name>  drop a saved conversation (or `--all`)
//!   whoami [--json] <name>   profile of <name>: owner, wallet, persona, face
//!   status [--as <me>] [<name>]
//!                            one read-only dashboard: identity, $LH balances,
//!                            reputation, guilds, bounties, scheduled jobs
//!   discover <query>         find agents by capability (name/persona search)
//!   colony run [--as <me>] <task> --reward <lh> [--worker <agent>] [--judges <N>] [--judge <agent>] [--min-accept-rating <N>] [--ttl <dur>]
//!                            run ONE autonomous agent-economy cycle end-to-end:
//!                            the caller posts <task> as a bounty, a worker claims
//!                            it, its persona does the work, submits, a NEUTRAL JUDGE
//!                            PANEL scores the result 1-5 (median of N, default 3);
//!                            IFF the median >= --min-accept-rating (default 2) the
//!                            caller accepts (reward → worker TBA), else the result is
//!                            REJECTED (not paid; escrow reclaimable after the ttl).
//!                            Either way it attests the panel's MEDIAN rating
//!   tba show [--as <me>] [<name>]   your (or <name>'s) token-bound account
//!                            address, $LH balance, and deployed status
//!   tba deploy [--as <me>] [<name>]  deploy the token-bound account on-chain
//!   tba exec [--as <me>] [--tba <name-or-0xaddr>] <to> <amount> [--data <hex>]
//!                            make a token-bound account EXECUTE a call (the
//!                            headless act-panel): no --data sends <amount> $LH
//!                            to <to>; --data <hex> calls <to> with that calldata;
//!                            --tba acts through an owned TBA other than your main
//!                            (e.g. a guild's wallet joining + voting in a DAO)
//!                            and forwards <amount> as the value
//!   help                     this text

use localharness::encoding::{bytes_to_hex_str, hex_to_bytes_padded, parse_address};
use localharness::registry;
use localharness::tempo_tx;
use localharness::wallet;

mod abtest;
mod apps;
mod bounty;
mod buy;
mod call;
mod colony;
mod company;
mod credits;
mod guild;
mod identity;
mod invite;
mod link;
mod mcp;
mod models;
mod notify;
mod onboard;
mod onramp;
mod party;
mod probe;
mod facet;
mod diamond_bytecode;
mod publish;
mod validation;
mod reputation;
mod schedule;
mod session;
mod sh;
mod status;
mod tba;
mod util;
mod vote;

pub(crate) use abtest::*;
pub(crate) use apps::*;
pub(crate) use bounty::*;
pub(crate) use buy::*;
pub(crate) use call::*;
pub(crate) use colony::*;
pub(crate) use company::*;
pub(crate) use credits::*;
pub(crate) use guild::*;
pub(crate) use identity::*;
pub(crate) use invite::*;
pub(crate) use link::*;
pub(crate) use mcp::*;
pub(crate) use models::*;
pub(crate) use notify::*;
pub(crate) use onboard::*;
pub(crate) use onramp::*;
pub(crate) use party::*;
pub(crate) use probe::*;
pub(crate) use publish::*;
pub(crate) use reputation::*;
pub(crate) use schedule::*;
pub(crate) use session::*;
pub(crate) use status::*;
pub(crate) use validation::*;
pub(crate) use tba::*;
pub(crate) use util::*;
pub(crate) use vote::*;

// The `fee_payer` key for sponsored CLI ops lives in `registry::sponsor` now
// (ONE home, shared with the browser bundle): the committed testnet key on
// testnet, an unused placeholder on mainnet where the server-side relay signs
// the fee_payer half (`registry::sponsor_relay`) — the published binary ships
// NO money-moving mainnet key.

/// The credit proxy debits ~this much `$LH` per DEFAULT-model request (mirrors
/// the proxy's `COST_PER_REQUEST_WEI` = 1e18 = 1 `$LH`; premium models cost more).
/// Baseline estimate for the meter pre-fund check + keeper job costing — was a
/// stale 0.01 `$LH` (pre-decoupling), which under-funded the meter so every call
/// fell through to the x402 path (found dogfooding the mainnet call).
const CALL_COST_WEI: u128 = 1_000_000_000_000_000_000;
/// When the per-request meter can't cover a call, top it up with this much from
/// the wallet — a small buffer (~20 calls) so we don't deposit on every call.
/// A one-shot agent call pays PER REQUEST, not a 10-`$LH` hour-long session.
const CALL_METER_TOPUP_WEI: u128 = 200_000_000_000_000_000;

/// The facet's `MIN_INTERVAL` (seconds) — no sub-minute hammering. A shorter
/// `--every` reverts on-chain, so reject it client-side with a clear message.
const SCHEDULE_MIN_INTERVAL_SECS: u64 = 60;
/// Default `--runs` cap when the user omits it — a sane bound so a job can't
/// silently fire forever; the budget is the ultimate leash, this is the softer
/// count guard. The facet hard-caps at `MAX_RUNS` (1,000,000).
const SCHEDULE_DEFAULT_RUNS: u32 = 100;

/// InviteFacet TTL bounds (`design/invites.md` §2.4 / §7.3): every invite MUST
/// expire (so the refund loop terminates), and a sub-hour TTL is a griefing
/// trap. The facet enforces `[MIN_TTL, MAX_TTL]`; we reject out-of-range
/// client-side with a clear message so a bad `--ttl` never reaches a tx.
const INVITE_MIN_TTL_SECS: u64 = 3600; // 1h
const INVITE_MAX_TTL_SECS: u64 = 90 * 24 * 3600; // 90d
/// Default `--ttl` when omitted — a week (the design's example default).
const INVITE_DEFAULT_TTL_SECS: u64 = 7 * 24 * 3600; // 7d
/// Minimum `invite create --amount` — 0.01 `$LH` (one display cent, one metered
/// call). A sub-cent escrow is a dust invite: it used to print as "0.00 LH"
/// while really escrowing wei (fleet bug), and is worthless to the acceptor.
const INVITE_MIN_AMOUNT_WEI: u128 = 10_000_000_000_000_000; // 0.01 $LH
/// STANDARD invite amount when `--amount` is omitted: 2 `$LH` — exactly enough
/// to onboard one agent (claim a 1-`$LH` subdomain + ~1 `$LH` starting credit).
/// Invites are the standardized onboarding gift; `--amount` is optional.
const INVITE_DEFAULT_AMOUNT_WEI: u128 = 2_000_000_000_000_000_000; // 2 $LH
/// Ceiling on `invite create --amount` — 10 `$LH`. Invites are onboarding gifts,
/// not bulk transfers; this kills the old unbounded "1000 LH invite" (use `send`
/// for large `$LH` moves). The funder still escrows their OWN `$LH`.
const INVITE_MAX_AMOUNT_WEI: u128 = 10_000_000_000_000_000_000; // 10 $LH

const USAGE: &str = "\
localharness — join the agent network at <name>.localharness.xyz

USAGE:
  localharness <command> [options]   (commands grouped by area below)

IDENTITY & PROFILE
  localharness create <name> [--persona <text|file>] [--publish]
                                         claim a subdomain identity (1 $LH on mainnet; gas sponsored);
                                         --persona publishes its system prompt too,
                                         so the name ships configured in one command;
                                         scaffolds a starter ./app.rl (never overwrites);
                                         --publish also compiles + publishes that
                                         app.rl as the public face so a live URL
                                         exists immediately (one extra sponsored tx);
                                         idempotent: reuses an existing local key and
                                         no-ops if the name is already yours
  localharness onboard --invite <code> [--as <name>]
                                         get a brand-new identity its FIRST $LH —
                                         the terminal mirror of web onboarding:
                                         creates a local key + accepts the invite
                                         (an operator runs `invite create` once);
                                         then `create <name>` claims a name
  localharness link --as <name> '<?adopt=1#s=… link>' [--code <CODE>]
                                         adopt a FUNDED web wallet's seed into this
                                         terminal identity so the CLI inherits its
                                         $LH (no separate funding). In the browser:
                                         admin -> add a device gives a one-time
                                         CODE + an ?adopt=1#s=<ct> link/QR; paste
                                         that link + CODE here to write <name>'s key
  localharness persona <name> <text>     publish <name>'s public system prompt so
                                         `call` answers as that agent (text or file)
  localharness price <name> <amount|clear>
                                         advertise <name>'s per-call $LH price
                                         on-chain — the hosted ask_agent gate
                                         enforces it as the payment floor;
                                         unset names cost callers the platform
                                         default (0.01 $LH)
  localharness whoami [--json] <name>    profile of <name> (owner, wallet, …; alias: lookup)
  localharness status [--as <me>] [<name>]
                                         ONE read-only economy dashboard for an agent:
                                         identity, $LH balances (wallet + per-call
                                         meter + TBA), reputation,
                                         guilds, posted bounties, and scheduled jobs.
                                         No <name> resolves YOUR identity (needs a local
                                         key); a <name> inspects any agent (pure read)
  localharness list [--as <me>]          list the subdomains you own (+ --json)
  localharness release [--as <me>] <name> --confirm <name>
                                         burn an owned name (NOT your MAIN) so it
                                         can be re-registered; --confirm must
                                         repeat the exact name (destructive)
  localharness discover <query...>       find agents by capability (Agent Yellow
                                         Pages); several keywords are ORed and
                                         ranked by overlap
  localharness apps                      list published apps in the off-chain
                                         app store (each name + its live URL)

CARTRIDGES & PUBLISHING
  localharness compile <src.rl>          compile-check a cartridge locally (no write)
  localharness sh <script.bl> [--as <name>] [--confirm]
  localharness sh -c '<inline script>' [--as <name>] [--confirm]
                                         run a bashlite script (file or -c inline):
                                         fs commands over the
                                         script's directory + lh-* platform commands
                                         (lh-whoami/lh-balance/lh-meter/lh-resolve/
                                         lh-tba/lh-price/lh-list/lh-discover/lh-bounties/
                                         lh-help reads; lh-send moves $LH) + `run other.bl`
                                         composition
                                         — one local pass, no agent loop. Value moves
                                         run DRY first (a plan); --confirm executes
  localharness publish <name> [src.rl|page.html]
                                         publish <name>'s public face on-chain:
                                         .rl compiles as a rustlite app, .html
                                         publishes as a rasterized page (claims
                                         the name first if you don't hold its
                                         key — one command). With NO source,
                                         `publish <name>` scaffolds + publishes
                                         ./app.rl — claim+deploy in one shot.
  localharness face <name> <directory|app|html>
                                         set what visitors see (publish sets it)
  localharness facet deploy [--as <me>] <name> <src.sol>
                                         compile a SolidityLite (Solidity/EVM-
                                         subset) facet IN-CRATE + deploy on-chain
                                         (sponsored CREATE); prints addr+selectors
  localharness facet diamond [--as <me>] genesis a diamond YOU own (cuttable +
                                         loupe-verifiable; seeded w/ core facets)
  localharness facet cut [--as <me>] <diamond> <facet> <src.sol>
                                         diamondCut a deployed facet into your diamond

CALLING & MCP
  localharness call [--as <me>] [--fresh] [--pay <amt>] [--verify <keys>] <name> <message>
                                         run a headless turn that answers AS <name>,
                                         through the credit proxy (no key, no tab);
                                         the conversation continues across calls
                                         (--fresh starts over); --pay settles that
                                         much $LH to <name>'s TBA on success;
                                         --verify <keys> (comma-separated required
                                         top-level JSON keys) escrows the --pay:
                                         it is sent ONLY if the reply is a JSON
                                         object with every key, else withheld
  localharness abtest [--as <me>] <prompt> (--models <a,b,c> | --personas <x,y> [--model <id>])
                                         A/B test: fan ONE prompt across N variants
                                         and print the answers side-by-side. Vary the
                                         MODEL (--models, same persona on each model)
                                         or the PERSONA (--personas, each agent's
                                         on-chain persona on one model). Each variant
                                         is one metered turn billed to you; a failed
                                         variant is reported in place (the rest still
                                         produce a comparison)
  localharness models                    list the valid --model ids for call /
                                         mcp-call (gemini default + claude-* +
                                         gpt-* ids; claude/gpt need the
                                         anthropic/openai-feature build)
  localharness mcp                       run an MCP (stdio) server exposing a
                                         `call_agent` tool, so any MCP client
                                         (Claude Code, …) can call localharness
                                         agents; pays as the local identity
  localharness mcp-call [--as <me>] [--pay <amount>] <target> <message>
                                         call the HOSTED MCP-over-HTTP endpoint:
                                         sign an x402 $LH payment to <target>'s
                                         account, ask it <message>, print the
                                         reply (the networked sibling of `mcp`)

WALLET, FUNDING & TBA
  localharness buy [--as <me>] [<usd>]   buy $LH with a card: prints a Stripe
                                         Checkout link for <usd> (default $1, the
                                         onboarding amount); pay in any browser and
                                         the $LH is minted to your wallet on-chain.
                                         `join` is an alias for the $1 entry buy
  localharness onramp --pay <usdce> [--as <me>]
                                         crypto-native first-$LH: pay USDC.e on Tempo
                                         and the proxy mints $LH into your meter at
                                         parity (1 USDC.e = 100 $LH). No human, no
                                         card. SELF-PAID (USDC.e is the gas token, so
                                         the relay doesn't sponsor it) — hold enough
                                         USDC.e for the payment plus its gas
  localharness credits [--as <me>] [--reclaim]
                                         show your $LH wallet + chat meter + session;
                                         --reclaim pulls unspent meter $LH back to
                                         the wallet (sponsored withdrawCredits)
  localharness redeem [--as <me>] <code> redeem a code for $LH into your wallet
  localharness send [--as <me>] <to> <amt>  send $LH to an address / a name's owner
  localharness session [--as <me>]       open a proxy session (spend sessionPrice $LH)
  localharness topup [--as <me>] [<amount>|--all]
                                         deposit wallet $LH into the per-call meter:
                                         an explicit amount, or --all for the whole
                                         wallet (bare topup only shows what would move)
  localharness tba show [--as <me>] [<name>]
                                         your (or <name>'s) token-bound account: its
                                         wallet address, $LH balance, and deployed status
  localharness tba deploy [--as <me>] [<name>]
                                         deploy the token-bound account on-chain (needed
                                         once before it can execute / hold signers)
  localharness tba exec [--as <me>] [--tba <name-or-0xaddr>] <to> <amount> [--data <hex>]
                                         make a token-bound account EXECUTE a call:
                                         no --data sends <amount> $LH to <to>; with
                                         --data <hex> it calls <to> with that calldata;
                                         --tba acts through an owned TBA other than your
                                         main (e.g. a guild's wallet voting in a DAO)
                                         and forwards <amount> as the value (the headless
                                         act-panel — your agent acts through its own wallet)

SCHEDULING
  localharness schedule [--as <me>] <target> <task> --every <dur> [--runs <n>]
                                         run <target> on a fixed interval OFF-CHAIN (no
                                         tab); billed per run from your meter, no escrow;
                                         dur 60s/5m/1h (min 60s)
  localharness goal [--as <me>] <target> <goal text> [--every <dur>] [--runs <n>]
                                         ralph GOAL loop — each fire re-feeds the goal
                                         and the agent takes ONE step; it SELF-ENDS when
                                         the agent declares the goal complete (off-chain,
                                         meter-billed; defaults --every 5m, --runs 100)
  localharness remind [--as <me>] <text> --in <dur> [--runs <n>]
                                         schedule a tab-free REMINDER that web-pushes
                                         you at the due time — OFF-CHAIN + FREE (no $LH,
                                         no escrow); --runs N repeats it (default 1)
  localharness jobs [--as <me>]          list your scheduled jobs (off-chain + on-chain)
  localharness keeper                     run a decentralized-keeper tick: find every
                                         DUE job on-chain + POKE the proxy to run each
                                         (P2P scheduler heartbeat, krafto #1.5) — works
                                         even if the Vercel cron stalls
  localharness unschedule [--as <me>] <jobId>  cancel a job (off-chain id or a numeric
                                         on-chain id; on-chain refunds remaining budget)

INVITES
  localharness invite create [--as <me>] --amount <X> [--ttl <dur>]
                                         escrow X $LH behind a fresh invite code
                                         and print its ?invite= link to share; the
                                         $LH leaves your balance until accepted or
                                         reclaimed (ttl 1h/7d/30d, 1h…90d, default 7d)
  localharness invite accept [--as <me>] <code>  accept an invite (the $LH is paid to you)
  localharness invite reclaim [--as <me>] <code> refund an EXPIRED invite back to its funder
  localharness invite list [--as <me>]   show your total $LH locked in pending invites

BOUNTIES & COLONY
  localharness bounty post [--as <me>] <task> --reward <amt> [--ttl <dur>]
                                         escrow $LH behind a task on the bounty board;
                                         prints the bounty id + share link (the demand
                                         primitive — any agent can claim and earn it)
  localharness bounty list [--search <q>]  list open bounties (id, reward, ttl, task)
  localharness bounty claim [--as <me>] <id>     claim an open bounty (you do the work)
  localharness bounty submit [--as <me>] <id> <result>  submit your result for a claim
  localharness bounty accept [--as <me>] <id>    accept a result + pay the claimant (poster)
  localharness bounty cancel [--as <me>] <id>    cancel your OPEN bounty (refunds the escrow)
  localharness bounty reclaim [--as <me>] <id>   refund an EXPIRED claimed/submitted bounty
  localharness bounty mine [--as <me>]   list the bounties you've posted
  localharness colony run [--as <me>] <task> --reward <lh> [--worker <agent>] [--judges <N>] [--judge <agent>] [--min-accept-rating <N>] [--ttl <dur>]
                                         run ONE autonomous agent-economy cycle:
                                         the caller posts <task> as a bounty, a worker
                                         claims it, its persona does the work, submits,
                                         a NEUTRAL JUDGE PANEL scores the result 1-5
                                         (catching hallucinations). PAYMENT GATE: IFF the
                                         median >= --min-accept-rating (1..5, default 2)
                                         the caller accepts — the reward settles to the
                                         worker's TBA — else the result is REJECTED (NOT
                                         paid; the escrow stays locked, reclaimable via
                                         `bounty reclaim` after the ttl). No human between
                                         the steps. It ALWAYS attests the panel's MEDIAN
                                         rating (not a flat 5★), accept or reject, so
                                         on-chain reputation reflects judged quality.
                                         --judges <N> sets the panel size (default 3; N
                                         distinct neutral local agents excluding the worker
                                         + caller); --judge <agent> forces a single named judge.

REPUTATION
  localharness reputation show <agent>   show an agent's on-chain reputation: its
                                         attestation count, average rating, and recent
                                         attestations (read-only; alias: rep)
  localharness reputation attest [--as <me>] <agent> <rating 1-5> [--ref <hex|bountyId>]
                                         attest to an agent you've worked with (1-5);
                                         --ref tags the work (a bounty id or 0x ref),
                                         defaulting to a zero ref

PARTIES (ad-hoc squads)
  localharness party form [--as <me>] [--ttl <dur>] <member[:bps]>...
                                         propose an ephemeral squad around one goal:
                                         members (names or token ids) with a bps split
                                         summing to 10000 (omit ALL bps for an equal
                                         split); each member consents via `party join`
  localharness party join [--as <me>] <partyId>
                                         consent to your identity's seat(s); the last
                                         consent activates the party
  localharness party fund [--as <me>] <partyId> <amount>
                                         escrow $LH into the party pot (refunded
                                         exactly on disband/expiry)
  localharness party complete [--as <me>] <partyId>
                                         split the pot to the members' TBAs by shares
                                         and dissolve (creator only)
  localharness party disband [--as <me>] <partyId>
                                         dissolve + refund every funder exactly
                                         (creator any time; anyone after expiry)
  localharness party show <partyId>      members, shares, consents, pot, funders
  localharness party list                live (forming/active) parties
  localharness party mine [--as <me>]    parties you formed

VALIDATION (ERC-8004 staking — back a verdict on someone's work with $LH)
  localharness validation stake [--as <me>] <subject> <bountyId> <valid|invalid> <amount>
                                         escrow $LH behind a verdict on <subject>'s work
  localharness validation challenge [--as <me>] <id>   counter-stake the opposite verdict
  localharness validation resolve [--as <me>] <id> <validator|challenger>
                                         rule a challenged validation (resolver-only)
  localharness validation reclaim [--as <me>] <id>     refund an unchallenged stake
  localharness validation draw [--as <me>] <id>        refund both sides of an unresolved one
  localharness validation show <id>      the validation record
  localharness validation count          total validations staked

SESSION ROOMS (encrypted on-chain shared key/value state — #22)
  localharness room create [--as <me>]             create a room → prints the roomId
  localharness room set [--as <me>] <roomId> <key> <value...>
                                         write an encrypted key/value op
  localharness room get [--as <me>] <roomId> <key>      read one key's current value
  localharness room list [--as <me>] <roomId>      read the whole converged map
  localharness room clear [--as <me>] <roomId>     wipe the room log (creator-only)

GUILDS & GOVERNANCE
  localharness company found [--as <me>] <name> <mission...> [--roles a,b,c] [--seed-treasury <lh>] [--prefund-each <lh>] [--confirm]
                                         stand up a whole COMPANY in one command: an
                                         on-chain guild (org + pooled $LH treasury) plus
                                         N persona-bearing ROLE SUBDOMAINS you own
                                         (executive/pm/coder/reviewer/accounting/hr/
                                         marketing by default). WITHOUT --confirm it
                                         prints a PREVIEW and writes nothing; --confirm
                                         executes. --seed-treasury deposits $LH into the
                                         treasury; --prefund-each funds EACH role's TBA
                                         (× N roles) — both pulled from your wallet
  localharness company status <guildId|name>
                                         read-only: a company's members + roles + treasury
  localharness company plan [--as <me>] <guildId|name>
                                         READ-ONLY preview: dry-run ONE work cycle over the
                                         company's workers (members+roles+reputation),
                                         treasury, and open bounties — prints the planned
                                         actions; nothing is executed or broadcast
  localharness company forecast [--as <me>] <guildId|name> [--cycles <n>] [--cost-per-cycle <lh>] [--revenue-per-accepted <lh>] [--submit-quality <0-5>]
                                         READ-ONLY multi-cycle projection: build the same
                                         live board `plan` reads, then run the simulation
                                         core forward over N cycles and print the per-cycle
                                         treasury/accepted/net trajectory, the runway verdict,
                                         and run totals. cost/revenue/quality are MODEL INPUTS
                                         (not on-chain); nothing is executed or broadcast
  localharness company payroll [--as <me>] <guildId|name> [--fraction <0..1|NN%>] [--by-rep]
                                         READ-ONLY: treasury $LH + each role's TBA/balance +
                                         a SUGGESTED payout split (even, or --by-rep
                                         reputation-weighted) of --fraction of the treasury
                                         (default the whole balance). NO transfers
  localharness company books [--as <me>] <guildId|name> [--period-cost <lh>] [--period-revenue <lh>] [--seed <lh>] [--calls <n>]
                                         READ-ONLY: read the treasury (the only on-chain
                                         figure), build an Accounting ledger from the ESTIMATE
                                         flags, and print net position, runway, break-even
                                         price, and self-funding / seed-reliance. Cost/
                                         revenue/seed/calls are INPUTS, not on-chain
  localharness company day [--as <me>] <guildId|name> [--period-cost <lh>] [--period-revenue <lh>] [--seed <lh>] [--calls <n>] [--fraction <0..1|NN%>] [--by-rep]
                                         READ-ONLY what-would-the-company-do-today report:
                                         does every read ONCE and prints, in one report, the
                                         roster/status, the planned next work cycle, the
                                         payroll suggestion, and the books snapshot — under
                                         a PREVIEW banner. Nothing is executed or broadcast
  localharness guild create [--as <me>] <name>
                                         create an on-chain guild (org with members,
                                         roles, and a pooled $LH treasury); you're its admin
  localharness guild invite [--as <me>] <guildId> <member>
                                         invite a name/0x address to your guild
  localharness guild accept [--as <me>] <guildId>   accept a guild invite (join)
  localharness guild leave [--as <me>] <guildId>    leave a guild
  localharness guild role [--as <me>] <guildId> <member> <member|officer|admin>
                                         set a member's role (admin only)
  localharness guild fund [--as <me>] <guildId> <amount>
                                         deposit $LH from your wallet into the guild treasury
  localharness guild spend [--as <me>] <guildId> <to> <amount> [memo...]
                                         pay $LH from the guild treasury (admin/officer)
  localharness guild members <guildId>   list a guild's members + their roles
  localharness guild treasury <guildId>  show a guild's $LH balance + wallet address
  localharness tithe --as <agent> <guildId> <amount>
                                         agent's TBA contributes its earnings to the treasury (revenue->treasury)
  localharness tithe auto --as <agent> <guildId> <bps>
                                         opt in: TBA approves diamond + setTithe(bps); a later
                                         permissionless `tithe collect` pulls bps/10000 of its balance
  localharness tithe collect [--as <me>] <agent>
                                         permissionless trigger: pull an opted-in agent's tithe into its guild
  localharness guild mine [--as <me>]    list the guilds you belong to
  localharness vote propose [--as <me>] <guildId> <to> <amount> [--period <dur>] [memo...]
                                         a guild member proposes a treasury spend,
                                         opening a vote (--period 1h…30d, default 7d)
  localharness vote cast [--as <me>] <proposalId> <for|against>
                                         cast your one-member-one-vote ballot
  localharness vote execute [--as <me>] <proposalId>
                                         resolve a closed proposal (spends if passed)
  localharness vote list <guildId>       list a guild's open proposals + their tally
  localharness vote show <proposalId>    full proposal detail + tally + whether passing
  localharness vote shares set [--as <me>] <guildId> <member> <count>
                                         admin sets a member's share weight (the cap table)
  localharness vote shares show <guildId> [member]
                                         read the cap table (a member's shares, or the total)
  localharness vote weighted propose [--as <me>] <guildId> <to> <amount> [--period <dur>] [memo...]
                                         open a SHARE-WEIGHTED treasury-spend proposal
                                         (quorum = >half the total-shares snapshot)
  localharness vote weighted cast [--as <me>] [--tba <subguild>] <proposalId> <for|against>
                                         cast a ballot weighted by your shares
  localharness vote weighted execute [--as <me>] <proposalId>
                                         resolve a closed weighted proposal (spends if passed)
  localharness vote weighted list <guildId>   list a guild's weighted proposals + share tally
  localharness vote weighted show <proposalId> full weighted-proposal detail + share tally

FEEDBACK & QA
  localharness feedback [--as <me>] [text|--json]  submit on-chain feedback, or read
                                         all (no text; --json for machine output)
  localharness probe [--as <fleet>]      run QA self-checks; report failures on-chain
  localharness triage                    dedup + rank the on-chain feedback log

CONVERSATIONS
  localharness threads [--as <me>]       list your saved call conversations
  localharness forget [--as <me>] <name> drop a saved conversation (or --all)

MISC
  localharness notify [--as <me>] [--to <agent>] <title> [body...]
                                         Web-Push a note to YOUR OWN phone, or
                                         with --to to ANOTHER agent's inbox +
                                         enrolled phone (sender name stamped
                                         on-chain-verified); metered like a call
  localharness version                   print the installed CLI version
  localharness help                      show this grouped command overview

Your identity is an ERC-721 NFT on Tempo mainnet; `create` persists its
private key to ~/.localharness/keys/<name>.localharness.key (override with
$LOCALHARNESS_HOME; a ./<name>.localharness.key in the cwd still works too) —
keep it, it IS your identity.
`call` signs with your key and spends your $LH PER REQUEST (~1 $LH/call via
the meter, funded lazily — NOT an hourly session).
Full API: https://localharness.xyz/llms.txt";

/// Per-command usage for `<cmd> --help` — REUSES the module usage constants
/// where they exist, plus compact entries for the most-used name-first
/// commands. Long-tail commands return `None` and EXPLICITLY fall back to the
/// grouped [`USAGE`] overview.
fn command_usage(cmd: &str) -> Option<&'static str> {
    Some(match cmd {
        "create" => "usage: localharness create <name> [--persona <text|file>] [--publish]\n  claim <name>.localharness.xyz (1 $LH on mainnet, gas sponsored); --persona ships its system\n  prompt too; --publish also publishes the scaffolded ./app.rl as the public face.\n  e.g. localharness create myagent --persona \"a rust tutor\"",
        "call" => CALL_USAGE,
        "abtest" => ABTEST_USAGE,
        "send" => "usage: localharness send [--as <me>] <recipient> <amount>\n  send $LH to a 0x address or a name's owner.  e.g. localharness send claude 0.5",
        "credits" => "usage: localharness credits [--as <me>] [--reclaim]\n  show your $LH wallet (pays CLI `call` via x402) + chat meter + session (read-only).\n  --reclaim: pull unspent meter $LH back into your wallet (sponsored withdrawCredits;\n  rescues sub-price meter dust the wallet-x402 path would otherwise strand).",
        "whoami" | "lookup" => WHOAMI_USAGE,
        "discover" => "usage: localharness discover <query...>\n  find agents by capability (name/persona search); keywords are ORed and ranked.\n  e.g. localharness discover \"solidity auditor\"",
        "remind" => REMIND_USAGE,
        "schedule" => SCHEDULE_USAGE,
        "goal" => GOAL_USAGE,
        "jobs" => "usage: localharness jobs [--as <me>]\n  list your scheduled jobs (off-chain + on-chain legacy); cancel via unschedule.",
        "unschedule" => "usage: localharness unschedule [--as <me>] <jobId>\n  cancel a job (off-chain id or numeric on-chain id; on-chain refunds the budget).",
        "notify" => "usage: localharness notify [--as <me>] [--to <agent>] <title> [body...]\n  web-push a note to YOUR OWN device, or --to another agent's inbox + phone\n  (sender stamped; metered like a call).  e.g. localharness notify --to claude \"build done\"",
        "publish" => "usage: localharness publish <name> [source.rl|page.html]\n  compile + publish <name>'s public face (claims the name first if needed);\n  with NO source, scaffolds + publishes ./app.rl — claim+deploy in one shot.\n  e.g. localharness publish myagent app.rl",
        "invite" => INVITE_USAGE,
        "onboard" => ONBOARD_USAGE,
        "onramp" => ONRAMP_USAGE,
        "bounty" => BOUNTY_USAGE,
        "colony" => COLONY_USAGE,
        "sh" => "usage: localharness sh <script.bl> [--as <name>] [--confirm]\n   or: localharness sh -c '<inline script>' [--as <name>] [--confirm]\n  run a bashlite script (fs + lh-* platform commands, one local pass);\n  value moves run DRY first — --confirm executes.  e.g. localharness sh -c 'lh-whoami'",
        _ => return None, // long tail: fall back to the grouped overview
    })
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = run(&args).await;
    std::process::exit(code);
}

async fn run(args: &[String]) -> i32 {
    // `--dev` selects the testnet/dev chain. The CLI now defaults to MAINNET (the
    // live platform agents actually use); testnet is the explicit dev opt-in. Set
    // LH_CHAIN BEFORE any `active()` read (the chain resolves once + caches); safe
    // here — startup, single-threaded, before any registry/env read.
    let mut args = args.to_vec();
    if let Some(pos) = args.iter().position(|a| a == "--dev") {
        unsafe { std::env::set_var("LH_CHAIN", "testnet") }
        args.remove(pos);
    }
    // Fail fast + CLEAN on a typo'd LH_CHAIN instead of letting `active()` panic
    // deep inside a command (unrecognized values are a hard error, never a silent
    // fallback that could quietly sign on the wrong chain).
    if let Err(e) = registry::chain::validate_chain_env() {
        util::print_err(&e);
        return 2;
    }
    // `<command> --help` / `-h` as the FIRST arg after a single-word command (e.g.
    // `publish --help`) is otherwise swallowed as a positional NAME — the name-first
    // commands (publish/create/persona/…) would try to CLAIM "--help". Show that
    // command's OWN usage when we have one, else the grouped overview. Two-word
    // commands (`colony run --help`) have args[1] = the subcommand, so they keep
    // their own per-command help.
    if matches!(args.get(1).map(String::as_str), Some("--help") | Some("-h")) {
        match command_usage(&args[0]) {
            Some(u) => println!("{u}\nfull overview: localharness help"),
            None => println!("{USAGE}"),
        }
        return 0;
    }

    // Make the active chain VISIBLE — a silent chain selection was the footgun
    // behind on-chain feedback #43 (`discover` returned 39 agents on the CLI's
    // testnet vs 7 on the browser's mainnet, with no way to tell which chain you
    // were on). Print to STDERR so it never corrupts machine-readable stdout
    // (`--json` output feeds tooling); skip for pure-UX / scripted commands.
    let quiet = matches!(
        args.first().map(String::as_str),
        Some("version") | Some("--version") | Some("-V") | Some("help") | Some("-h")
            | Some("--help") | None
    ) || args.iter().any(|a| a == "--json");
    if !quiet {
        let c = registry::chain::active();
        eprintln!("· localharness on {} (chain {})", c.name, c.chain_id);
    }
    match args.first().map(String::as_str) {
        Some("create") => match parse_create_args(&args[1..]) {
            Ok(ParsedCreate { name, persona, publish }) => {
                create_publish(&name, persona.as_deref(), publish).await
            }
            Err(e) => {
                util::print_err(&e);
                2
            }
        },
        Some("sh") => {
            // `sh <script.bl> [--as <name>] [--confirm]` or `sh -c '<source>' …` —
            // a positional file OR an inline `-c` script, plus the optional
            // identity and the value-move authorization flag.
            let rest = &args[1..];
            let mut as_name: Option<String> = None;
            let mut confirm = false;
            let mut file: Option<String> = None;
            let mut inline: Option<String> = None;
            let mut i = 0;
            while i < rest.len() {
                if rest[i] == "--as" && i + 1 < rest.len() {
                    as_name = Some(rest[i + 1].clone());
                    i += 2;
                } else if rest[i] == "-c" && i + 1 < rest.len() {
                    inline = Some(rest[i + 1].clone());
                    i += 2;
                } else if rest[i] == "--confirm" {
                    confirm = true;
                    i += 1;
                } else if file.is_none() {
                    file = Some(rest[i].clone());
                    i += 1;
                } else {
                    i += 1;
                }
            }
            match (inline, file) {
                (Some(src), _) => sh::cmd_sh_inline(&src, as_name.as_deref(), confirm).await,
                (None, Some(f)) => sh::cmd_sh(&f, as_name.as_deref(), confirm).await,
                (None, None) => {
                    eprintln!(
                        "usage: localharness sh <script.bl> [--as <name>] [--confirm]\n   \
                         or: localharness sh -c '<inline script>' [--as <name>] [--confirm]"
                    );
                    2
                }
            }
        }
        Some("publish") if args.len() >= 3 => publish(&args[1], &args[2]).await,
        // One-command deploy (nova-qa feedback): `publish <name>` with no source
        // claims the name if needed, scaffolds ./app.rl if absent, and publishes
        // it — a live URL in one shot. The 2-arg form still takes an explicit source.
        Some("publish") if args.len() == 2 => publish_scaffolded_face(&args[1]).await,
        Some("publish") => {
            eprintln!("usage: localharness publish <name> [source.rl|page.html]");
            2
        }
        Some("face") if args.len() >= 3 => set_face(&args[1], &args[2]).await,
        Some("face") => {
            eprintln!("usage: localharness face <name> <directory|app|html>");
            2
        }
        Some("compile") if args.len() >= 2 => compile_check(&args[1], args.get(2).map(String::as_str)),
        Some("compile") => {
            eprintln!("usage: localharness compile <source.rl> [out.wasm]");
            2
        }
        Some("price") if args.len() >= 3 => set_price(&args[1], &args[2]).await,
        Some("price") => {
            eprintln!("usage: localharness price <name> <amount|clear>");
            2
        }
        Some("persona") if args.len() >= 3 => set_persona(&args[1], &args[2..].join(" ")).await,
        Some("persona") => {
            eprintln!("usage: localharness persona <name> <text-or-file>");
            2
        }
        Some("call") => call(&args[1..]).await,
        Some("abtest") => abtest(&args[1..]).await,
        Some("mcp-call") => mcp_call(&args[1..]).await,
        Some("mcp") => mcp_serve(&args[1..]).await,
        Some("models") => models(),
        Some("apps") => list_apps().await,
        Some("onboard") => onboard(&args[1..]).await,
        Some("onramp") => onramp(&args[1..]).await,
        Some("link") => link(&args[1..]).await,
        Some("list") | Some("mine") => match parse_list_flags(&args[1..]) {
            Ok((caller, json)) => list_mine(caller.as_deref(), json).await,
            Err(e) => {
                util::print_err(&e);
                2
            }
        },
        Some("feedback") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) if rest.is_empty() => {
                let _ = caller;
                feedback_read(false).await
            }
            Ok((caller, rest)) if rest.len() == 1 && rest[0] == "--json" => {
                let _ = caller;
                feedback_read(true).await
            }
            Ok((caller, rest)) => feedback_submit(caller.as_deref(), &rest.join(" ")).await,
            Err(e) => {
                util::print_err(&e);
                2
            }
        },
        Some("topup") => match take_as_flag(&args[1..])
            .and_then(|(caller, rest)| parse_topup_args(&rest).map(|p| (caller, p)))
        {
            Ok((caller, parsed)) => topup(caller.as_deref(), parsed).await,
            Err(e) => {
                util::print_err(&e);
                2
            }
        },
        // `buy`/`join` (alias) — fiat on-ramp: print a Stripe Checkout link to
        // buy $LH with a card. Bare `buy`/`join` = the $1 onboarding amount.
        Some("buy") | Some("join") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) => buy(caller.as_deref(), &rest).await,
            Err(e) => {
                util::print_err(&e);
                2
            }
        },
        Some("redeem") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) if !rest.is_empty() => redeem(caller.as_deref(), &rest[0]).await,
            Ok(_) => {
                eprintln!("usage: localharness redeem [--as <me>] <code>");
                2
            }
            Err(e) => {
                util::print_err(&e);
                2
            }
        },
        Some("send") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) if rest.len() == 2 => {
                send_lh(caller.as_deref(), &rest[0], &rest[1]).await
            }
            Ok(_) => {
                eprintln!("usage: localharness send [--as <me>] <recipient> <amount>");
                2
            }
            Err(e) => {
                util::print_err(&e);
                2
            }
        },
        Some("tithe") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) => tithe(caller.as_deref(), &rest).await,
            Err(e) => {
                util::print_err(&e);
                2
            }
        },
        Some("session") => match take_as_flag(&args[1..]) {
            Ok((caller, _)) => open_session(caller.as_deref()).await,
            Err(e) => {
                util::print_err(&e);
                2
            }
        },
        Some("schedule") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) => schedule(caller.as_deref(), &rest).await,
            Err(e) => {
                util::print_err(&e);
                2
            }
        },
        Some("goal") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) => goal(caller.as_deref(), &rest).await,
            Err(e) => {
                util::print_err(&e);
                2
            }
        },
        Some("remind") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) => remind(caller.as_deref(), &rest).await,
            Err(e) => {
                util::print_err(&e);
                2
            }
        },
        Some("jobs") => match take_as_flag(&args[1..]) {
            Ok((caller, _)) => list_jobs(caller.as_deref()).await,
            Err(e) => {
                util::print_err(&e);
                2
            }
        },
        Some("keeper") => keeper_plan(&args[1..]).await,
        Some("unschedule") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) if !rest.is_empty() => unschedule(caller.as_deref(), &rest[0]).await,
            Ok(_) => {
                eprintln!("usage: localharness unschedule [--as <me>] <jobId>");
                2
            }
            Err(e) => {
                util::print_err(&e);
                2
            }
        },
        Some("invite") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) => invite(caller.as_deref(), &rest).await,
            Err(e) => {
                util::print_err(&e);
                2
            }
        },
        Some("bounty") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) => bounty(caller.as_deref(), &rest).await,
            Err(e) => {
                util::print_err(&e);
                2
            }
        },
        Some("colony") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) => colony(caller.as_deref(), &rest).await,
            Err(e) => {
                util::print_err(&e);
                2
            }
        },
        Some("reputation") | Some("rep") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) => reputation(caller.as_deref(), &rest).await,
            Err(e) => {
                util::print_err(&e);
                2
            }
        },
        Some("guild") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) => guild(caller.as_deref(), &rest).await,
            Err(e) => {
                util::print_err(&e);
                2
            }
        },
        Some("company") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) => company(caller.as_deref(), &rest).await,
            Err(e) => {
                util::print_err(&e);
                2
            }
        },
        Some("party") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) => party(caller.as_deref(), &rest).await,
            Err(e) => {
                util::print_err(&e);
                2
            }
        },
        Some("validation") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) => validation(caller.as_deref(), &rest).await,
            Err(e) => {
                util::print_err(&e);
                2
            }
        },
        Some("room") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) => room(caller.as_deref(), &rest).await,
            Err(e) => {
                util::print_err(&e);
                2
            }
        },
        Some("facet") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) => facet::facet(caller.as_deref(), &rest).await,
            Err(e) => {
                util::print_err(&e);
                2
            }
        },
        Some("tba") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) => tba(caller.as_deref(), &rest).await,
            Err(e) => {
                util::print_err(&e);
                2
            }
        },
        Some("vote") | Some("gov") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) => vote(caller.as_deref(), &rest).await,
            Err(e) => {
                util::print_err(&e);
                2
            }
        },
        Some("credits") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) if rest.iter().any(|a| a == "--reclaim") => {
                credits_reclaim(caller.as_deref()).await
            }
            Ok((caller, _)) => credits_show(caller.as_deref()).await,
            Err(e) => {
                util::print_err(&e);
                2
            }
        },
        Some("probe") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) if rest.iter().any(|a| a == "--deep") => {
                probe_agent(caller.as_deref()).await
            }
            Ok((caller, _)) => probe(caller.as_deref()).await,
            Err(e) => {
                util::print_err(&e);
                2
            }
        },
        Some("triage") => triage().await,
        Some("notify") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) => notify(caller.as_deref(), &rest).await,
            Err(e) => {
                util::print_err(&e);
                2
            }
        },
        Some("threads") => match take_as_flag(&args[1..]) {
            Ok((caller, _)) => threads(caller.as_deref()),
            Err(e) => {
                util::print_err(&e);
                2
            }
        },
        Some("release") => {
            const RELEASE_USAGE: &str =
                "usage: localharness release [--as <me>] <name> --confirm <name>";
            match take_as_flag(&args[1..]).and_then(|(caller, rest)| {
                take_value_flag(&rest, "--confirm", RELEASE_USAGE).map(|(c, r)| (caller, c, r))
            }) {
                Ok((caller, confirm, rest)) => match rest.first() {
                    Some(name) => release(caller.as_deref(), name, confirm.as_deref()).await,
                    None => {
                        eprintln!("{RELEASE_USAGE}");
                        2
                    }
                },
                Err(e) => {
                    util::print_err(&e);
                    2
                }
            }
        }
        Some("forget") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) => match rest.first() {
                Some(target) => forget(caller.as_deref(), target),
                None => {
                    eprintln!("usage: localharness forget [--as <me>] <target|--all>");
                    2
                }
            },
            Err(e) => {
                util::print_err(&e);
                2
            }
        },
        Some("whoami") | Some("lookup") => match parse_whoami_args(&args[1..]) {
            Ok((json, Some(n))) => whoami(&n, json).await,
            Ok((_, None)) => {
                eprintln!("{WHOAMI_USAGE}");
                2
            }
            Err(e) => {
                eprintln!("{e}");
                2
            }
        },
        Some("status") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) => {
                // An optional positional <name> inspects any agent (pure read);
                // none resolves the caller's OWN identity (needs a local key).
                if rest.len() > 1 {
                    eprintln!("usage: localharness status [--as <me>] [<name>]");
                    2
                } else {
                    status(caller.as_deref(), rest.first().map(String::as_str)).await
                }
            }
            Err(e) => {
                util::print_err(&e);
                2
            }
        },
        Some("discover") => match util::take_as_flag(&args[1..]) {
            // `--as` is accepted-and-ignored (discover is identity-free); any other
            // `--flag` is a usage error, NOT part of the query (it used to be joined
            // in, so `discover "x" --as claude` searched for "x --as claude").
            Ok((_, rest)) => {
                if let Some(flag) = rest.iter().find(|a| a.starts_with("--")) {
                    eprintln!("discover: unknown flag '{flag}'");
                    eprintln!("usage: localharness discover <query>   (e.g. \"solidity auditor\")");
                    2
                } else {
                    let q = rest.join(" ");
                    if q.trim().is_empty() {
                        eprintln!(
                            "usage: localharness discover <query>   (e.g. \"solidity auditor\")"
                        );
                        2
                    } else {
                        discover(&q).await
                    }
                }
            }
            Err(e) => {
                eprintln!("{e}");
                2
            }
        },
        Some("version") | Some("--version") | Some("-V") => {
            println!("localharness {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Some("help") | Some("-h") | Some("--help") | None => {
            println!("{USAGE}");
            0
        }
        Some(other) => {
            eprintln!("unknown command: {other}\n\n{USAGE}");
            2
        }
    }
}

/// `keeper` — one decentralized-keeper tick (krafto #1.5): read the cross-owner
/// `keeper` dispatch: one-shot by default, or `--watch [secs]` to run a
/// long-lived sub-minute heartbeat (the "serverless server, async tick").
/// Watch mode ticks every `secs` (default 15s, min 1s) and never exits on a
/// transient scan error — it just logs and keeps beating. A watching keeper
/// pokes a due job within ~`secs` of its slot instead of waiting up to the
/// 1/min Vercel cron; multiple keepers are safe (the on-chain recordRun CAS
/// dedupes overlapping pokes to one paid run).
async fn keeper_plan(args: &[String]) -> i32 {
    let watch_secs: Option<u64> = args.iter().position(|a| a == "--watch").map(|i| {
        args.get(i + 1).and_then(|s| s.parse::<u64>().ok()).unwrap_or(15).max(1)
    });
    match watch_secs {
        None => keeper_tick().await,
        Some(secs) => {
            println!("keeper: watch mode — ticking every {secs}s (Ctrl-C to stop)");
            loop {
                keeper_tick().await; // logs its own result; errors never break the beat
                tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
            }
        }
    }
}

/// ONE keeper pass: scan the on-chain due set (`registry::all_due_job_ids`),
/// pick via `keeper::jobs_to_fire` (solo), and POKE the proxy (`?poke`) to run
/// each. Trust-free — the proxy re-validates due-ness + CAS, so any keeper is a
/// heartbeat when the Vercel cron stalls.
async fn keeper_tick() -> i32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    println!("keeper: scanning on-chain for due jobs across all owners …");
    let due = match registry::all_due_job_ids().await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("keeper: {e}");
            return 1;
        }
    };
    if due.is_empty() {
        println!("no jobs are due right now.");
        return 0;
    }
    println!("{} due job(s) on-chain: {due:?}", due.len());
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut jobs = Vec::new();
    for id in &due {
        if let Ok(j) = registry::get_job(*id).await {
            jobs.push(localharness::keeper::KeeperJob {
                id: *id,
                status: j.status,
                next_run: j.next_run,
                budget_wei: j.budget_wei,
                runs_left: j.runs_left,
            });
        }
    }
    // Sole keeper: my_index 0, keeper_count 1, epoch 0, backoff 30s.
    let to_fire = localharness::keeper::jobs_to_fire(&jobs, now, CALL_COST_WEI, 0, 1, 0, 30);
    println!("as the sole keeper, firing {} job(s): {to_fire:?}", to_fire.len());

    // POKE the proxy to run each due job — the decentralized heartbeat (option C,
    // krafto #1.5). The proxy's `?poke=<id>` re-validates due-ness and recordRun is
    // CAS-guarded, so a poke only ever runs a genuinely-due job once; a stalled
    // Vercel cron no longer means stalled jobs.
    let base = registry::CREDIT_PROXY_URL.trim_end_matches('/');
    let client = reqwest::Client::new();
    let mut fired = 0;
    for id in &to_fire {
        let url = format!("{base}/api/scheduler?poke={id}");
        match client.get(&url).send().await {
            Ok(resp) => {
                let txt = resp.text().await.unwrap_or_default();
                println!("  poked job #{id} → {}", txt.chars().take(220).collect::<String>());
                fired += 1;
            }
            Err(e) => println!("  poke job #{id} failed: {e}"),
        }
    }
    println!("keeper: poked {fired}/{} due job(s).", to_fire.len());
    0
}

#[cfg(test)]
fn args(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_documents_every_command() {
        // Every dispatchable subcommand must appear in the help text, so a new
        // command can't ship undocumented for beta testers reading `help`.
        for cmd in [
            "create", "compile", "publish", "face", "persona", "call", "abtest", "list", "buy",
            "feedback", "probe", "triage", "threads", "forget", "whoami", "status",
            "invite", "bounty", "colony", "reputation", "guild", "company", "party", "validation", "vote", "tba",
            "room", "schedule", "goal", "remind", "jobs", "unschedule", "notify", "models", "sh",
            "onboard", "onramp", "link",
        ] {
            assert!(
                USAGE.contains(cmd),
                "`{cmd}` is dispatchable but missing from the help/USAGE text"
            );
        }
    }

    #[test]
    fn usage_lists_every_localharnesslite_command() {
        // The `sh` help blurb enumerates the lh-* surface an agent gets in a
        // bashlite shell. It's hand-written prose, so guard it against drift the
        // same way `lh-help` itself is guarded — every command must appear, or an
        // agent reading `localharness help` won't discover it.
        for cmd in [
            "lh-whoami", "lh-balance", "lh-meter", "lh-resolve", "lh-tba", "lh-price",
            "lh-list", "lh-discover", "lh-bounties", "lh-help", "lh-send",
        ] {
            assert!(USAGE.contains(cmd), "`{cmd}` is missing from the `sh` help blurb in USAGE");
        }
    }

    #[test]
    fn per_command_help_is_the_commands_own_usage_not_the_overview() {
        // `<cmd> --help` must print that command's exact syntax, not the ~400-line
        // grouped overview (live dogfood: users couldn't get one command's syntax).
        for (cmd, needle) in [
            ("remind", "--in <dur>"),
            ("call", "<target> <message>"),
            ("schedule", "--every <dur>"),
            ("bounty", "bounty <post|list"),
            ("sh", "bashlite"),
            ("whoami", "whoami [--json]"),
        ] {
            let u = command_usage(cmd).unwrap_or_else(|| panic!("`{cmd}` missing from the table"));
            assert!(u.contains(needle), "`{cmd}` usage lost its syntax line: {u}");
            assert_ne!(u, USAGE, "`{cmd}` must not return the grouped overview");
        }
        // Long-tail commands explicitly fall back to the overview.
        assert!(command_usage("version").is_none());
        assert!(command_usage("nonsense").is_none());
    }

    #[test]
    fn sponsor_key_is_valid_and_derives_documented_address() {
        // The committed testnet sponsor (now ONE home: `registry::sponsor`) pays
        // fees for EVERY sponsored CLI op on testnet. If it's stale or mistyped,
        // all onboarding silently fails. The MAINNET fee_payer is NOT embedded at
        // all — the server relay signs it (`registry::sponsor_relay`).
        let signer = registry::sponsor::fee_payer().expect("sponsor key must parse");
        let addr = bytes_to_hex_str(&wallet::address(&signer));
        assert_eq!(
            addr.to_ascii_lowercase(),
            "0x0aff88ad13ef24cac5befd0f9dc3a05df79a922c",
            "sponsor key no longer derives the documented sponsor address"
        );
    }

    #[test]
    fn llms_txt_publishes_canonical_onchain_constants() {
        // The agent-facing spec is read by agents to orient on-chain. It must
        // not drift from the code's source of truth — stale addresses would
        // send an agent to the wrong chain/contract. These handles are
        // `chain::active()`-derived, so this validates the ACTIVE chain: a default
        // run checks the Moderato values, a `--features mainnet` build checks
        // the mainnet values appear in the SERVED doc (the web bundle builds
        // `--features mainnet`, so this is what visitors' agents actually read).
        // Automates the manual "audit llms.txt vs registry.rs" pass.
        let spec = include_str!("../../../web/llms.txt");
        assert!(
            spec.contains(registry::REGISTRY_ADDRESS()),
            "llms.txt missing canonical diamond address {}",
            registry::REGISTRY_ADDRESS()
        );
        assert!(
            spec.contains(registry::LOCALHARNESS_TOKEN_ADDRESS()),
            "llms.txt missing the $LH token address {}",
            registry::LOCALHARNESS_TOKEN_ADDRESS()
        );
        assert!(
            spec.contains(registry::RPC_URL()),
            "llms.txt missing the RPC URL {}",
            registry::RPC_URL()
        );
        assert!(
            spec.contains(&registry::CHAIN_ID().to_string()),
            "llms.txt missing chain id {}",
            registry::CHAIN_ID()
        );
    }

    #[test]
    fn llms_txt_does_not_present_testnet_as_the_live_platform() {
        // The live WEB platform always runs on Tempo MAINNET (independent of
        // which chain the CLI binary was compiled for). Containing both chains'
        // addresses is fine — the testnet ones live on a clearly-labeled CLI
        // line. What must NEVER happen — and is the exact mechanism that codified
        // the chain drift — is the TESTNET (Moderato) diamond being presented as
        // the live/active web-platform address. We pin the testnet diamond and
        // assert it never appears on the authoritative "live web platform runs
        // on … Diamond proxy at <addr>" line, on EITHER build.
        let spec = include_str!("../../../web/llms.txt");
        let testnet_diamond = registry::chain::MODERATO.diamond.to_ascii_lowercase();
        for line in spec.lines() {
            let lower = line.to_ascii_lowercase();
            if lower.contains("live web platform runs on") {
                assert!(
                    !lower.contains(&testnet_diamond),
                    "llms.txt presents the TESTNET diamond {} as the live web \
                     platform address — chain drift: {line}",
                    registry::chain::MODERATO.diamond
                );
            }
        }
    }

    #[test]
    fn no_stale_per_request_price_claim() {
        // 0.47.0 decoupled $LH from dollars: a request costs 1 $LH via the meter,
        // not the old "~0.01 $LH" estimate — that 0.01 is now only the x402
        // agent-advertised DEFAULT (a different mechanism). Conflating them told
        // users ~0.01 while the meter charged 100× (found dogfooding). Guard the
        // stale tilde "~0.01 $LH" claim out of the onboarding front door AND the
        // CLI tips that quote the meter cost. skill.md additionally can't carry a
        // bare "0.01 $LH" per-message price; the CLI legitimately prints "0.01
        // $LH" (invite floor / x402 default) so its tips are checked only for the
        // tilde form. (main.rs holds these sentinels, so it isn't scanned — edit
        // its two tips beside this guard.)
        let tilde = "~0.01 $LH";
        for (src, name) in [
            (include_str!("../../../web/skill.md"), "skill.md"),
            (include_str!("call.rs"), "call.rs"),
            (include_str!("publish.rs"), "publish.rs"),
            (include_str!("notify.rs"), "notify.rs"),
        ] {
            assert!(
                !src.contains(tilde),
                "{name} contains the stale per-request price claim {tilde:?} \
                 (the meter charges 1 $LH since 0.47.0, not ~0.01)"
            );
        }
        assert!(
            !include_str!("../../../web/skill.md").contains("0.01 $LH"),
            "skill.md carries a bare \"0.01 $LH\" per-message price (1 $LH since 0.47.0)"
        );
    }
}
