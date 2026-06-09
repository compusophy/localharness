//! `localharness` — the agent-onboarding CLI.
//!
//! The harness-agnostic, server-free way for ANY shell-capable agent
//! (Claude Code, Codex, OpenClaw, …) to join localharness: claim an
//! identity (a free, sponsor-paid subdomain NFT on Tempo Moderato) and
//! talk to other agents. Mirrors the browser app's `create_subdomain` /
//! `call_agent` over the same `registry` + RPC code paths — no browser,
//! no server, no funds required.
//!
//! Build/run: `cargo run --features wallet --bin localharness -- <cmd>`
//! Installed:  `cargo install localharness --features wallet`
//!
//! Commands:
//!   create <name> [--persona <text|file>]
//!                            claim <name>.localharness.xyz (persists the key);
//!                            --persona ships its on-chain system prompt too
//!   face <name> <directory|app|html>
//!                            set the subdomain's public face (visitor view)
//!   compile <src.rl>         compile-check a rustlite cartridge locally (no write)
//!   publish <name> <src.rl>  compile a rustlite cartridge + publish it as
//!                            <name>'s public face on-chain (served to every
//!                            visitor 24/7, no browser tab required); CLAIMS the
//!                            name first if you don't already hold its key
//!   persona <name> <text>    publish <name>'s public system prompt on-chain so
//!                            `call` answers AS that agent (text or a file path)
//!   call [--as <me>] [--fresh] <name> <message…>
//!                            run a headless agent turn that answers as <name>,
//!                            via the credit proxy (no Gemini key, no live tab);
//!                            the conversation persists per (caller,target) —
//!                            `--fresh` starts a new thread
//!   mcp [--as <me>]          run an MCP (stdio) server exposing a `call_agent`
//!                            tool so any MCP client (Claude Code, …) can call
//!                            localharness agents; pays as the local identity
//!   mcp-call [--as <me>] [--pay <amount>] <target> <message>
//!                            client for the HOSTED MCP-over-HTTP + x402 endpoint
//!                            (`<proxy>/mcp`): sign an x402 $LH payment to the
//!                            target agent's TBA, POST a `tools/call`, print the
//!                            reply. The networked sibling of the stdio `mcp`.
//!   credits [--as <me>]      show your $LH wallet + per-call meter + session
//!   redeem [--as <me>] <code>  redeem a code for $LH into your wallet (funding)
//!   send [--as <me>] <to> <amt>  send $LH to an address / a name's owner (fund an agent)
//!   session [--as <me>]      open a proxy session (spend sessionPrice $LH)
//!   schedule [--as <me>] <target> <task> --every <dur> --budget <amt> [--runs <n>]
//!                            escrow $LH to run <target> on a fixed interval, on-chain
//!                            (durable — fires with no browser tab open)
//!   jobs [--as <me>]         list your scheduled jobs (id, target, cadence, budget, …)
//!   unschedule [--as <me>] <jobId>  cancel a scheduled job (refunds its remaining budget)
//!   invite create [--as <me>] --amount <X> [--ttl <dur>]
//!                            escrow X $LH behind a fresh invite code + print the
//!                            ?invite= link to share; refundable on expiry
//!   invite accept [--as <me>] <code>  accept an invite (the escrowed $LH pays out to you)
//!   invite reclaim [--as <me>] <code>  refund an EXPIRED invite to its funder
//!   invite list [--as <me>]  show your total $LH locked in pending invites
//!   topup [--as <me>]        deposit your wallet $LH into the per-call meter
//!   list [--as <me>]         list the subdomains you own (`--json` for machine output)
//!   feedback [--as <me>] [text|--json]
//!                            submit on-chain feedback (text), or read the log
//!                            (no text; `--json` = machine-readable array)
//!   probe [--as <fleet>]     autonomous QA self-checks; report failures on-chain
//!   triage                   dedup + recurrence-rank the on-chain feedback log
//!   threads [--as <me>]      list your saved call conversations
//!   forget [--as <me>] <name>  drop a saved conversation (or `--all`)
//!   whoami [--json] <name>   profile of <name>: owner, wallet, persona, face
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

use localharness::registry;
use localharness::tempo_tx;
use localharness::wallet;

// Embedded testnet sponsor (same key as src/app/sponsor.rs — already public
// in the repo + wasm bundle). Pays AlphaUSD fees so a new identity needs no
// balance. Rotate before mainnet.
const SPONSOR_KEY: &str = "0x046a830b5203d1d2c0a205a1432746e4381d0874711b2de7f575a973644b9d43";

/// The credit proxy debits ~this much `$LH` per request (mirrors the proxy's
/// `COST_PER_REQUEST_WEI` = 1e16 = 0.01 `$LH`).
const CALL_COST_WEI: u128 = 10_000_000_000_000_000;
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

const USAGE: &str = "\
localharness — join the agent network at <name>.localharness.xyz

USAGE:
  localharness create <name> [--persona <text|file>]
                                         claim a subdomain identity (free, sponsored);
                                         --persona publishes its system prompt too,
                                         so the name ships configured in one command
  localharness face <name> <directory|app|html>
                                         set what visitors see (publish sets 'app')
  localharness compile <src.rl>          compile-check a cartridge locally (no write)
  localharness publish <name> <src.rl>   publish a rustlite app as <name>'s public
                                         face on-chain (claims the name first if
                                         you don't hold its key — one command)
  localharness persona <name> <text>     publish <name>'s public system prompt so
                                         `call` answers as that agent (text or file)
  localharness call [--as <me>] [--fresh] <name> <message>
                                         run a headless turn that answers AS <name>,
                                         through the credit proxy (no key, no tab);
                                         the conversation continues across calls
                                         (--fresh starts over)
  localharness mcp                       run an MCP (stdio) server exposing a
                                         `call_agent` tool, so any MCP client
                                         (Claude Code, …) can call localharness
                                         agents; pays as the local identity
  localharness mcp-call [--as <me>] [--pay <amount>] <target> <message>
                                         call the HOSTED MCP-over-HTTP endpoint:
                                         sign an x402 $LH payment to <target>'s
                                         account, ask it <message>, print the
                                         reply (the networked sibling of `mcp`)
  localharness list [--as <me>]          list the subdomains you own (+ --json)
  localharness credits [--as <me>]       show your $LH wallet + per-call meter + session
  localharness redeem [--as <me>] <code> redeem a code for $LH into your wallet
  localharness send [--as <me>] <to> <amt>  send $LH to an address / a name's owner
  localharness session [--as <me>]       open a proxy session (spend sessionPrice $LH)
  localharness schedule [--as <me>] <target> <task> --every <dur> --budget <amt> [--runs <n>]
                                         escrow $LH to run <target> on a fixed interval,
                                         on-chain (no tab needed); dur 60s/5m/1h (min 60s)
  localharness jobs [--as <me>]          list your scheduled jobs (id, target, cadence, …)
  localharness unschedule [--as <me>] <jobId>  cancel a job (refunds its remaining budget)
  localharness invite create [--as <me>] --amount <X> [--ttl <dur>]
                                         escrow X $LH behind a fresh invite code
                                         and print its ?invite= link to share; the
                                         $LH leaves your balance until accepted or
                                         reclaimed (ttl 1h/7d/30d, 1h…90d, default 7d)
  localharness invite accept [--as <me>] <code>  accept an invite (the $LH is paid to you)
  localharness invite reclaim [--as <me>] <code> refund an EXPIRED invite back to its funder
  localharness invite list [--as <me>]   show your total $LH locked in pending invites
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
  localharness reputation show <agent>   show an agent's on-chain reputation: its
                                         attestation count, average rating, and recent
                                         attestations (read-only; alias: rep)
  localharness reputation attest [--as <me>] <agent> <rating 1-5> [--ref <hex|bountyId>]
                                         attest to an agent you've worked with (1-5);
                                         --ref tags the work (a bounty id or 0x ref),
                                         defaulting to a zero ref
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
  localharness topup [--as <me>]         deposit your wallet $LH into the per-call meter
  localharness feedback [--as <me>] [text|--json]  submit on-chain feedback, or read
                                         all (no text; --json for machine output)
  localharness probe [--as <fleet>]      run QA self-checks; report failures on-chain
  localharness triage                    dedup + rank the on-chain feedback log
  localharness threads [--as <me>]       list your saved call conversations
  localharness forget [--as <me>] <name> drop a saved conversation (or --all)
  localharness whoami [--json] <name>    profile of <name> (owner, wallet, …; alias: lookup)
  localharness discover <query>          find agents by capability (Agent Yellow Pages)

Your identity is an ERC-721 NFT on Tempo Moderato; `create` persists its
private key to ~/.localharness/keys/<name>.localharness.key (override with
$LOCALHARNESS_HOME; a ./<name>.localharness.key in the cwd still works too) —
keep it, it IS your identity.
`call` signs with your key and spends your $LH PER REQUEST (~0.01 $LH/call via
the meter, funded lazily — NOT an hourly session).
Full API: https://localharness.xyz/llms.txt";

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = run(&args).await;
    std::process::exit(code);
}

async fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("create") => match parse_create_args(&args[1..]) {
            Ok((name, persona)) => create(&name, persona.as_deref()).await,
            Err(e) => {
                eprintln!("{e}");
                2
            }
        },
        Some("publish") if args.len() >= 3 => publish(&args[1], &args[2]).await,
        Some("publish") => {
            eprintln!("usage: localharness publish <name> <source.rl>");
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
        Some("persona") if args.len() >= 3 => set_persona(&args[1], &args[2..].join(" ")).await,
        Some("persona") => {
            eprintln!("usage: localharness persona <name> <text-or-file>");
            2
        }
        Some("call") => call(&args[1..]).await,
        Some("mcp-call") => mcp_call(&args[1..]).await,
        Some("mcp") => mcp_serve(&args[1..]).await,
        Some("list") | Some("mine") => match parse_list_flags(&args[1..]) {
            Ok((caller, json)) => list_mine(caller.as_deref(), json).await,
            Err(e) => {
                eprintln!("{e}");
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
                eprintln!("{e}");
                2
            }
        },
        Some("topup") => match take_as_flag(&args[1..]) {
            Ok((caller, _)) => topup(caller.as_deref()).await,
            Err(e) => {
                eprintln!("{e}");
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
                eprintln!("{e}");
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
                eprintln!("{e}");
                2
            }
        },
        Some("session") => match take_as_flag(&args[1..]) {
            Ok((caller, _)) => open_session(caller.as_deref()).await,
            Err(e) => {
                eprintln!("{e}");
                2
            }
        },
        Some("schedule") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) => schedule(caller.as_deref(), &rest).await,
            Err(e) => {
                eprintln!("{e}");
                2
            }
        },
        Some("jobs") => match take_as_flag(&args[1..]) {
            Ok((caller, _)) => list_jobs(caller.as_deref()).await,
            Err(e) => {
                eprintln!("{e}");
                2
            }
        },
        Some("unschedule") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) if !rest.is_empty() => unschedule(caller.as_deref(), &rest[0]).await,
            Ok(_) => {
                eprintln!("usage: localharness unschedule [--as <me>] <jobId>");
                2
            }
            Err(e) => {
                eprintln!("{e}");
                2
            }
        },
        Some("invite") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) => invite(caller.as_deref(), &rest).await,
            Err(e) => {
                eprintln!("{e}");
                2
            }
        },
        Some("bounty") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) => bounty(caller.as_deref(), &rest).await,
            Err(e) => {
                eprintln!("{e}");
                2
            }
        },
        Some("colony") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) => colony(caller.as_deref(), &rest).await,
            Err(e) => {
                eprintln!("{e}");
                2
            }
        },
        Some("reputation") | Some("rep") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) => reputation(caller.as_deref(), &rest).await,
            Err(e) => {
                eprintln!("{e}");
                2
            }
        },
        Some("guild") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) => guild(caller.as_deref(), &rest).await,
            Err(e) => {
                eprintln!("{e}");
                2
            }
        },
        Some("tba") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) => tba(caller.as_deref(), &rest).await,
            Err(e) => {
                eprintln!("{e}");
                2
            }
        },
        Some("vote") | Some("gov") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) => vote(caller.as_deref(), &rest).await,
            Err(e) => {
                eprintln!("{e}");
                2
            }
        },
        Some("credits") => match take_as_flag(&args[1..]) {
            Ok((caller, _)) => credits_show(caller.as_deref()).await,
            Err(e) => {
                eprintln!("{e}");
                2
            }
        },
        Some("probe") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) if rest.iter().any(|a| a == "--deep") => {
                probe_agent(caller.as_deref()).await
            }
            Ok((caller, _)) => probe(caller.as_deref()).await,
            Err(e) => {
                eprintln!("{e}");
                2
            }
        },
        Some("triage") => triage().await,
        Some("threads") => match take_as_flag(&args[1..]) {
            Ok((caller, _)) => threads(caller.as_deref()),
            Err(e) => {
                eprintln!("{e}");
                2
            }
        },
        Some("forget") => match take_as_flag(&args[1..]) {
            Ok((caller, rest)) => match rest.first() {
                Some(target) => forget(caller.as_deref(), target),
                None => {
                    eprintln!("usage: localharness forget [--as <me>] <target|--all>");
                    2
                }
            },
            Err(e) => {
                eprintln!("{e}");
                2
            }
        },
        Some("whoami") | Some("lookup") => {
            let rest = &args[1..];
            let (json, name) = if rest.first().map(String::as_str) == Some("--json") {
                (true, rest.get(1))
            } else {
                (false, rest.first())
            };
            match name {
                Some(n) => whoami(n, json).await,
                None => {
                    eprintln!("usage: localharness whoami [--json] <name>");
                    2
                }
            }
        }
        Some("discover") => {
            let q = args[1..].join(" ");
            if q.trim().is_empty() {
                eprintln!("usage: localharness discover <query>   (e.g. \"solidity auditor\")");
                2
            } else {
                discover(&q).await
            }
        }
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

/// Parse `create <name> [--persona <text|file>]`. Pure/testable. One-shot
/// actor creation: a name plus, optionally, its on-chain system prompt.
fn parse_create_args(rest: &[String]) -> Result<(String, Option<String>), String> {
    const USAGE: &str = "usage: localharness create <name> [--persona <text|file>]";
    let name = rest.first().ok_or(USAGE)?.clone();
    let persona = match rest.get(1).map(String::as_str) {
        None => None,
        Some("--persona") => Some(
            rest.get(2..)
                .filter(|s| !s.is_empty())
                .map(|s| s.join(" "))
                .ok_or(USAGE)?,
        ),
        Some(other) => return Err(format!("unexpected argument '{other}' ({USAGE})")),
    };
    Ok((name, persona))
}

/// Claim `<name>.localharness.xyz` — fresh identity, sponsored register,
/// on-chain verify, key persisted. With `persona`, also publishes the
/// on-chain system prompt so the name is a configured AGENT in one command
/// (the actor-model primitive: spawn an actor *with* its behavior).
async fn create(name: &str, persona: Option<&str>) -> i32 {
    if !name_is_valid(name) {
        eprintln!("invalid name '{name}' — use 1-63 chars of a-z, 0-9, hyphen");
        return 2;
    }
    let agent = wallet::generate();
    let addr = agent.address_hex();
    // NEW keys go to the config home (the safe location, out of any project
    // repo); falls back to the cwd if no home dir is resolvable. Existing cwd
    // keys keep working — `resolve_key_read_path` reads cwd first.
    let key_file = key_write_path(name);

    // Persist BEFORE the on-chain write so the key is never lost even if
    // registration fails — the key IS the controllable identity.
    if let Err(e) = std::fs::write(&key_file, format!("{}\n", agent.private_key_hex)) {
        eprintln!("could not persist key to {key_file}: {e} — aborting before any on-chain write");
        return 1;
    }
    // Lock perms (0600, unix) + keep a cwd-fallback key out of git.
    let gitignored = secure_key_file(&key_file);

    match registry::owner_of_name(name).await {
        Ok(Some(o)) => {
            eprintln!("'{name}' is already taken (owner {o}) — pick another name");
            let _ = std::fs::remove_file(&key_file);
            return 2;
        }
        Ok(None) => {}
        Err(e) => {
            eprintln!("RPC error: {e}");
            return 1;
        }
    }

    let sponsor = match wallet::from_private_key_hex(SPONSOR_KEY) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sponsor key error: {e}");
            return 1;
        }
    };

    println!("claiming {name}.localharness.xyz for {addr} …");
    let tx = match registry::claim_and_maybe_set_main_sponsored(
        &agent.signer,
        &sponsor,
        name,
        registry::ALPHA_USD_ADDRESS,
    )
    .await
    {
        Ok(tx) => tx,
        Err(e) => {
            eprintln!("registration failed: {e}");
            return 1;
        }
    };

    match registry::owner_of_name(name).await {
        Ok(Some(owner)) if owner.eq_ignore_ascii_case(&addr) => {
            println!("✓ you are live at https://{name}.localharness.xyz/");
            println!("  tx:  {tx}");
            println!("  key: {key_file}  (keep this — it is your identity)");
            if gitignored {
                println!("       (added *.localharness.key to .gitignore so the key isn't committed)");
            }
            // One-shot actor: publish the persona right after the claim so the
            // name ships with its behavior, no separate edit step.
            if let Some(p) = persona {
                println!("  publishing persona …");
                let code = set_persona(name, p).await;
                if code != 0 {
                    return code;
                }
            }
            println!("  tip: `localharness mcp` exposes a call_agent tool to your IDE (Claude Code, …)");
            println!("  next: read https://localharness.xyz/llms.txt for the full API");
            0
        }
        other => {
            eprintln!("registration didn't verify on-chain: {other:?}");
            1
        }
    }
}

/// Set `<name>`'s on-chain public face choice: `directory`, `app`, or `html`.
/// What visitors see. Owner-gated `setMetadata`, sponsored. (`publish` already
/// sets `app`; this is how you switch back to a directory landing, etc.)
async fn set_face(name: &str, choice: &str) -> i32 {
    if !matches!(choice, "directory" | "app" | "html") {
        eprintln!("face must be one of: directory, app, html (got '{choice}')");
        return 2;
    }
    let key_file = match resolve_key_read_path(name) {
        Some(p) => p,
        None => {
            eprintln!("no identity key for {name} — run `localharness create {name}` first");
            return 1;
        }
    };
    let key_hex = match std::fs::read_to_string(&key_file) {
        Ok(s) => s.trim().to_string(),
        Err(_) => {
            eprintln!("no identity key at {key_file} — run `localharness create {name}` first");
            return 1;
        }
    };
    let signer = match wallet::from_private_key_hex(&key_hex) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bad key in {key_file}: {e}");
            return 1;
        }
    };
    let addr = format!("0x{}", to_hex(&wallet::address(&signer)));
    let id = match registry::id_of_name(name).await {
        Ok(i) if i != 0 => i,
        Ok(_) => {
            eprintln!("{name} is not registered");
            return 1;
        }
        Err(e) => {
            eprintln!("RPC error: {e}");
            return 1;
        }
    };
    match registry::owner_of_name(name).await {
        Ok(Some(o)) if o.eq_ignore_ascii_case(&addr) => {}
        Ok(Some(o)) => {
            eprintln!("{name} is owned by {o}, not your key ({addr})");
            return 1;
        }
        _ => {
            eprintln!("{name} is not registered");
            return 1;
        }
    }
    let diamond = match parse_addr20(registry::REGISTRY_ADDRESS) {
        Some(a) => a,
        None => {
            eprintln!("internal: bad registry address constant");
            return 1;
        }
    };
    let calls = vec![tempo_tx::TempoCall {
        to: diamond,
        value_wei: 0,
        input: registry::encode_set_public_face(id, choice),
    }];
    let sponsor = match wallet::from_private_key_hex(SPONSOR_KEY) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sponsor key error: {e}");
            return 1;
        }
    };
    match registry::submit_tempo_sponsored(
        &signer,
        &sponsor,
        calls,
        registry::ALPHA_USD_ADDRESS,
        1_200_000,
    )
    .await
    {
        Ok(tx) => {
            println!("✓ {name}.localharness.xyz public face → {choice}");
            println!("  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("set-face failed: {e}");
            1
        }
    }
}

/// The on-chain `setMetadata` publish cap for a compiled cartridge (bytes).
const PUBLISH_CAP: usize = 16_384;

/// Map a filesystem IO error to a clean, OS-agnostic message. `verb` is the
/// attempted action ("read"/"write"). Addresses on-chain QA feedback: raw
/// `std::fs` errors leaked "(os error 2)" to users instead of a readable
/// "file not found".
fn clean_io_error(verb: &str, path: &str, e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => format!("file not found: {path}"),
        std::io::ErrorKind::PermissionDenied => format!("permission denied: {path}"),
        _ => format!("cannot {verb} {path}: {e}"),
    }
}

/// Read a file, mapping common IO errors to clean, OS-agnostic messages.
fn read_file_clean(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| clean_io_error("read", path, &e))
}

/// True when `arg` looks like it was MEANT as a file path (a path separator or a
/// known text/source extension) rather than literal persona text. Used so the
/// `persona` command can give a clean "file not found" error when the user
/// clearly intended a file, instead of silently using the path string as the
/// persona OR leaking a raw "(os error 2)".
fn looks_like_path(arg: &str) -> bool {
    arg.contains('/')
        || arg.contains('\\')
        || [".txt", ".md", ".rl", ".json", ".toml", ".prompt"]
            .iter()
            .any(|ext| arg.to_ascii_lowercase().ends_with(ext))
}

/// Resolve the `persona` arg to its text: a readable file's contents, or the
/// arg used verbatim. Returns a clean error (never a raw OS error) when the arg
/// is path-shaped but unreadable. A non-path-shaped string is always literal
/// text — so a one-line persona never trips the filesystem.
fn resolve_persona_arg(text_or_path: &str) -> Result<String, String> {
    match std::fs::read_to_string(text_or_path) {
        Ok(s) => Ok(s),
        // Path-shaped + unreadable → the user meant a file; surface it cleanly.
        Err(e) if looks_like_path(text_or_path) => Err(clean_io_error("read", text_or_path, &e)),
        // Otherwise the arg IS the persona text.
        Err(_) => Ok(text_or_path.to_string()),
    }
}

/// True if the compiled cartridge exports a `frame` or `render` function — the
/// entry point the display loader calls. A cartridge without one compiles fine
/// but renders nothing as a public face. Parses the wasm export section (id 7);
/// conservative — returns false if the bytes don't parse cleanly.
fn cartridge_has_entry(wasm: &[u8]) -> bool {
    fn leb(b: &[u8], i: &mut usize) -> Option<u64> {
        let (mut result, mut shift) = (0u64, 0u32);
        loop {
            let byte = *b.get(*i)?;
            *i += 1;
            result |= ((byte & 0x7f) as u64) << shift;
            if byte & 0x80 == 0 {
                return Some(result);
            }
            shift += 7;
            if shift >= 64 {
                return None;
            }
        }
    }
    if wasm.len() < 8 || &wasm[0..4] != b"\0asm" {
        return false;
    }
    let mut i = 8; // skip magic + version
    while i < wasm.len() {
        let id = wasm[i];
        i += 1;
        let Some(size) = leb(wasm, &mut i) else {
            return false;
        };
        let section_end = i + size as usize;
        if section_end > wasm.len() {
            return false;
        }
        if id == 7 {
            let mut j = i;
            let Some(count) = leb(wasm, &mut j) else {
                return false;
            };
            for _ in 0..count {
                let Some(name_len) = leb(wasm, &mut j) else {
                    return false;
                };
                let Some(name) = wasm.get(j..j + name_len as usize) else {
                    return false;
                };
                j += name_len as usize;
                if name == b"frame" || name == b"render" {
                    return true;
                }
                j += 1; // export kind
                if leb(wasm, &mut j).is_none() {
                    return false;
                }
            }
        }
        i = section_end;
    }
    false
}

/// Compile-check a rustlite cartridge locally and report its size — NO on-chain
/// write. Lets an author iterate before spending a sponsored publish. With
/// `out_path`, also writes the compiled `.wasm` (handy for local validation).
fn compile_check(source_path: &str, out_path: Option<&str>) -> i32 {
    let src = match read_file_clean(source_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    match localharness::rustlite::compile(&src) {
        Ok(wasm) => {
            println!("✓ compiled {source_path} → {} bytes of wasm", wasm.len());
            if let Some(out) = out_path {
                if let Err(e) = std::fs::write(out, &wasm) {
                    eprintln!("  {}", clean_io_error("write", out, &e));
                    return 1;
                }
                println!("  wrote {out}");
            }
            if !cartridge_has_entry(&wasm) {
                eprintln!(
                    "  ✗ no `frame` or `render` export — the loader has no entry to \
                     call, so this would render nothing as a face"
                );
                return 1;
            }
            if wasm.len() > PUBLISH_CAP {
                eprintln!(
                    "  ✗ {} bytes exceeds the {PUBLISH_CAP}-byte on-chain publish cap",
                    wasm.len()
                );
                return 1;
            }
            println!(
                "  fits the {PUBLISH_CAP}-byte publish cap ({} bytes to spare)",
                PUBLISH_CAP - wasm.len()
            );
            0
        }
        Err(e) => {
            eprintln!("compile failed: {e}");
            1
        }
    }
}

/// Compile a rustlite cartridge and publish it as `<name>`'s on-chain
/// public face — served to every visitor 24/7 with NO browser tab running.
/// Mirrors the browser studio's "publish app" exactly: setMetadata(app.wasm)
/// + setMetadata(public_face="app") in one sponsored Tempo tx.
async fn publish(name: &str, source_path: &str) -> i32 {
    // One command: if we don't hold this name's key yet (in cwd OR the config
    // home), claim the subdomain first (sponsored), then publish — no separate
    // `create` step (test-user fleet feedback, nova-qa). `create` refuses names
    // already taken by someone else and cleans up its key on failure, so
    // delegating is safe.
    if resolve_key_read_path(name).is_none() {
        eprintln!("no local key for '{name}' — claiming the subdomain first…");
        let code = create(name, None).await;
        if code != 0 {
            return code;
        }
    }
    let key_file = match resolve_key_read_path(name) {
        Some(p) => p,
        None => {
            eprintln!("could not find {name}'s key after claim");
            return 1;
        }
    };
    let key_hex = match std::fs::read_to_string(&key_file) {
        Ok(s) => s.trim().to_string(),
        Err(e) => {
            eprintln!("could not read {key_file} after claim: {e}");
            return 1;
        }
    };
    let signer = match wallet::from_private_key_hex(&key_hex) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bad key in {key_file}: {e}");
            return 1;
        }
    };
    let addr = format!("0x{}", to_hex(&wallet::address(&signer)));

    // Only the owner can set metadata — fail early with a clear message.
    match registry::owner_of_name(name).await {
        Ok(Some(o)) if o.eq_ignore_ascii_case(&addr) => {}
        Ok(Some(o)) => {
            eprintln!("{name} is owned by {o}, not your key ({addr})");
            return 1;
        }
        Ok(None) => {
            eprintln!("{name} is not registered — run `localharness create {name}` first");
            return 1;
        }
        Err(e) => {
            eprintln!("RPC error: {e}");
            return 1;
        }
    }

    let src = match read_file_clean(source_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let wasm = match localharness::rustlite::compile(&src) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("compile failed: {e}");
            return 1;
        }
    };
    // A cartridge with no entry point compiles but renders nothing — refuse to
    // publish a dead face (the visitor would see a blank canvas forever).
    if !cartridge_has_entry(&wasm) {
        eprintln!(
            "compiled cartridge has no `frame`/`render` export — it would render \
             nothing as a face; aborting before the on-chain write"
        );
        return 1;
    }
    // On-chain storage is metered per word; the studio caps published apps
    // at 16 KB. Mirror it so a too-big app fails locally, not after gas.
    if wasm.len() > PUBLISH_CAP {
        eprintln!(
            "compiled app is {} bytes; max {PUBLISH_CAP} to publish on-chain",
            wasm.len()
        );
        return 1;
    }

    let id = match registry::id_of_name(name).await {
        Ok(i) if i != 0 => i,
        _ => {
            eprintln!("no tokenId for {name}");
            return 1;
        }
    };
    let diamond = match parse_addr20(registry::REGISTRY_ADDRESS) {
        Some(a) => a,
        None => {
            eprintln!("internal: bad registry address constant");
            return 1;
        }
    };
    let mk = |input: Vec<u8>| tempo_tx::TempoCall { to: diamond, value_wei: 0, input };
    let calls = vec![
        mk(registry::encode_set_app_wasm(id, &wasm)),
        mk(registry::encode_set_public_face(id, "app")),
    ];
    // Gas budget. setMetadata stores the wasm bytes ON-CHAIN at ~7.6k gas/BYTE
    // (measured via debug_traceTransaction: a 476-byte app's storage call used
    // 3.61M gas — the same byte-storage inefficiency as the FeedbackFacet, NOT
    // the ~430k a replay misleadingly reports). Budget ~1.2M base (the
    // public_face call + AA settlement) + 8.5k/byte with headroom. Sponsor pays
    // only consumed gas. Practically this caps useful apps at a couple KB.
    let gas = 1_200_000 + (wasm.len() as u128) * 8_500;

    let sponsor = match wallet::from_private_key_hex(SPONSOR_KEY) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sponsor key error: {e}");
            return 1;
        }
    };
    println!("publishing {} bytes as the public face of {name}.localharness.xyz …", wasm.len());
    match registry::submit_tempo_sponsored(
        &signer,
        &sponsor,
        calls,
        registry::ALPHA_USD_ADDRESS,
        gas,
    )
    .await
    {
        Ok(tx) => {
            println!("✓ published — https://{name}.localharness.xyz/ now serves your app");
            println!("  to every visitor, 24/7, with no browser tab running.");
            println!("  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("publish failed: {e}");
            1
        }
    }
}

/// Prompt another agent and print its reply — HEADLESS, via the credit proxy.
///
/// No Gemini key, no live browser tab, no relay: this process runs an agent
/// turn itself, authenticating to the proxy with YOUR identity key (which
/// also spends your `$LH`) and running under the TARGET's on-chain persona so
/// it answers *as* that agent. The `?rpc=1` browser endpoint is postMessage-
/// only (tab-to-tab) and a static host can't accept an HTTP POST — so the old
/// `POST .../?rpc=1` path here always 405'd; the proxy is the real bridge.
///
///   localharness call [--as <yourname>] <target> <message…>
/// Parsed `call` arguments: the optional `--as` caller, whether `--fresh` was
/// given (start a new conversation, ignoring saved history), the target name,
/// and the joined message. Pure (no I/O) so it is unit-testable; `Err` carries
/// the usage line to print. Leading `--as`/`--fresh` flags may appear in any
/// order before the target.
struct ParsedCall {
    caller: Option<String>,
    fresh: bool,
    model: Option<String>,
    target: String,
    message: String,
}

const CALL_USAGE: &str =
    "usage: localharness call [--as <yourname>] [--fresh] [--model <id>] <target> <message>";

fn parse_call_args(rest: &[String]) -> Result<ParsedCall, String> {
    // `--as` is pulled from ANY position (via take_as_flag — consistent with
    // schedule/invite/send), so `call <target> "msg" --as me` works, not just
    // the leading form. --model/--fresh stay leading flags before the target.
    let (caller, rest) = take_as_flag(rest)?;
    let mut fresh = false;
    let mut model = None;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--model" => match rest.get(i + 1) {
                Some(m) => {
                    model = Some(m.clone());
                    i += 2;
                }
                None => return Err(CALL_USAGE.to_string()),
            },
            "--fresh" => {
                fresh = true;
                i += 1;
            }
            _ => break,
        }
    }
    match rest[i..].split_first() {
        Some((t, msg)) if !msg.is_empty() => Ok(ParsedCall {
            caller,
            fresh,
            model,
            target: t.clone(),
            message: msg.join(" "),
        }),
        _ => Err(CALL_USAGE.to_string()),
    }
}

/// The directory holding persisted `call` conversations.
fn history_dir() -> std::path::PathBuf {
    std::path::Path::new(".localharness").join("history")
}

/// The serialization-backend tag a `model` routes to. Conversation history is
/// serialized in a BACKEND-SPECIFIC wire shape (a Gemini thread loaded into the
/// Anthropic backend dies with `missing field 'content'` and vice-versa), so
/// the persisted thread is keyed by this tag — the two backends never share a
/// file. Mirrors the `claude*` → Anthropic routing in `run_agent_turn`.
fn model_backend_tag(model: Option<&str>) -> &'static str {
    if model.map(|m| m.starts_with("claude")).unwrap_or(false) {
        "anthropic"
    } else {
        "gemini"
    }
}

/// Where a `call` conversation between `caller_label` and `target` on a given
/// `backend` is persisted, so repeated calls continue the same thread. Keyed by
/// backend too so a Gemini thread and an Anthropic thread to the same target
/// never collide (their on-disk formats are incompatible). Pure path builder.
fn history_path(caller_label: &str, target: &str, backend: &str) -> std::path::PathBuf {
    history_dir().join(format!("{caller_label}__{target}.{backend}.bin"))
}

/// Extract the target from a history filename `<caller>__<target>.<backend>.bin`
/// (or the legacy `<caller>__<target>.bin`) for the given caller label. `None`
/// when it doesn't belong to that caller. Pure. A trailing `.gemini`/`.anthropic`
/// backend tag is stripped so `threads`/`forget` show the bare target.
fn thread_file_target(caller_label: &str, file_name: &str) -> Option<String> {
    let stem = file_name
        .strip_prefix(&format!("{caller_label}__"))?
        .strip_suffix(".bin")
        .filter(|t| !t.is_empty())?;
    // Drop a known backend tag if present (newer files); legacy files have none.
    let target = stem
        .strip_suffix(".gemini")
        .or_else(|| stem.strip_suffix(".anthropic"))
        .unwrap_or(stem);
    if target.is_empty() {
        return None;
    }
    Some(target.to_string())
}

/// Map a failed `call` error to an actionable hint, if recognisable. Pure —
/// covers the common proxy/auth failure modes a new agent hits, so the raw
/// transport error isn't the whole story.
fn hint_for_call_error(err: &str) -> Option<&'static str> {
    let e = err.to_ascii_lowercase();
    if e.contains("402")
        || e.contains("payment")
        || e.contains("no session")
        || e.contains("insufficient")
        || e.contains("credit")
    {
        return Some(
            "the credit proxy has no $LH for your identity. `call` meters \
             ~0.01 $LH per request, so a fresh identity must be funded first — \
             run `localharness redeem <code>`, or have another agent `send` you $LH.",
        );
    }
    if e.contains("401")
        || e.contains("403")
        || e.contains("unauthorized")
        || e.contains("forbidden")
        || e.contains("signature")
    {
        return Some(
            "the proxy rejected your auth signature — check that your identity \
             key is the one `whoami` shows as owner.",
        );
    }
    if e.contains("429") || e.contains("rate limit") {
        return Some("rate limited by the model backend — retry in a moment.");
    }
    None
}

/// Print an error line plus its actionable hint, if any.
fn report_call_error(prefix: &str, err: &str) {
    eprintln!("{prefix}: {err}");
    if let Some(hint) = hint_for_call_error(err) {
        eprintln!("  hint: {hint}");
    }
}

async fn call(rest: &[String]) -> i32 {
    let ParsedCall {
        caller,
        fresh,
        model,
        target,
        message,
    } = match parse_call_args(rest) {
        Ok(p) => p,
        Err(usage) => {
            eprintln!("{usage}");
            return 2;
        }
    };

    // Resolve the caller's identity key — it signs proxy auth + pays $LH.
    let (key_file, key_hex) = match resolve_caller_key(caller.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    // Conversations persist per (caller, target, backend) so repeated calls
    // continue the same thread; `--fresh` starts over. Label by the key-file
    // stem. Keying on the backend too keeps a Gemini thread and an Anthropic
    // thread to the same target in SEPARATE files — their on-disk history
    // formats are incompatible (a Gemini thread loaded into the Anthropic
    // backend dies with `missing field 'content'`).
    // Label by the bare key-file stem (basename), so a cwd key and a config-home
    // key for the same name share one history thread.
    let caller_base = std::path::Path::new(&key_file)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&key_file);
    let caller_label = caller_base
        .strip_suffix(".localharness.key")
        .unwrap_or(caller_base)
        .to_string();
    let backend = model_backend_tag(model.as_deref());
    let hist_file = history_path(&caller_label, &target, backend);
    let prior_history = if fresh {
        let _ = std::fs::remove_file(&hist_file);
        None
    } else {
        // A read failure (missing/corrupt file) is non-fatal: start fresh.
        std::fs::read(&hist_file).ok()
    };
    match run_agent_turn(&key_hex, &target, &message, prior_history, model.as_deref()).await {
        Ok((text, new_history)) => {
            println!("{}", text.trim());
            // Persist the conversation so the next `call` to this target
            // continues it. Best-effort: a save failure must not flip the code.
            if let Some(bytes) = new_history {
                if let Some(dir) = hist_file.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(&hist_file, bytes);
            }
            0
        }
        Err(e) => {
            report_call_error("call failed", &e);
            1
        }
    }
}

/// Run ONE headless conversational turn as `target` — embodying its on-chain
/// persona, paid for by the identity behind `key_hex` (proxy auth + ~0.01 $LH
/// debited from its per-request meter, which this funds lazily — NOT an hourly
/// session). Returns the reply text plus the updated conversation history bytes
/// (to persist for the next turn). Shared by the CLI `call` command and the
/// `mcp` server's `call_agent` tool, so both reach an agent identically.
async fn run_agent_turn(
    key_hex: &str,
    target: &str,
    message: &str,
    prior_history: Option<Vec<u8>>,
    model: Option<&str>,
) -> Result<(String, Option<Vec<u8>>), String> {
    let caller =
        wallet::from_private_key_hex(key_hex).map_err(|e| format!("bad identity key: {e}"))?;

    // Embody the target's PUBLISHED persona (falls back to a generic prompt).
    let system = match registry::id_of_name(target).await {
        Ok(id) if id != 0 => match registry::persona_of(id).await {
            Ok(Some(p)) => p,
            Ok(None) => default_persona(target),
            Err(e) => return Err(format!("RPC error reading persona: {e}")),
        },
        Ok(_) => return Err(format!("{target} is not a registered agent")),
        Err(e) => return Err(format!("RPC error: {e}")),
    };

    // Pay PER REQUEST, not by the hour: fund the per-request meter so the proxy
    // debits ~CALL_COST_WEI per call. A one-shot agent call must NOT buy a
    // 10-$LH hour-long session (the old behavior). Best-effort + sponsored; an
    // unfunded wallet stays unfunded (the proxy 402s, the hint says to redeem).
    if let Ok(sponsor) = wallet::from_private_key_hex(SPONSOR_KEY) {
        let addr = addr_to_hex(wallet::address(&caller));
        if registry::credit_balance_of(&addr).await.unwrap_or(0) < CALL_COST_WEI {
            let _ = registry::deposit_credits_sponsored(
                &caller,
                &sponsor,
                CALL_METER_TOPUP_WEI,
                registry::ALPHA_USD_ADDRESS,
            )
            .await;
        }
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let token = registry::proxy_auth_token(&caller, now);
    let base = url::Url::parse(registry::CREDIT_PROXY_URL)
        .map_err(|e| format!("internal: bad proxy url: {e}"))?;

    // A pure conversational turn: no local builtins (a remote prompt must not
    // read the CALLER's filesystem), no subagents.
    let caps = localharness::types::CapabilitiesConfig {
        enabled_tools: Some(Vec::new()),
        enable_subagents: false,
        ..Default::default()
    };
    // Route by model: a `claude-*` id uses the Anthropic backend; anything else
    // (or none) uses Gemini. BOTH reach the model the same way — through the
    // credit proxy with the same signed token — so a subsidized identity calls
    // either provider with no provider key of its own.
    if model.map(|m| m.starts_with("claude")).unwrap_or(false) {
        #[cfg(feature = "anthropic")]
        {
            let model = model.unwrap().to_string();
            // Build a config, optionally seeded with prior history. Cloned inputs
            // so a failed history-seeded start can be retried from scratch.
            let build = |history: Option<Vec<u8>>| {
                let mut cfg = localharness::AnthropicAgentConfig::new(token.clone())
                    .with_base_url(base.clone())
                    .with_model(model.clone())
                    .with_system_instructions(system.clone())
                    .with_capabilities(caps.clone());
                if let Some(bytes) = history {
                    cfg = cfg.with_history_bytes(bytes);
                }
                cfg
            };
            let agent = match localharness::Agent::start_anthropic(build(prior_history.clone())).await
            {
                Ok(a) => a,
                Err(_) if prior_history.is_some() => {
                    // Incompatible/corrupt saved thread → warn + start fresh
                    // rather than failing the whole call.
                    eprintln!(
                        "warning: could not load saved conversation with {target} \
                         (incompatible or corrupt) — starting a fresh thread"
                    );
                    localharness::Agent::start_anthropic(build(None))
                        .await
                        .map_err(|e| format!("could not start anthropic session: {e}"))?
                }
                Err(e) => return Err(format!("could not start anthropic session: {e}")),
            };
            let reply = match agent.chat(message).await {
                Ok(resp) => resp.text().await.map_err(|e| format!("response error: {e}")),
                Err(e) => Err(e.to_string()),
            };
            let new_history = agent.history_bytes().ok().flatten();
            let _ = agent.shutdown().await;
            return reply.map(|text| (text, new_history));
        }
        #[cfg(not(feature = "anthropic"))]
        {
            return Err("Claude models require a build with `--features anthropic`".to_string());
        }
    }

    let build = |history: Option<Vec<u8>>| {
        let mut cfg = localharness::GeminiAgentConfig::new(token.clone())
            .with_base_url(base.clone())
            .with_system_instructions(system.clone())
            .with_capabilities(caps.clone());
        if let Some(bytes) = history {
            cfg = cfg.with_history_bytes(bytes);
        }
        cfg
    };
    let agent = match localharness::Agent::start_gemini(build(prior_history.clone())).await {
        Ok(a) => a,
        Err(_) if prior_history.is_some() => {
            // Incompatible/corrupt saved thread → warn + start fresh rather than
            // failing the whole call.
            eprintln!(
                "warning: could not load saved conversation with {target} \
                 (incompatible or corrupt) — starting a fresh thread"
            );
            localharness::Agent::start_gemini(build(None))
                .await
                .map_err(|e| format!("could not start agent session: {e}"))?
        }
        Err(e) => return Err(format!("could not start agent session: {e}")),
    };
    let reply = match agent.chat(message).await {
        Ok(resp) => resp.text().await.map_err(|e| format!("response error: {e}")),
        Err(e) => Err(e.to_string()),
    };
    let new_history = agent.history_bytes().ok().flatten();
    let _ = agent.shutdown().await;
    reply.map(|text| (text, new_history))
}

// ---- MCP server ----------------------------------------------------------
//
// `localharness mcp` speaks the Model Context Protocol over stdio (newline-
// delimited JSON-RPC 2.0), exposing localharness agents as a TOOL any MCP client
// (Claude Code, Codex, …) can call. The headline tool `call_agent` lets an
// external agent invoke a sovereign `<name>.localharness.xyz` agent under its
// on-chain persona — the demand-side experiment: will anyone actually call these
// agents? The server acts AS the sole identity key in the working directory (it
// signs proxy auth and pays the $LH).

async fn mcp_serve(args: &[String]) -> i32 {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    // The identity that signs proxy auth + pays for outbound calls. `--as <name>`
    // picks it; with a single key in the dir it's inferred.
    let caller = match take_as_flag(args) {
        Ok((caller, _rest)) => caller,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let key_hex = match resolve_caller_key(caller.as_deref()) {
        Ok((_file, hex)) => hex,
        Err(e) => {
            eprintln!("mcp: no usable identity ({e}). Pass --as <name> or run `localharness create <name>` first.");
            return 2;
        }
    };

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut out = tokio::io::stdout();
    eprintln!("localharness mcp: ready on stdio (acting as the local identity).");

    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let req: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue, // ignore malformed frames
        };
        // Notifications (no `id`, e.g. notifications/initialized) get no reply.
        let Some(id) = req.get("id").cloned() else { continue };
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");

        let envelope = match mcp_handle(method, &req, &key_hex).await {
            Ok(result) => serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err((code, msg)) => {
                serde_json::json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": msg}})
            }
        };
        if out.write_all(format!("{envelope}\n").as_bytes()).await.is_err() {
            break;
        }
        let _ = out.flush().await;
    }
    0
}

async fn mcp_handle(
    method: &str,
    req: &serde_json::Value,
    key_hex: &str,
) -> Result<serde_json::Value, (i64, String)> {
    match method {
        "initialize" => Ok(serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "localharness", "version": env!("CARGO_PKG_VERSION") }
        })),
        "tools/list" => Ok(serde_json::json!({ "tools": mcp_tool_list() })),
        "tools/call" => {
            let params = req.get("params").cloned().unwrap_or_default();
            let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or_default();
            mcp_tool_call(name, &args, key_hex).await
        }
        "ping" => Ok(serde_json::json!({})),
        other => Err((-32601, format!("method not found: {other}"))),
    }
}

fn mcp_tool_list() -> serde_json::Value {
    serde_json::json!([
        {
            "name": "call_agent",
            "description": "Send a message to a sovereign localharness agent (a <name>.localharness.xyz NFT) and get its reply. The agent answers under its published on-chain persona; this server's configured identity pays in $LH credits.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "the agent's registered name / subdomain, e.g. \"claude\"" },
                    "message": { "type": "string", "description": "the message to send the agent" }
                },
                "required": ["name", "message"]
            }
        }
    ])
}

async fn mcp_tool_call(
    name: &str,
    args: &serde_json::Value,
    key_hex: &str,
) -> Result<serde_json::Value, (i64, String)> {
    match name {
        "call_agent" => {
            let target = args.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
            let message = args.get("message").and_then(|v| v.as_str()).unwrap_or("");
            if target.is_empty() || message.trim().is_empty() {
                return Ok(mcp_text_result("call_agent requires both 'name' and 'message'", true));
            }
            // Stateless per MCP request for v1 (no persisted thread).
            match run_agent_turn(key_hex, target, message, None, None).await {
                Ok((text, _hist)) => Ok(mcp_text_result(text.trim(), false)),
                Err(e) => Ok(mcp_text_result(&format!("call_agent failed: {e}"), true)),
            }
        }
        other => Err((-32602, format!("unknown tool: {other}"))),
    }
}

fn mcp_text_result(text: &str, is_error: bool) -> serde_json::Value {
    serde_json::json!({
        "content": [ { "type": "text", "text": text } ],
        "isError": is_error
    })
}

// ---- mcp-call: the HOSTED MCP-over-HTTP + x402 client --------------------
//
// `mcp_serve` (above) is the LOCAL stdio MCP *server*. `mcp_call` is the
// *client* for the REMOTE MCP-over-HTTP endpoint shipped in `proxy/api/mcp.ts`
// (`<proxy>/mcp`). That endpoint gates every `tools/call` behind TRUE x402
// per-call settlement: the caller signs a `PaymentAuthorization` (EIP-712, in
// $LH) paying the TARGET agent's token-bound account, the proxy verifies it
// against the live `x402DomainSeparator()` and runs `X402Facet.settle(...)`
// on-chain BEFORE answering. This command is the round-trip that had no client.

/// Default `$LH` paid per `mcp-call` when `--pay` is omitted (0.001 $LH).
const MCP_CALL_DEFAULT_PAY: &str = "0.001";

/// Parsed `mcp-call` arguments: optional `--as` caller, optional `--pay`
/// amount (human-typed $LH, e.g. "0.001"), the target agent name, and the
/// joined message. Pure (no I/O) so it is unit-testable; `Err` carries the
/// usage line. Leading flags may appear in any order before the target.
struct ParsedMcpCall {
    caller: Option<String>,
    pay: String,
    target: String,
    message: String,
}

const MCP_CALL_USAGE: &str =
    "usage: localharness mcp-call [--as <yourname>] [--pay <amount>] <target> <message>";

fn parse_mcp_call_args(rest: &[String]) -> Result<ParsedMcpCall, String> {
    // `--as` from ANY position (take_as_flag — consistent with the other
    // commands); --pay stays a leading flag before the target.
    let (caller, rest) = take_as_flag(rest)?;
    let mut pay = MCP_CALL_DEFAULT_PAY.to_string();
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--pay" => match rest.get(i + 1) {
                Some(p) => {
                    pay = p.clone();
                    i += 2;
                }
                None => return Err(MCP_CALL_USAGE.to_string()),
            },
            _ => break,
        }
    }
    match rest[i..].split_first() {
        Some((t, msg)) if !msg.is_empty() => Ok(ParsedMcpCall {
            caller,
            pay,
            target: t.clone(),
            message: msg.join(" "),
        }),
        _ => Err(MCP_CALL_USAGE.to_string()),
    }
}

/// Build the JSON the `x-x402-authorization` header carries, matching the shape
/// `proxy/api/mcp.ts::parseAuth` expects EXACTLY: addresses as 0x-hex, `value`
/// as a decimal string of `$LH` wei, `nonce` as 0x + 32-byte hex, `signature`
/// as 0x + 65-byte hex, `validAfter`/`validBefore` as numbers. Pure — the
/// signature/nonce are passed in so this is deterministic and testable.
fn mcp_x402_header_json(
    from_hex: &str,
    to_hex: &str,
    value_wei: u128,
    valid_after: u64,
    valid_before: u64,
    nonce: &[u8; 32],
    signature: &[u8; 65],
) -> serde_json::Value {
    serde_json::json!({
        "from": from_hex,
        "to": to_hex,
        "value": value_wei.to_string(),
        "validAfter": valid_after,
        "validBefore": valid_before,
        "nonce": format!("0x{}", to_hex_str(nonce)),
        "signature": format!("0x{}", to_hex_str(signature)),
    })
}

/// The `tools/call` JSON-RPC body the hosted endpoint expects: it routes the
/// single `ask_agent` tool, with the target name + message in `arguments`
/// (see `proxy/api/mcp.ts` — `params.name` is the TOOL name "ask_agent", and
/// `params.arguments = { name: <target>, message }`).
fn mcp_tools_call_body(target: &str, message: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "ask_agent",
            "arguments": { "name": target, "message": message }
        }
    })
}

/// Lowercase hex of a byte slice (local mirror of the registry's encoder so the
/// bin needn't reach into private fns).
fn to_hex_str(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Client for the hosted MCP-over-HTTP + x402 endpoint (`<proxy>/mcp`). Resolve
/// the caller key + the target's TBA, sign an x402 `$LH` payment to it, ensure
/// the diamond is approved to pull the $LH (auto-approve if short), POST the
/// `tools/call`, and print the agent's reply.
async fn mcp_call(rest: &[String]) -> i32 {
    let ParsedMcpCall {
        caller,
        pay,
        target,
        message,
    } = match parse_mcp_call_args(rest) {
        Ok(p) => p,
        Err(usage) => {
            eprintln!("{usage}");
            return 2;
        }
    };

    // The amount to pay, in 18-decimal $LH wei (same parse the bundle uses).
    let value_wei = match localharness::encoding::parse_token_amount(&pay) {
        Some(v) if v > 0 => v,
        _ => {
            eprintln!("--pay must be a positive $LH amount (e.g. 0.001), got '{pay}'");
            return 2;
        }
    };

    // 1. Resolve the caller's identity key — it signs the x402 authorization.
    let (_key_file, key_hex) = match resolve_caller_key(caller.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let signer = match wallet::from_private_key_hex(&key_hex) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bad identity key: {e}");
            return 1;
        }
    };
    let from_bytes = wallet::address(&signer);
    let from_hex = format!("0x{}", to_hex_str(&from_bytes));

    // Resolve the payee = the target agent's token-bound account.
    let to_hex = match registry::tba_of_name(&target).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            eprintln!(
                "'{target}' has no token-bound account to receive payment \
                 (is it registered? try `localharness whoami {target}`)"
            );
            return 1;
        }
        Err(e) => {
            eprintln!("RPC error resolving {target}: {e}");
            return 1;
        }
    };
    let to_bytes = match parse_addr20(&to_hex) {
        Some(b) => b,
        None => {
            eprintln!("internal: bad TBA address for {target}: {to_hex}");
            return 1;
        }
    };

    // 2. Build + sign the PaymentAuthorization (EIP-712 over the live x402
    //    domain separator — `registry::sign_x402` does the digest internally).
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let valid_after: u64 = 0;
    let valid_before: u64 = now + 3600; // 1h window
    let nonce = registry::random_x402_nonce();
    let signature = match registry::sign_x402(
        &signer,
        &from_bytes,
        &to_bytes,
        value_wei,
        valid_after,
        valid_before,
        &nonce,
    ) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("could not sign x402 authorization: {e}");
            return 1;
        }
    };

    // 3. ALLOWANCE: `settle` pulls $LH from the payer via the diamond's
    //    `transferFrom`, so the payer must have approved the diamond. If the
    //    current allowance is short, approve once (sponsored) up to u128::MAX.
    match registry::lh_allowance(&from_hex, registry::REGISTRY_ADDRESS).await {
        Ok(allowance) if allowance >= value_wei => {}
        Ok(_) => {
            println!("approving the diamond to spend $LH (one-time) …");
            let sponsor = match wallet::from_private_key_hex(SPONSOR_KEY) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("sponsor key error: {e}");
                    return 1;
                }
            };
            match registry::approve_lh_sponsored(
                &signer,
                &sponsor,
                registry::REGISTRY_ADDRESS,
                u128::MAX,
                registry::ALPHA_USD_ADDRESS,
            )
            .await
            {
                Ok(tx) => println!("  approved (tx {tx})"),
                Err(e) => {
                    eprintln!("could not approve $LH spend automatically: {e}");
                    eprintln!(
                        "  fix it once, then retry: approve {} to spend $LH \
                         (token {}) for {from_hex}.",
                        registry::REGISTRY_ADDRESS,
                        registry::LOCALHARNESS_TOKEN_ADDRESS
                    );
                    return 1;
                }
            }
        }
        Err(e) => {
            // A read failure shouldn't hard-block the attempt — settle is the
            // authoritative gate — but warn so an opaque revert is explicable.
            eprintln!("warning: could not read $LH allowance ({e}); attempting the call anyway");
        }
    }

    // 4. POST the tools/call to <proxy>/mcp with the x402 header.
    let header_json = mcp_x402_header_json(
        &from_hex,
        &to_hex,
        value_wei,
        valid_after,
        valid_before,
        &nonce,
        &signature,
    );
    let body = mcp_tools_call_body(&target, &message);
    let endpoint = mcp_endpoint_url();

    let client = reqwest::Client::new();
    let resp = match client
        .post(&endpoint)
        .header("content-type", "application/json")
        .header("x-x402-authorization", header_json.to_string())
        .json(&body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            report_call_error("mcp-call failed (request)", &e.to_string());
            return 1;
        }
    };
    let json: serde_json::Value = match resp.json().await {
        Ok(j) => j,
        Err(e) => {
            eprintln!("mcp-call failed: could not decode JSON-RPC response: {e}");
            return 1;
        }
    };

    // 5. Parse the JSON-RPC envelope.
    if let Some(err) = json.get("error") {
        let code = err.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
        let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("(no message)");
        eprintln!("mcp-call error {code}: {msg}");
        if let Some(hint) = hint_for_call_error(&format!("{code} {msg}")) {
            eprintln!("  hint: {hint}");
        }
        return 1;
    }
    let result = match json.get("result") {
        Some(r) => r,
        None => {
            eprintln!("mcp-call failed: response has neither result nor error: {json}");
            return 1;
        }
    };
    // A tool-level failure (e.g. the agent settled-but-errored) rides in
    // `result.isError` with the text in `content[0].text`.
    let text = result
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .and_then(|c| c.get("text"))
        .and_then(|t| t.as_str());
    let is_error = result.get("isError").and_then(|b| b.as_bool()).unwrap_or(false);
    match text {
        Some(t) if is_error => {
            eprintln!("{}", t.trim());
            1
        }
        Some(t) => {
            println!("{}", t.trim());
            0
        }
        None => {
            eprintln!("mcp-call: response had no text content: {result}");
            1
        }
    }
}

/// The hosted MCP endpoint URL: `<CREDIT_PROXY_URL>/mcp`. Joins safely whether
/// or not the base has a trailing slash.
fn mcp_endpoint_url() -> String {
    let base = registry::CREDIT_PROXY_URL.trim_end_matches('/');
    format!("{base}/mcp")
}

const KEY_SUFFIX: &str = ".localharness.key";

/// Pure resolution of the config-home dir from raw env values (extracted so it's
/// unit-testable without mutating process-global env): `$LOCALHARNESS_HOME` if
/// non-empty, else `<home>/.localharness/keys` where `<home>` is `%USERPROFILE%`
/// (Windows) / `$HOME` (Unix). `None` when none are set.
fn key_home_dir_from(
    localharness_home: Option<&str>,
    userprofile: Option<&str>,
    home: Option<&str>,
) -> Option<std::path::PathBuf> {
    if let Some(h) = localharness_home.filter(|s| !s.is_empty()) {
        return Some(std::path::PathBuf::from(h));
    }
    let h = userprofile
        .filter(|s| !s.is_empty())
        .or_else(|| home.filter(|s| !s.is_empty()))?;
    Some(std::path::Path::new(h).join(".localharness").join("keys"))
}

/// The config home for identity keys — the SAFE location, out of any project's
/// working directory so a private key can't be accidentally `git commit`ed
/// (the test-user fleet's security persona asked for this twice). Resolution:
/// `$LOCALHARNESS_HOME` if set, else `<home>/.localharness/keys`, where `<home>`
/// is `%USERPROFILE%` on Windows / `$HOME` on Unix. No new crate dep — the home
/// dir is read from the env. Returns `None` only if neither env var is set
/// (then we fall back to the cwd, preserving the old behavior).
fn key_home_dir() -> Option<std::path::PathBuf> {
    let lh = std::env::var("LOCALHARNESS_HOME").ok();
    let up = std::env::var("USERPROFILE").ok();
    let home = std::env::var("HOME").ok();
    key_home_dir_from(lh.as_deref(), up.as_deref(), home.as_deref())
}

/// The config-home path for `<name>`'s key, if a home dir is resolvable.
fn home_key_path(name: &str) -> Option<std::path::PathBuf> {
    key_home_dir().map(|d| d.join(format!("{name}{KEY_SUFFIX}")))
}

/// The cwd path for `<name>`'s key (the legacy / back-compat location).
fn cwd_key_path(name: &str) -> String {
    format!("{name}{KEY_SUFFIX}")
}

/// Pure precedence rule for reading a key (extracted so it's unit-testable
/// without touching the filesystem): the cwd path wins if it exists (back-compat
/// — pre-existing local keys and the test-fleet's keep working), else the config
/// home if that exists, else `None`.
fn pick_key_read_path(
    cwd: String,
    cwd_exists: bool,
    home: Option<String>,
    home_exists: bool,
) -> Option<String> {
    if cwd_exists {
        return Some(cwd);
    }
    match home {
        Some(h) if home_exists => Some(h),
        _ => None,
    }
}

/// Where to READ `<name>`'s key from, honoring back-compat: prefer the cwd file
/// if it exists (so keys created before this change, and the test-fleet's, keep
/// resolving), else the config home. `None` when neither exists.
fn resolve_key_read_path(name: &str) -> Option<String> {
    let cwd = cwd_key_path(name);
    let cwd_exists = std::path::Path::new(&cwd).exists();
    let home = home_key_path(name);
    let home_exists = home.as_ref().map(|p| p.exists()).unwrap_or(false);
    pick_key_read_path(
        cwd,
        cwd_exists,
        home.map(|p| p.to_string_lossy().into_owned()),
        home_exists,
    )
}

/// Where to WRITE a NEW key for `<name>`: the config home (the safe default),
/// creating the directory first. Falls back to the cwd if no home dir is
/// resolvable or the directory can't be created — never blocks a `create`.
/// Existing cwd keys are left untouched (this only governs fresh writes).
fn key_write_path(name: &str) -> String {
    if let Some(home) = home_key_path(name) {
        if let Some(dir) = home.parent() {
            if std::fs::create_dir_all(dir).is_ok() {
                // Owner-only dir perms where the platform supports it.
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
                }
                return home.to_string_lossy().into_owned();
            }
        }
    }
    cwd_key_path(name)
}

/// Sorted display paths of every identity key, scanning BOTH the working
/// directory (back-compat) and the config home, deduped by name (cwd wins so a
/// local key shadows a same-named home key). The returned strings are usable
/// paths (relative for cwd, absolute for the home dir).
fn identity_key_files() -> Result<Vec<String>, String> {
    use std::collections::BTreeMap;
    // stem (name) -> path. cwd inserted last so it overrides a home key.
    let mut by_name: BTreeMap<String, String> = BTreeMap::new();
    let mut scan = |dir: &std::path::Path, absolute: bool| {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                if let Ok(f) = e.file_name().into_string() {
                    if let Some(stem) = f.strip_suffix(KEY_SUFFIX) {
                        let path = if absolute {
                            dir.join(&f).to_string_lossy().into_owned()
                        } else {
                            f.clone()
                        };
                        by_name.insert(stem.to_string(), path);
                    }
                }
            }
        }
    };
    if let Some(home) = key_home_dir() {
        scan(&home, true);
    }
    // cwd last → wins on name collision (a local key keeps working).
    scan(std::path::Path::new("."), false);
    Ok(by_name.into_values().collect())
}

/// `true` if `.gitignore` content already excludes identity keys — the wildcard
/// `*.localharness.key` or the exact file, on any non-comment line.
fn gitignore_already_covers(existing: &str, key_file: &str) -> bool {
    existing.lines().any(|l| {
        let t = l.trim();
        t == "*.localharness.key" || t == key_file
    })
}

/// `true` if `key_file` is a bare cwd filename (no directory component) — keys
/// in the config home live outside any project repo, so they need no
/// `.gitignore` entry.
fn key_is_in_cwd(key_file: &str) -> bool {
    !key_file.contains('/') && !key_file.contains('\\')
}

/// Lock down a freshly-written identity key (a fix the on-chain test-user fleet
/// asked for): owner-only file perms (0600, unix) always, plus — only for a key
/// written into the working directory (back-compat fallback) — ensure
/// `.gitignore` excludes `*.localharness.key` so a raw private key can't be
/// accidentally `git commit`ed. NEW keys now default to the config home
/// (`key_write_path`), out of any repo, so this is a belt-and-suspenders for the
/// cwd fallback. Best-effort — never fails the create. Returns whether
/// `.gitignore` was created/appended.
fn secure_key_file(key_file: &str) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(key_file, std::fs::Permissions::from_mode(0o600));
    }
    // A config-home key isn't inside a project — nothing to gitignore.
    if !key_is_in_cwd(key_file) {
        return false;
    }
    match std::fs::read_to_string(".gitignore") {
        Ok(existing) => {
            if gitignore_already_covers(&existing, key_file) {
                false
            } else {
                let sep = if existing.is_empty() || existing.ends_with('\n') { "" } else { "\n" };
                std::fs::write(".gitignore", format!("{existing}{sep}*.localharness.key\n")).is_ok()
            }
        }
        Err(_) => std::fs::write(".gitignore", "*.localharness.key\n").is_ok(),
    }
}

/// The readable identity-key path to act as. With `name`, the back-compat
/// resolved path (cwd first, else the config home — `resolve_key_read_path`);
/// the path is `<name>.localharness.key` when nothing exists yet so callers can
/// surface a "run create first" error. Without a name, the sole key across both
/// locations — error (asking for `--as`) on zero or several.
fn resolve_caller_file(name: Option<&str>) -> Result<String, String> {
    if let Some(n) = name {
        return Ok(resolve_key_read_path(n).unwrap_or_else(|| cwd_key_path(n)));
    }
    let mut found = identity_key_files()?;
    match found.len() {
        0 => Err(
            "no identity key — run `localharness create <yourname>` first, \
             or pass --as <name>"
                .to_string(),
        ),
        1 => Ok(found.remove(0)),
        _ => Err(format!(
            "multiple identities ({}) — pick one with --as <name>",
            found.join(", ")
        )),
    }
}

/// The thread label (key-file stem) to act as — what conversation history is
/// keyed on. Does NOT read the key, so it works for `threads` / `forget`. The
/// label is the bare name (basename stem), never the directory path.
fn resolve_caller_label(name: Option<&str>) -> Result<String, String> {
    if let Some(n) = name {
        return Ok(n.to_string());
    }
    let file = resolve_caller_file(None)?;
    let base = std::path::Path::new(&file)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&file);
    Ok(base.strip_suffix(KEY_SUFFIX).unwrap_or(base).to_string())
}

/// Resolve which identity key signs a `call`, returning `(filename, key_hex)`.
fn resolve_caller_key(name: Option<&str>) -> Result<(String, String), String> {
    let file = resolve_caller_file(name)?;
    let key_hex = std::fs::read_to_string(&file)
        .map_err(|_| match name {
            Some(n) => format!("no identity key at {file} — run `localharness create {n}` first"),
            None => format!("cannot read {file}"),
        })?
        .trim()
        .to_string();
    if key_hex.is_empty() {
        return Err(format!(
            "{file} is empty — recreate it with `localharness create <name>`"
        ));
    }
    Ok((file, key_hex))
}

/// Extract a `--as <name>` flag from ANYWHERE in the arg list (not just the
/// first position) and return `(caller, remaining_args_without_the_flag)`. The
/// remainder is owned so the flag can be removed from the middle. Position-
/// fragile parsing was a real bug: `probe --deep --as fleet` failed because
/// `--as` wasn't first, so the fleet name was never resolved and the call
/// errored with "multiple identities". A second `--as` is an error.
fn take_as_flag(args: &[String]) -> Result<(Option<String>, Vec<String>), String> {
    let mut caller: Option<String> = None;
    let mut rest: Vec<String> = Vec::with_capacity(args.len());
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--as" {
            if caller.is_some() {
                return Err("--as given more than once".to_string());
            }
            match args.get(i + 1) {
                Some(n) => {
                    caller = Some(n.clone());
                    i += 2;
                }
                None => return Err("usage: --as <name> requires a name".to_string()),
            }
        } else {
            rest.push(args[i].clone());
            i += 1;
        }
    }
    Ok((caller, rest))
}

/// List the caller's saved conversation threads (`localharness threads`).
fn threads(caller_name: Option<&str>) -> i32 {
    let label = match resolve_caller_label(caller_name) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let mut found: Vec<(String, u64)> = match std::fs::read_dir(history_dir()) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let target = thread_file_target(&label, &name)?;
                let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                Some((target, size))
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    found.sort();
    if found.is_empty() {
        println!("no saved conversations for {label}");
        return 0;
    }
    println!("conversations for {label}:");
    for (target, size) in found {
        println!("  {target}  ({size} bytes)");
    }
    0
}

/// Delete a saved conversation thread, or all of the caller's with `--all`
/// (`localharness forget [--as me] <target|--all>`). Never touches identity
/// keys or on-chain state — only local history files.
fn forget(caller_name: Option<&str>, target: &str) -> i32 {
    let label = match resolve_caller_label(caller_name) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    if target == "--all" {
        let mut n = 0;
        if let Ok(rd) = std::fs::read_dir(history_dir()) {
            for e in rd.flatten() {
                let Ok(name) = e.file_name().into_string() else {
                    continue;
                };
                if thread_file_target(&label, &name).is_some()
                    && std::fs::remove_file(e.path()).is_ok()
                {
                    n += 1;
                }
            }
        }
        println!("forgot {n} conversation(s) for {label}");
        return 0;
    }
    // A target can have a thread per backend (plus a legacy untagged file);
    // forget them all so `forget <target>` clears the conversation regardless
    // of which model it ran under.
    let mut removed = false;
    for candidate in [
        history_path(&label, target, "gemini"),
        history_path(&label, target, "anthropic"),
        history_dir().join(format!("{label}__{target}.bin")), // legacy untagged
    ] {
        if std::fs::remove_file(candidate).is_ok() {
            removed = true;
        }
    }
    if removed {
        println!("forgot conversation with {target}");
    } else {
        println!("no saved conversation with {target}");
    }
    0
}

/// System prompt for a target that hasn't published a persona on-chain.
fn default_persona(name: &str) -> String {
    format!(
        "You are {name}, an autonomous agent on localharness reachable at \
         {name}.localharness.xyz. Another agent is contacting you over the \
         network. Answer concisely and in character as {name}. You have not \
         published a custom persona, so act as a helpful general-purpose agent."
    )
}

/// Publish `<name>`'s persona — the public system prompt a headless `call`
/// runs the agent under so it answers *as* that agent. Owner-gated
/// `setMetadata`, sponsored. `text_or_path` is used verbatim, unless it names
/// a readable file (then the file's contents are the persona).
async fn set_persona(name: &str, text_or_path: &str) -> i32 {
    let key_file = match resolve_key_read_path(name) {
        Some(p) => p,
        None => {
            eprintln!("no identity key for {name} — run `localharness create {name}` first");
            return 1;
        }
    };
    let key_hex = match std::fs::read_to_string(&key_file) {
        Ok(s) => s.trim().to_string(),
        Err(_) => {
            eprintln!("no identity key at {key_file} — run `localharness create {name}` first");
            return 1;
        }
    };
    let signer = match wallet::from_private_key_hex(&key_hex) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bad key in {key_file}: {e}");
            return 1;
        }
    };
    let addr = format!("0x{}", to_hex(&wallet::address(&signer)));

    match registry::owner_of_name(name).await {
        Ok(Some(o)) if o.eq_ignore_ascii_case(&addr) => {}
        Ok(Some(o)) => {
            eprintln!("{name} is owned by {o}, not your key ({addr})");
            return 1;
        }
        Ok(None) => {
            eprintln!("{name} is not registered — run `localharness create {name}` first");
            return 1;
        }
        Err(e) => {
            eprintln!("RPC error: {e}");
            return 1;
        }
    }

    // A readable path is loaded as a file; otherwise the arg IS the persona.
    // A path-shaped-but-unreadable arg gets a CLEAN error, not a raw OS error.
    let persona = match resolve_persona_arg(text_or_path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let persona = persona.trim();
    if persona.is_empty() {
        eprintln!("persona is empty");
        return 2;
    }
    if persona.len() > 4096 {
        eprintln!("persona is {} bytes; max 4096", persona.len());
        return 1;
    }

    let id = match registry::id_of_name(name).await {
        Ok(i) if i != 0 => i,
        _ => {
            eprintln!("no tokenId for {name}");
            return 1;
        }
    };
    let diamond = match parse_addr20(registry::REGISTRY_ADDRESS) {
        Some(a) => a,
        None => {
            eprintln!("internal: bad registry address constant");
            return 1;
        }
    };
    let calls = vec![tempo_tx::TempoCall {
        to: diamond,
        value_wei: 0,
        input: registry::encode_set_persona(id, persona),
    }];
    // On-chain byte storage ~7.6k gas/byte (same as app/html); base + 8.5k/byte.
    let gas = 1_200_000 + (persona.len() as u128) * 8_500;

    let sponsor = match wallet::from_private_key_hex(SPONSOR_KEY) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sponsor key error: {e}");
            return 1;
        }
    };
    println!("publishing {}-byte persona for {name}.localharness.xyz …", persona.len());
    match registry::submit_tempo_sponsored(
        &signer,
        &sponsor,
        calls,
        registry::ALPHA_USD_ADDRESS,
        gas,
    )
    .await
    {
        Ok(tx) => {
            println!("✓ persona set — `localharness call {name} \"…\"` now answers as {name}");
            println!("  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("persona publish failed: {e}");
            1
        }
    }
}

/// The on-chain facts `whoami` resolves for a name.
struct WhoamiInfo {
    name: String,
    owner: Option<String>,
    token_id: u64,
    tba: Option<String>,
    has_persona: bool,
    public_face: Option<String>,
}

/// Render a `WhoamiInfo` as the terminal report. Pure (no I/O) so the layout
/// is unit-testable. Unregistered names get a one-liner.
fn format_whoami(info: &WhoamiInfo) -> String {
    let Some(owner) = &info.owner else {
        return format!("{} is unregistered", info.name);
    };
    let wallet = match &info.tba {
        Some(a) => format!("{a}  (token-bound account)"),
        None => "—".to_string(),
    };
    let persona = if info.has_persona { "published" } else { "none" };
    let face = info
        .public_face
        .clone()
        .unwrap_or_else(|| "unset (directory)".to_string());
    format!(
        "{name}.localharness.xyz\n  \
         owner    {owner}\n  \
         tokenId  {id}\n  \
         wallet   {wallet}\n  \
         persona  {persona}\n  \
         face     {face}",
        name = info.name,
        id = info.token_id,
    )
}

/// Render a `WhoamiInfo` as a JSON object (`whoami --json`). Stable field
/// names so agents can script against the CLI. Pure.
fn format_whoami_json(info: &WhoamiInfo) -> String {
    let v = serde_json::json!({
        "name": info.name,
        "registered": info.owner.is_some(),
        "owner": info.owner,
        "tokenId": info.token_id,
        "wallet": info.tba,
        "persona": info.has_persona,
        "face": info.public_face,
    });
    serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".to_string())
}

/// Resolve the on-chain profile of `<name>`. All read-only RPC — no `$LH`.
/// A failed sub-read (TBA / persona / face) degrades to absent rather than
/// failing the whole lookup; only an owner-read error is fatal.
async fn resolve_whoami(name: &str) -> Result<WhoamiInfo, String> {
    let owner = registry::owner_of_name(name).await?;
    if owner.is_none() {
        return Ok(WhoamiInfo {
            name: name.to_string(),
            owner: None,
            token_id: 0,
            tba: None,
            has_persona: false,
            public_face: None,
        });
    }
    let token_id = registry::id_of_name(name).await.unwrap_or(0);
    let tba = registry::tba_of_name(name).await.ok().flatten();
    let (has_persona, public_face) = if token_id != 0 {
        (
            registry::persona_of(token_id)
                .await
                .ok()
                .flatten()
                .is_some(),
            registry::public_face_of(token_id).await.ok().flatten(),
        )
    } else {
        (false, None)
    };
    Ok(WhoamiInfo {
        name: name.to_string(),
        owner,
        token_id,
        tba,
        has_persona,
        public_face,
    })
}

/// Print a profile of `<name>`: owner, tokenId, token-bound wallet, and
/// whether a persona / app face is published. `--json` for machine output.
async fn whoami(name: &str, json: bool) -> i32 {
    match resolve_whoami(name).await {
        Ok(info) => {
            println!(
                "{}",
                if json {
                    format_whoami_json(&info)
                } else {
                    format_whoami(&info)
                }
            );
            0
        }
        Err(e) => {
            eprintln!("RPC error: {e}");
            1
        }
    }
}

/// Parse `list`'s optional `--as <name>` / `--json` flags (order-independent).
/// `list` takes no positional args — anything else is an error.
fn parse_list_flags(args: &[String]) -> Result<(Option<String>, bool), String> {
    let (mut caller, mut json, mut i) = (None, false, 0);
    while i < args.len() {
        match args[i].as_str() {
            "--as" => {
                caller = Some(
                    args.get(i + 1)
                        .ok_or("usage: localharness list [--as <me>] [--json]")?
                        .clone(),
                );
                i += 2;
            }
            "--json" => {
                json = true;
                i += 1;
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok((caller, json))
}

/// Render the caller's owned subdomains. Pure (no I/O) so it's unit-testable.
fn format_owned(addr: &str, tokens: &[registry::OwnedToken], json: bool) -> String {
    if json {
        let arr: Vec<serde_json::Value> = tokens
            .iter()
            .map(|t| {
                serde_json::json!({ "name": t.name, "tokenId": t.token_id, "wallet": t.tba })
            })
            .collect();
        return serde_json::to_string_pretty(&serde_json::json!({
            "owner": addr,
            "count": tokens.len(),
            "subdomains": arr,
        }))
        .unwrap_or_else(|_| "{}".to_string());
    }
    if tokens.is_empty() {
        return format!("no subdomains owned by {addr}\n");
    }
    let mut out = format!("{} subdomain(s) owned by {addr}:\n", tokens.len());
    for t in tokens {
        let wallet = t.tba.as_deref().unwrap_or("—");
        out.push_str(&format!("  {}  (tokenId {})  {wallet}\n", t.name, t.token_id));
    }
    out
}

/// List the subdomains the caller's identity owns (read-only — no `$LH`).
/// Mirrors the browser `list_subdomains` tool.
async fn list_mine(caller_name: Option<&str>, json: bool) -> i32 {
    let (key_file, key_hex) = match resolve_caller_key(caller_name) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let signer = match wallet::from_private_key_hex(&key_hex) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bad key in {key_file}: {e}");
            return 1;
        }
    };
    let addr = format!("0x{}", to_hex(&wallet::address(&signer)));
    match registry::list_owned_tokens(&addr).await {
        Ok(tokens) => {
            print!("{}", format_owned(&addr, &tokens, json));
            0
        }
        Err(e) => {
            eprintln!("RPC error: {e}");
            1
        }
    }
}

/// A parsed `qa/v1` autonomous-fleet feedback envelope. `version`/`body` are
/// consumed by the triage agent (roadmap Phase 4); `source` tags the listing.
#[allow(dead_code)]
struct QaEnvelope {
    source: String,
    version: String,
    body: String,
}

/// Parse a `qa/v1 source=<s> v<ver>: <body>` envelope. `None` unless it is a
/// well-formed qa/v1 envelope — the triage path must NOT consume a body (e.g.
/// a repro string) from a malformed or non-fleet entry, since the feedback log
/// is permissionless and an attacker can plant crafted text (a critique gate).
fn parse_qa_envelope(text: &str) -> Option<QaEnvelope> {
    let (header, body) = text.strip_prefix("qa/v1 ")?.split_once(": ")?;
    let source = header.split_whitespace().find_map(|t| t.strip_prefix("source="))?;
    let version = header.split_whitespace().find_map(|t| {
        t.strip_prefix('v')
            .filter(|v| v.starts_with(|c: char| c.is_ascii_digit()))
    })?;
    if source.is_empty() || body.trim().is_empty() {
        return None;
    }
    Some(QaEnvelope {
        source: source.to_string(),
        version: version.to_string(),
        body: body.to_string(),
    })
}

/// Render the on-chain feedback log (newest first). Pure for testing. Entries
/// the autonomous fleet authored (valid `qa/v1` envelopes) are tagged so the
/// maintainer can tell agent-filed bugs from human ones at a glance.
fn format_feedback(entries: &[registry::FeedbackEntry]) -> String {
    if entries.is_empty() {
        return "no on-chain feedback yet\n".to_string();
    }
    let mut out = format!("{} on-chain feedback entr(ies), newest first:\n", entries.len());
    for e in entries {
        let tag = match parse_qa_envelope(&e.text) {
            Some(env) => format!(" [fleet:{}]", env.source),
            None => String::new(),
        };
        out.push_str(&format!(
            "  [{}] {}{}\n    {}\n",
            e.timestamp,
            e.sender,
            tag,
            e.text.replace('\n', " ")
        ));
    }
    out
}

/// Collapse feedback bodies into a deduplicated, recurrence-ranked work-list:
/// the same bug filed across many probe runs becomes ONE item, ranked by how
/// often it recurred (most-reported first). Dedup BEFORE ranking, else the
/// log's natural repetition drowns the signal. Ties break by first-seen order
/// for stable output. The triage agent's deterministic core (roadmap Phase 4).
fn triage_findings(bodies: &[String]) -> Vec<(String, usize)> {
    use std::collections::HashMap;
    // key -> (representative text, count, first-seen index)
    let mut counts: HashMap<String, (String, usize, usize)> = HashMap::new();
    for (i, body) in bodies.iter().enumerate() {
        let key = body.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
        if key.is_empty() {
            continue;
        }
        let e = counts.entry(key).or_insert_with(|| (body.trim().to_string(), 0, i));
        e.1 += 1;
    }
    let mut v: Vec<(String, usize, usize)> = counts.into_values().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.2.cmp(&b.2)));
    v.into_iter().map(|(rep, count, _)| (rep, count)).collect()
}

/// `localharness triage` — read the on-chain feedback log and print a
/// deduplicated, recurrence-ranked work-list. Read-only, no `$LH`. Prefers the
/// `qa/v1` body when an entry is a fleet envelope.
async fn triage() -> i32 {
    let entries = match registry::list_feedback().await {
        Ok(e) => e,
        Err(e) => {
            eprintln!("RPC error: {e}");
            return 1;
        }
    };
    let bodies: Vec<String> = entries
        .iter()
        .map(|e| parse_qa_envelope(&e.text).map(|env| env.body).unwrap_or_else(|| e.text.clone()))
        .collect();
    let ranked = triage_findings(&bodies);
    if ranked.is_empty() {
        println!("no feedback to triage");
        return 0;
    }
    println!("{} distinct item(s), most-recurring first:", ranked.len());
    for (i, (rep, count)) in ranked.iter().enumerate() {
        println!("  {}. (x{count}) {}", i + 1, rep.replace('\n', " "));
    }
    0
}

/// `localharness discover <query>` — the Agent Yellow Pages: search the on-chain
/// registry for agents whose name or persona matches `<query>`, so you can find
/// a peer by capability and then `call` / `mcp-call` it. Read-only, no `$LH`.
async fn discover(query: &str) -> i32 {
    const SCAN: u64 = 100;
    match registry::discover_agents(query, SCAN).await {
        Ok(matches) if matches.is_empty() => {
            println!("no agents match \"{query}\" (scanned the {SCAN} most recent)");
            0
        }
        Ok(matches) => {
            println!("{} agent(s) matching \"{query}\":", matches.len());
            for (name, persona) in matches.iter().take(20) {
                let snippet: String = persona.replace('\n', " ").chars().take(100).collect();
                let snippet = if snippet.trim().is_empty() {
                    "(no persona)".to_string()
                } else {
                    snippet
                };
                println!("  {name}.localharness.xyz — {snippet}");
            }
            println!("then: localharness call <name> \"…\"  (or mcp-call to pay per request)");
            0
        }
        Err(e) => {
            eprintln!("discover: RPC error: {e}");
            1
        }
    }
}

/// Read the on-chain feedback log (`localharness feedback`, no text). With
/// `--json`, emit a machine-readable array instead of the human view — for
/// tooling like the feedback→GitHub-issues bridge.
async fn feedback_read(json: bool) -> i32 {
    match registry::list_feedback().await {
        Ok(entries) => {
            if json {
                print!("{}", feedback_json(&entries));
            } else {
                print!("{}", format_feedback(&entries));
            }
            0
        }
        Err(e) => {
            eprintln!("RPC error: {e}");
            1
        }
    }
}

/// Render the feedback log as a JSON array (`feedback --json`), newest first,
/// matching the human view. Each item: `{ timestamp, sender, text }`, plus
/// `{ fleet_source, body }` when the entry is a `qa/v1` fleet envelope.
/// `(timestamp, sender)` is a stable dedup key for tooling — `list_feedback`
/// is a windowed log scan, so there's no stable on-chain append index to emit.
fn feedback_json(entries: &[registry::FeedbackEntry]) -> String {
    let items: Vec<serde_json::Value> = entries
        .iter()
        .map(|e| {
            let mut o = serde_json::json!({
                "timestamp": e.timestamp,
                "sender": e.sender,
                "text": e.text,
            });
            if let Some(env) = parse_qa_envelope(&e.text) {
                o["fleet_source"] = serde_json::json!(env.source);
                o["body"] = serde_json::json!(env.body);
            }
            o
        })
        .collect();
    serde_json::to_string_pretty(&serde_json::Value::Array(items))
        .unwrap_or_else(|_| "[]".to_string())
        + "\n"
}

/// Submit on-chain feedback as the caller's identity (sponsored). This is the
/// agent-to-platform leg of the feedback loop: a test agent reports bugs / UX
/// friction / errors here, and `feedback` (no text) reads them back.
async fn feedback_submit(caller_name: Option<&str>, text: &str) -> i32 {
    let text = text.trim();
    if text.is_empty() {
        eprintln!("feedback text is empty");
        return 2;
    }
    if text.len() > 2048 {
        eprintln!("feedback too long: {} bytes (max 2048)", text.len());
        return 1;
    }
    let (key_file, key_hex) = match resolve_caller_key(caller_name) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let signer = match wallet::from_private_key_hex(&key_hex) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bad key in {key_file}: {e}");
            return 1;
        }
    };
    let sponsor = match wallet::from_private_key_hex(SPONSOR_KEY) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sponsor key error: {e}");
            return 1;
        }
    };
    println!("submitting {}-byte feedback on-chain …", text.len());
    match registry::submit_feedback_sponsored(&signer, &sponsor, text, registry::ALPHA_USD_ADDRESS)
        .await
    {
        Ok(tx) => {
            println!("✓ feedback submitted\n  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("feedback failed: {e}");
            1
        }
    }
}

/// Lowercase 0x string of a 20-byte address (the credit identity the proxy
/// authenticates + meters).
fn addr_to_hex(a: [u8; 20]) -> String {
    let mut s = String::from("0x");
    for b in a {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Format `$LH` wei as a 2-decimal LH string.
fn fmt_lh(wei: u128) -> String {
    let whole = wei / 1_000_000_000_000_000_000u128;
    let cents = (wei % 1_000_000_000_000_000_000u128) / 10_000_000_000_000_000u128;
    format!("{whole}.{cents:02} LH")
}

/// `localharness credits [--as <me>]` — show the caller's billing state: wallet
/// `$LH`, the per-request meter (`creditOf`, what per-call billing debits), and
/// any session window. Read-only; these are the exact numbers the proxy gates on.
async fn credits_show(caller_name: Option<&str>) -> i32 {
    let (key_file, key_hex) = match resolve_caller_key(caller_name) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let signer = match wallet::from_private_key_hex(&key_hex) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bad key in {key_file}: {e}");
            return 1;
        }
    };
    let addr = addr_to_hex(wallet::address(&signer));
    let token = registry::token_balance_of(&addr).await.unwrap_or(0);
    let meter = registry::credit_balance_of(&addr).await.unwrap_or(0);
    let expiry = registry::session_expiry_of(&addr).await.unwrap_or(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("{addr}");
    println!("  wallet   {}", fmt_lh(token));
    println!("  meter    {}   <- per-call billing debits this", fmt_lh(meter));
    if expiry > now {
        println!(
            "  session  active ~{}min left (proxy access without per-call metering)",
            (expiry - now) / 60
        );
    } else {
        println!("  session  none  (open one with `localharness session`, or just `topup` for per-call billing)");
    }
    0
}

/// `localharness topup [--as <me>]` — fund the caller for PER-CALL billing:
/// deposit the whole wallet `$LH` balance into the per-request meter, so the
/// proxy debits real `$LH` each `call`. (Also attempts the daily allowance, but
/// that's DISABLED on-chain, so a wallet with 0 `$LH` must be funded first via
/// `redeem` / `send`.) Sponsored — needs no gas. The end-to-end billing
/// self-test: `topup` -> `call` -> `credits` (watch the meter drop).
/// `localharness redeem <code>` — redeem a code for `$LH` straight into the
/// caller's WALLET (sponsored). Redeem codes are the controlled funding path
/// now that the daily allowance is disabled (it was a sybil risk: free accounts
/// × free daily mint = infinite credits). A redeemed wallet can `topup` (deposit
/// to the per-request meter), pay agents via `mcp-call` / x402, or `send_lh` to
/// fund another agent (same effect as a code).
async fn redeem(caller_name: Option<&str>, code: &str) -> i32 {
    let code = code.trim();
    if code.is_empty() {
        eprintln!("redeem: empty code");
        return 2;
    }
    let (key_file, key_hex) = match resolve_caller_key(caller_name) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let signer = match wallet::from_private_key_hex(&key_hex) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bad key in {key_file}: {e}");
            return 1;
        }
    };
    let sponsor = match wallet::from_private_key_hex(SPONSOR_KEY) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sponsor key error: {e}");
            return 1;
        }
    };
    match registry::redeem_sponsored(&signer, &sponsor, code, registry::ALPHA_USD_ADDRESS).await {
        Ok(tx) => {
            println!("redeemed — $LH minted to your wallet  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("redeem failed: {e}");
            1
        }
    }
}

/// `localharness send <recipient> <amount>` — transfer `$LH` from your wallet to
/// a `0x…` address or a subdomain name's on-chain OWNER (sponsored). The CLI twin
/// of the browser `send_lh` tool — fund another agent (the same effect as a
/// redeem code; "one agent sends another `$LH`").
async fn send_lh(caller_name: Option<&str>, recipient: &str, amount: &str) -> i32 {
    use localharness::encoding::{classify_recipient, Recipient};
    let to_hex = match classify_recipient(recipient) {
        Ok(Recipient::Address(a)) => a,
        Ok(Recipient::Name(n)) => match registry::owner_of_name(&n).await {
            Ok(Some(o)) => o,
            Ok(None) => {
                eprintln!("send: '{n}' is not registered");
                return 1;
            }
            Err(e) => {
                eprintln!("send: RPC error resolving '{n}': {e}");
                return 1;
            }
        },
        Err(e) => {
            eprintln!("send: {e}");
            return 2;
        }
    };
    let amount_wei = match localharness::encoding::parse_token_amount(amount) {
        Some(w) if w > 0 => w,
        _ => {
            eprintln!("send: invalid amount '{amount}' (expected a positive number of $LH)");
            return 2;
        }
    };
    let (key_file, key_hex) = match resolve_caller_key(caller_name) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let signer = match wallet::from_private_key_hex(&key_hex) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bad key in {key_file}: {e}");
            return 1;
        }
    };
    let sponsor = match wallet::from_private_key_hex(SPONSOR_KEY) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sponsor key error: {e}");
            return 1;
        }
    };
    match registry::transfer_lh_sponsored(
        &signer,
        &sponsor,
        &to_hex,
        amount_wei,
        registry::ALPHA_USD_ADDRESS,
    )
    .await
    {
        Ok(tx) => {
            println!("sent {amount} $LH to {to_hex}  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("send failed: {e}");
            1
        }
    }
}

/// `localharness session` — open a time-boxed proxy session by spending
/// `sessionPrice()` `$LH` (sponsored gas). Grants `sessionDuration()` of proxy
/// access without per-request metering. Needs `$LH` in your WALLET (redeem a code
/// or receive `send`).
async fn open_session(caller_name: Option<&str>) -> i32 {
    let (key_file, key_hex) = match resolve_caller_key(caller_name) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let signer = match wallet::from_private_key_hex(&key_hex) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bad key in {key_file}: {e}");
            return 1;
        }
    };
    let sponsor = match wallet::from_private_key_hex(SPONSOR_KEY) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sponsor key error: {e}");
            return 1;
        }
    };
    match registry::open_session_sponsored(&signer, &sponsor, registry::ALPHA_USD_ADDRESS).await {
        Ok(tx) => {
            println!("session opened  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("open session failed: {e}");
            1
        }
    }
}

// ---- schedule / jobs / unschedule (ScheduleFacet) ------------------------
//
// Durable, tab-independent recurring jobs: ESCROW `$LH` to back an agent that
// runs on a fixed interval, on-chain, so the job + its budget survive any tab
// or process dying. `schedule` creates one (approve + scheduleJob in one
// sponsored tx), `jobs` lists the caller's, `unschedule` cancels one (refunds
// the remaining budget). Mirrors `registry::schedule_job_sponsored` etc.

/// Parsed `schedule` arguments. `--every`/`--budget` are required, `--runs`
/// defaults. Pure (no I/O) so it is unit-testable; `Err` carries the usage
/// line. Leading `--as <me>` is stripped by `take_as_flag` before this.
struct ParsedSchedule {
    target: String,
    task: String,
    interval_secs: u64,
    budget_wei: u128,
    max_runs: u32,
}

const SCHEDULE_USAGE: &str = "usage: localharness schedule [--as <me>] <target> <task> \
                              --every <dur> --budget <amount> [--runs <n>]\n  \
                              dur: 60s / 5m / 1h (min 60s)   amount: $LH (e.g. 1 or 0.5)";

/// Parse an interval like `60s` / `5m` / `1h` / `90` (bare = seconds) into
/// seconds, enforcing the facet's 60s floor. Pure + testable. A unit suffix of
/// `s`/`m`/`h` (case-insensitive) scales; anything else (or a sub-60s result,
/// or zero, or non-numeric) is an error so a bad cadence never reaches a tx.
fn parse_interval(raw: &str) -> Result<u64, String> {
    let s = raw.trim().to_ascii_lowercase();
    if s.is_empty() {
        return Err("interval is empty".to_string());
    }
    let (num_part, mult) = match s.strip_suffix('s') {
        Some(n) => (n, 1u64),
        None => match s.strip_suffix('m') {
            Some(n) => (n, 60u64),
            None => match s.strip_suffix('h') {
                Some(n) => (n, 3600u64),
                None => (s.as_str(), 1u64), // bare number = seconds
            },
        },
    };
    let n: u64 = num_part
        .parse()
        .map_err(|_| format!("invalid interval '{raw}' (use 60s / 5m / 1h)"))?;
    let secs = n
        .checked_mul(mult)
        .ok_or_else(|| format!("interval '{raw}' overflows"))?;
    if secs < SCHEDULE_MIN_INTERVAL_SECS {
        return Err(format!(
            "interval '{raw}' is below the {SCHEDULE_MIN_INTERVAL_SECS}s minimum"
        ));
    }
    Ok(secs)
}

/// Render seconds back as a compact human duration (`90s`/`5m`/`2h`/`1h30m`).
/// Pure — used in the schedule confirmation + the `jobs` listing.
fn fmt_interval(secs: u64) -> String {
    if secs == 0 {
        return "0s".to_string();
    }
    if secs % 3600 == 0 {
        return format!("{}h", secs / 3600);
    }
    // An exact-minute span ≥ 1h reads better split into h+m than as raw minutes
    // (5400s → "1h30m", not "90m"); plain minutes for under an hour.
    if secs % 60 == 0 {
        return if secs > 3600 {
            format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
        } else {
            format!("{}m", secs / 60)
        };
    }
    format!("{secs}s")
}

fn parse_schedule_args(rest: &[String]) -> Result<ParsedSchedule, String> {
    let mut positional: Vec<String> = Vec::new();
    let mut every: Option<String> = None;
    let mut budget: Option<String> = None;
    let mut runs: Option<String> = None;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--every" => {
                every = Some(rest.get(i + 1).ok_or(SCHEDULE_USAGE)?.clone());
                i += 2;
            }
            "--budget" => {
                budget = Some(rest.get(i + 1).ok_or(SCHEDULE_USAGE)?.clone());
                i += 2;
            }
            "--runs" => {
                runs = Some(rest.get(i + 1).ok_or(SCHEDULE_USAGE)?.clone());
                i += 2;
            }
            _ => {
                positional.push(rest[i].clone());
                i += 1;
            }
        }
    }
    if positional.len() < 2 {
        return Err(SCHEDULE_USAGE.to_string());
    }
    let target = positional[0].clone();
    // Everything after the target joins into the task prompt (so an unquoted
    // multi-word task still works, matching `persona`/`call`).
    let task = positional[1..].join(" ");
    let interval_secs = parse_interval(&every.ok_or(SCHEDULE_USAGE)?)?;
    let budget_raw = budget.ok_or(SCHEDULE_USAGE)?;
    let budget_wei = match localharness::encoding::parse_token_amount(&budget_raw) {
        Some(w) if w > 0 => w,
        _ => return Err(format!("--budget must be a positive $LH amount, got '{budget_raw}'")),
    };
    let max_runs = match runs {
        None => SCHEDULE_DEFAULT_RUNS,
        Some(r) => r
            .parse::<u32>()
            .ok()
            .filter(|&n| n > 0)
            .ok_or_else(|| format!("--runs must be a positive integer, got '{r}'"))?,
    };
    Ok(ParsedSchedule {
        target,
        task,
        interval_secs,
        budget_wei,
        max_runs,
    })
}

/// `localharness schedule [--as <me>] <target> <task> --every <dur> --budget
/// <amount> [--runs <n>]` — escrow `$LH` to run `<target>` on a fixed interval,
/// on-chain (no tab needed). Resolves the target name → tokenId, escrows the
/// budget (approve + scheduleJob in one sponsored tx), and prints the schedule.
async fn schedule(caller_name: Option<&str>, rest: &[String]) -> i32 {
    let ParsedSchedule {
        target,
        task,
        interval_secs,
        budget_wei,
        max_runs,
    } = match parse_schedule_args(rest) {
        Ok(p) => p,
        Err(usage) => {
            eprintln!("{usage}");
            return 2;
        }
    };
    if task.trim().is_empty() {
        eprintln!("schedule: task is empty");
        return 2;
    }

    let (key_file, key_hex) = match resolve_caller_key(caller_name) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let signer = match wallet::from_private_key_hex(&key_hex) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bad key in {key_file}: {e}");
            return 1;
        }
    };

    // Resolve the target agent's tokenId (the facet rejects an unregistered
    // target with `UnregisteredTarget`, so fail early with a clear message).
    let target_id = match registry::id_of_name(&target).await {
        Ok(id) if id != 0 => id,
        Ok(_) => {
            eprintln!("schedule: '{target}' is not a registered agent");
            return 1;
        }
        Err(e) => {
            eprintln!("schedule: RPC error resolving '{target}': {e}");
            return 1;
        }
    };

    let sponsor = match wallet::from_private_key_hex(SPONSOR_KEY) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sponsor key error: {e}");
            return 1;
        }
    };

    let every = fmt_interval(interval_secs);
    println!(
        "scheduling {target} every {every}, budget {}, up to {max_runs} run(s) …",
        fmt_lh(budget_wei)
    );
    match registry::schedule_job_sponsored(
        &signer,
        &sponsor,
        target_id,
        task.as_bytes(),
        interval_secs,
        budget_wei,
        max_runs,
        registry::ALPHA_USD_ADDRESS,
    )
    .await
    {
        Ok(tx) => {
            // The new job id is the last entry in the owner's jobsOf index.
            let addr = addr_to_hex(wallet::address(&signer));
            let id_note = match registry::jobs_of(&addr).await {
                Ok(ids) if !ids.is_empty() => format!("job #{}", ids[ids.len() - 1]),
                _ => "scheduled".to_string(),
            };
            println!("✓ {id_note}: {target} every {every}, budget {}, ~{max_runs} runs", fmt_lh(budget_wei));
            println!("  the escrowed $LH backs it 24/7 — it fires with no browser tab open.");
            println!("  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("schedule failed: {e}");
            1
        }
    }
}

const INVITE_USAGE: &str = "\
usage: localharness invite <create|accept|reclaim|list> ...
  invite create [--as <me>] --amount <X> [--ttl <dur>]   escrow X $LH behind a fresh
                                                          code; prints the share link
  invite accept [--as <me>] <code>                        accept an invite (paid to you)
  invite reclaim [--as <me>] <code>                       refund an EXPIRED invite
  invite list [--as <me>]                                 your total escrowed $LH
  dur: 1h / 7d / 30d   (1h … 90d, default 7d)   amount: $LH (e.g. 100 or 10.5)";

/// Parse an invite TTL like `1h` / `7d` / `30m` / `3600` (bare = seconds) into
/// seconds, enforcing the facet's `[MIN_TTL, MAX_TTL]` = 1h…90d bound. Pure +
/// testable: a `s`/`m`/`h`/`d` suffix (case-insensitive) scales; anything else
/// (or out-of-range, or zero, or non-numeric) errors so a bad `--ttl` never
/// reaches a tx.
fn parse_ttl(raw: &str) -> Result<u64, String> {
    let s = raw.trim().to_ascii_lowercase();
    if s.is_empty() {
        return Err("ttl is empty".to_string());
    }
    let (num_part, mult) = match s.strip_suffix('d') {
        Some(n) => (n, 86_400u64),
        None => match s.strip_suffix('h') {
            Some(n) => (n, 3600u64),
            None => match s.strip_suffix('m') {
                Some(n) => (n, 60u64),
                None => match s.strip_suffix('s') {
                    Some(n) => (n, 1u64),
                    None => (s.as_str(), 1u64), // bare number = seconds
                },
            },
        },
    };
    let n: u64 = num_part
        .parse()
        .map_err(|_| format!("invalid ttl '{raw}' (use 1h / 7d / 30d)"))?;
    let secs = n
        .checked_mul(mult)
        .ok_or_else(|| format!("ttl '{raw}' overflows"))?;
    if secs < INVITE_MIN_TTL_SECS {
        return Err(format!("ttl '{raw}' is below the 1h minimum"));
    }
    if secs > INVITE_MAX_TTL_SECS {
        return Err(format!("ttl '{raw}' exceeds the 90d maximum"));
    }
    Ok(secs)
}

/// Render a TTL in seconds as a compact human duration (`1h`/`7d`/`90d`/`36h`).
/// Pure — used in the create confirmation.
fn fmt_ttl(secs: u64) -> String {
    if secs != 0 && secs % 86_400 == 0 {
        return format!("{}d", secs / 86_400);
    }
    if secs % 3600 == 0 {
        return format!("{}h", secs / 3600);
    }
    if secs % 60 == 0 {
        return format!("{}m", secs / 60);
    }
    format!("{secs}s")
}

/// Generate a fresh, link-safe invite code: `inv-<amount_lh>-<10 base32 chars>`.
/// The random tail is base32 (Crockford-ish, `[a-z2-9]`) of CSPRNG bytes, so the
/// code is lowercase-ASCII (=> `bytes(code)` is exactly what the facet keccaks)
/// and URL-safe. Mirrors `add-redeem-codes.sh`'s `lh-<amount>-<10 chars>` shape
/// but with the `inv-` prefix (so the `?invite=` router can tell invite from
/// redeem codes by prefix — `design/invites.md` §5.1). The plaintext is the
/// bearer secret: it lives ONLY here, never on-chain (only its hash is stored).
fn gen_invite_code(amount_label: &str) -> String {
    // Crockford base32 minus the visually-ambiguous 0/1/i/l/o/u — link-safe,
    // case-insensitive-readable. 10 chars of it ≈ 50 bits, plenty for a code.
    const ALPHABET: &[u8; 32] = b"abcdefghjkmnpqrstvwxyz23456789ab";
    let bytes = registry::random_x402_nonce(); // 32 CSPRNG bytes (getrandom)
    let mut tail = String::with_capacity(10);
    for &b in bytes.iter().take(10) {
        tail.push(ALPHABET[(b & 0x1f) as usize] as char);
    }
    format!("inv-{amount_label}-{tail}")
}

/// Parsed `invite create` flags.
struct ParsedInviteCreate {
    amount_label: String,
    amount_wei: u128,
    ttl_secs: u64,
}

fn parse_invite_create_args(rest: &[String]) -> Result<ParsedInviteCreate, String> {
    let mut amount: Option<String> = None;
    let mut ttl: Option<String> = None;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--amount" => {
                amount = Some(rest.get(i + 1).ok_or(INVITE_USAGE)?.clone());
                i += 2;
            }
            "--ttl" => {
                ttl = Some(rest.get(i + 1).ok_or(INVITE_USAGE)?.clone());
                i += 2;
            }
            other => return Err(format!("unexpected argument '{other}'\n{INVITE_USAGE}")),
        }
    }
    let amount_label = amount.ok_or_else(|| format!("invite create needs --amount <X $LH>\n{INVITE_USAGE}"))?;
    let amount_wei = match localharness::encoding::parse_token_amount(&amount_label) {
        Some(w) if w > 0 => w,
        _ => return Err(format!("--amount must be a positive $LH amount, got '{amount_label}'")),
    };
    let ttl_secs = match ttl {
        None => INVITE_DEFAULT_TTL_SECS,
        Some(raw) => parse_ttl(&raw)?,
    };
    Ok(ParsedInviteCreate { amount_label, amount_wei, ttl_secs })
}

/// `localharness invite <create|accept|reclaim|list>` — user-funded, refundable
/// `$LH` invite codes (InviteFacet). The subcommand router.
async fn invite(caller: Option<&str>, rest: &[String]) -> i32 {
    match rest.first().map(String::as_str) {
        Some("create") => invite_create(caller, &rest[1..]).await,
        Some("accept") => match rest.get(1) {
            Some(code) => invite_accept(caller, code).await,
            None => {
                eprintln!("usage: localharness invite accept [--as <me>] <code>");
                2
            }
        },
        Some("reclaim") => match rest.get(1) {
            Some(code) => invite_reclaim(caller, code).await,
            None => {
                eprintln!("usage: localharness invite reclaim [--as <me>] <code>");
                2
            }
        },
        Some("list") => invite_list(caller).await,
        _ => {
            eprintln!("{INVITE_USAGE}");
            2
        }
    }
}

/// `invite create --amount <X> [--ttl <dur>]` — generate a fresh code, escrow
/// the `$LH` behind its hash (approve + createInvite in one sponsored tx), and
/// print the plaintext code + the `?invite=` share link. The plaintext is shown
/// ONCE and never stored — copy it now.
async fn invite_create(caller: Option<&str>, rest: &[String]) -> i32 {
    let ParsedInviteCreate { amount_label, amount_wei, ttl_secs } = match parse_invite_create_args(rest) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let (key_file, key_hex) = match resolve_caller_key(caller) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let signer = match wallet::from_private_key_hex(&key_hex) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bad key in {key_file}: {e}");
            return 1;
        }
    };
    let sponsor = match wallet::from_private_key_hex(SPONSOR_KEY) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sponsor key error: {e}");
            return 1;
        }
    };

    let code = gen_invite_code(&amount_label);
    let code_hash = registry::invite_code_hash(&code);
    println!(
        "creating invite for {} (expires in {}) …",
        fmt_lh(amount_wei),
        fmt_ttl(ttl_secs)
    );
    match registry::create_invite_sponsored(
        &signer,
        &sponsor,
        code_hash,
        amount_wei,
        ttl_secs,
        registry::ALPHA_USD_ADDRESS,
    )
    .await
    {
        Ok(tx) => {
            println!("✓ invite created — {} escrowed, expires in {}", fmt_lh(amount_wei), fmt_ttl(ttl_secs));
            println!("  code:  {code}");
            println!("  link:  https://localharness.xyz/?invite={code}");
            println!("  share this with ONE person you trust — it's a bearer secret, shown only now.");
            println!("  it returns to you on `invite reclaim {code}` after it expires unclaimed.");
            println!("  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("invite create failed: {e}");
            1
        }
    }
}

/// `invite accept <code>` — accept an invite; the escrowed `$LH` is paid to the
/// caller. The plaintext `code` is hashed on-chain to find the invite.
async fn invite_accept(caller: Option<&str>, code: &str) -> i32 {
    let code = code.trim();
    if code.is_empty() {
        eprintln!("invite accept: empty code");
        return 2;
    }
    let (key_file, key_hex) = match resolve_caller_key(caller) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let signer = match wallet::from_private_key_hex(&key_hex) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bad key in {key_file}: {e}");
            return 1;
        }
    };
    let sponsor = match wallet::from_private_key_hex(SPONSOR_KEY) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sponsor key error: {e}");
            return 1;
        }
    };
    match registry::accept_invite_sponsored(&signer, &sponsor, code, registry::ALPHA_USD_ADDRESS).await {
        Ok(tx) => {
            println!("✓ invite accepted — the escrowed $LH is now in your wallet  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("invite accept failed: {e}");
            1
        }
    }
}

/// `invite reclaim <code>` — refund an EXPIRED, unclaimed invite back to its
/// funder. Permissionless (the `$LH` only goes to the recorded funder); hash the
/// code locally and call `reclaimInvite(codeHash)`.
async fn invite_reclaim(caller: Option<&str>, code: &str) -> i32 {
    let code = code.trim();
    if code.is_empty() {
        eprintln!("invite reclaim: empty code");
        return 2;
    }
    let (key_file, key_hex) = match resolve_caller_key(caller) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let signer = match wallet::from_private_key_hex(&key_hex) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bad key in {key_file}: {e}");
            return 1;
        }
    };
    let sponsor = match wallet::from_private_key_hex(SPONSOR_KEY) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sponsor key error: {e}");
            return 1;
        }
    };
    let code_hash = registry::invite_code_hash(code);
    match registry::reclaim_invite_sponsored(&signer, &sponsor, code_hash, registry::ALPHA_USD_ADDRESS).await {
        Ok(tx) => {
            println!("✓ invite reclaimed — the escrowed $LH is refunded to its funder  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("invite reclaim failed: {e}");
            1
        }
    }
}

/// `invite list` — show the caller's total `$LH` locked in pending invites
/// (`escrowedOf`). The MVP facet doesn't index invites by funder, so this is the
/// outstanding-escrow total, not a per-invite enumeration.
async fn invite_list(caller: Option<&str>) -> i32 {
    let (key_file, key_hex) = match resolve_caller_key(caller) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let signer = match wallet::from_private_key_hex(&key_hex) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bad key in {key_file}: {e}");
            return 1;
        }
    };
    let addr = addr_to_hex(wallet::address(&signer));
    match registry::escrowed_of(&addr).await {
        Ok(escrowed) => {
            println!("{addr}");
            println!("  escrowed  {}   <- $LH locked in your pending (Open) invites", fmt_lh(escrowed));
            if escrowed == 0 {
                println!("  no outstanding invites.");
            } else {
                println!("  reclaim an expired one with `invite reclaim <code>` to get its $LH back.");
            }
            println!("  (per-invite listing isn't on-chain-indexed; keep the codes you create.)");
            0
        }
        Err(e) => {
            eprintln!("invite list failed: {e}");
            1
        }
    }
}

// ---- bounty post/list/claim/submit/accept/cancel/mine (BountyFacet) ------
//
// The DEMAND primitive / agent-economy task board: a poster ESCROWS `$LH` behind
// a task; any agent claims it (identified by THEIR OWN tokenId), submits a
// result, and is paid the escrow when the poster accepts. `post` creates one
// (approve + postBounty in one sponsored tx), `list` shows the open board (with
// `--search` ranking), `claim`/`submit`/`accept`/`cancel` drive the lifecycle,
// `mine` lists the caller's posted bounties. Mirrors `registry::*_bounty_*`.

const BOUNTY_USAGE: &str = "\
usage: localharness bounty <post|list|claim|submit|accept|cancel|mine> ...
  bounty post [--as <me>] <task...> --reward <amt> [--ttl <dur>]   escrow $LH behind a task
  bounty list [--search <q>]                          list OPEN bounties (--search ranks)
  bounty claim [--as <me>] <id>                        claim an open bounty (you do the work)
  bounty submit [--as <me>] <id> <result...>           submit your result for a claim
  bounty accept [--as <me>] <id>                       accept a result + pay out (poster)
  bounty cancel [--as <me>] <id>                       cancel your OPEN bounty (refunds escrow)
  bounty reclaim [--as <me>] <id>                      refund an EXPIRED claimed/submitted bounty
  bounty mine [--as <me>]                              list bounties you've posted
  dur: 1h / 7d / 30d   (1h … 90d, default 7d)   amount: $LH (e.g. 5 or 0.5)";

/// How many open bounties `bounty list` / `discover_bounties` scan from the
/// board's head. A sane page bound — the board is small at launch scale; bump
/// when an index/cursor walk is worth it.
const BOUNTY_LIST_SCAN: u64 = 100;

/// Parse a bounty `id` argument (`#7` or `7`). Pure + testable.
fn parse_bounty_id(raw: &str) -> Result<u64, String> {
    raw.trim()
        .trim_start_matches('#')
        .parse::<u64>()
        .map_err(|_| format!("invalid bounty id '{raw}'"))
}

/// Parsed `bounty post` arguments. The task is the joined positional remainder
/// (so an unquoted multi-word task works, matching `schedule`/`persona`).
struct ParsedBountyPost {
    task: String,
    reward_label: String,
    reward_wei: u128,
    ttl_secs: u64,
}

fn parse_bounty_post_args(rest: &[String]) -> Result<ParsedBountyPost, String> {
    let mut positional: Vec<String> = Vec::new();
    let mut reward: Option<String> = None;
    let mut ttl: Option<String> = None;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--reward" => {
                reward = Some(rest.get(i + 1).ok_or(BOUNTY_USAGE)?.clone());
                i += 2;
            }
            "--ttl" => {
                ttl = Some(rest.get(i + 1).ok_or(BOUNTY_USAGE)?.clone());
                i += 2;
            }
            _ => {
                positional.push(rest[i].clone());
                i += 1;
            }
        }
    }
    if positional.is_empty() {
        return Err(format!("bounty post needs a <task>\n{BOUNTY_USAGE}"));
    }
    let task = positional.join(" ");
    let reward_label =
        reward.ok_or_else(|| format!("bounty post needs --reward <X $LH>\n{BOUNTY_USAGE}"))?;
    let reward_wei = match localharness::encoding::parse_token_amount(&reward_label) {
        Some(w) if w > 0 => w,
        _ => return Err(format!("--reward must be a positive $LH amount, got '{reward_label}'")),
    };
    // Reuse the invite TTL parser + 1h…90d bound (`parse_ttl`); same refundable
    // escrow-expiry semantics.
    let ttl_secs = match ttl {
        None => INVITE_DEFAULT_TTL_SECS,
        Some(raw) => parse_ttl(&raw)?,
    };
    Ok(ParsedBountyPost { task, reward_label, reward_wei, ttl_secs })
}

/// `localharness bounty <subcommand>` — the bounty-board router.
async fn bounty(caller: Option<&str>, rest: &[String]) -> i32 {
    match rest.first().map(String::as_str) {
        Some("post") => bounty_post(caller, &rest[1..]).await,
        Some("list") => bounty_list(&rest[1..]).await,
        Some("claim") => match rest.get(1) {
            Some(id) => bounty_claim(caller, id).await,
            None => {
                eprintln!("usage: localharness bounty claim [--as <me>] <id>");
                2
            }
        },
        Some("submit") => {
            if rest.len() < 3 {
                eprintln!("usage: localharness bounty submit [--as <me>] <id> <result...>");
                return 2;
            }
            bounty_submit(caller, &rest[1], &rest[2..].join(" ")).await
        }
        Some("accept") => match rest.get(1) {
            Some(id) => bounty_accept(caller, id).await,
            None => {
                eprintln!("usage: localharness bounty accept [--as <me>] <id>");
                2
            }
        },
        Some("cancel") => match rest.get(1) {
            Some(id) => bounty_cancel(caller, id).await,
            None => {
                eprintln!("usage: localharness bounty cancel [--as <me>] <id>");
                2
            }
        },
        Some("reclaim") => match rest.get(1) {
            Some(id) => bounty_reclaim(caller, id).await,
            None => {
                eprintln!("usage: localharness bounty reclaim [--as <me>] <id>");
                2
            }
        },
        Some("mine") => bounty_mine(caller).await,
        _ => {
            eprintln!("{BOUNTY_USAGE}");
            2
        }
    }
}

/// `bounty post <task> --reward <amt> [--ttl <dur>]` — escrow `$LH` behind a task
/// (approve + postBounty in one sponsored tx), print the new bounty id + share
/// link. The reward leaves the poster's balance the moment it mines; it pays the
/// claimant on `accept` or is refunded on `cancel`.
async fn bounty_post(caller: Option<&str>, rest: &[String]) -> i32 {
    let ParsedBountyPost { task, reward_label, reward_wei, ttl_secs } =
        match parse_bounty_post_args(rest) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("{e}");
                return 2;
            }
        };
    let _ = reward_label;
    if task.trim().is_empty() {
        eprintln!("bounty post: task is empty");
        return 2;
    }
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    println!(
        "posting bounty: reward {}, expires in {} …",
        fmt_lh(reward_wei),
        fmt_ttl(ttl_secs)
    );
    match registry::post_bounty_sponsored(
        &signer,
        &sponsor,
        task.as_bytes(),
        reward_wei,
        ttl_secs,
        registry::ALPHA_USD_ADDRESS,
    )
    .await
    {
        Ok(tx) => {
            // The new bounty id is the last entry in the poster's bountiesOf index.
            let addr = addr_to_hex(wallet::address(&signer));
            let id_note = match registry::bounties_of(&addr).await {
                Ok(ids) if !ids.is_empty() => Some(ids[ids.len() - 1]),
                _ => None,
            };
            match id_note {
                Some(id) => {
                    println!("✓ bounty #{id} posted — {} escrowed, expires in {}", fmt_lh(reward_wei), fmt_ttl(ttl_secs));
                    println!("  link:  https://localharness.xyz/?bounty={id}");
                    println!("  any agent can `bounty claim {id}`, do the work, and `bounty submit {id} <result>`.");
                }
                None => {
                    println!("✓ bounty posted — {} escrowed, expires in {}", fmt_lh(reward_wei), fmt_ttl(ttl_secs));
                    println!("  see it with `bounty mine`.");
                }
            }
            println!("  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("bounty post failed: {e}");
            1
        }
    }
}

/// Render one open-board row for `bounty list`. Pure (no I/O) so the layout is
/// unit-testable: id, reward, expiry (relative), task snippet.
fn format_bounty_row(id: u64, b: &registry::Bounty, task: &str, now: u64) -> String {
    let when = if b.expiry == 0 {
        "—".to_string()
    } else if b.expiry <= now {
        "EXPIRED".to_string()
    } else {
        format!("in {}", fmt_interval(b.expiry - now))
    };
    let snippet: String = task.replace('\n', " ").chars().take(70).collect();
    format!(
        "  #{id}  reward {reward}  expires {when}  [{status}]\n      {snippet}",
        reward = fmt_lh(b.reward_wei),
        status = b.status_label(),
    )
}

/// `bounty list [--search <q>]` — list OPEN bounties. With `--search`, rank by
/// query-vs-task via `discover_bounties`; without, show the open board head.
/// Read-only, no `$LH`.
async fn bounty_list(rest: &[String]) -> i32 {
    // Optional `--search <q>` (q may be multi-word).
    let query = match rest.first().map(String::as_str) {
        Some("--search") => {
            let q = rest[1..].join(" ");
            if q.trim().is_empty() {
                eprintln!("usage: localharness bounty list [--search <query>]");
                return 2;
            }
            Some(q)
        }
        Some(other) => {
            eprintln!("unexpected argument '{other}'\nusage: localharness bounty list [--search <query>]");
            return 2;
        }
        None => None,
    };

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    if let Some(q) = query {
        match registry::discover_bounties(&q, BOUNTY_LIST_SCAN).await {
            Ok(hits) => {
                if hits.is_empty() {
                    println!("no open bounties match '{q}'");
                    return 0;
                }
                println!("{} open bounty match(es) for '{q}':", hits.len());
                for (id, task, reward) in hits {
                    // A reward-only line keeps `discover_bounties`' (id, task,
                    // reward) shape without a second per-id read.
                    let snippet: String = task.replace('\n', " ").chars().take(70).collect();
                    println!("  #{id}  reward {}\n      {snippet}", fmt_lh(reward));
                }
                0
            }
            Err(e) => {
                eprintln!("bounty list failed: {e}");
                1
            }
        }
    } else {
        let ids = match registry::open_bounties(0, BOUNTY_LIST_SCAN).await {
            Ok(ids) => ids,
            Err(e) => {
                eprintln!("bounty list failed: {e}");
                return 1;
            }
        };
        if ids.is_empty() {
            println!("no open bounties — post one with `bounty post <task> --reward <amt>`");
            return 0;
        }
        println!("{} open bounty(ies):", ids.len());
        for id in ids {
            let b = match registry::get_bounty(id).await {
                Ok(b) => b,
                Err(e) => {
                    println!("  #{id}  (could not read: {e})");
                    continue;
                }
            };
            let task = registry::task_of_bounty(id).await.unwrap_or_default();
            println!("{}", format_bounty_row(id, &b, &task, now));
        }
        0
    }
}

/// `bounty claim <id>` — claim an open bounty. Resolves the CALLER'S OWN tokenId
/// as `claimantTokenId` (the identity that earns the reward), then calls
/// `claimBounty(id, claimantTokenId)`.
async fn bounty_claim(caller: Option<&str>, id_arg: &str) -> i32 {
    let bounty_id = match parse_bounty_id(id_arg) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    // Resolve the caller's OWN registered tokenId (NOT the bounty poster's). The
    // facet credits the reward to this identity, so it must be one the caller
    // controls. See `resolve_own_token_id`.
    let claimant_token_id = match resolve_own_token_id(caller, &signer).await {
        Ok(id) => id,
        Err(e) => {
            eprintln!("bounty claim: {e}");
            return 1;
        }
    };
    println!("claiming bounty #{bounty_id} as token #{claimant_token_id} …");
    match registry::claim_bounty_sponsored(
        &signer,
        &sponsor,
        bounty_id,
        claimant_token_id,
        registry::ALPHA_USD_ADDRESS,
    )
    .await
    {
        Ok(tx) => {
            println!("✓ bounty #{bounty_id} claimed by token #{claimant_token_id}");
            println!("  do the work, then `bounty submit {bounty_id} <result>`.  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("bounty claim failed: {e}");
            1
        }
    }
}

/// `bounty submit <id> <result>` — submit your result for a claimed bounty
/// (`submitResult(id, result)`). The poster then `accept`s to pay you.
async fn bounty_submit(caller: Option<&str>, id_arg: &str, result: &str) -> i32 {
    let bounty_id = match parse_bounty_id(id_arg) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    if result.trim().is_empty() {
        eprintln!("bounty submit: result is empty");
        return 2;
    }
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    println!("submitting result for bounty #{bounty_id} …");
    match registry::submit_result_sponsored(
        &signer,
        &sponsor,
        bounty_id,
        result.as_bytes(),
        registry::ALPHA_USD_ADDRESS,
    )
    .await
    {
        Ok(tx) => {
            println!("✓ result submitted for bounty #{bounty_id} — awaiting the poster's accept  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("bounty submit failed: {e}");
            1
        }
    }
}

/// `bounty accept <id>` — the poster accepts the submitted result and pays the
/// escrowed `$LH` out to the claimant (`acceptResult(id)`).
async fn bounty_accept(caller: Option<&str>, id_arg: &str) -> i32 {
    let bounty_id = match parse_bounty_id(id_arg) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    println!("accepting bounty #{bounty_id}'s result + paying the claimant …");
    match registry::accept_result_sponsored(&signer, &sponsor, bounty_id, registry::ALPHA_USD_ADDRESS).await {
        Ok(tx) => {
            println!("✓ bounty #{bounty_id} accepted — the escrowed $LH is paid to the claimant  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("bounty accept failed: {e}");
            1
        }
    }
}

/// `bounty cancel <id>` — the poster cancels their bounty; the facet refunds the
/// full escrow (`cancelBounty(id)`, allowed before payout).
async fn bounty_cancel(caller: Option<&str>, id_arg: &str) -> i32 {
    let bounty_id = match parse_bounty_id(id_arg) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    println!("cancelling bounty #{bounty_id} (refunding its escrow) …");
    match registry::cancel_bounty_sponsored(&signer, &sponsor, bounty_id, registry::ALPHA_USD_ADDRESS).await {
        Ok(tx) => {
            println!("✓ bounty #{bounty_id} cancelled — the escrowed $LH is refunded to you  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("bounty cancel failed: {e}");
            1
        }
    }
}

/// `bounty reclaim <id>` — refund an EXPIRED bounty whose work was never accepted
/// (`reclaimExpired(id)`). This is the recovery path for a bounty stranded in
/// Claimed/Submitted (where `bounty cancel` reverts `NotOpen`): once the TTL has
/// elapsed the escrow refunds 100% to the poster. Permissionless to call on-chain,
/// but the facet always pays the POSTER, so a non-poster gains nothing.
async fn bounty_reclaim(caller: Option<&str>, id_arg: &str) -> i32 {
    let bounty_id = match parse_bounty_id(id_arg) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    println!("reclaiming expired bounty #{bounty_id} (refunding its escrow to the poster) …");
    match registry::reclaim_expired_sponsored(&signer, &sponsor, bounty_id, registry::ALPHA_USD_ADDRESS).await {
        Ok(tx) => {
            println!("✓ bounty #{bounty_id} reclaimed — the escrowed $LH is refunded to its poster  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!(
                "bounty reclaim failed: {e}\n  \
                 (a bounty can only be reclaimed AFTER its ttl expires, and only while it has \
                 not been accepted/cancelled/already-reclaimed)"
            );
            1
        }
    }
}

/// `bounty mine [--as <me>]` — list the bounties the caller has POSTED
/// (`bountiesOf` + a `getBounty`/`taskOf` per id). Read-only, no `$LH`.
async fn bounty_mine(caller: Option<&str>) -> i32 {
    let (key_file, key_hex) = match resolve_caller_key(caller) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let signer = match wallet::from_private_key_hex(&key_hex) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bad key in {key_file}: {e}");
            return 1;
        }
    };
    let addr = addr_to_hex(wallet::address(&signer));
    let ids = match registry::bounties_of(&addr).await {
        Ok(ids) => ids,
        Err(e) => {
            eprintln!("RPC error: {e}");
            return 1;
        }
    };
    if ids.is_empty() {
        println!("no bounties posted by {addr}");
        return 0;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("{} bounty(ies) posted by {addr}:", ids.len());
    for id in ids {
        let b = match registry::get_bounty(id).await {
            Ok(b) => b,
            Err(e) => {
                println!("  #{id}  (could not read: {e})");
                continue;
            }
        };
        let task = registry::task_of_bounty(id).await.unwrap_or_default();
        println!("{}", format_bounty_row(id, &b, &task, now));
    }
    0
}

// ---- reputation (attestation-based on-chain agent reputation) -------------
//
// A peer-attestation reputation primitive over ReputationFacet: `reputation
// show <agent>` reads an agent's running `(count, sum)` + recent attestations;
// `reputation attest <agent> <rating> [--ref ...]` records a 1-5 rating about a
// piece of work (a bounty id or a 0x ref). The colony engine's [7/7] step
// auto-attests the worker, so the demand flywheel keeps reputation flowing.

const REPUTATION_USAGE: &str = "\
usage: localharness reputation <show|attest> ...   (alias: rep)
  reputation show <agent>                              an agent's count, avg rating, recent attestations
  reputation attest [--as <me>] <agent> <rating 1-5> [--ref <hex|bountyId>]
                                                       attest to an agent you've worked with (1-5)
  --ref tags the work: a bounty id (left-padded to bytes32) or a 0x… 32-byte ref;
  it defaults to a zero ref. You can't attest to yourself or re-attest the same
  (agent, ref) pair.";

/// Turn a `--ref` argument into a `bytes32` workRef: a `0x…` value is parsed as a
/// raw 32-byte ref (left-padded if shorter); a bare integer is treated as a bounty
/// id and left-padded big-endian into the low 8 bytes (the SAME `bytes32(bountyId)`
/// the colony [7/7] step uses). `None` → the zero ref. Pure + testable.
fn parse_work_ref(raw: Option<&str>) -> Result<[u8; 32], String> {
    let mut out = [0u8; 32];
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(out); // default: zero ref
    };
    if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
        if hex.is_empty() || hex.len() > 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("--ref hex must be 1..32 bytes of hex, got '{raw}'"));
        }
        // Right-align the supplied bytes (left-pad with zeros) into the 32-byte word.
        let bytes = decode_hex_even(hex)?;
        out[32 - bytes.len()..].copy_from_slice(&bytes);
        return Ok(out);
    }
    // A bare integer → bounty id, left-padded big-endian into the low 8 bytes.
    match raw.trim_start_matches('#').parse::<u64>() {
        Ok(id) => {
            out[24..32].copy_from_slice(&id.to_be_bytes());
            Ok(out)
        }
        Err(_) => Err(format!(
            "--ref must be a 0x… hex ref or a bounty id (integer), got '{raw}'"
        )),
    }
}

/// Decode an even-length hex string (no `0x`) into bytes, left-padding an odd
/// nibble count by prefixing a `0`. Helper for [`parse_work_ref`].
fn decode_hex_even(hex: &str) -> Result<Vec<u8>, String> {
    let padded = if hex.len() % 2 == 1 { format!("0{hex}") } else { hex.to_string() };
    (0..padded.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&padded[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

/// `bytes32(bountyId)` — a bounty id left-padded big-endian into the low 8 bytes
/// of a 32-byte word, the canonical workRef the colony [7/7] step attests with.
fn bounty_work_ref(bounty_id: u64) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[24..32].copy_from_slice(&bounty_id.to_be_bytes());
    out
}

/// `localharness reputation <subcommand>` — the reputation router.
async fn reputation(caller: Option<&str>, rest: &[String]) -> i32 {
    match rest.first().map(String::as_str) {
        Some("show") => match rest.get(1) {
            Some(agent) => reputation_show(agent).await,
            None => {
                eprintln!("usage: localharness reputation show <agent>");
                2
            }
        },
        Some("attest") => reputation_attest(caller, &rest[1..]).await,
        _ => {
            eprintln!("{REPUTATION_USAGE}");
            2
        }
    }
}

/// `reputation show <agent>` — resolve the name→tokenId, then print its
/// attestation count, average rating (sum/count), and recent attestations.
/// Read-only, no `$LH`.
async fn reputation_show(agent: &str) -> i32 {
    let token_id = match registry::id_of_name(agent).await {
        Ok(0) | Err(_) => {
            eprintln!("reputation show: '{agent}' is not a registered agent (check the name)");
            return 1;
        }
        Ok(id) => id,
    };
    let (count, sum) = match registry::reputation_of(token_id).await {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("RPC error reading reputation: {e}");
            return 1;
        }
    };
    println!("reputation of {agent} (token #{token_id}):");
    if count == 0 {
        println!("  no attestations yet — be the first with `reputation attest {agent} <1-5>`");
        return 0;
    }
    // Average to 2 dp without floats: (sum*100)/count rounded.
    let avg_x100 = (sum * 100 + count / 2) / count;
    println!("  attestations: {count}");
    println!("  average rating: {}.{:02} / 5  (sum {sum})", avg_x100 / 100, avg_x100 % 100);
    // Recent attestations (the head of the list).
    match registry::attestations_of(token_id, 0, REPUTATION_SHOW_LIMIT).await {
        Ok(rows) if !rows.is_empty() => {
            println!("  recent attestations:");
            for (attester, rating, work_ref) in rows {
                // Surface a bounty-id workRef compactly when the high bytes are 0.
                let ref_note = format_work_ref(&work_ref);
                println!("    {rating}★  by {attester}{ref_note}");
            }
        }
        Ok(_) => {}
        Err(e) => println!("  (could not list attestations: {e})"),
    }
    0
}

/// How many recent attestations `reputation show` lists. A small page head — the
/// list is small at launch scale.
const REPUTATION_SHOW_LIMIT: u64 = 10;

/// Render a workRef for display: a zero ref shows nothing; a ref whose high 24
/// bytes are zero is shown as its low-8 bounty id; otherwise the full 0x-hex.
/// Pure (operates on the `0x…` string from `attestations_of`).
fn format_work_ref(work_ref_hex: &str) -> String {
    let hex = work_ref_hex.trim_start_matches("0x");
    if hex.len() != 64 || hex.chars().all(|c| c == '0') {
        return String::new(); // zero / malformed ref → no note
    }
    // High 48 nibbles (24 bytes) zero → a left-padded integer (bounty id).
    if hex[..48].chars().all(|c| c == '0') {
        if let Ok(id) = u64::from_str_radix(&hex[48..], 16) {
            return format!("  (work #{id})");
        }
    }
    format!("  (ref 0x{}…)", &hex[..8])
}

/// `reputation attest <agent> <rating 1-5> [--ref <hex|bountyId>]` — attest to an
/// agent you've worked with. Resolves the agent name→tokenId, signs `attest` as
/// the caller, and surfaces a duplicate/self/bad-rating revert clearly.
async fn reputation_attest(caller: Option<&str>, rest: &[String]) -> i32 {
    // Positional: <agent> <rating>; flag: --ref <value>.
    let mut positional: Vec<&str> = Vec::new();
    let mut work_ref_arg: Option<&str> = None;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--ref" => {
                match rest.get(i + 1) {
                    Some(v) => work_ref_arg = Some(v),
                    None => {
                        eprintln!("--ref needs a value\n{REPUTATION_USAGE}");
                        return 2;
                    }
                }
                i += 2;
            }
            other => {
                positional.push(other);
                i += 1;
            }
        }
    }
    let (agent, rating_arg) = match positional.as_slice() {
        [agent, rating] => (*agent, *rating),
        _ => {
            eprintln!("usage: localharness reputation attest [--as <me>] <agent> <rating 1-5> [--ref <hex|bountyId>]");
            return 2;
        }
    };
    let rating = match rating_arg.trim().parse::<u8>() {
        Ok(r) if (1..=5).contains(&r) => r,
        _ => {
            eprintln!("rating must be an integer 1-5, got '{rating_arg}'");
            return 2;
        }
    };
    let work_ref = match parse_work_ref(work_ref_arg) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };

    let token_id = match registry::id_of_name(agent).await {
        Ok(0) | Err(_) => {
            eprintln!("reputation attest: '{agent}' is not a registered agent (check the name)");
            return 1;
        }
        Ok(id) => id,
    };
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    println!("attesting {rating}★ to {agent} (token #{token_id}) …");
    match registry::attest_sponsored(
        &signer,
        &sponsor,
        token_id,
        rating,
        work_ref,
        registry::ALPHA_USD_ADDRESS,
    )
    .await
    {
        Ok(tx) => {
            println!("✓ attested {rating}★ to {agent} — it's now on-chain  tx: {tx}");
            println!("  see it with `reputation show {agent}`.");
            0
        }
        Err(e) => {
            eprintln!("reputation attest failed: {e}");
            1
        }
    }
}

// ---- colony (the agent economy's first autonomous cycle) ------------------
//
// `colony run` composes the bounty lifecycle + a headless `call` + a headless
// JUDGE PANEL into ONE self-driving turn of the demand flywheel: the platform
// (the caller) POSTS real work as an escrowed bounty, a WORKER agent claims it,
// the worker's on-chain persona DOES the work (an LLM turn via the credit proxy),
// the worker submits the result, a NEUTRAL JUDGE PANEL (median of N, default 3)
// scores it 1-5 for genuine + accurate task-fit, the caller accepts — settling
// the reward to the worker's token-bound account — and finally ATTESTS the
// PANEL'S MEDIAN rating on-chain (NOT a hardcoded 5★), so the worker's reputation
// reflects judged quality and rewards no hallucination. The panel EXCLUDES the
// worker AND the caller (neutrality), which matters because that reputation now
// DRIVES the PICK step. No human orchestrates the steps. The result + judge TEXT
// are LLM turns (they vary); the CYCLE mechanics
// (post→claim→submit→accept→payout→attest) are deterministic. Every on-chain step
// reuses the SAME helpers as the `bounty` subcommands (`post_bounty_sponsored` /
// `claim_bounty_sponsored` / `submit_result_sponsored` / `accept_result_sponsored`
// / `attest_sponsored`) and the work + each judge reuse the SAME headless turn as
// `call` (`run_agent_turn`), so it adds no new on-chain surface.

const COLONY_USAGE: &str = "\
usage: localharness colony run [--as <me>] <task> --reward <lh> [--worker <agent>] [--judges <N>] [--judge <agent>] [--min-accept-rating <N>] [--ttl <dur>]
  Run ONE autonomous agent-economy cycle end-to-end:
    1. the caller (--as, default your sole identity) POSTS <task> as a bounty escrowing <reward> $LH
    2. a WORKER is picked: --worker <agent>, else the reputation-aware top discover() match for <task>
    3. the worker CLAIMS the bounty (reward bound to the worker's token-bound account)
    4. the worker's on-chain persona DOES the work via a headless `call`
    5. the worker SUBMITS the produced result
    6. a NEUTRAL JUDGE PANEL scores the result 1-5 for genuine + accurate task-fit (catches
       hallucinations); the worker's rating is the MEDIAN of the panel
    7. PAYMENT GATE — IFF the median >= --min-accept-rating the caller ACCEPTS → the escrowed
       $LH settles to the worker's TBA; otherwise the result is REJECTED (NOT paid — the escrow
       stays locked and is reclaimable via `bounty reclaim` after the ttl)
    8. the caller ATTESTS to the worker (the panel's MEDIAN rating, workRef = the bounty id) →
       reputation — ALWAYS, accept OR reject (a rejected low rating must still hit the chain)
  --reward <lh>          the $LH reward to escrow (e.g. 0.02)            [required]
  --worker <agent>       the worker subdomain (its key must be local);
                         omit to auto-pick the best discover() match
  --judges <N>           size of the auto-selected neutral judge panel (default 3); N DISTINCT
                         local agents EXCLUDING the worker AND the caller are chosen, the median
                         of their ratings is attested. Fewer than N → uses what's available (min 1)
  --judge <agent>        force a SINGLE named judge (a panel of exactly that one agent; its key
                         must be local); overrides --judges
  --min-accept-rating N  PAYMENT GATE (1..5, default 2): the colony accepts + pays IFF the panel
                         median is >= N. A median below N is REJECTED — the worker is NOT paid and
                         the escrow stays locked (reclaim it after the ttl). Default 2 ⇒ a median
                         of 1 (clear failure / hallucination) is rejected; 2-5 are paid
  --ttl <dur>            bounty expiry (1h/7d/30d, 1h…90d, default 7d)
  The worker MUST be a fleet/owned agent whose key is in your keys dir
  (it signs its own claim + submit). The neutral panel makes the reputation signal
  TRUSTWORTHY — which matters because reputation now DRIVES the PICK step. On any
  step failure the bounty id + the CORRECT recovery command is printed (`bounty
  cancel` while OPEN, else `bounty reclaim` after the ttl) — never a silent
  half-state. The colony is economically rational: it pays ONLY for work the
  neutral panel rates at/above the bar; a sub-bar result is rejected (no payment,
  escrow recoverable) yet STILL attested so reputation reflects it. If no neutral
  agent exists the caller acts as a lone fallback judge; if ALL judge turns fail
  the median defaults to a neutral 3★.";

/// Build the impartial-judge prompt for the [6/8] JUDGE step. The judge scores
/// the worker's `result` against the `task` on a 1-5 scale, explicitly checking
/// for ACCURACY/hallucination (with the serverless-localharness context baked in
/// so a "binds a port / control API" style fabrication scores low). The reply's
/// first line MUST be a single 1-5 digit; the rest is rationale.
fn colony_judge_prompt(task: &str, result: &str) -> String {
    format!(
        "You are an impartial judge scoring a bounty result.\n\
         TASK: {task}\n\
         WORKER RESULT: {result}\n\n\
         Score 1-5 whether the result genuinely AND ACCURATELY addresses the task \
         (5 = excellent, specific, correct; 1 = irrelevant, wrong, or HALLUCINATED). \
         IMPORTANT context for accuracy-checking: localharness is SERVERLESS — it runs \
         on the Tempo chain + the browser + a Vercel edge proxy; there is NO local \
         server/daemon/control-API/port binding. A result that claims to fix or find \
         such a thing is HALLUCINATED and scores low.\n\n\
         Output ONLY a single digit 1-5 on the first line, then one short line of rationale."
    )
}

/// Parse a judge's reply into `(rating, rationale)`. The rating is the FIRST
/// `1..=5` digit anywhere in the reply (the prompt asks for it on line 1, but a
/// chatty model may prepend a word); unparseable → a neutral default of 3. The
/// rationale is the first non-empty line that is not just the bare rating digit.
/// Pure + testable.
fn parse_judge_rating(reply: &str) -> (u8, String) {
    let rating = reply
        .chars()
        .find_map(|c| c.to_digit(10).filter(|d| (1..=5).contains(d)))
        .map(|d| d as u8)
        .unwrap_or(3);
    let rationale = reply
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && l.trim_matches(|c: char| !c.is_alphanumeric()).len() > 1)
        .unwrap_or("")
        .to_string();
    (rating, rationale)
}

/// Aggregate a NEUTRAL JUDGE PANEL's per-judge ratings into a single MEDIAN
/// rating (the robust, outlier-resistant centre — one rogue judge can't swing
/// it the way a mean would). Pure + testable.
///
/// Rule: sort the ratings ascending; **odd N** → the middle element; **even N**
/// → the LOWER-MIDDLE element (`[n/2 - 1]`) — a deliberately conservative tie
/// break so a split panel never rounds reputation UP. An EMPTY slice → a neutral
/// `3` (the same default the colony uses when every judge turn fails, so the
/// cycle completes with an honest, non-inflated rating). The result is always in
/// `1..=5` given `1..=5` inputs (median of in-range values is in range).
fn median_rating(ratings: &[u8]) -> u8 {
    if ratings.is_empty() {
        return 3;
    }
    let mut sorted = ratings.to_vec();
    sorted.sort_unstable();
    let n = sorted.len();
    // Odd: the true middle. Even: the lower-middle (conservative — don't inflate).
    let idx = if n % 2 == 1 { n / 2 } else { n / 2 - 1 };
    sorted[idx]
}

/// PAYMENT GATE for `colony run`: should the caller ACCEPT (pay) given the panel
/// `median` and the `--min-accept-rating` threshold? Pure + testable — the colony
/// becomes economically rational by paying ONLY for work the neutral panel rates
/// AT OR ABOVE the bar (no contract change: a sub-bar result is simply NOT
/// accepted, so its escrow stays locked and is `reclaimExpired`-recoverable after
/// the ttl). Rule: `median >= min`. With the default `min = 2`, a median of 1
/// (the clear-failure / hallucination band) is REJECTED while 2..=5 are paid.
/// Inputs are clamped to the 1..=5 rating range so a stray 0 can never sneak a
/// payment past a `min = 1` floor.
fn should_accept(median: u8, min: u8) -> bool {
    median.clamp(1, 5) >= min.clamp(1, 5)
}

/// Default payment-gate threshold for `colony run` (`--min-accept-rating`). A
/// median of 1 (clear failure / hallucination) is rejected; 2..=5 are paid.
const COLONY_DEFAULT_MIN_ACCEPT: u8 = 2;

/// Parsed `colony run` arguments. The task is the joined positional remainder
/// (so an unquoted multi-word task works, matching `bounty post`).
struct ParsedColonyRun {
    task: String,
    reward_wei: u128,
    worker: Option<String>,
    /// An explicit single-judge override (`--judge <agent>`) — a panel of exactly
    /// that one neutral agent. `None` → auto-select a panel of `judges` agents.
    judge: Option<String>,
    /// Target panel size for the auto-selected NEUTRAL JUDGE PANEL (`--judges N`,
    /// default [`COLONY_DEFAULT_PANEL`]). Ignored when `judge` is set.
    judges: usize,
    /// PAYMENT GATE (`--min-accept-rating N`, default [`COLONY_DEFAULT_MIN_ACCEPT`]):
    /// the caller accepts + pays IFF the panel median is `>= min_accept`. A median
    /// below it is REJECTED (the worker is NOT paid; the escrow is reclaimable after
    /// the ttl). Validated to 1..=5 at parse time.
    min_accept: u8,
    ttl_secs: u64,
}

/// Default neutral-judge panel size for `colony run` (median of N). Odd so the
/// median is a clean middle value with no even-split tie.
const COLONY_DEFAULT_PANEL: usize = 3;

/// Parse `colony run` flags. Pure/testable — mirrors `parse_bounty_post_args`
/// plus a `--worker` override.
fn parse_colony_run_args(rest: &[String]) -> Result<ParsedColonyRun, String> {
    let mut positional: Vec<String> = Vec::new();
    let mut reward: Option<String> = None;
    let mut worker: Option<String> = None;
    let mut judge: Option<String> = None;
    let mut judges: Option<String> = None;
    let mut min_accept: Option<String> = None;
    let mut ttl: Option<String> = None;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--reward" => {
                reward = Some(rest.get(i + 1).ok_or(COLONY_USAGE)?.clone());
                i += 2;
            }
            "--worker" => {
                worker = Some(rest.get(i + 1).ok_or(COLONY_USAGE)?.clone());
                i += 2;
            }
            "--judge" => {
                judge = Some(rest.get(i + 1).ok_or(COLONY_USAGE)?.clone());
                i += 2;
            }
            "--judges" => {
                judges = Some(rest.get(i + 1).ok_or(COLONY_USAGE)?.clone());
                i += 2;
            }
            "--min-accept-rating" => {
                min_accept = Some(rest.get(i + 1).ok_or(COLONY_USAGE)?.clone());
                i += 2;
            }
            "--ttl" => {
                ttl = Some(rest.get(i + 1).ok_or(COLONY_USAGE)?.clone());
                i += 2;
            }
            _ => {
                positional.push(rest[i].clone());
                i += 1;
            }
        }
    }
    if positional.is_empty() {
        return Err(format!("colony run needs a <task>\n{COLONY_USAGE}"));
    }
    let task = positional.join(" ");
    let reward_label =
        reward.ok_or_else(|| format!("colony run needs --reward <X $LH>\n{COLONY_USAGE}"))?;
    let reward_wei = match localharness::encoding::parse_token_amount(&reward_label) {
        Some(w) if w > 0 => w,
        _ => return Err(format!("--reward must be a positive $LH amount, got '{reward_label}'")),
    };
    let ttl_secs = match ttl {
        None => INVITE_DEFAULT_TTL_SECS,
        Some(raw) => parse_ttl(&raw)?,
    };
    let judges = match judges {
        None => COLONY_DEFAULT_PANEL,
        Some(raw) => match raw.trim().parse::<usize>() {
            Ok(n) if n >= 1 => n,
            _ => return Err(format!("--judges must be a positive integer, got '{raw}'")),
        },
    };
    // The PAYMENT GATE threshold (1..=5). Rejects 0 and out-of-band N so a median
    // can be compared against a real rating bar; default is the clear-failure floor.
    let min_accept = match min_accept {
        None => COLONY_DEFAULT_MIN_ACCEPT,
        Some(raw) => match raw.trim().parse::<u8>() {
            Ok(n) if (1..=5).contains(&n) => n,
            _ => return Err(format!("--min-accept-rating must be 1..5, got '{raw}'")),
        },
    };
    Ok(ParsedColonyRun { task, reward_wei, worker, judge, judges, min_accept, ttl_secs })
}

/// `localharness colony <subcommand>` — the colony-engine router.
async fn colony(caller: Option<&str>, rest: &[String]) -> i32 {
    match rest.first().map(String::as_str) {
        Some("run") => colony_run(caller, &rest[1..]).await,
        _ => {
            eprintln!("{COLONY_USAGE}");
            2
        }
    }
}

/// A drivable worker candidate for the colony PICK step: a discover-matched
/// agent whose key is local, decorated with on-chain reputation. `task_rank` is
/// its 0-based position in `discover_agents` (lower = better task fit; name-hit
/// before persona-hit, newest-first within a tier). `rep_count`/`rep_sum` are the
/// raw `reputationOf` pair (attestation count + rating sum, sum ≤ 5·count). Pure
/// data so the selection rule below is unit-testable with no network.
#[derive(Debug, Clone)]
struct WorkerCandidate {
    name: String,
    task_rank: usize,
    rep_count: u64,
    rep_sum: u64,
}

impl WorkerCandidate {
    /// Average rating in milli-units (so 5.0★ = 5000), `0` when never attested.
    /// Integer math keeps the selection rule exact + reproducible (no float
    /// ordering surprises). An unproven agent (count 0) sorts as avg 0 — below
    /// any proven one at the same task-fit tier, but still eligible.
    fn avg_milli(&self) -> u64 {
        if self.rep_count == 0 {
            0
        } else {
            (self.rep_sum * 1000) / self.rep_count
        }
    }
}

/// Candidates whose `task_rank` is within this many positions of the BEST
/// (rank-0) match are treated as "similar task fit" and decided on reputation.
/// Outside the band, the better task fit wins outright — so a wildly-irrelevant
/// high-reputation agent can never out-rank a clearly more task-relevant one.
/// Discover returns name-hits before persona-hits, so a small band keeps
/// reputation as the decider among genuinely comparable agents only.
const COLONY_TASK_FIT_BAND: usize = 3;

/// The reputation-aware selection RULE (pure + testable). Picks the best worker
/// from `candidates` (each already filtered to "task-relevant AND locally
/// keyed"). The blend, in strict priority order:
///   1. **Task-fit tier** (primary) — group candidates by discover proximity:
///      everything within `COLONY_TASK_FIT_BAND` positions of the top match is
///      one tier; a meaningfully worse task match is a lower tier. Better tier
///      always wins (task fit dominates).
///   2. **Average rating** (secondary) — within a tier, higher avg★ wins, so
///      proven good work beats unproven at comparable task fit.
///   3. **Attestation count** (tertiary tiebreak) — more attestations wins when
///      avg ties (a 5.0 from 3 beats a 5.0 from 1).
///   4. **Discover rank** (final tiebreak) — the original task-fit order, so the
///      result is deterministic.
/// An agent with NO attestations (avg 0) is eligible but ranks below a proven one
/// in the same tier. Returns `None` only for an empty slice.
fn pick_reputation_aware(candidates: &[WorkerCandidate]) -> Option<&WorkerCandidate> {
    let best_rank = candidates.iter().map(|c| c.task_rank).min()?;
    // Tier 0 = within the band of the best; higher tiers = progressively worse
    // task fit (one tier per band-width step beyond the best).
    let tier = |c: &WorkerCandidate| (c.task_rank - best_rank) / (COLONY_TASK_FIT_BAND + 1);
    candidates.iter().min_by(|a, b| {
        tier(a)
            .cmp(&tier(b)) // 1. lower tier (better task fit) first
            .then_with(|| b.avg_milli().cmp(&a.avg_milli())) // 2. higher avg★ first
            .then_with(|| b.rep_count.cmp(&a.rep_count)) // 3. more attestations first
            .then_with(|| a.task_rank.cmp(&b.task_rank)) // 4. better discover rank first
    })
}

/// A one-line, human-readable justification for a PICK — so the choice is
/// transparent in the colony transcript. Pure.
fn colony_pick_reasoning(c: &WorkerCandidate) -> String {
    let fit = if c.task_rank == 0 {
        "top task match".to_string()
    } else {
        format!("task match #{}", c.task_rank + 1)
    };
    if c.rep_count == 0 {
        format!("picked {} — no reputation yet, {} among local agents", c.name, fit)
    } else {
        let whole = c.avg_milli() / 1000;
        let frac = (c.avg_milli() % 1000) / 100; // one decimal place
        let plural = if c.rep_count == 1 { "attestation" } else { "attestations" };
        format!(
            "picked {} — reputation {whole}.{frac} from {} {plural} ({fit} among local agents)",
            c.name, c.rep_count
        )
    }
}

/// Pure: extract the significant search keywords from a free-form `task`, so a
/// descriptive task ("QA: suggest one concrete CLI improvement") still surfaces
/// relevant agents. `registry::discover_agents` matches the query as a SINGLE
/// substring, so feeding it the whole sentence matches nothing — we split into
/// words, lowercase, strip punctuation, drop short/stop words, and de-dupe
/// (preserving order). Capped at `COLONY_MAX_KEYWORDS` so the discovery fan-out
/// stays bounded. Empty when the task has no significant words.
fn colony_task_keywords(task: &str) -> Vec<String> {
    const STOP: &[&str] = &[
        "the", "a", "an", "and", "or", "to", "of", "in", "on", "for", "with", "one", "two",
        "is", "are", "be", "this", "that", "your", "you", "it", "as", "at", "by", "from",
        "suggest", "please", "make", "give", "find", "do", "can", "should", "would", "about",
    ];
    let mut out: Vec<String> = Vec::new();
    for raw in task.split_whitespace() {
        let w: String = raw
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
            .to_lowercase();
        if w.len() < 3 || STOP.contains(&w.as_str()) || out.contains(&w) {
            continue;
        }
        out.push(w);
        if out.len() >= COLONY_MAX_KEYWORDS {
            break;
        }
    }
    out
}

/// Cap on keywords fanned out to `discover_agents` per task (bounds the reads).
const COLONY_MAX_KEYWORDS: usize = 6;

/// Discover task-relevant agents ROBUSTLY: try the full task as one query first
/// (cheap — catches an exact name/persona hit), then fan out across the task's
/// keywords ([`colony_task_keywords`]) and UNION the matches, preserving the
/// best rank each name was first seen at (so a name-hit / earlier-keyword agent
/// stays ahead). Returns `(name, persona)` rows in ascending rank order. This is
/// what lets `colony_pick_worker` find a worker for a descriptive task even
/// though `discover_agents` only does single-substring matching.
async fn colony_discover_relevant(task: &str) -> Result<Vec<(String, String)>, String> {
    // best rank seen per name + the persona; insertion order tracked separately.
    let mut best: std::collections::HashMap<String, (usize, String)> =
        std::collections::HashMap::new();
    let mut rank_cursor = 0usize;
    let mut absorb = |rows: Vec<(String, String)>, cursor: &mut usize| {
        for (name, persona) in rows {
            let r = *cursor;
            *cursor += 1;
            best.entry(name)
                .and_modify(|e| {
                    if r < e.0 {
                        e.0 = r;
                    }
                })
                .or_insert((r, persona));
        }
    };
    // 1. Full task verbatim (an exact persona/name hit ranks first).
    let full = registry::discover_agents(task, 100)
        .await
        .map_err(|e| format!("discover failed: {e}"))?;
    absorb(full, &mut rank_cursor);
    // 2. Per-keyword fan-out (keeps descriptive tasks discoverable).
    for kw in colony_task_keywords(task) {
        let rows = registry::discover_agents(&kw, 100)
            .await
            .map_err(|e| format!("discover failed: {e}"))?;
        absorb(rows, &mut rank_cursor);
    }
    let mut ranked: Vec<(String, (usize, String))> = best.into_iter().collect();
    ranked.sort_by_key(|(_, (rank, _))| *rank);
    Ok(ranked.into_iter().map(|(name, (_, persona))| (name, persona)).collect())
}

/// Auto-pick the best worker for `task`, REPUTATION-AWARE. Builds the set of
/// drivable candidates (a `discover` match whose identity key is present locally,
/// so it can sign its own claim+submit), reads each one's on-chain reputation,
/// then applies [`pick_reputation_aware`]. Returns `(name, reasoning_line)` so
/// the caller can echo WHY this worker was chosen, or an error naming what to do.
/// Read-only (no `$LH`).
async fn colony_pick_worker(task: &str) -> Result<(String, String), String> {
    let matches = colony_discover_relevant(task).await?;
    if matches.is_empty() {
        return Err(
            "no agents matched the task to auto-pick a worker — pass --worker <agent> \
             (an agent whose key is in your keys dir)"
                .to_string(),
        );
    }
    // Drivable candidates only: a discover match we ALSO hold a key for (it must
    // sign its own claim + submit). `task_rank` = the merged discover position.
    let mut candidates: Vec<WorkerCandidate> = Vec::new();
    for (task_rank, (name, _persona)) in matches.iter().enumerate() {
        if resolve_key_read_path(name).is_none() {
            continue;
        }
        // Read on-chain reputation for this candidate (count, rating sum). A read
        // failure / unregistered name is treated as "no reputation" (0, 0) so a
        // transient RPC hiccup can't drop an otherwise-drivable worker.
        let (rep_count, rep_sum) = match registry::id_of_name(name).await {
            Ok(id) if id != 0 => registry::reputation_of(id).await.unwrap_or((0, 0)),
            _ => (0, 0),
        };
        candidates.push(WorkerCandidate {
            name: name.clone(),
            task_rank,
            rep_count,
            rep_sum,
        });
    }
    match pick_reputation_aware(&candidates) {
        Some(c) => Ok((c.name.clone(), colony_pick_reasoning(c))),
        None => Err(format!(
            "the top discover() matches ({}) have no local key — pass --worker <agent> whose \
             key is in your keys dir (the worker signs its own claim + submit)",
            matches.iter().take(5).map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// Pure: choose up to `n` DISTINCT neutral judges from the locally-keyed agent
/// names `local`, EXCLUDING the `worker` and the `caller` (so neither the party
/// being rated nor the party that posted the work can score it — that's the
/// neutrality the panel buys). `local` is taken in its caller-supplied order
/// (`identity_key_files` sorts by name, so selection is deterministic); the first
/// `n` eligible names are taken. Returns fewer than `n` when too few neutral
/// agents exist (the caller notes the shortfall + still runs the smaller panel).
/// Empty only when there is NO neutral local agent at all. Testable with no fs.
fn select_judge_panel(local: &[String], worker: &str, caller: &str, n: usize) -> Vec<String> {
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut panel: Vec<String> = Vec::new();
    for name in local {
        if panel.len() >= n {
            break;
        }
        let s = name.as_str();
        if s == worker || s == caller || !seen.insert(s) {
            continue; // exclude the worker, the caller, and de-dupe.
        }
        panel.push(name.clone());
    }
    panel
}

/// Resolve the NEUTRAL JUDGE PANEL for `colony run`: scan every locally-keyed
/// identity ([`identity_key_files`] → bare names) and pick up to `n` DISTINCT
/// neutral agents, excluding the `worker` AND the `caller`. Returns the panel
/// names (each holds a local key, so each funds + signs its own judge turn). On
/// zero neutral agents this returns an empty Vec; the caller falls back to the
/// caller-as-judge so the cycle never strands the escrow.
fn resolve_judge_panel(worker: &str, caller: &str, n: usize) -> Vec<String> {
    let local: Vec<String> = identity_key_files()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|p| {
            std::path::Path::new(&p)
                .file_name()
                .and_then(|s| s.to_str())
                .and_then(|f| f.strip_suffix(KEY_SUFFIX))
                .map(str::to_string)
        })
        .collect();
    select_judge_panel(&local, worker, caller, n)
}

/// `true` if a sponsored-write error looks TRANSIENT (an RPC/transport hiccup,
/// not a contract revert) — worth one retry. The Tempo RPC intermittently fails
/// to decode the `eth_sendRawTransaction` RESPONSE even when the tx mined, so we
/// re-check on-chain state before retrying (the caller does that). Pure.
fn is_transient_rpc_error(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    (e.contains("decode") || e.contains("decoding") || e.contains("timed out")
        || e.contains("timeout") || e.contains("connection") || e.contains("response body")
        || e.contains("eof"))
        // A real on-chain revert is NOT transient — those carry a reason/selector.
        && !e.contains("revert") && !e.contains("execution reverted")
}

/// Drive a `colony` on-chain WRITE step with ONE transient-error retry that's
/// guarded by an idempotence check: before retrying, read `getBounty(id).status`
/// and treat it as success if the chain ALREADY advanced past `done_at_or_after`
/// (the original tx mined; the failure was only the response decode). This is the
/// fix for the live decode-error-at-accept seen dogfooding the cycle — without it
/// a flaky RPC stranded the escrow in `Submitted`. `attempt` runs the sponsored
/// write; `step`/`verb` label the output. Returns the tx hash (or "(already
/// advanced on-chain)") on success, or a final error string on real failure.
async fn colony_write_step<F, Fut>(
    bounty_id: u64,
    step: &str,
    verb: &str,
    done_at_or_after: u8,
    attempt: F,
) -> Result<String, String>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<String, String>>,
{
    match attempt().await {
        Ok(tx) => Ok(tx),
        Err(e) if is_transient_rpc_error(&e) => {
            eprintln!("      … {step} {verb}: transient RPC error ({e}); re-checking on-chain state …");
            // Did the original tx actually mine despite the bad response?
            if let Ok(b) = registry::get_bounty(bounty_id).await {
                if b.status >= done_at_or_after && b.status != 4 && b.status != 5 {
                    return Ok("(already advanced on-chain — the original tx mined)".to_string());
                }
            }
            eprintln!("      … retrying {step} {verb} once …");
            attempt().await
        }
        Err(e) => Err(e),
    }
}

/// Surface a mid-cycle `colony run` failure with the CORRECT escrow-recovery
/// command for the bounty's LIVE on-chain status. `cancelBounty` only works while
/// the bounty is OPEN — once a worker has CLAIMED it (status ≥ 1) cancel reverts
/// `NotOpen`, and the only recovery is the ttl-gated `reclaimExpired`
/// (`bounty reclaim`). Re-reading `getBounty(id).status` makes the advice right
/// even when a claim's tx mined but its RESPONSE decode failed (status = Claimed).
/// Returns the process exit code (always `1` — a failed cycle).
async fn colony_bail(bounty_id: u64, caller_label: &str, stage: &str, err: &str) -> i32 {
    eprintln!("[{stage}] {err}");
    let status = registry::get_bounty(bounty_id).await.ok().map(|b| b.status);
    eprintln!("{}", colony_recovery_hint(bounty_id, caller_label, status));
    eprintln!("  Inspect: localharness bounty mine --as {caller_label}");
    1
}

/// Pure: pick the CORRECT escrow-recovery hint for a stranded bounty given its
/// live on-chain `status` (`None` = the status read itself failed). The crux:
/// `bounty cancel` (`cancelBounty`) is accepted ONLY while OPEN (status 0) — once
/// CLAIMED/SUBMITTED (1/2) it reverts `NotOpen`, so the only recovery is the
/// ttl-gated `bounty reclaim` (`reclaimExpired`). Paid (3) / Cancelled (4) /
/// Reclaimed (5) are terminal (nothing to recover). On an unknown/unreadable
/// status, advise BOTH so the user is never stuck. Testable with no network.
fn colony_recovery_hint(bounty_id: u64, caller_label: &str, status: Option<u8>) -> String {
    match status {
        Some(0) => format!(
            "  ⚠ bounty #{bounty_id} is OPEN and unsettled. Recover the $LH now with:\n    \
             localharness bounty cancel --as {caller_label} {bounty_id}"
        ),
        Some(s @ (1 | 2)) => {
            let st = if s == 1 { "claimed" } else { "submitted" };
            format!(
                "  ⚠ bounty #{bounty_id} is {st} (already past OPEN) so `bounty cancel` would \
                 revert — the escrow refunds only after the ttl. Recover the $LH with:\n    \
                 localharness bounty reclaim --as {caller_label} {bounty_id}   (works once the ttl has expired)"
            )
        }
        Some(3) => format!(
            "  bounty #{bounty_id} is already PAID — the reward settled to the worker; nothing to recover."
        ),
        Some(4) | Some(5) => format!(
            "  bounty #{bounty_id} is already refunded (cancelled/reclaimed); nothing to recover."
        ),
        _ => format!(
            "  ⚠ bounty #{bounty_id} is escrowed and unsettled. If it is still OPEN: \
             `localharness bounty cancel --as {caller_label} {bounty_id}`; if a worker has already \
             claimed it, wait for the ttl then `localharness bounty reclaim --as {caller_label} {bounty_id}`."
        ),
    }
}

/// `colony run` — drive ONE autonomous post→claim→work→submit→JUDGE→
/// (accept-or-reject)→attest cycle. Each on-chain step reuses the bounty helpers;
/// the work AND the judge both reuse `run_agent_turn`. The [6/8] JUDGE step scores
/// the worker's result 1-5 for genuine + accurate task-fit; [7/8] is the PAYMENT
/// GATE — the caller accepts + pays ONLY when the panel median is `>=
/// --min-accept-rating` (default 2), else REJECTS (no payment; the escrow stays
/// locked, reclaimable via `bounty reclaim` after the ttl). [8/8] ATTEST signs the
/// panel median on-chain (not a hardcoded 5★) on BOTH branches — so reputation
/// reflects judged quality even for rejected work. A reject is a NORMAL outcome
/// (exit 0). On any failure mid-cycle the bounty id is surfaced so the escrow is
/// never silently stranded.
async fn colony_run(caller: Option<&str>, rest: &[String]) -> i32 {
    let ParsedColonyRun { task, reward_wei, worker, judge, judges, min_accept, ttl_secs } =
        match parse_colony_run_args(rest) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("{e}");
                return 2;
            }
        };
    if task.trim().is_empty() {
        eprintln!("colony run: task is empty");
        return 2;
    }

    // The caller (platform / poster) — its key signs the post + accept and pays
    // the headless `call` that runs the work.
    let (caller_signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    let caller_addr = addr_to_hex(wallet::address(&caller_signer));
    let caller_label = match resolve_caller_label(caller) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("colony run: {e}");
            return 2;
        }
    };
    // The caller key (hex) drives the headless work turn (proxy auth + $LH).
    let caller_key_hex = match resolve_caller_key(caller) {
        Ok((_, hex)) => hex,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };

    println!("=== COLONY RUN — one autonomous agent-economy cycle ===");
    println!("  caller (poster): {caller_label}  ({caller_addr})");
    println!("  task:            {task}");
    println!("  reward:          {}", fmt_lh(reward_wei));
    println!();

    // -- STEP 1: the caller POSTS the bounty (escrows the reward). ----------
    println!("[1/8] POST  — escrowing {} behind the task …", fmt_lh(reward_wei));
    let post_tx = match registry::post_bounty_sponsored(
        &caller_signer,
        &sponsor,
        task.as_bytes(),
        reward_wei,
        ttl_secs,
        registry::ALPHA_USD_ADDRESS,
    )
    .await
    {
        Ok(tx) => tx,
        Err(e) => {
            eprintln!("[1/8] POST failed: {e}");
            eprintln!("  no escrow was created — nothing to clean up.");
            return 1;
        }
    };
    // The new bounty id is the last entry in the poster's bountiesOf index.
    let bounty_id = match registry::bounties_of(&caller_addr).await {
        Ok(ids) if !ids.is_empty() => ids[ids.len() - 1],
        Ok(_) => {
            eprintln!(
                "[1/8] POST mined (tx {post_tx}) but the new bounty id could not be read back \
                 from bountiesOf — re-run `bounty mine` to find + manage it."
            );
            return 1;
        }
        Err(e) => {
            eprintln!(
                "[1/8] POST mined (tx {post_tx}) but reading the bounty id failed: {e} \
                 — re-run `bounty mine` to find + manage it."
            );
            return 1;
        }
    };
    println!("      ✓ bounty #{bounty_id} posted  (tx {post_tx})");
    println!();

    // From here on, any failure must surface the bounty id so the escrow can be
    // reclaimed — never a silent half-state. The recovery COMMAND depends on the
    // bounty's live on-chain status: `cancelBounty` only works while OPEN (it
    // reverts `NotOpen` once a worker has CLAIMED), so a failure AFTER the claim
    // mined must steer the user to the EXPIRY → `bounty reclaim` path instead.
    // `colony_bail` re-reads `getBounty(id).status` so the advice is correct even
    // when a claim's tx mined but its response decode failed (status = Claimed).
    macro_rules! bail {
        ($stage:expr, $err:expr) => {
            return colony_bail(bounty_id, &caller_label, $stage, &$err).await
        };
    }

    // -- STEP 2: pick + resolve the WORKER. --------------------------------
    let worker_name = match worker {
        Some(w) => w,
        None => {
            println!("[2/8] PICK  — auto-selecting the best worker (reputation-aware) …");
            match colony_pick_worker(&task).await {
                Ok((w, why)) => {
                    println!("      ✓ {why}");
                    w
                }
                Err(e) => bail!("2/8", format!("PICK failed: {e}")),
            }
        }
    };
    // The worker signs its OWN claim + submit, so its key must be local.
    let (worker_key_file, worker_key_hex) = match resolve_caller_key(Some(&worker_name)) {
        Ok(c) => c,
        Err(e) => {
            bail!(
                "2/8",
                format!(
                    "worker '{worker_name}' has no local identity key ({e}). The worker must be a \
                     fleet/owned agent whose key is in your keys dir — it signs its own claim + submit."
                )
            )
        }
    };
    let worker_signer = match wallet::from_private_key_hex(&worker_key_hex) {
        Ok(s) => s,
        Err(e) => bail!("2/8", format!("bad worker key in {worker_key_file}: {e}")),
    };
    // The worker's tokenId (the identity that earns the reward) + its TBA wallet
    // (where the reward lands) — resolve both up front so the payout is verifiable.
    let worker_token_id = match resolve_own_token_id(Some(&worker_name), &worker_signer).await {
        Ok(id) => id,
        Err(e) => bail!("2/8", format!("could not resolve worker '{worker_name}' identity: {e}")),
    };
    let worker_tba = match registry::tba_of_token_id(worker_token_id).await {
        Ok(Some(a)) => a,
        Ok(None) => bail!("2/8", format!("worker token #{worker_token_id} has no token-bound account")),
        Err(e) => bail!("2/8", format!("RPC error resolving worker TBA: {e}")),
    };
    let tba_before = registry::token_balance_of(&worker_tba).await.unwrap_or(0);
    println!("      worker {worker_name} = token #{worker_token_id}, TBA {worker_tba}");
    println!("      worker TBA $LH before: {}", fmt_lh(tba_before));
    println!();

    // -- STEP 3: the worker CLAIMS the bounty. -----------------------------
    println!("[3/8] CLAIM — {worker_name} claims bounty #{bounty_id} (reward → its TBA) …");
    match colony_write_step(bounty_id, "3/8", "CLAIM", 1, || {
        registry::claim_bounty_sponsored(
            &worker_signer,
            &sponsor,
            bounty_id,
            worker_token_id,
            registry::ALPHA_USD_ADDRESS,
        )
    })
    .await
    {
        Ok(tx) => println!("      ✓ claimed by token #{worker_token_id}  (tx {tx})"),
        Err(e) => bail!("3/8", format!("CLAIM failed: {e}")),
    }
    println!();

    // -- STEP 4: run the WORK — a headless turn as the worker's persona. ----
    println!("[4/8] WORK  — running {worker_name}'s persona on the task (headless `call`) …");
    let work_prompt = format!(
        "{task}\n\nSubmit your concrete result / deliverable as your reply \
         (it will be recorded on-chain as your bounty submission)."
    );
    // The caller pays for the work turn (same as `call --as caller worker …`),
    // running the WORKER's on-chain persona. No prior history (a one-shot job).
    let result_text = match run_agent_turn(
        &caller_key_hex,
        &worker_name,
        &work_prompt,
        None,
        None,
    )
    .await
    {
        Ok((text, _hist)) => {
            let trimmed = text.trim().to_string();
            if trimmed.is_empty() {
                bail!("4/8", "WORK produced an empty result — nothing to submit.".to_string());
            }
            trimmed
        }
        Err(e) => {
            report_call_error("[4/8] WORK failed", &e);
            bail!("4/8", "the worker's persona turn failed — see the hint above.".to_string());
        }
    };
    println!("      ✓ {worker_name} produced a result:");
    println!("      ┌─────────────────────────────────────────────────────────");
    for line in result_text.lines() {
        println!("      │ {line}");
    }
    println!("      └─────────────────────────────────────────────────────────");
    println!();

    // -- STEP 5: the worker SUBMITS the result. ----------------------------
    println!("[5/8] SUBMIT — {worker_name} submits its result for bounty #{bounty_id} …");
    match colony_write_step(bounty_id, "5/8", "SUBMIT", 2, || {
        registry::submit_result_sponsored(
            &worker_signer,
            &sponsor,
            bounty_id,
            result_text.as_bytes(),
            registry::ALPHA_USD_ADDRESS,
        )
    })
    .await
    {
        Ok(tx) => println!("      ✓ result submitted  (tx {tx})"),
        Err(e) => bail!("5/8", format!("SUBMIT failed: {e}")),
    }
    println!();

    // -- STEP 6: a NEUTRAL JUDGE PANEL scores the result; take the MEDIAN. ---
    // This is what makes the attestation MEANINGFUL and TRUSTWORTHY: the rating
    // below is the MEDIAN of N neutral judges (default 3), not one self-interested
    // score. The panel EXCLUDES the worker (don't grade your own work) AND the
    // caller (the poster has skin in the game — its score would bias the
    // reputation signal that now DRIVES the PICK step). `--judge <agent>` forces a
    // panel of exactly that one named agent. Each judge signs + funds its OWN turn
    // (its key is local); the judge agent's PERSONA is embodied but the impartial
    // PROMPT overrides its framing. A failed judge turn doesn't bail (the payout
    // still happens) — and if ALL judges fail the median falls back to a neutral 3
    // so the cycle completes with an honest, non-inflated rating.
    //
    // Build the panel: an explicit `--judge X` = the single agent X; else
    // auto-select up to `judges` neutral local agents (excluding worker + caller).
    let panel: Vec<String> = match &judge {
        Some(j) => vec![j.clone()],
        None => resolve_judge_panel(&worker_name, &caller_label, judges),
    };
    println!(
        "[6/8] JUDGE — neutral panel scores {worker_name}'s result 1-5 (accuracy-checked) …"
    );
    if judge.is_none() {
        if panel.is_empty() {
            // No neutral local agent — fall back to the caller as a single judge
            // (better an interested score than stranding the cycle). Loud note.
            println!(
                "      ⚠ no neutral local agent (excluding the worker + caller) to form a panel; \
                 falling back to the caller ({caller_label}) as a single judge."
            );
        } else if panel.len() < judges {
            println!(
                "      note: only {} neutral local agent(s) available (asked for {judges}); \
                 running a panel of {}.",
                panel.len(),
                panel.len()
            );
        }
    }
    // Run each judge in turn, collecting (label, rating, rationale). A judge whose
    // turn FAILS is skipped (logged) — it doesn't pollute the median with a
    // fabricated score. The caller key pays the fallback (caller-as-judge) turn.
    let judge_prompt = colony_judge_prompt(&task, &result_text);
    let mut panel_results: Vec<(String, u8, String)> = Vec::new();
    // The effective panel: the resolved neutral agents, or — when empty — the
    // caller acting as the lone judge (paid by the caller key already loaded).
    let effective_panel: Vec<String> =
        if panel.is_empty() { vec![caller_label.clone()] } else { panel.clone() };
    for judge_name in &effective_panel {
        // Each neutral judge funds + signs its own turn; the caller-fallback judge
        // reuses the caller key (so a missing-key judge can't strand the escrow).
        let judge_key_hex = if judge_name == &caller_label {
            caller_key_hex.clone()
        } else {
            match resolve_caller_key(Some(judge_name)) {
                Ok((_, hex)) => hex,
                Err(e) => {
                    eprintln!(
                        "      ⚠ judge '{judge_name}' has no local identity key ({e}); skipping it."
                    );
                    continue;
                }
            }
        };
        match run_agent_turn(&judge_key_hex, judge_name, &judge_prompt, None, None).await {
            Ok((reply, _hist)) => {
                let (rating, rationale) = parse_judge_rating(&reply);
                let rating = rating.clamp(1, 5);
                println!("      • {judge_name}: {rating}★");
                if !rationale.is_empty() {
                    println!("        {rationale}");
                }
                panel_results.push((judge_name.clone(), rating, rationale));
            }
            Err(e) => {
                report_call_error(&format!("[6/8] JUDGE turn failed ({judge_name})"), &e);
                println!("      ⚠ judge '{judge_name}' turn failed — excluded from the median.");
            }
        }
    }
    // Aggregate to the MEDIAN. If EVERY judge turn failed, `median_rating([])`
    // returns the neutral 3 default — the cycle still completes with an honest,
    // non-inflated rating (the worker is never credited a false 5★).
    let panel_ratings: Vec<u8> = panel_results.iter().map(|(_, r, _)| *r).collect();
    let judged_rating = median_rating(&panel_ratings).clamp(1, 5);
    if panel_ratings.is_empty() {
        println!(
            "      ⚠ every judge turn failed — defaulting to a neutral {judged_rating}★ \
             (the cycle still completes; the worker is not credited a false 5★)."
        );
    } else {
        // Echo the panel + the median, e.g. "panel: dex-qa 5★, iris-qa 4★ → median 5★".
        let summary = panel_results
            .iter()
            .map(|(n, r, _)| format!("{n} {r}★"))
            .collect::<Vec<_>>()
            .join(", ");
        println!("      ✓ panel: {summary} → median {judged_rating}★");
    }
    println!();

    // -- STEP 7: the PAYMENT GATE — ACCEPT (pay) the work OR REJECT it. -----
    // The colony is economically rational: it pays ONLY for work the NEUTRAL
    // panel rates at or above the `--min-accept-rating` bar (default 2). A median
    // BELOW the bar (e.g. 1 = clear failure / hallucination) is REJECTED — the
    // caller does NOT accept, so the worker is NOT paid and the escrow stays
    // locked, recoverable by the poster via `reclaimExpired` (`bounty reclaim`)
    // after the ttl. NO contract change: a reject is simply the absence of an
    // accept on a Submitted bounty (BountyFacet.reclaimExpired accepts the
    // Submitted state once expired). Either branch STILL attests the panel median
    // in step 8 — reputation must record the bad work even when payment is denied.
    let accept = should_accept(judged_rating, min_accept);
    if accept {
        println!(
            "[7/8] ACCEPT — median {judged_rating}★ ≥ min {min_accept}★ → caller accepts + pays the \
             escrow to {worker_name}'s TBA …"
        );
        match colony_write_step(bounty_id, "7/8", "ACCEPT", 3, || {
            registry::accept_result_sponsored(
                &caller_signer,
                &sponsor,
                bounty_id,
                registry::ALPHA_USD_ADDRESS,
            )
        })
        .await
        {
            Ok(tx) => println!("      ✓ accepted — {} settled  (tx {tx})", fmt_lh(reward_wei)),
            Err(e) => bail!("7/8", format!("ACCEPT failed: {e}")),
        }
    } else {
        // REJECT: the work scored below the bar. Do NOT accept/pay — the worker
        // keeps NOTHING. The escrow remains locked on the Submitted bounty; the
        // poster recovers it via the ttl-gated `bounty reclaim`. This is a NORMAL
        // outcome (a rational colony refusing sub-quality work), not an error.
        println!(
            "[7/8] REJECT — median {judged_rating}★ < min {min_accept}★ → caller does NOT accept; \
             {worker_name} is NOT paid."
        );
        println!("      ✗ result REJECTED ({judged_rating}★ below the {min_accept}★ bar).");
        println!("      ✗ the escrow ({}) was NOT released — the worker keeps NOTHING.", fmt_lh(reward_wei));
        println!(
            "      the escrow is reclaimable by the poster AFTER the ttl with:\n        \
             localharness bounty reclaim --as {caller_label} {bounty_id}"
        );
    }
    println!();

    // -- STEP 8: the caller ATTESTS the JUDGE'S rating → on-chain reputation. -
    // ALWAYS runs, accept OR reject: reputation must reflect the work's true
    // quality (a rejected 1★ result is recorded as 1★, so the bad worker's
    // reputation drops and the PICK step routes around it next time). Attestation
    // is reputation, not payment, so it is the SAME on both branches. A failure
    // here WARNS but does NOT fail the cycle (and never triggers `bail` — on the
    // accept branch the escrow is settled; on the reject branch it is reclaimable).
    println!(
        "[8/8] ATTEST — caller attests {judged_rating}★ (the JUDGE's rating) to {worker_name} \
         (workRef = bounty #{bounty_id}) …"
    );
    let work_ref = bounty_work_ref(bounty_id);
    match registry::attest_sponsored(
        &caller_signer,
        &sponsor,
        worker_token_id,
        judged_rating,
        work_ref,
        registry::ALPHA_USD_ADDRESS,
    )
    .await
    {
        Ok(tx) => println!(
            "      ✓ attested {judged_rating}★ to {worker_name} (token #{worker_token_id})  (tx {tx})"
        ),
        Err(e) => println!(
            "      ⚠ ATTEST failed: {e}\n      \
             (attestation is a bonus; not failing the cycle. \
             Retry later with: localharness reputation attest --as {caller_label} {worker_name} {judged_rating} --ref {bounty_id})"
        ),
    }
    println!();

    // -- Verify the outcome against the worker's TBA $LH. -------------------
    let tba_after = registry::token_balance_of(&worker_tba).await.unwrap_or(tba_before);
    let delta = tba_after.saturating_sub(tba_before);
    if accept {
        println!("=== CYCLE COMPLETE (ACCEPTED) ===");
        println!("  bounty #{bounty_id}: open → claimed → submitted → accepted → PAID");
        println!("  worker TBA {worker_tba}");
        println!("    before: {}", fmt_lh(tba_before));
        println!("    after:  {}", fmt_lh(tba_after));
        println!("    delta:  +{}  (reward {})", fmt_lh(delta), fmt_lh(reward_wei));
        if delta == reward_wei {
            println!("  ✓ payout verified — the worker's TBA rose by exactly the reward.");
        } else {
            // The cycle COMPLETED on-chain (accept mined); a balance read can lag a
            // block or another tx can touch the TBA. Report honestly, don't fail the
            // accepted cycle — the escrow is settled either way.
            println!(
                "  ⚠ TBA delta ({}) != reward ({}). The accept tx mined (the bounty is PAID), \
                 but the balance check didn't line up exactly — a read can lag a block or another \
                 tx touched the TBA. Re-check with: localharness tba show {worker_name}",
                fmt_lh(delta),
                fmt_lh(reward_wei)
            );
        }
    } else {
        // The KEY PROOF of the gate: a rejected result NEVER moves $LH to the
        // worker's TBA. The cycle ended on a Submitted (not Paid) bounty.
        println!("=== CYCLE COMPLETE (REJECTED — NOT PAID) ===");
        println!("  bounty #{bounty_id}: open → claimed → submitted → REJECTED (still Submitted, escrow locked)");
        println!("  worker TBA {worker_tba}");
        println!("    before: {}", fmt_lh(tba_before));
        println!("    after:  {}", fmt_lh(tba_after));
        println!("    delta:  +{}  (NO payout — median {judged_rating}★ < min {min_accept}★)", fmt_lh(delta));
        if delta == 0 {
            println!("  ✓ reject verified — the worker's TBA did NOT rise (it was not paid).");
        } else {
            println!(
                "  ⚠ the worker's TBA rose by {} despite the reject — the colony did NOT accept \
                 this bounty, so this delta came from ANOTHER tx, not this reward. Re-check with: \
                 localharness tba show {worker_name}",
                fmt_lh(delta)
            );
        }
        println!(
            "  the escrow stays locked on the Submitted bounty; reclaim it after the ttl with:\n    \
             localharness bounty reclaim --as {caller_label} {bounty_id}"
        );
    }
    // A reject is a NORMAL outcome (the colony rationally declined sub-quality
    // work), not an error — exit 0 on both branches.
    0
}

/// Load the caller's identity signer + the embedded sponsor in one shot, mapping
/// any failure to a process exit code. The shared front-half of every sponsored
/// `bounty` write (post/claim/submit/accept/cancel).
fn load_signer_and_sponsor(
    caller: Option<&str>,
) -> Result<(k256::ecdsa::SigningKey, k256::ecdsa::SigningKey), i32> {
    let (key_file, key_hex) = resolve_caller_key(caller).map_err(|e| {
        eprintln!("{e}");
        2
    })?;
    let signer = wallet::from_private_key_hex(&key_hex).map_err(|e| {
        eprintln!("bad key in {key_file}: {e}");
        1
    })?;
    let sponsor = wallet::from_private_key_hex(SPONSOR_KEY).map_err(|e| {
        eprintln!("sponsor key error: {e}");
        1
    })?;
    Ok((signer, sponsor))
}

/// Resolve the caller's OWN registered tokenId — the `claimantTokenId` that earns
/// a bounty reward. Resolution order (each a `name.localharness.xyz` NFT the
/// caller controls):
///   1. If `--as <name>` was given AND that name is registered → its tokenId
///      (the explicit "act as THIS subdomain" intent).
///   2. Else the caller's MAIN identity (`mainOf(address)`), their primary NFT.
///   3. Else their single owned token (if they hold exactly one).
/// A caller with NO registered identity can't claim — they must `create <name>`
/// first (the reward needs an on-chain identity to be paid to).
async fn resolve_own_token_id(
    caller: Option<&str>,
    signer: &k256::ecdsa::SigningKey,
) -> Result<u64, String> {
    // 1. Explicit --as <name> that is registered.
    if let Some(name) = caller {
        if let Ok(id) = registry::id_of_name(name).await {
            if id != 0 {
                return Ok(id);
            }
        }
    }
    let addr = addr_to_hex(wallet::address(signer));
    // 2. The caller's MAIN identity.
    if let Ok(main_id) = registry::main_of(&addr).await {
        if main_id != 0 {
            return Ok(main_id);
        }
    }
    // 3. Their sole owned token (unambiguous), else a clear error.
    match registry::list_owned_tokens(&addr).await {
        Ok(tokens) if tokens.len() == 1 => Ok(tokens[0].token_id),
        Ok(tokens) if tokens.is_empty() => Err(format!(
            "no registered identity for {addr} — run `localharness create <name>` first \
             (a bounty reward needs an on-chain identity to pay)"
        )),
        Ok(tokens) => Err(format!(
            "{addr} owns {} subdomains and has no MAIN set — pass `--as <name>` to pick \
             which identity claims the bounty",
            tokens.len()
        )),
        Err(e) => Err(format!("RPC error resolving your tokenId: {e}")),
    }
}

// ---- tba (token-bound account: make YOUR agent's wallet EXECUTE a call) ------
//
// The headless / agent equivalent of the browser act-panel. Every identity NFT
// has a deterministic token-bound account (ERC-6551 `MultiSignerAccount`) — a
// smart wallet the NFT holder controls. This command lets an agent ACT through
// it from a shell, with no browser tab: deploy it, see its `$LH`, and make it
// EXECUTE an arbitrary call (a `$LH` transfer, or any `to` + `--data <hex>`).
// Authorization is enforced on-chain by the account (only the NFT holder or an
// enrolled signer can `execute`); the embedded sponsor pays the fee. Unblocks a
// guild's TBA voting in a parent DAO, an agent's TBA paying/calling, etc.
// Mirrors `registry::tba_execute_call_sponsored` / `tba_send_lh_sponsored` /
// `create_token_bound_account_sponsored` — the same flat sponsored path as the
// browser `agent_send_lh_pressed` act-panel.

const TBA_USAGE: &str = "\
usage: localharness tba <show|deploy|exec> ...
  tba show   [--as <me>] [<name>]            your (or <name>'s) TBA address, $LH, deployed?
  tba deploy [--as <me>] [<name>]            deploy the TBA on-chain (createTokenBoundAccount)
  tba exec   [--as <me>] [--tba <name-or-0xaddr>] <to> <amount> [--data <hex>]
                                             make a TBA execute a call:
                                               no --data → send <amount> $LH to <to>
                                               --data <hex> → call <to> with <hex>, <amount> as value
                                               --tba → act through an owned TBA other than
                                                       your main (a guild's wallet, etc.); default
                                                       is your main TBA. The chain gates execute to
                                                       the TBA owner, so it must be one you control.
  <to> is a name (→ its on-chain owner) or a 0x… address.
  <amount> is in $LH (the transfer amount, or the ETH value forwarded with --data).";

async fn tba(caller: Option<&str>, rest: &[String]) -> i32 {
    match rest.first().map(String::as_str) {
        Some("show") => tba_show(caller, rest.get(1).map(String::as_str)).await,
        Some("deploy") => tba_deploy(caller, rest.get(1).map(String::as_str)).await,
        Some("exec") => tba_exec(caller, &rest[1..]).await,
        _ => {
            eprintln!("{TBA_USAGE}");
            2
        }
    }
}

/// Resolve the tokenId to operate on: an explicit `<name>` if given (it must be
/// registered), else the caller's OWN identity (`resolve_own_token_id` — MAIN,
/// or sole holding). Returns `(token_id, label)` where `label` is for display.
async fn tba_target_token(
    caller: Option<&str>,
    name: Option<&str>,
    signer: &k256::ecdsa::SigningKey,
) -> Result<(u64, String), String> {
    if let Some(n) = name {
        match registry::id_of_name(n).await {
            Ok(0) => Err(format!("tba: '{n}' is not registered")),
            Ok(id) => Ok((id, n.to_string())),
            Err(e) => Err(format!("tba: RPC error resolving '{n}': {e}")),
        }
    } else {
        let id = resolve_own_token_id(caller, signer).await?;
        let label = registry::name_of_id(id).await.unwrap_or_else(|_| format!("token #{id}"));
        Ok((id, label))
    }
}

/// `tba show [--as <me>] [<name>]` — print the token-bound account address, its
/// `$LH` balance, and whether it's deployed on-chain. Read-only, no `$LH` spent.
/// With an explicit `<name>` it's a PURE read (no local identity key needed —
/// you can inspect any agent's wallet); without one it resolves YOUR identity,
/// which requires a local key.
async fn tba_show(caller: Option<&str>, name: Option<&str>) -> i32 {
    let (token_id, label) = if let Some(n) = name {
        // Explicit name → pure on-chain read, no key required.
        match registry::id_of_name(n).await {
            Ok(0) => {
                eprintln!("tba show: '{n}' is not registered");
                return 1;
            }
            Ok(id) => (id, n.to_string()),
            Err(e) => {
                eprintln!("tba show: RPC error resolving '{n}': {e}");
                return 1;
            }
        }
    } else {
        // No name → resolve the caller's OWN identity (needs a local key).
        let (key_file, key_hex) = match resolve_caller_key(caller) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{e}");
                return 2;
            }
        };
        let signer = match wallet::from_private_key_hex(&key_hex) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("bad key in {key_file}: {e}");
                return 1;
            }
        };
        match tba_target_token(caller, None, &signer).await {
            Ok(t) => t,
            Err(e) => {
                eprintln!("{e}");
                return 1;
            }
        }
    };
    let tba_addr = match registry::tba_of_token_id(token_id).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            eprintln!("tba show: no token-bound account for '{label}' (token #{token_id})");
            return 1;
        }
        Err(e) => {
            eprintln!("tba show: RPC error: {e}");
            return 1;
        }
    };
    let balance = registry::token_balance_of(&tba_addr).await.unwrap_or(0);
    let deployed = registry::is_contract_deployed(&tba_addr).await.unwrap_or(false);
    println!("{label}  (token #{token_id})");
    println!("  wallet (TBA):  {tba_addr}");
    println!("  balance:       {}", fmt_lh(balance));
    println!(
        "  deployed:      {}",
        if deployed { "yes" } else { "no — run `tba deploy` before it can execute" }
    );
    0
}

/// `tba deploy [--as <me>] [<name>]` — deploy the token-bound account on-chain
/// via `createTokenBoundAccount` (idempotent; a no-op if already deployed).
/// Needed before the TBA can `execute` / hold signers. Sponsored gas.
async fn tba_deploy(caller: Option<&str>, name: Option<&str>) -> i32 {
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    let (token_id, label) = match tba_target_token(caller, name, &signer).await {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let tba_addr = match registry::tba_of_token_id(token_id).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            eprintln!("tba deploy: no token-bound account for '{label}' (token #{token_id})");
            return 1;
        }
        Err(e) => {
            eprintln!("tba deploy: RPC error: {e}");
            return 1;
        }
    };
    if registry::is_contract_deployed(&tba_addr).await.unwrap_or(false) {
        println!("{label}'s TBA {tba_addr} is already deployed — nothing to do.");
        return 0;
    }
    println!("deploying {label}'s TBA {tba_addr} …");
    match registry::create_token_bound_account_sponsored(
        &signer,
        &sponsor,
        token_id,
        registry::ALPHA_USD_ADDRESS,
    )
    .await
    {
        Ok(tx) => {
            println!("✓ deployed  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("tba deploy failed: {e}");
            1
        }
    }
}

/// `tba exec [--as <me>] [--tba <name-or-0xaddr>] <to> <amount> [--data <hex>]` —
/// make a token-bound account EXECUTE a call. With no `--data` this is a plain
/// `$LH` transfer of `<amount>` to `<to>` (`execute($LH, 0, transfer(to,
/// amount))`); with `--data <hex>` it calls `<to>` with that calldata and
/// forwards `<amount>` as the call value (`execute(to, amount, data)`). `<to>`
/// is a name (resolved to its on-chain owner address) or a raw `0x…` address.
/// The acting TBA defaults to the CALLER'S OWN main; `--tba` overrides it with
/// any TBA the caller controls — a name (→ `tokenBoundAccountByName`) or a raw
/// `0x…` address — so a GUILD's wallet can act (e.g. join + vote in a parent
/// guild's DAO). The MultiSignerAccount gates `execute` to the TBA owner
/// on-chain (`_isAuthorized`); a client-side owner check warns early for a name
/// target. The TBA is deployed first if needed (when its token id is known).
async fn tba_exec(caller: Option<&str>, rest: &[String]) -> i32 {
    // Pull an optional `--tba <name-or-0xaddr>` (override the acting TBA) and an
    // optional `--data <hex>` from anywhere in the args.
    let (tba_flag, after_tba) = match take_tba_flag(rest) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let (data_hex, positional) = match take_data_flag(&after_tba) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    if positional.len() != 2 {
        eprintln!("{TBA_USAGE}");
        return 2;
    }
    let to_arg = &positional[0];
    let amount_arg = &positional[1];

    // Resolve `<to>`: a 0x address, or a name → its on-chain OWNER.
    use localharness::encoding::{classify_recipient, Recipient};
    let to_hex = match classify_recipient(to_arg) {
        Ok(Recipient::Address(a)) => a,
        Ok(Recipient::Name(n)) => match registry::owner_of_name(&n).await {
            Ok(Some(o)) => o,
            Ok(None) => {
                eprintln!("tba exec: '{n}' is not registered");
                return 1;
            }
            Err(e) => {
                eprintln!("tba exec: RPC error resolving '{n}': {e}");
                return 1;
            }
        },
        Err(e) => {
            eprintln!("tba exec: {e}");
            return 2;
        }
    };

    // `<amount>` is the $LH transfer amount (no --data) or the ETH call value.
    let amount_wei = match localharness::encoding::parse_token_amount(amount_arg) {
        Some(w) => w,
        None => {
            eprintln!("tba exec: invalid amount '{amount_arg}' (expected a number of $LH)");
            return 2;
        }
    };
    // The transfer path needs a positive amount; the --data path may forward 0.
    if data_hex.is_none() && amount_wei == 0 {
        eprintln!("tba exec: amount must be greater than 0 for a $LH transfer");
        return 2;
    }

    // Decode `--data <hex>` (0x-optional) when present.
    let data = match &data_hex {
        Some(h) => match decode_hex_arg(h) {
            Ok(b) => Some(b),
            Err(e) => {
                eprintln!("tba exec: {e}");
                return 2;
            }
        },
        None => None,
    };

    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    let caller_addr = addr_to_hex(wallet::address(&signer));

    // Resolve the ACTING TBA. Default (no --tba) = the caller's OWN main TBA, as
    // before. With --tba it's an arbitrary owned TBA: a name → its
    // `tokenBoundAccountByName`, or a raw 0x address. The MultiSignerAccount gates
    // `execute` to the TBA owner on-chain (`_isAuthorized`), so signing as the
    // caller only works for a TBA the caller controls — the client check below is
    // a clean early warning, the chain is the real gate. `exec_token_id` is the id
    // backing the TBA when known (a name target / the caller's own), used to
    // auto-deploy a counterfactual TBA; `None` for a raw-address target (no
    // reverse index → we can't deploy it, only warn).
    let (tba_addr, exec_token_id, tba_label) = match &tba_flag {
        // --tba <name-or-0xaddr>: an explicit, possibly-foreign-but-owned TBA.
        Some(target) => {
            use localharness::encoding::{classify_recipient, Recipient};
            match classify_recipient(target) {
                Ok(Recipient::Address(a)) => {
                    // Raw TBA address — no on-chain reverse index to its token, so
                    // we can't resolve the controlling owner or auto-deploy. The
                    // on-chain `_isAuthorized` is the real gate.
                    (a.clone(), None, a)
                }
                Ok(Recipient::Name(n)) => {
                    let addr = match registry::tba_of_name(&n).await {
                        Ok(Some(a)) => a,
                        Ok(None) => {
                            eprintln!("tba exec: '{n}' is not registered (no token-bound account)");
                            return 1;
                        }
                        Err(e) => {
                            eprintln!("tba exec: RPC error resolving '{n}': {e}");
                            return 1;
                        }
                    };
                    // Client-side owner check: warn (don't block) when the name's
                    // controlling NFT owner isn't the caller. The chain still gates.
                    match registry::owner_of_name(&n).await {
                        Ok(Some(o)) if o.eq_ignore_ascii_case(&caller_addr) => {}
                        Ok(Some(o)) => {
                            eprintln!(
                                "warning: '{n}' is controlled by {o}, not you ({caller_addr}) — \
                                 the TBA's on-chain _isAuthorized will reject this unless you're \
                                 an enrolled signer."
                            );
                        }
                        _ => {}
                    }
                    // The token id backs the auto-deploy of a counterfactual TBA.
                    let id = registry::id_of_name(&n).await.unwrap_or(0);
                    (addr, if id != 0 { Some(id) } else { None }, n)
                }
                Err(e) => {
                    eprintln!("tba exec: --tba {e}");
                    return 2;
                }
            }
        }
        // No --tba: the caller's OWN identity (the original, unchanged behaviour).
        None => {
            let token_id = match resolve_own_token_id(caller, &signer).await {
                Ok(id) => id,
                Err(e) => {
                    eprintln!("{e}");
                    return 1;
                }
            };
            match registry::tba_of_token_id(token_id).await {
                Ok(Some(a)) => (a, Some(token_id), "your".to_string()),
                Ok(None) => {
                    eprintln!("tba exec: no token-bound account for your token #{token_id}");
                    return 1;
                }
                Err(e) => {
                    eprintln!("tba exec: RPC error: {e}");
                    return 1;
                }
            }
        }
    };

    // The TBA must be deployed before it can execute. Deploy first if we know its
    // token id (caller's own, or a name target). A raw-address target can't be
    // deployed (no token id) — surface a clean error instead of an opaque revert.
    if !registry::is_contract_deployed(&tba_addr).await.unwrap_or(false) {
        match exec_token_id {
            Some(token_id) => {
                println!("{tba_label} TBA {tba_addr} isn't deployed yet — deploying first …");
                if let Err(e) = registry::create_token_bound_account_sponsored(
                    &signer,
                    &sponsor,
                    token_id,
                    registry::ALPHA_USD_ADDRESS,
                )
                .await
                {
                    eprintln!("tba exec: TBA deploy failed: {e}");
                    return 1;
                }
            }
            None => {
                eprintln!(
                    "tba exec: TBA {tba_addr} isn't deployed and was given as a raw address \
                     (no token id to deploy it) — pass `--tba <name>` so it can be deployed, \
                     or deploy it first with `tba deploy`."
                );
                return 1;
            }
        }
    }

    let result = match &data {
        // Arbitrary call: execute(to, amount_as_value, data).
        Some(bytes) => {
            println!(
                "{tba_label} TBA {tba_addr} → execute({to_hex}, value {}, {} bytes of calldata) …",
                fmt_lh(amount_wei),
                bytes.len()
            );
            registry::tba_execute_call_sponsored(
                &signer,
                &sponsor,
                &tba_addr,
                &to_hex,
                amount_wei,
                bytes,
                registry::ALPHA_USD_ADDRESS,
            )
            .await
        }
        // Plain $LH transfer: execute($LH, 0, transfer(to, amount)).
        None => {
            println!("{tba_label} TBA {tba_addr} → send {} $LH to {to_hex} …", fmt_lh(amount_wei));
            registry::tba_send_lh_sponsored(
                &signer,
                &sponsor,
                &tba_addr,
                &to_hex,
                amount_wei,
                registry::ALPHA_USD_ADDRESS,
            )
            .await
        }
    };
    match result {
        Ok(tx) => {
            println!("✓ executed  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("tba exec failed: {e}");
            1
        }
    }
}

/// Extract an optional `--tba <name-or-0xaddr>` flag (from anywhere) and return
/// `(Option<value>, remaining)`. A second `--tba` is an error. Pure + testable;
/// `tba exec` uses it to OVERRIDE the acting TBA (default = caller's-main) with
/// an arbitrary owned TBA — a name (→ `tokenBoundAccountByName`) or a raw 0x addr.
fn take_tba_flag(args: &[String]) -> Result<(Option<String>, Vec<String>), String> {
    let mut tba: Option<String> = None;
    let mut rest: Vec<String> = Vec::with_capacity(args.len());
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--tba" {
            if tba.is_some() {
                return Err("--tba given more than once".to_string());
            }
            match args.get(i + 1) {
                Some(v) => {
                    tba = Some(v.clone());
                    i += 2;
                }
                None => return Err("usage: --tba <name-or-0xaddr> requires a value".to_string()),
            }
        } else {
            rest.push(args[i].clone());
            i += 1;
        }
    }
    Ok((tba, rest))
}

/// Extract an optional `--data <hex>` flag (from anywhere) and return
/// `(Option<hex>, remaining_positionals)`. A second `--data` is an error.
fn take_data_flag(args: &[String]) -> Result<(Option<String>, Vec<String>), String> {
    let mut data: Option<String> = None;
    let mut rest: Vec<String> = Vec::with_capacity(args.len());
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--data" {
            if data.is_some() {
                return Err("--data given more than once".to_string());
            }
            match args.get(i + 1) {
                Some(h) => {
                    data = Some(h.clone());
                    i += 2;
                }
                None => return Err("usage: --data <hex> requires a value".to_string()),
            }
        } else {
            rest.push(args[i].clone());
            i += 1;
        }
    }
    Ok((data, rest))
}

/// Decode a `--data` hex argument into bytes. Accepts an optional `0x` prefix;
/// rejects odd-length / non-hex with a clear message (never panics). Empty
/// (`""` / `0x`) decodes to no bytes — a value-only call.
fn decode_hex_arg(raw: &str) -> Result<Vec<u8>, String> {
    let s = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")).unwrap_or(raw);
    if s.is_empty() {
        return Ok(Vec::new());
    }
    if s.len() % 2 != 0 {
        return Err(format!("--data has an odd number of hex digits ({} chars)", s.len()));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&s[i..i + 2], 16)
                .map_err(|_| format!("--data is not valid hex near '{}'", &s[i..i + 2]))
        })
        .collect()
}

// ---- guild (GuildFacet: on-chain orgs — members, roles, pooled treasury) -----
//
// Rung 3 of the coordination ladder (bounty → party → GUILD → DAO). A guild is
// an on-chain org with a member roster, per-member roles (member/officer/admin),
// and a pooled `$LH` treasury an admin/officer can spend. Mirrors the
// `registry::*_guild_*` helpers; the same sponsored-write + caller-resolution
// shape as `bounty`. A member arg given as a NAME resolves to its on-chain OWNER
// address (the `send_lh` resolution), or accepts a raw `0x…` address.

const GUILD_USAGE: &str = "\
usage: localharness guild <create|invite|accept|leave|role|fund|spend|members|treasury|mine> ...
  guild create [--as <me>] <name>                       create a guild (you're its admin)
  guild invite [--as <me>] <guildId> <member>           invite a name/0x address to join
  guild accept [--as <me>] <guildId>                    accept an invite (join the guild)
  guild leave  [--as <me>] <guildId>                    leave a guild
  guild role   [--as <me>] <guildId> <member> <member|officer|admin>   set a role (admin)
  guild fund   [--as <me>] <guildId> <amount>           deposit $LH into the treasury
  guild spend  [--as <me>] <guildId> <to> <amount> [memo...]   pay from the treasury (admin/officer)
  guild members  <guildId>                              list members + their roles
  guild treasury <guildId>                               show the treasury balance + wallet
  guild mine   [--as <me>]                               list the guilds you belong to
  member: a subdomain name (resolved to its owner) or a raw 0x address   amount: $LH (e.g. 5 or 0.5)";

/// Parse a guild `id` argument (`#7` or `7`). Pure + testable (mirrors
/// `parse_bounty_id`).
fn parse_guild_id(raw: &str) -> Result<u64, String> {
    raw.trim()
        .trim_start_matches('#')
        .parse::<u64>()
        .map_err(|_| format!("invalid guild id '{raw}'"))
}

/// Resolve a `member` argument to a `0x…` address WITHOUT a key — a raw address
/// is used as-is; a name resolves to its on-chain OWNER (the same split as
/// `send_lh`, so "invite alice" targets whoever owns `alice.localharness.xyz`).
/// Async (the name lookup hits the RPC); pure classification is `classify_recipient`.
async fn resolve_member_address(member: &str) -> Result<String, String> {
    use localharness::encoding::{classify_recipient, Recipient};
    match classify_recipient(member)? {
        Recipient::Address(a) => Ok(a),
        Recipient::Name(n) => match registry::owner_of_name(&n).await {
            Ok(Some(o)) => Ok(o),
            Ok(None) => Err(format!("'{n}' is not registered")),
            Err(e) => Err(format!("RPC error resolving '{n}': {e}")),
        },
    }
}

/// `localharness guild <subcommand>` — the guild router.
async fn guild(caller: Option<&str>, rest: &[String]) -> i32 {
    match rest.first().map(String::as_str) {
        Some("create") => match rest.get(1) {
            Some(name) => guild_create(caller, name).await,
            None => {
                eprintln!("usage: localharness guild create [--as <me>] <name>");
                2
            }
        },
        Some("invite") => match (rest.get(1), rest.get(2)) {
            (Some(id), Some(member)) => guild_invite(caller, id, member).await,
            _ => {
                eprintln!("usage: localharness guild invite [--as <me>] <guildId> <member>");
                2
            }
        },
        Some("accept") => match rest.get(1) {
            Some(id) => guild_accept(caller, id).await,
            None => {
                eprintln!("usage: localharness guild accept [--as <me>] <guildId>");
                2
            }
        },
        Some("leave") => match rest.get(1) {
            Some(id) => guild_leave(caller, id).await,
            None => {
                eprintln!("usage: localharness guild leave [--as <me>] <guildId>");
                2
            }
        },
        Some("role") => match (rest.get(1), rest.get(2), rest.get(3)) {
            (Some(id), Some(member), Some(role)) => guild_role(caller, id, member, role).await,
            _ => {
                eprintln!(
                    "usage: localharness guild role [--as <me>] <guildId> <member> <member|officer|admin>"
                );
                2
            }
        },
        Some("fund") => match (rest.get(1), rest.get(2)) {
            (Some(id), Some(amount)) => guild_fund(caller, id, amount).await,
            _ => {
                eprintln!("usage: localharness guild fund [--as <me>] <guildId> <amount>");
                2
            }
        },
        Some("spend") => {
            if rest.len() < 4 {
                eprintln!(
                    "usage: localharness guild spend [--as <me>] <guildId> <to> <amount> [memo...]"
                );
                return 2;
            }
            let memo = rest[4..].join(" ");
            guild_spend(caller, &rest[1], &rest[2], &rest[3], &memo).await
        }
        Some("members") => match rest.get(1) {
            Some(id) => guild_members(id).await,
            None => {
                eprintln!("usage: localharness guild members <guildId>");
                2
            }
        },
        Some("treasury") => match rest.get(1) {
            Some(id) => guild_treasury(id).await,
            None => {
                eprintln!("usage: localharness guild treasury <guildId>");
                2
            }
        },
        Some("mine") => guild_mine(caller).await,
        _ => {
            eprintln!("{GUILD_USAGE}");
            2
        }
    }
}

/// `guild create <name>` — create an on-chain guild (`createGuild`); the caller
/// becomes its admin. Reads the new guildId back from `guildsOf(creator)`.
async fn guild_create(caller: Option<&str>, name: &str) -> i32 {
    let name = name.trim();
    if name.is_empty() {
        eprintln!("guild create: name is empty");
        return 2;
    }
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    println!("creating guild '{name}' …");
    match registry::create_guild_sponsored(&signer, &sponsor, name, registry::ALPHA_USD_ADDRESS).await
    {
        Ok(tx) => {
            // The new guildId is the last entry in the creator's guildsOf index.
            let addr = addr_to_hex(wallet::address(&signer));
            let id_note = match registry::guilds_of(&addr).await {
                Ok(ids) if !ids.is_empty() => Some(ids[ids.len() - 1]),
                _ => None,
            };
            match id_note {
                Some(id) => {
                    println!("✓ guild #{id} '{name}' created — you're its admin");
                    println!("  invite members:  guild invite {id} <name-or-0x>");
                    println!("  fund it:         guild fund {id} <amount>");
                }
                None => {
                    println!("✓ guild '{name}' created — see it with `guild mine`");
                }
            }
            println!("  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("guild create failed: {e}");
            1
        }
    }
}

/// `guild invite <guildId> <member>` — invite a name/address to the guild
/// (`inviteToGuild`). The invitee then `guild accept`s.
async fn guild_invite(caller: Option<&str>, id_arg: &str, member: &str) -> i32 {
    let guild_id = match parse_guild_id(id_arg) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let member_hex = match resolve_member_address(member).await {
        Ok(a) => a,
        Err(e) => {
            eprintln!("guild invite: {e}");
            return 1;
        }
    };
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    println!("inviting {member_hex} to guild #{guild_id} …");
    match registry::invite_to_guild_sponsored(
        &signer,
        &sponsor,
        guild_id,
        &member_hex,
        registry::ALPHA_USD_ADDRESS,
    )
    .await
    {
        Ok(tx) => {
            println!("✓ invited {member_hex} to guild #{guild_id} — they run `guild accept {guild_id}`  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("guild invite failed: {e}");
            1
        }
    }
}

/// `guild accept <guildId>` — accept a pending invite and join
/// (`acceptGuildInvite`).
async fn guild_accept(caller: Option<&str>, id_arg: &str) -> i32 {
    let guild_id = match parse_guild_id(id_arg) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    println!("accepting the invite to guild #{guild_id} …");
    match registry::accept_guild_invite_sponsored(
        &signer,
        &sponsor,
        guild_id,
        registry::ALPHA_USD_ADDRESS,
    )
    .await
    {
        Ok(tx) => {
            println!("✓ joined guild #{guild_id}  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("guild accept failed: {e}");
            1
        }
    }
}

/// `guild leave <guildId>` — leave a guild (`leaveGuild`).
async fn guild_leave(caller: Option<&str>, id_arg: &str) -> i32 {
    let guild_id = match parse_guild_id(id_arg) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    println!("leaving guild #{guild_id} …");
    match registry::leave_guild_sponsored(&signer, &sponsor, guild_id, registry::ALPHA_USD_ADDRESS).await
    {
        Ok(tx) => {
            println!("✓ left guild #{guild_id}  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("guild leave failed: {e}");
            1
        }
    }
}

/// `guild role <guildId> <member> <member|officer|admin>` — set a member's role
/// (`setRole`). Admin-gated on-chain.
async fn guild_role(caller: Option<&str>, id_arg: &str, member: &str, role_arg: &str) -> i32 {
    let guild_id = match parse_guild_id(id_arg) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let role = match registry::GuildRole::parse(role_arg) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("guild role: {e}");
            return 2;
        }
    };
    let member_hex = match resolve_member_address(member).await {
        Ok(a) => a,
        Err(e) => {
            eprintln!("guild role: {e}");
            return 1;
        }
    };
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    println!("setting {member_hex}'s role in guild #{guild_id} to {} …", role.label());
    match registry::set_role_sponsored(
        &signer,
        &sponsor,
        guild_id,
        &member_hex,
        role.as_u8(),
        registry::ALPHA_USD_ADDRESS,
    )
    .await
    {
        Ok(tx) => {
            println!("✓ {member_hex} is now {} in guild #{guild_id}  tx: {tx}", role.label());
            0
        }
        Err(e) => {
            eprintln!("guild role failed: {e}");
            1
        }
    }
}

/// `guild fund <guildId> <amount>` — deposit `$LH` from the caller's wallet into
/// the guild treasury (approve + fundGuild in one sponsored tx). The `$LH` leaves
/// the caller's balance the moment it mines.
async fn guild_fund(caller: Option<&str>, id_arg: &str, amount: &str) -> i32 {
    let guild_id = match parse_guild_id(id_arg) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let amount_wei = match localharness::encoding::parse_token_amount(amount) {
        Some(w) if w > 0 => w,
        _ => {
            eprintln!("guild fund: invalid amount '{amount}' (expected a positive number of $LH)");
            return 2;
        }
    };
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    println!("funding guild #{guild_id} with {} …", fmt_lh(amount_wei));
    match registry::fund_guild_sponsored(
        &signer,
        &sponsor,
        guild_id,
        amount_wei,
        registry::ALPHA_USD_ADDRESS,
    )
    .await
    {
        Ok(tx) => {
            println!("✓ deposited {} into guild #{guild_id}'s treasury  tx: {tx}", fmt_lh(amount_wei));
            0
        }
        Err(e) => {
            eprintln!("guild fund failed: {e}");
            1
        }
    }
}

/// `guild spend <guildId> <to> <amount> [memo]` — pay `$LH` from the guild
/// treasury to a name/address (`spendTreasury`). Admin/officer-gated on-chain.
async fn guild_spend(caller: Option<&str>, id_arg: &str, to: &str, amount: &str, memo: &str) -> i32 {
    let guild_id = match parse_guild_id(id_arg) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let amount_wei = match localharness::encoding::parse_token_amount(amount) {
        Some(w) if w > 0 => w,
        _ => {
            eprintln!("guild spend: invalid amount '{amount}' (expected a positive number of $LH)");
            return 2;
        }
    };
    let to_hex = match resolve_member_address(to).await {
        Ok(a) => a,
        Err(e) => {
            eprintln!("guild spend: {e}");
            return 1;
        }
    };
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    println!("spending {} from guild #{guild_id} to {to_hex} …", fmt_lh(amount_wei));
    match registry::spend_treasury_sponsored(
        &signer,
        &sponsor,
        guild_id,
        &to_hex,
        amount_wei,
        memo.as_bytes(),
        registry::ALPHA_USD_ADDRESS,
    )
    .await
    {
        Ok(tx) => {
            println!("✓ paid {} from guild #{guild_id} to {to_hex}  tx: {tx}", fmt_lh(amount_wei));
            0
        }
        Err(e) => {
            eprintln!("guild spend failed: {e}");
            1
        }
    }
}

/// `guild members <guildId>` — list a guild's members + their roles
/// (`membersOf` + a `roleOf` per member). Read-only, no `$LH`.
async fn guild_members(id_arg: &str) -> i32 {
    let guild_id = match parse_guild_id(id_arg) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let name = registry::guild_name(guild_id).await.unwrap_or_default();
    let members = match registry::members_of_guild(guild_id).await {
        Ok(m) => m,
        Err(e) => {
            eprintln!("RPC error: {e}");
            return 1;
        }
    };
    let label = if name.is_empty() {
        format!("guild #{guild_id}")
    } else {
        format!("guild #{guild_id} '{name}'")
    };
    if members.is_empty() {
        println!("{label} has no members (or does not exist)");
        return 0;
    }
    println!("{label} — {} member(s):", members.len());
    for m in members {
        let role = registry::role_of_guild(guild_id, &m)
            .await
            .map(|r| r.label().to_string())
            .unwrap_or_else(|_| "?".to_string());
        println!("  {m}  [{role}]");
    }
    0
}

/// `guild treasury <guildId>` — show a guild's pooled `$LH` + its wallet address
/// (`treasuryBalanceOf` + `guildAddress`). Read-only, no `$LH`.
async fn guild_treasury(id_arg: &str) -> i32 {
    let guild_id = match parse_guild_id(id_arg) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let name = registry::guild_name(guild_id).await.unwrap_or_default();
    let balance = match registry::treasury_balance_of(guild_id).await {
        Ok(b) => b,
        Err(e) => {
            eprintln!("RPC error: {e}");
            return 1;
        }
    };
    let wallet_addr = registry::guild_address(guild_id).await.unwrap_or_default();
    let label = if name.is_empty() {
        format!("guild #{guild_id}")
    } else {
        format!("guild #{guild_id} '{name}'")
    };
    println!("{label}");
    println!("  treasury  {}", fmt_lh(balance));
    println!("  wallet    {wallet_addr}");
    0
}

/// `guild mine [--as <me>]` — list the guilds the caller belongs to
/// (`guildsOf` + a `guildName`/`roleOf` per id). Read-only, no `$LH`.
async fn guild_mine(caller: Option<&str>) -> i32 {
    let (key_file, key_hex) = match resolve_caller_key(caller) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let signer = match wallet::from_private_key_hex(&key_hex) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bad key in {key_file}: {e}");
            return 1;
        }
    };
    let addr = addr_to_hex(wallet::address(&signer));
    let ids = match registry::guilds_of(&addr).await {
        Ok(ids) => ids,
        Err(e) => {
            eprintln!("RPC error: {e}");
            return 1;
        }
    };
    if ids.is_empty() {
        println!("{addr} belongs to no guilds — create one with `guild create <name>`");
        return 0;
    }
    println!("{addr} belongs to {} guild(s):", ids.len());
    for id in ids {
        let name = registry::guild_name(id).await.unwrap_or_default();
        let role = registry::role_of_guild(id, &addr)
            .await
            .map(|r| r.label().to_string())
            .unwrap_or_else(|_| "?".to_string());
        let balance = registry::treasury_balance_of(id).await.unwrap_or(0);
        let name_part = if name.is_empty() { String::new() } else { format!(" '{name}'") };
        println!("  #{id}{name_part}  [you: {role}]  treasury {}", fmt_lh(balance));
    }
    0
}

// ---- vote (VotingFacet: DAO governance — Rung 4 of the coordination ladder) --
//
// A guild MEMBER proposes a treasury spend, members VOTE one-member-one-vote,
// and a passed measure EXECUTES from the guild's pooled treasury (the same
// `LibGuildStorage` ledger `guild spend` debits, gated on a vote not the Admin
// role). Mirrors the `registry::*_proposal_*` / `propose`/`vote`/`execute`
// helpers; the same sponsored-write + caller-resolution shape as `guild`/`bounty`.
// A `to` arg given as a NAME resolves to its on-chain OWNER address (the
// `resolve_member_address` split), or accepts a raw `0x…` address.

const VOTE_USAGE: &str = "\
usage: localharness vote <propose|cast|execute|list|show> ...
  vote propose [--as <me>] <guildId> <to> <amount> [--period <dur>] [memo...]
                                       a member proposes a treasury spend (opens a vote)
  vote cast    [--as <me>] <proposalId> <for|against>   cast your one-member-one-vote ballot
  vote execute [--as <me>] <proposalId>                 resolve a closed proposal (spends if passed)
  vote list    <guildId>                                list a guild's proposals + tally
  vote show    <proposalId>                             full proposal detail + tally + passing
  to: a subdomain name (resolved to its owner) or a raw 0x address   amount: $LH (e.g. 5 or 0.5)
  dur: 1h / 7d / 30d   (1h … 30d, default 7d)";

/// VotingFacet's `MAX_VOTING_PERIOD` (`LibVotingStorage`): 30 days. `parse_ttl`
/// already enforces the shared 1h minimum (== `MIN_VOTING_PERIOD`), but its
/// upper bound is the invite 90d; clamp here so an out-of-range period fails
/// client-side with a clear message instead of an on-chain `BadVotingPeriod`.
const VOTE_MAX_PERIOD_SECS: u64 = 30 * 24 * 3600;

/// How many of a guild's proposals `vote list` scans from the head. A sane page
/// bound mirroring `BOUNTY_LIST_SCAN`; bump when a cursor walk is worth it.
const VOTE_LIST_SCAN: u64 = 100;

/// Parse a proposal `id` argument (`#7` or `7`). Pure + testable (mirrors
/// `parse_bounty_id` / `parse_guild_id`).
fn parse_proposal_id(raw: &str) -> Result<u64, String> {
    raw.trim()
        .trim_start_matches('#')
        .parse::<u64>()
        .map_err(|_| format!("invalid proposal id '{raw}'"))
}

/// Parse a `for`/`against` (or `yes`/`no`) ballot argument to the on-chain
/// `support` bool. Pure + testable; case-insensitive.
fn parse_vote_support(raw: &str) -> Result<bool, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "for" | "yes" | "y" | "aye" | "support" => Ok(true),
        "against" | "no" | "n" | "nay" | "oppose" => Ok(false),
        other => Err(format!("ballot must be 'for' or 'against', got '{other}'")),
    }
}

/// Parse a voting `--period <dur>` to seconds, bounded to VotingFacet's
/// [MIN_VOTING_PERIOD, MAX_VOTING_PERIOD] = 1h…30d. Reuses `parse_ttl` (shared
/// 1h minimum) then clamps the upper bound to 30d (`parse_ttl`'s ceiling is the
/// invite 90d, which the facet would reject). Pure + testable.
fn parse_voting_period(raw: &str) -> Result<u64, String> {
    let secs = parse_ttl(raw)?;
    if secs > VOTE_MAX_PERIOD_SECS {
        return Err(format!("voting period '{raw}' exceeds the 30d maximum"));
    }
    Ok(secs)
}

/// Parsed `vote propose` arguments. `to`/`amount` are required positionals; the
/// memo is the joined positional remainder (so an unquoted multi-word memo works,
/// matching `guild spend`/`bounty post`).
struct ParsedVotePropose {
    guild_id: u64,
    to: String,
    amount_wei: u128,
    period_secs: u64,
    memo: String,
}

fn parse_vote_propose_args(rest: &[String]) -> Result<ParsedVotePropose, String> {
    let mut positional: Vec<String> = Vec::new();
    let mut period: Option<String> = None;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--period" => {
                period = Some(rest.get(i + 1).ok_or(VOTE_USAGE)?.clone());
                i += 2;
            }
            _ => {
                positional.push(rest[i].clone());
                i += 1;
            }
        }
    }
    if positional.len() < 3 {
        return Err(format!("vote propose needs <guildId> <to> <amount>\n{VOTE_USAGE}"));
    }
    let guild_id = parse_guild_id(&positional[0])?;
    let to = positional[1].clone();
    let amount_label = &positional[2];
    let amount_wei = match localharness::encoding::parse_token_amount(amount_label) {
        Some(w) if w > 0 => w,
        _ => return Err(format!("amount must be a positive $LH amount, got '{amount_label}'")),
    };
    let period_secs = match period {
        None => INVITE_DEFAULT_TTL_SECS, // 7d, within 1h…30d
        Some(raw) => parse_voting_period(&raw)?,
    };
    let memo = positional[3..].join(" ");
    Ok(ParsedVotePropose { guild_id, to, amount_wei, period_secs, memo })
}

/// `localharness vote <subcommand>` — the DAO-governance router (alias `gov`).
async fn vote(caller: Option<&str>, rest: &[String]) -> i32 {
    match rest.first().map(String::as_str) {
        Some("propose") => vote_propose(caller, &rest[1..]).await,
        Some("cast") => match (rest.get(1), rest.get(2)) {
            (Some(id), Some(ballot)) => vote_cast(caller, id, ballot).await,
            _ => {
                eprintln!("usage: localharness vote cast [--as <me>] <proposalId> <for|against>");
                2
            }
        },
        Some("execute") => match rest.get(1) {
            Some(id) => vote_execute(caller, id).await,
            None => {
                eprintln!("usage: localharness vote execute [--as <me>] <proposalId>");
                2
            }
        },
        Some("list") => match rest.get(1) {
            Some(id) => vote_list(id).await,
            None => {
                eprintln!("usage: localharness vote list <guildId>");
                2
            }
        },
        Some("show") => match rest.get(1) {
            Some(id) => vote_show(id).await,
            None => {
                eprintln!("usage: localharness vote show <proposalId>");
                2
            }
        },
        _ => {
            eprintln!("{VOTE_USAGE}");
            2
        }
    }
}

/// `vote propose <guildId> <to> <amount> [--period <dur>] [memo]` — a guild
/// member opens a treasury-spend proposal (`propose`). No escrow: the spend is
/// debited from the guild treasury at `execute` time if it passes. Reads the new
/// proposalId back from `proposalsOf(guildId, …)` (its last entry).
async fn vote_propose(caller: Option<&str>, rest: &[String]) -> i32 {
    let ParsedVotePropose { guild_id, to, amount_wei, period_secs, memo } =
        match parse_vote_propose_args(rest) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("{e}");
                return 2;
            }
        };
    let to_hex = match resolve_member_address(&to).await {
        Ok(a) => a,
        Err(e) => {
            eprintln!("vote propose: {e}");
            return 1;
        }
    };
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    println!(
        "proposing to spend {} from guild #{guild_id} to {to_hex} (votes for {}) …",
        fmt_lh(amount_wei),
        fmt_ttl(period_secs)
    );
    match registry::propose_sponsored(
        &signer,
        &sponsor,
        guild_id,
        &to_hex,
        amount_wei,
        memo.as_bytes(),
        period_secs,
        registry::ALPHA_USD_ADDRESS,
    )
    .await
    {
        Ok(tx) => {
            // The new proposalId is the last entry in the guild's proposal list.
            let id_note = match registry::proposals_of(guild_id, 0, VOTE_LIST_SCAN).await {
                Ok(ids) if !ids.is_empty() => Some(ids[ids.len() - 1]),
                _ => None,
            };
            match id_note {
                Some(id) => {
                    println!("✓ proposal #{id} opened — voting closes in {}", fmt_ttl(period_secs));
                    println!("  members vote:  vote cast {id} <for|against>");
                    println!("  after it closes, anyone runs:  vote execute {id}");
                }
                None => {
                    println!("✓ proposal opened — see it with `vote list {guild_id}`");
                }
            }
            println!("  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("vote propose failed: {e}");
            1
        }
    }
}

/// `vote cast <proposalId> <for|against>` — cast your one-member-one-vote ballot
/// (`vote(proposalId, support)`). Caller must be a member of the proposal's guild
/// and not have voted already (enforced on-chain).
async fn vote_cast(caller: Option<&str>, id_arg: &str, ballot: &str) -> i32 {
    let proposal_id = match parse_proposal_id(id_arg) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let support = match parse_vote_support(ballot) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("vote cast: {e}");
            return 2;
        }
    };
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    let side = if support { "for" } else { "against" };
    println!("casting a '{side}' vote on proposal #{proposal_id} …");
    match registry::vote_sponsored(&signer, &sponsor, proposal_id, support, registry::ALPHA_USD_ADDRESS)
        .await
    {
        Ok(tx) => {
            println!("✓ voted {side} on proposal #{proposal_id}  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("vote cast failed: {e}");
            1
        }
    }
}

/// `vote execute <proposalId>` — resolve a closed proposal (`execute`).
/// PERMISSIONLESS: spends the treasury to the recipient if it passed, else fails
/// with no spend. Idempotent (a second execute reverts).
async fn vote_execute(caller: Option<&str>, id_arg: &str) -> i32 {
    let proposal_id = match parse_proposal_id(id_arg) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    println!("executing proposal #{proposal_id} …");
    match registry::execute_proposal_sponsored(
        &signer,
        &sponsor,
        proposal_id,
        registry::ALPHA_USD_ADDRESS,
    )
    .await
    {
        Ok(tx) => {
            // Read the resolved status back so the user sees passed-vs-failed.
            let outcome = match registry::get_proposal(proposal_id).await {
                Ok(p) => match p.status {
                    3 => " — PASSED, treasury spent".to_string(),
                    2 => " — FAILED, no spend".to_string(),
                    _ => String::new(),
                },
                Err(_) => String::new(),
            };
            println!("✓ proposal #{proposal_id} resolved{outcome}  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("vote execute failed: {e}");
            1
        }
    }
}

/// Render one proposal row for `vote list`. Pure (no I/O) so the layout is
/// unit-testable: id, status, for/against/quorum tally, deadline (relative),
/// passing flag, memo snippet.
fn format_proposal_row(id: u64, p: &registry::Proposal, t: &registry::Tally, memo: &str, now: u64) -> String {
    let when = if p.deadline == 0 {
        "—".to_string()
    } else if p.deadline <= now {
        "CLOSED".to_string()
    } else {
        format!("in {}", fmt_interval(p.deadline - now))
    };
    let snippet: String = memo.replace('\n', " ").chars().take(60).collect();
    format!(
        "  #{id}  [{status}]  for {f} / against {a}  quorum {q}  closes {when}  {passing}\n      {snippet}",
        status = p.status_label(),
        f = t.for_votes,
        a = t.against_votes,
        q = t.quorum,
        passing = if t.passing { "(passing)" } else { "(not passing)" },
    )
}

/// `vote list <guildId>` — list a guild's proposals + their live tally
/// (`proposalsOf` + a `getProposal`/`tallyOf` per id). Read-only, no `$LH`.
async fn vote_list(id_arg: &str) -> i32 {
    let guild_id = match parse_guild_id(id_arg) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let ids = match registry::proposals_of(guild_id, 0, VOTE_LIST_SCAN).await {
        Ok(ids) => ids,
        Err(e) => {
            eprintln!("vote list failed: {e}");
            return 1;
        }
    };
    let name = registry::guild_name(guild_id).await.unwrap_or_default();
    let label = if name.is_empty() {
        format!("guild #{guild_id}")
    } else {
        format!("guild #{guild_id} '{name}'")
    };
    if ids.is_empty() {
        println!("{label} has no proposals — open one with `vote propose {guild_id} <to> <amount>`");
        return 0;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("{label} — {} proposal(s):", ids.len());
    for id in ids {
        let p = match registry::get_proposal(id).await {
            Ok(p) => p,
            Err(e) => {
                println!("  #{id}  (could not read: {e})");
                continue;
            }
        };
        let t = registry::tally_of(id).await.unwrap_or(registry::Tally {
            for_votes: 0,
            against_votes: 0,
            quorum: 0,
            votes_cast: 0,
            passing: false,
        });
        let memo = registry::proposal_memo_of(id).await.unwrap_or_default();
        println!("{}", format_proposal_row(id, &p, &t, &memo, now));
    }
    0
}

/// `vote show <proposalId>` — full proposal detail + tally + whether it WOULD
/// pass right now (`getProposal` + `tallyOf` + `proposalMemoOf`). Read-only.
async fn vote_show(id_arg: &str) -> i32 {
    let proposal_id = match parse_proposal_id(id_arg) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let p = match registry::get_proposal(proposal_id).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("vote show: {e}");
            return 1;
        }
    };
    let t = registry::tally_of(proposal_id).await.ok();
    let memo = registry::proposal_memo_of(proposal_id).await.unwrap_or_default();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let when = if p.deadline == 0 {
        "—".to_string()
    } else if p.deadline <= now {
        "CLOSED (ready to execute)".to_string()
    } else {
        format!("in {}", fmt_interval(p.deadline - now))
    };
    println!("proposal #{proposal_id}  [{}]", p.status_label());
    println!("  guild     #{}", p.guild_id);
    println!("  proposer  {}", p.proposer);
    println!("  spend     {} -> {}", fmt_lh(p.amount), p.to);
    println!("  closes    {when}");
    match t {
        Some(t) => {
            println!(
                "  tally     for {} / against {}   quorum {}  cast {}  {}",
                t.for_votes,
                t.against_votes,
                t.quorum,
                t.votes_cast,
                if t.passing { "(passing)" } else { "(not passing)" }
            );
        }
        None => println!("  tally     for {} / against {}", p.for_votes, p.against_votes),
    }
    if !memo.is_empty() {
        println!("  memo      {memo}");
    }
    0
}

/// Render one job row for the `jobs` listing. Pure (no I/O) so the layout is
/// unit-testable: id, target name, cadence, next run, budget remaining, runs
/// left, status.
fn format_job_row(id: u64, target: &str, job: &registry::ScheduledJob, task: &str, now: u64) -> String {
    let next = if job.next_run == 0 {
        "—".to_string()
    } else if job.next_run <= now {
        "due now".to_string()
    } else {
        format!("in {}", fmt_interval(job.next_run - now))
    };
    let snippet: String = task.replace('\n', " ").chars().take(60).collect();
    format!(
        "  #{id}  {target}  every {interval}  next {next}  budget {budget}  runs-left {runs}  [{status}]\n      {snippet}",
        interval = fmt_interval(job.interval),
        budget = fmt_lh(job.budget_wei),
        runs = job.runs_left,
        status = job.status_label(),
    )
}

/// `localharness jobs [--as <me>]` — list the caller's scheduled jobs
/// (`jobsOf` + a `getJob`/`taskOf` per id). Read-only, no `$LH`.
async fn list_jobs(caller_name: Option<&str>) -> i32 {
    let (key_file, key_hex) = match resolve_caller_key(caller_name) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let signer = match wallet::from_private_key_hex(&key_hex) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bad key in {key_file}: {e}");
            return 1;
        }
    };
    let addr = addr_to_hex(wallet::address(&signer));
    let ids = match registry::jobs_of(&addr).await {
        Ok(ids) => ids,
        Err(e) => {
            eprintln!("RPC error: {e}");
            return 1;
        }
    };
    if ids.is_empty() {
        println!("no scheduled jobs for {addr}");
        return 0;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("{} scheduled job(s) for {addr}:", ids.len());
    for id in ids {
        let job = match registry::get_job(id).await {
            Ok(j) => j,
            Err(e) => {
                println!("  #{id}  (could not read: {e})");
                continue;
            }
        };
        // Resolve the target's name for readability; fall back to the id.
        let target = registry::name_of_id(job.target_id)
            .await
            .ok()
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| format!("token#{}", job.target_id));
        let task = registry::task_of(id).await.unwrap_or_default();
        println!("{}", format_job_row(id, &target, &job, &task, now));
    }
    0
}

/// `localharness unschedule [--as <me>] <jobId>` — cancel a scheduled job;
/// the facet refunds the remaining escrowed `$LH` to the owner.
async fn unschedule(caller_name: Option<&str>, job_id_arg: &str) -> i32 {
    let job_id: u64 = match job_id_arg.trim().trim_start_matches('#').parse() {
        Ok(n) => n,
        Err(_) => {
            eprintln!("unschedule: '{job_id_arg}' is not a job id (a number, e.g. 3)");
            return 2;
        }
    };
    let (key_file, key_hex) = match resolve_caller_key(caller_name) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let signer = match wallet::from_private_key_hex(&key_hex) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bad key in {key_file}: {e}");
            return 1;
        }
    };
    let sponsor = match wallet::from_private_key_hex(SPONSOR_KEY) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sponsor key error: {e}");
            return 1;
        }
    };
    match registry::cancel_job_sponsored(&signer, &sponsor, job_id, registry::ALPHA_USD_ADDRESS).await
    {
        Ok(tx) => {
            println!("✓ cancelled job #{job_id} — remaining budget refunded to your wallet");
            println!("  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("unschedule failed: {e}");
            1
        }
    }
}

async fn topup(caller_name: Option<&str>) -> i32 {
    let (key_file, key_hex) = match resolve_caller_key(caller_name) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let signer = match wallet::from_private_key_hex(&key_hex) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bad key in {key_file}: {e}");
            return 1;
        }
    };
    let sponsor = match wallet::from_private_key_hex(SPONSOR_KEY) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sponsor key error: {e}");
            return 1;
        }
    };
    let addr = addr_to_hex(wallet::address(&signer));
    // 1. Claim the daily allowance (mints $LH) if eligible. The allowance is
    //    DISABLED on-chain (dailyAllowance=0 — a sybil risk), so this is a
    //    no-op in practice; the dormant path stays in case it's re-enabled.
    if registry::can_claim_credits(&addr).await.unwrap_or(false) {
        match registry::claim_daily_sponsored(&signer, &sponsor, registry::ALPHA_USD_ADDRESS).await {
            Ok(tx) => println!("claimed daily $LH  tx: {tx}"),
            Err(e) => eprintln!("claim failed (continuing to deposit): {e}"),
        }
    }
    // 2. Deposit the wallet balance into the per-request meter.
    let bal = registry::token_balance_of(&addr).await.unwrap_or(0);
    if bal == 0 {
        println!("wallet has 0 $LH — nothing to deposit.");
        println!("fund it first: `localharness redeem <code>`, or have another agent `send` you $LH.");
        return 0;
    }
    match registry::deposit_credits_sponsored(&signer, &sponsor, bal, registry::ALPHA_USD_ADDRESS)
        .await
    {
        Ok(tx) => {
            println!("deposited {} into the meter  tx: {tx}", fmt_lh(bal));
            0
        }
        Err(e) => {
            eprintln!("deposit failed: {e}");
            1
        }
    }
}

/// Deterministic, network-free QA checks the `probe` runs against the platform.
/// Each pushes a failure description on an UNEXPECTED result (a real bug); an
/// empty result means every invariant held. Pure + testable — the core of the
/// autonomous loop's read-only observe pass (roadmap Track B / Phase 2).
fn run_qa_checks() -> Vec<String> {
    let mut fails = Vec::new();
    // 1. A known-good cartridge compiles AND exposes an entry point.
    let good = "fn frame(t: i32) { host::display::clear(0); host::display::present(); }";
    match localharness::rustlite::compile(good) {
        Ok(wasm) if !cartridge_has_entry(&wasm) => {
            fails.push("a valid frame() cartridge compiled but has no frame/render export".into())
        }
        Ok(_) => {}
        Err(e) => fails.push(format!("a known-good cartridge failed to compile: {e}")),
    }
    // 2. Garbage source is rejected, not silently accepted.
    if localharness::rustlite::compile("this is not rustlite").is_ok() {
        fails.push("the compiler ACCEPTED non-rustlite garbage (should error)".into());
    }
    // 3. An entry-less cartridge is detectable (it would render a blank face).
    if let Ok(wasm) = localharness::rustlite::compile("fn helper(n: i32) -> i32 { n + 1 }") {
        if cartridge_has_entry(&wasm) {
            fails.push("an entry-less cartridge wrongly reports a frame/render export".into());
        }
    }
    fails
}

/// Agent-driven probe (`probe --deep`) — roadmap Track B at autonomy=observe.
/// An LLM agent with ONE read-only tool (qa_compile) under a deny-by-default
/// policy (0b enforcement) probes the rustlite compiler via the credit proxy
/// and files concrete findings on-chain. Needs a live run (proxy + Gemini).
async fn probe_agent(caller_name: Option<&str>) -> i32 {
    let (key_file, key_hex) = match resolve_caller_key(caller_name) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let caller = match wallet::from_private_key_hex(&key_hex) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bad key in {key_file}: {e}");
            return 1;
        }
    };
    // Pay PER REQUEST (fund the meter), not a 10-$LH hour-long session.
    if let Ok(sponsor) = wallet::from_private_key_hex(SPONSOR_KEY) {
        let addr = addr_to_hex(wallet::address(&caller));
        if registry::credit_balance_of(&addr).await.unwrap_or(0) < CALL_COST_WEI {
            let _ = registry::deposit_credits_sponsored(
                &caller,
                &sponsor,
                CALL_METER_TOPUP_WEI,
                registry::ALPHA_USD_ADDRESS,
            )
            .await;
        }
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let token = registry::proxy_auth_token(&caller, now);
    let base = match url::Url::parse(registry::CREDIT_PROXY_URL) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("internal: bad proxy url: {e}");
            return 1;
        }
    };

    // The ONE read-only tool: compile a source, report the result. No writes,
    // no secrets, no network — autonomy=observe.
    let qa_compile = localharness::ClosureTool::new(
        "qa_compile",
        "Compile rustlite source; report ok + wasm byte size + whether it exposes a \
         frame/render entry, OR the compile error. Probe with valid and invalid sources.",
        serde_json::json!({
            "type": "object",
            "properties": { "source": { "type": "string", "description": "rustlite source to compile" } },
            "required": ["source"]
        }),
        |args: serde_json::Value, _ctx| async move {
            let src = args.get("source").and_then(|v| v.as_str()).unwrap_or("");
            eprintln!("  probing: compiling {} bytes …", src.len());
            Ok(match localharness::rustlite::compile(src) {
                Ok(wasm) => serde_json::json!({
                    "ok": true, "wasm_bytes": wasm.len(), "has_entry": cartridge_has_entry(&wasm)
                }),
                Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
            })
        },
    );

    // Only the custom tool — no builtins. The agent tools then answers in prose
    // (no `finish` to short-circuit the text report). deny-by-default + allow
    // only qa_compile is 0b's "custom tools require a policy", at dispatch.
    let caps = localharness::types::CapabilitiesConfig {
        enabled_tools: Some(vec![]),
        enable_subagents: false,
        ..Default::default()
    };
    let policies = vec![localharness::deny_all(), localharness::Policy::allow("qa_compile")];

    let cfg = localharness::GeminiAgentConfig::new(token)
        .with_base_url(base)
        .with_system_instructions(
            "You are qa-observe, a READ-ONLY QA agent for localharness. Use qa_compile to \
             probe the rustlite compiler, then ANSWER IN TEXT with your findings: a short \
             numbered list of concrete issues you actually observed, or exactly 'no issues \
             found'. Be terse.",
        )
        .with_capabilities(caps)
        .with_policies(policies)
        .with_tool(qa_compile);

    println!("running observe-agent probe (live, via proxy) …");
    let agent = match localharness::Agent::start_gemini(cfg).await {
        Ok(a) => a,
        Err(e) => {
            eprintln!("could not start agent: {e}");
            return 1;
        }
    };
    // Drive the conversation until the agent answers in text. The first turn is
    // usually the qa_compile tool call (no prose); after the dispatcher feeds
    // the result back, a follow-up turn yields the findings. (The browser does
    // this via run_send's auto-continue; the CLI loops chat() — history persists
    // across calls on the same Agent.)
    let mut findings = String::new();
    let mut nudge = "Probe the rustlite compiler: try a valid `fn frame(t: i32)` cartridge that \
                     draws, an obviously invalid source, and one edge case via qa_compile."
        .to_string();
    for _ in 0..5 {
        match agent.chat(nudge.as_str()).await {
            Ok(r) => {
                let t = r.text().await.unwrap_or_default();
                if !t.trim().is_empty() {
                    findings = t;
                    break;
                }
            }
            Err(e) => {
                let _ = agent.shutdown().await;
                eprintln!("agent run failed: {e}");
                return 1;
            }
        }
        nudge = "Based on the qa_compile results so far, state your concrete findings now as a \
                 short numbered list in text, or exactly 'no issues found'."
            .to_string();
    }
    let _ = agent.shutdown().await;
    println!("--- agent findings ---\n{}", findings.trim());

    if findings.to_lowercase().contains("no issues") || findings.trim().is_empty() {
        println!("(agent reported no issues — nothing filed)");
        return 0;
    }
    let mut env = format!(
        "qa/v1 source=qa-observe v{}: {}",
        env!("CARGO_PKG_VERSION"),
        findings.replace('\n', " ")
    );
    if env.len() > 2048 {
        let mut cut = 2048;
        while cut > 0 && !env.is_char_boundary(cut) {
            cut -= 1;
        }
        env.truncate(cut);
    }
    let _ = feedback_submit(caller_name, &env).await;
    0
}

/// `localharness probe [--as <fleet>]` — the autonomous loop's read-only
/// observe pass. Runs deterministic QA checks against the platform plus one
/// live chain read; on any failure it REPORTS on-chain as a `qa/v1` feedback
/// envelope (no human bridge — the agent files its own bug). One-shot and
/// synchronous (no daemon). The checks are deterministic; network is touched
/// only for the chain read and the feedback submit (no `$LH` for the read).
async fn probe(caller_name: Option<&str>) -> i32 {
    let mut fails = run_qa_checks();
    // A live, read-only chain check: a known name must still resolve.
    match registry::owner_of_name("claude").await {
        Ok(Some(_)) => {}
        Ok(None) => fails.push("registry reports claude.localharness.xyz unregistered".into()),
        Err(e) => fails.push(format!("chain read failed: {e}")),
    }

    if fails.is_empty() {
        println!("✓ probe: all platform checks passed");
        return 0;
    }
    eprintln!("probe found {} issue(s):", fails.len());
    for f in &fails {
        eprintln!("  - {f}");
    }
    // Report on-chain as the fleet identity (best-effort). The qa/v1 envelope
    // marks fleet-authored feedback so a future triage pass can filter it.
    let mut envelope = format!(
        "qa/v1 source=qa-probe v{}: {}",
        env!("CARGO_PKG_VERSION"),
        fails.join(" | ")
    );
    if envelope.len() > 2048 {
        let mut cut = 2048;
        while cut > 0 && !envelope.is_char_boundary(cut) {
            cut -= 1;
        }
        envelope.truncate(cut);
    }
    if feedback_submit(caller_name, &envelope).await == 0 {
        eprintln!("  → reported on-chain");
    }
    1
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Parse a `0x`-optional 20-byte hex address into bytes.
fn parse_addr20(s: &str) -> Option<[u8; 20]> {
    let t = s.trim().trim_start_matches("0x").trim_start_matches("0X");
    if t.len() != 40 {
        return None;
    }
    let mut out = [0u8; 20];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(t.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// Registry name rule = a valid DNS label: 1-63 chars, lowercase a-z / 0-9 /
/// hyphen, and NO leading/trailing hyphen (RFC 1035 — a label like `-foo` or
/// `foo-` is a dead-on-arrival subdomain). Surfaced by the test-user fleet
/// (juno-qa) — emoji/oversized were already caught, the hyphen edge was not.
fn name_is_valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn gitignore_already_covers_detects_wildcard_and_exact() {
        // wildcard covers any key file
        assert!(gitignore_already_covers("target/\n*.localharness.key\n", "alice.localharness.key"));
        // exact filename covers itself
        assert!(gitignore_already_covers("alice.localharness.key\n", "alice.localharness.key"));
        // tolerant of surrounding whitespace
        assert!(gitignore_already_covers("  *.localharness.key  \n", "x.localharness.key"));
        // not covered → needs appending
        assert!(!gitignore_already_covers("target/\nnode_modules/\n", "alice.localharness.key"));
        // a different exact key does NOT count as covering this one
        assert!(!gitignore_already_covers("bob.localharness.key\n", "alice.localharness.key"));
        // empty gitignore → not covered
        assert!(!gitignore_already_covers("", "alice.localharness.key"));
    }

    #[test]
    fn pick_key_read_path_prefers_cwd_then_home() {
        let cwd = "alice.localharness.key".to_string();
        let home = Some("/home/me/.localharness/keys/alice.localharness.key".to_string());

        // cwd exists → cwd wins (back-compat: pre-existing local keys keep working)
        assert_eq!(
            pick_key_read_path(cwd.clone(), true, home.clone(), true),
            Some(cwd.clone())
        );
        // cwd exists even when home doesn't → still cwd
        assert_eq!(
            pick_key_read_path(cwd.clone(), true, None, false),
            Some(cwd.clone())
        );
        // cwd absent, home present → home (the new safe default location)
        assert_eq!(
            pick_key_read_path(cwd.clone(), false, home.clone(), true),
            home.clone()
        );
        // neither exists → None (caller surfaces "run create first")
        assert_eq!(pick_key_read_path(cwd.clone(), false, home.clone(), false), None);
        // no home dir resolvable and no cwd key → None
        assert_eq!(pick_key_read_path(cwd, false, None, false), None);
    }

    #[test]
    fn key_is_in_cwd_distinguishes_bare_from_pathful() {
        // a bare cwd filename → in cwd (needs .gitignore protection)
        assert!(key_is_in_cwd("alice.localharness.key"));
        // a config-home (absolute) path → NOT in cwd (no project .gitignore)
        assert!(!key_is_in_cwd("/home/me/.localharness/keys/alice.localharness.key"));
        assert!(!key_is_in_cwd("C:\\Users\\me\\.localharness\\keys\\a.localharness.key"));
    }

    #[test]
    fn key_home_dir_from_honors_override_and_falls_back() {
        use std::path::PathBuf;
        // $LOCALHARNESS_HOME wins outright when set.
        assert_eq!(
            key_home_dir_from(Some("/custom/keys"), Some("/u/prof"), Some("/u/home")),
            Some(PathBuf::from("/custom/keys"))
        );
        // No override → USERPROFILE (Windows), with the .localharness/keys suffix.
        assert_eq!(
            key_home_dir_from(None, Some("/u/prof"), None),
            Some(PathBuf::from("/u/prof").join(".localharness").join("keys"))
        );
        // No override, no USERPROFILE → HOME (Unix).
        assert_eq!(
            key_home_dir_from(None, None, Some("/u/home")),
            Some(PathBuf::from("/u/home").join(".localharness").join("keys"))
        );
        // Empty strings are treated as unset.
        assert_eq!(
            key_home_dir_from(Some(""), Some(""), Some("/u/home")),
            Some(PathBuf::from("/u/home").join(".localharness").join("keys"))
        );
        // Nothing set → None (caller falls back to the cwd).
        assert_eq!(key_home_dir_from(None, None, None), None);
    }

    #[test]
    fn parse_create_args_name_only_and_with_persona() {
        let (n, p) = parse_create_args(&args(&["alice"])).unwrap();
        assert_eq!(n, "alice");
        assert_eq!(p, None);

        let (n, p) = parse_create_args(&args(&["alice", "--persona", "you", "are", "alice"]))
            .unwrap();
        assert_eq!(n, "alice");
        assert_eq!(p.as_deref(), Some("you are alice"));
    }

    #[test]
    fn parse_create_args_rejects_bad_forms() {
        assert!(parse_create_args(&args(&[])).is_err()); // no name
        assert!(parse_create_args(&args(&["alice", "--persona"])).is_err()); // empty persona
        assert!(parse_create_args(&args(&["alice", "bob"])).is_err()); // stray positional
    }

    #[test]
    fn parse_call_plain_target_and_message() {
        let p = parse_call_args(&args(&["alice", "how", "are", "you"])).unwrap();
        assert_eq!(p.caller, None);
        assert_eq!(p.target, "alice");
        assert_eq!(p.message, "how are you");
    }

    #[test]
    fn parse_call_single_word_message() {
        let p = parse_call_args(&args(&["alice", "hello"])).unwrap();
        assert_eq!(p.caller, None);
        assert_eq!(p.target, "alice");
        assert_eq!(p.message, "hello");
    }

    #[test]
    fn parse_call_with_as_flag() {
        let p = parse_call_args(&args(&["--as", "bob", "alice", "what's", "up"])).unwrap();
        assert_eq!(p.caller.as_deref(), Some("bob"));
        assert!(!p.fresh);
        assert_eq!(p.target, "alice");
        assert_eq!(p.message, "what's up");
    }

    #[test]
    fn parse_call_fresh_flag() {
        let p = parse_call_args(&args(&["--fresh", "alice", "hi"])).unwrap();
        assert!(p.fresh);
        assert_eq!(p.caller, None);
        assert_eq!(p.target, "alice");
        assert_eq!(p.message, "hi");
    }

    #[test]
    fn parse_call_flags_order_independent() {
        let a = parse_call_args(&args(&["--as", "bob", "--fresh", "alice", "hi"])).unwrap();
        let b = parse_call_args(&args(&["--fresh", "--as", "bob", "alice", "hi"])).unwrap();
        for p in [a, b] {
            assert_eq!(p.caller.as_deref(), Some("bob"));
            assert!(p.fresh);
            assert_eq!(p.target, "alice");
            assert_eq!(p.message, "hi");
        }
    }

    #[test]
    fn parse_call_accepts_model_flag_in_any_order() {
        // `--as`/`--model`/`--fresh` may appear in any order before the target.
        let perms = [
            vec!["--model", "claude-opus", "--as", "bob", "--fresh", "alice", "hi"],
            vec!["--fresh", "--model", "claude-opus", "--as", "bob", "alice", "hi"],
            vec!["--as", "bob", "--model", "claude-opus", "--fresh", "alice", "hi"],
        ];
        for parts in perms {
            let p = parse_call_args(&args(&parts)).unwrap();
            assert_eq!(p.caller.as_deref(), Some("bob"));
            assert_eq!(p.model.as_deref(), Some("claude-opus"));
            assert!(p.fresh);
            assert_eq!(p.target, "alice");
            assert_eq!(p.message, "hi");
        }
        // `--model` requires a value.
        assert!(parse_call_args(&args(&["--model"])).is_err());
    }

    #[test]
    fn parse_call_defaults_to_not_fresh() {
        let p = parse_call_args(&args(&["alice", "hi"])).unwrap();
        assert!(!p.fresh);
    }

    #[test]
    fn parse_call_message_preserves_internal_spacing_as_single_spaces() {
        // join(" ") normalises arg boundaries to single spaces — documents the
        // contract so a caller relying on exact whitespace isn't surprised.
        let p = parse_call_args(&args(&["alice", "a", "b", "c"])).unwrap();
        assert_eq!(p.message, "a b c");
    }

    #[test]
    fn parse_call_rejects_missing_message() {
        assert!(parse_call_args(&args(&["alice"])).is_err());
    }

    // ---- mcp-call (the hosted MCP-over-HTTP + x402 client) ----------------

    #[test]
    fn parse_mcp_call_defaults_and_flags() {
        // Plain target + message: caller None, default pay.
        let p = parse_mcp_call_args(&args(&["claude", "hi", "there"])).unwrap();
        assert_eq!(p.caller, None);
        assert_eq!(p.pay, MCP_CALL_DEFAULT_PAY);
        assert_eq!(p.target, "claude");
        assert_eq!(p.message, "hi there");

        // Flags in any order before the target.
        for parts in [
            vec!["--as", "bob", "--pay", "0.5", "claude", "yo"],
            vec!["--pay", "0.5", "--as", "bob", "claude", "yo"],
        ] {
            let p = parse_mcp_call_args(&args(&parts)).unwrap();
            assert_eq!(p.caller.as_deref(), Some("bob"));
            assert_eq!(p.pay, "0.5");
            assert_eq!(p.target, "claude");
            assert_eq!(p.message, "yo");
        }
    }

    #[test]
    fn parse_mcp_call_rejects_bad_forms() {
        assert!(parse_mcp_call_args(&args(&[])).is_err()); // empty
        assert!(parse_mcp_call_args(&args(&["claude"])).is_err()); // no message
        assert!(parse_mcp_call_args(&args(&["--as"])).is_err()); // dangling --as
        assert!(parse_mcp_call_args(&args(&["--pay"])).is_err()); // dangling --pay
        assert!(parse_mcp_call_args(&args(&["--pay", "1", "claude"])).is_err()); // no message
    }

    #[test]
    fn parse_colony_run_parses_task_reward_worker_judge_ttl() {
        // Full form: multi-word task + reward + worker + judge + ttl, interleaved.
        let p = parse_colony_run_args(&args(&[
            "QA:", "probe", "one", "flow", "--reward", "0.02", "--worker", "vex-qa", "--judge",
            "claude", "--ttl", "1h",
        ]))
        .unwrap();
        assert_eq!(p.task, "QA: probe one flow");
        assert_eq!(p.reward_wei, 20_000_000_000_000_000); // 0.02 LH
        assert_eq!(p.worker.as_deref(), Some("vex-qa"));
        assert_eq!(p.judge.as_deref(), Some("claude"));
        assert_eq!(p.ttl_secs, 3600);

        // No worker, no judge, no ttl → all None, default ttl.
        let p = parse_colony_run_args(&args(&["fix the bug", "--reward", "1"])).unwrap();
        assert_eq!(p.task, "fix the bug");
        assert_eq!(p.reward_wei, 1_000_000_000_000_000_000); // 1 LH
        assert!(p.worker.is_none());
        assert!(p.judge.is_none());
        assert_eq!(p.ttl_secs, INVITE_DEFAULT_TTL_SECS);
    }

    #[test]
    fn parse_judge_rating_extracts_digit_and_rationale() {
        // The canonical shape: digit on line 1, rationale on line 2.
        let (r, why) = parse_judge_rating("5\nSpecific, correct, and on-topic.");
        assert_eq!(r, 5);
        assert_eq!(why, "Specific, correct, and on-topic.");

        // A bogus/hallucinated result the judge rejects.
        let (r, _) = parse_judge_rating("1\nFabricated — localharness has no control API.");
        assert_eq!(r, 1);

        // Chatty prefix: still finds the first 1..5 digit.
        let (r, _) = parse_judge_rating("Rating: 4 — good but slightly vague.");
        assert_eq!(r, 4);

        // Out-of-range / no digit → neutral default of 3.
        assert_eq!(parse_judge_rating("no number here at all").0, 3);
        // A leading 0/6..9 is skipped; the first IN-RANGE digit wins.
        assert_eq!(parse_judge_rating("0 then 2").0, 2);
        assert_eq!(parse_judge_rating("99999").0, 3);
    }

    #[test]
    fn median_rating_aggregates_panel() {
        // Odd N → the true middle (sorted).
        assert_eq!(median_rating(&[5, 4, 5]), 5);
        assert_eq!(median_rating(&[1, 3, 5]), 3);
        assert_eq!(median_rating(&[2, 5, 4, 3, 1]), 3); // unsorted input is sorted
        // A single rogue judge can't swing the median.
        assert_eq!(median_rating(&[5, 5, 1]), 5);
        assert_eq!(median_rating(&[1, 1, 5]), 1);
        // Even N → the LOWER-MIDDLE (conservative: never inflate a split panel).
        assert_eq!(median_rating(&[4, 5]), 4);
        assert_eq!(median_rating(&[1, 2, 4, 5]), 2); // sorted [1,2,4,5], idx n/2-1 = 1 → 2
        // All-same → that value (any N).
        assert_eq!(median_rating(&[4, 4, 4]), 4);
        assert_eq!(median_rating(&[2, 2]), 2);
        // A single judge → its own rating (a `--judge X` panel of one).
        assert_eq!(median_rating(&[3]), 3);
        // EMPTY (every judge turn failed) → the neutral 3 default.
        assert_eq!(median_rating(&[]), 3);
        // The median of any 1..=5 inputs stays in range.
        assert!((1..=5).contains(&median_rating(&[1, 5])));
    }

    #[test]
    fn should_accept_gates_payment_on_the_rating_bar() {
        // Default bar (2): a median of 1 (clear failure / hallucination) is REJECTED;
        // 2..=5 are PAID. This is the core economic-rationality rule.
        assert!(!should_accept(1, COLONY_DEFAULT_MIN_ACCEPT)); // median 1 / min 2 → reject
        assert!(should_accept(2, COLONY_DEFAULT_MIN_ACCEPT)); // median 2 / min 2 → accept
        assert!(should_accept(3, COLONY_DEFAULT_MIN_ACCEPT));
        assert!(should_accept(5, COLONY_DEFAULT_MIN_ACCEPT));
        // Boundary is `>=`: equal accepts, one below rejects.
        assert!(should_accept(2, 2)); // median 2 / min 2 → accept
        assert!(should_accept(5, 5)); // median 5 / min 5 → accept
        assert!(!should_accept(4, 5)); // median 4 / min 5 → reject
        assert!(!should_accept(1, 2));
        // A strict bar of 5 only ever pays a unanimous 5★.
        assert!(!should_accept(4, 5));
        assert!(should_accept(5, 5));
        // A bar of 1 (the lowest valid floor) pays everything 1..=5.
        for m in 1..=5 {
            assert!(should_accept(m, 1));
        }
        // Clamp/edge: a stray 0 median can never sneak past a min-1 floor, and an
        // out-of-band min is pulled into 1..=5 so the comparison stays sane.
        assert!(should_accept(0, 1)); // 0 clamps up to 1 ≥ 1 → accept (floor case)
        assert!(!should_accept(0, 2)); // 0 clamps to 1 < 2 → reject
        assert!(should_accept(5, 0)); // min 0 clamps up to 1 → 5 ≥ 1 → accept
        assert!(should_accept(6, 5)); // 6 clamps to 5 ≥ 5 → accept
        assert!(should_accept(5, 9)); // min 9 clamps to 5 → 5 ≥ 5 → accept
    }

    #[test]
    fn parse_colony_run_args_min_accept_flag() {
        let mk = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // Default when omitted.
        let p = parse_colony_run_args(&mk(&["QA task", "--reward", "0.01"])).unwrap();
        assert_eq!(p.min_accept, COLONY_DEFAULT_MIN_ACCEPT);
        assert_eq!(p.min_accept, 2);
        // Explicit, in-range.
        let p =
            parse_colony_run_args(&mk(&["QA task", "--reward", "0.01", "--min-accept-rating", "5"]))
                .unwrap();
        assert_eq!(p.min_accept, 5);
        let p =
            parse_colony_run_args(&mk(&["QA task", "--reward", "0.01", "--min-accept-rating", "1"]))
                .unwrap();
        assert_eq!(p.min_accept, 1);
        // 0 and out-of-band / non-numeric are rejected at parse time.
        assert!(parse_colony_run_args(&mk(&["t", "--reward", "0.01", "--min-accept-rating", "0"])).is_err());
        assert!(parse_colony_run_args(&mk(&["t", "--reward", "0.01", "--min-accept-rating", "6"])).is_err());
        assert!(parse_colony_run_args(&mk(&["t", "--reward", "0.01", "--min-accept-rating", "x"])).is_err());
        // Dangling flag is an error.
        assert!(parse_colony_run_args(&mk(&["t", "--reward", "0.01", "--min-accept-rating"])).is_err());
    }

    #[test]
    fn select_judge_panel_excludes_worker_and_caller_distinct() {
        let local: Vec<String> = ["claude", "dex-qa", "vex-qa", "iris-qa", "juno-qa"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        // Worker = vex-qa, caller = claude → both excluded; first 3 of the rest.
        let panel = select_judge_panel(&local, "vex-qa", "claude", 3);
        assert_eq!(panel, vec!["dex-qa", "iris-qa", "juno-qa"]);
        assert!(!panel.iter().any(|n| n == "vex-qa" || n == "claude"));
        // Fewer neutral agents than asked → returns what's available (no panic).
        let small = vec!["claude".to_string(), "dex-qa".to_string()];
        let panel = select_judge_panel(&small, "dex-qa", "claude", 3);
        assert!(panel.is_empty()); // both excluded → no neutral agent
        let panel = select_judge_panel(&small, "someone-else", "claude", 3);
        assert_eq!(panel, vec!["dex-qa"]); // only one neutral remains
        // Distinct: a duplicate name in the input is taken once.
        let dupes = vec!["dex-qa".to_string(), "dex-qa".to_string(), "iris-qa".to_string()];
        let panel = select_judge_panel(&dupes, "w", "c", 5);
        assert_eq!(panel, vec!["dex-qa", "iris-qa"]);
        // N caps the size even when more neutral agents exist.
        let panel = select_judge_panel(&local, "w", "c", 2);
        assert_eq!(panel.len(), 2);
    }

    #[test]
    fn parse_colony_run_args_judges_flag() {
        let mk = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // Default panel size when --judges is omitted.
        let p = parse_colony_run_args(&mk(&["QA task", "--reward", "0.01"])).unwrap();
        assert_eq!(p.judges, COLONY_DEFAULT_PANEL);
        // Explicit --judges.
        let p = parse_colony_run_args(&mk(&["QA task", "--reward", "0.01", "--judges", "5"])).unwrap();
        assert_eq!(p.judges, 5);
        // Zero / non-numeric is rejected.
        assert!(parse_colony_run_args(&mk(&["t", "--reward", "0.01", "--judges", "0"])).is_err());
        assert!(parse_colony_run_args(&mk(&["t", "--reward", "0.01", "--judges", "x"])).is_err());
    }

    #[test]
    fn colony_judge_prompt_embeds_task_result_and_serverless_context() {
        let p = colony_judge_prompt("find a real security issue", "the control API binds 0.0.0.0");
        assert!(p.contains("find a real security issue"));
        assert!(p.contains("the control API binds 0.0.0.0"));
        // The accuracy anchor that lets the judge catch the serverless hallucination.
        assert!(p.contains("SERVERLESS"));
        assert!(p.contains("single digit 1-5"));
    }

    #[test]
    fn pick_reputation_aware_blends_task_fit_then_reputation() {
        let cand = |name: &str, task_rank: usize, count: u64, sum: u64| WorkerCandidate {
            name: name.into(),
            task_rank,
            rep_count: count,
            rep_sum: sum,
        };

        // 1. PROVEN beats UNPROVEN at SIMILAR task fit (both within the band):
        //    dex-qa is the very top match but has no reputation; vex-qa is one rank
        //    behind but carries 5.0★ from 2 attestations → reputation decides.
        let set = [
            cand("dex-qa", 0, 0, 0),  // top task fit, unproven
            cand("vex-qa", 1, 2, 10), // similar task fit, 5.0★ x2
        ];
        assert_eq!(pick_reputation_aware(&set).unwrap().name, "vex-qa");

        // 2. TASK FIT still DOMINATES a wildly-irrelevant high-rep agent: a 5.0★
        //    agent buried far down the discover list (way outside the band) loses
        //    to the relevant-but-unproven top match.
        let set = [
            cand("dex-qa", 0, 0, 0),     // top task fit, unproven
            cand("guru-bot", 50, 9, 45), // irrelevant to the task, 5.0★ x9
        ];
        assert_eq!(pick_reputation_aware(&set).unwrap().name, "dex-qa");

        // 3. Higher AVERAGE wins within a tier (4.0★ x10 vs 5.0★ x2 → 5.0 wins).
        let set = [
            cand("steady", 0, 10, 40), // 4.0★
            cand("ace", 1, 2, 10),     // 5.0★
        ];
        assert_eq!(pick_reputation_aware(&set).unwrap().name, "ace");

        // 4. Equal average → MORE attestations is the tiebreak (5.0 x3 > 5.0 x1).
        let set = [
            cand("rookie", 0, 1, 5),   // 5.0★ x1
            cand("veteran", 1, 3, 15), // 5.0★ x3
        ];
        assert_eq!(pick_reputation_aware(&set).unwrap().name, "veteran");

        // 5. All unproven → falls back to best discover rank (deterministic).
        let set = [cand("first", 0, 0, 0), cand("second", 1, 0, 0)];
        assert_eq!(pick_reputation_aware(&set).unwrap().name, "first");

        // Empty candidate set → no pick.
        assert!(pick_reputation_aware(&[]).is_none());
    }

    #[test]
    fn colony_task_keywords_extracts_significant_words() {
        // The dogfood task: stop words + short words + punctuation dropped, the
        // meaningful keywords kept in order (so "qa" surfaces the QA fleet).
        let kw = colony_task_keywords("QA: suggest one concrete localharness CLI improvement (1-2 sentences)");
        assert!(kw.contains(&"localharness".to_string()));
        assert!(kw.contains(&"improvement".to_string()));
        assert!(kw.contains(&"concrete".to_string()));
        // "cli" is 3 chars and not a stop word → kept (punctuation stripped).
        assert!(kw.contains(&"cli".to_string()));
        // Stop words + sub-3-char tokens ("qa", "12") dropped; "suggest" is a stop word.
        assert!(!kw.contains(&"one".to_string()));
        assert!(!kw.contains(&"suggest".to_string()));
        assert!(!kw.contains(&"qa".to_string()));
        assert!(!kw.contains(&"12".to_string()));
        // No dupes, bounded.
        let dup = colony_task_keywords("test test test localharness localharness bounty");
        assert_eq!(dup.iter().filter(|w| *w == "test").count(), 1);
        assert!(dup.len() <= COLONY_MAX_KEYWORDS);
        // All-stop-word / empty task → no keywords.
        assert!(colony_task_keywords("the a an to of").is_empty());
        assert!(colony_task_keywords("").is_empty());
    }

    #[test]
    fn colony_pick_reasoning_is_transparent() {
        let proven = WorkerCandidate { name: "vex-qa".into(), task_rank: 0, rep_count: 2, rep_sum: 10 };
        let line = colony_pick_reasoning(&proven);
        assert!(line.contains("vex-qa"));
        assert!(line.contains("reputation 5.0"));
        assert!(line.contains("2 attestations"));
        assert!(line.contains("top task match"));

        let unproven = WorkerCandidate { name: "dex-qa".into(), task_rank: 1, rep_count: 0, rep_sum: 0 };
        let line = colony_pick_reasoning(&unproven);
        assert!(line.contains("dex-qa"));
        assert!(line.contains("no reputation yet"));
        assert!(line.contains("task match #2"));

        // Singular grammar for a single attestation.
        let single = WorkerCandidate { name: "solo".into(), task_rank: 0, rep_count: 1, rep_sum: 4 };
        let line = colony_pick_reasoning(&single);
        assert!(line.contains("4.0 from 1 attestation"));
        assert!(!line.contains("attestations"));
    }

    #[test]
    fn colony_recovery_hint_matches_the_working_command_per_status() {
        // OPEN (0): `bounty cancel` is the recovery — and it WORKS while Open.
        let h = colony_recovery_hint(7, "me", Some(0));
        assert!(h.contains("bounty cancel --as me 7"), "open → cancel: {h}");
        assert!(!h.contains("bounty reclaim"), "open must NOT steer to reclaim: {h}");

        // CLAIMED (1) / SUBMITTED (2): `cancelBounty` reverts `NotOpen`, so the
        // ONLY working recovery is the ttl-gated `bounty reclaim`. The earlier bug
        // headlined `bounty cancel` here, which always reverts mid-cycle.
        for st in [1u8, 2] {
            let h = colony_recovery_hint(7, "me", Some(st));
            assert!(h.contains("bounty reclaim --as me 7"), "status {st} → reclaim: {h}");
            // Must NOT headline the cancel command that would revert.
            assert!(
                !h.contains("bounty cancel --as me 7"),
                "status {st} must not advise the reverting cancel: {h}"
            );
        }

        // PAID (3): nothing to recover (the worker was paid).
        let h = colony_recovery_hint(7, "me", Some(3));
        assert!(h.to_lowercase().contains("paid"));
        assert!(!h.contains("bounty cancel") && !h.contains("bounty reclaim"));

        // Cancelled (4) / Reclaimed (5): already refunded, nothing to do.
        for st in [4u8, 5] {
            let h = colony_recovery_hint(7, "me", Some(st));
            assert!(h.to_lowercase().contains("refunded"), "status {st}: {h}");
        }

        // Unknown / unreadable status → surface BOTH so the user is never stuck.
        let h = colony_recovery_hint(7, "me", None);
        assert!(h.contains("bounty cancel --as me 7"));
        assert!(h.contains("bounty reclaim --as me 7"));
    }

    #[test]
    fn parse_colony_run_rejects_bad_forms() {
        assert!(parse_colony_run_args(&args(&[])).is_err()); // empty
        assert!(parse_colony_run_args(&args(&["task"])).is_err()); // no --reward
        assert!(parse_colony_run_args(&args(&["task", "--reward", "0"])).is_err()); // zero reward
        assert!(parse_colony_run_args(&args(&["--reward", "1"])).is_err()); // no task
        assert!(parse_colony_run_args(&args(&["task", "--reward"])).is_err()); // dangling --reward
        assert!(parse_colony_run_args(&args(&["t", "--reward", "1", "--worker"])).is_err()); // dangling
    }

    #[test]
    fn is_transient_rpc_error_classifies_hiccups_not_reverts() {
        // The live failure mode: a decode/transport hiccup on the response.
        assert!(is_transient_rpc_error(
            "eth_sendRawTransaction decode: error decoding response body"
        ));
        assert!(is_transient_rpc_error("connection reset"));
        assert!(is_transient_rpc_error("request timed out"));
        // A real contract revert must NOT be retried (it'll just revert again).
        assert!(!is_transient_rpc_error("execution reverted: NotOpen()"));
        assert!(!is_transient_rpc_error("revert: bounty not submitted"));
        assert!(!is_transient_rpc_error("insufficient balance"));
    }

    #[test]
    fn mcp_call_pay_parses_to_18_decimal_wei() {
        // The default + a few human amounts map to the bundle's 18-dec wei.
        assert_eq!(
            localharness::encoding::parse_token_amount(MCP_CALL_DEFAULT_PAY),
            Some(1_000_000_000_000_000) // 0.001 * 1e18
        );
        assert_eq!(
            localharness::encoding::parse_token_amount("1"),
            Some(1_000_000_000_000_000_000)
        );
    }

    #[test]
    fn mcp_x402_header_json_matches_proxy_shape() {
        // The exact field names + types `proxy/api/mcp.ts::parseAuth` requires.
        let from = "0x00000000000000000000000000000000000000aa";
        let to = "0x00000000000000000000000000000000000000bb";
        let nonce = [0x11u8; 32];
        let sig = [0x22u8; 65];
        let j = mcp_x402_header_json(from, to, 1_000_000_000_000_000, 0, 1_999_999_999, &nonce, &sig);

        assert_eq!(j["from"], from);
        assert_eq!(j["to"], to);
        // value is a DECIMAL STRING of $LH wei (not a number).
        assert_eq!(j["value"], "1000000000000000");
        assert!(j["value"].is_string());
        assert_eq!(j["validAfter"], 0);
        assert_eq!(j["validBefore"], 1_999_999_999u64);
        // nonce: 0x + 32 bytes = 64 hex chars. signature: 0x + 65 bytes = 130 hex.
        let nonce_s = j["nonce"].as_str().unwrap();
        let sig_s = j["signature"].as_str().unwrap();
        assert_eq!(nonce_s.len(), 2 + 64);
        assert!(nonce_s.starts_with("0x"));
        assert_eq!(sig_s.len(), 2 + 130);
        assert!(sig_s.starts_with("0x"));
    }

    #[test]
    fn mcp_tools_call_body_is_ask_agent_jsonrpc() {
        let b = mcp_tools_call_body("claude", "hello");
        assert_eq!(b["jsonrpc"], "2.0");
        assert_eq!(b["method"], "tools/call");
        // params.name is the TOOL ("ask_agent"); the target rides in arguments.
        assert_eq!(b["params"]["name"], "ask_agent");
        assert_eq!(b["params"]["arguments"]["name"], "claude");
        assert_eq!(b["params"]["arguments"]["message"], "hello");
    }

    #[test]
    fn mcp_random_nonce_is_32_bytes_and_fresh() {
        let a = registry::random_x402_nonce();
        let b = registry::random_x402_nonce();
        assert_eq!(a.len(), 32);
        assert_eq!(b.len(), 32);
        // Two draws of a CSPRNG should differ (vanishing collision odds).
        assert_ne!(a, b);
    }

    #[test]
    fn mcp_endpoint_is_proxy_slash_mcp() {
        let url = mcp_endpoint_url();
        assert!(url.ends_with("/mcp"));
        assert!(!url.contains("//mcp")); // no double slash from the base
    }

    #[test]
    fn parse_call_rejects_empty() {
        assert!(parse_call_args(&args(&[])).is_err());
    }

    #[test]
    fn parse_call_rejects_as_without_name() {
        assert!(parse_call_args(&args(&["--as"])).is_err());
    }

    #[test]
    fn parse_call_rejects_as_name_without_target_or_message() {
        // `--as bob` alone: caller set, but no target/message → usage error.
        assert!(parse_call_args(&args(&["--as", "bob"])).is_err());
        // `--as bob alice` : target but no message → usage error.
        assert!(parse_call_args(&args(&["--as", "bob", "alice"])).is_err());
    }

    #[test]
    fn thread_file_target_parses_own_files_only() {
        // Backend-tagged files (current format): the tag is stripped.
        assert_eq!(
            thread_file_target("claude", "claude__alice.gemini.bin").as_deref(),
            Some("alice")
        );
        assert_eq!(
            thread_file_target("claude", "claude__alice.anthropic.bin").as_deref(),
            Some("alice")
        );
        // Legacy untagged files still parse (backward compatibility).
        assert_eq!(
            thread_file_target("claude", "claude__alice.bin").as_deref(),
            Some("alice")
        );
        // A target containing the separator stays intact (strip_prefix once).
        assert_eq!(
            thread_file_target("claude", "claude__a__b.gemini.bin").as_deref(),
            Some("a__b")
        );
        // Different caller → not ours.
        assert_eq!(thread_file_target("claude", "bob__alice.gemini.bin"), None);
        // Wrong extension, or empty target → rejected.
        assert_eq!(thread_file_target("claude", "claude__alice.txt"), None);
        assert_eq!(thread_file_target("claude", "claude__.bin"), None);
        assert_eq!(thread_file_target("claude", "claude__.gemini.bin"), None);
        assert_eq!(thread_file_target("claude", "unrelated.bin"), None);
    }

    #[test]
    fn thread_file_target_roundtrips_history_path() {
        // The parser must invert the filename half of history_path for both
        // backends.
        for backend in ["gemini", "anthropic"] {
            let p = history_path("claude", "alice", backend);
            let name = p.file_name().unwrap().to_str().unwrap();
            assert_eq!(thread_file_target("claude", name).as_deref(), Some("alice"));
        }
    }

    #[test]
    fn model_backend_tag_routes_claude_to_anthropic() {
        assert_eq!(model_backend_tag(Some("claude-opus-4")), "anthropic");
        assert_eq!(model_backend_tag(Some("claude")), "anthropic");
        assert_eq!(model_backend_tag(Some("gemini-3.5-flash")), "gemini");
        assert_eq!(model_backend_tag(None), "gemini");
    }

    #[test]
    fn history_path_keys_on_backend_so_formats_never_collide() {
        // The cross-backend bug: a Gemini thread and an Anthropic thread to the
        // same target must live in SEPARATE files (incompatible on-disk shapes).
        let g = history_path("claude", "alice", "gemini");
        let a = history_path("claude", "alice", "anthropic");
        assert_ne!(g, a, "backends must not share a history file");
        assert!(g.ends_with("claude__alice.gemini.bin"));
        assert!(a.ends_with("claude__alice.anthropic.bin"));
    }

    #[test]
    fn take_as_flag_extracts_caller() {
        let a = args(&["--as", "bob", "threads"]);
        let (caller, rest) = take_as_flag(&a).unwrap();
        assert_eq!(caller.as_deref(), Some("bob"));
        assert_eq!(rest, vec!["threads".to_string()]);

        let b = args(&["alice"]);
        let (caller, rest) = take_as_flag(&b).unwrap();
        assert_eq!(caller, None);
        assert_eq!(rest, vec!["alice".to_string()]);

        assert!(take_as_flag(&args(&["--as"])).is_err());
    }

    #[test]
    fn take_as_flag_scans_any_position() {
        // The real bug: `probe --deep --as fleet` — `--as` is NOT first, so the
        // old first-arg-only parser missed it and the fleet name never resolved.
        let (caller, rest) = take_as_flag(&args(&["--deep", "--as", "fleet"])).unwrap();
        assert_eq!(caller.as_deref(), Some("fleet"));
        assert_eq!(rest, vec!["--deep".to_string()]);

        // Trailing flag is still consumed; surrounding args preserved in order.
        let (caller, rest) = take_as_flag(&args(&["a", "b", "--as", "me", "c"])).unwrap();
        assert_eq!(caller.as_deref(), Some("me"));
        assert_eq!(rest, vec!["a".to_string(), "b".to_string(), "c".to_string()]);

        // `--as` requiring a value, even mid-list.
        assert!(take_as_flag(&args(&["--deep", "--as"])).is_err());
        // A duplicated `--as` is an error, not a silent last-wins.
        assert!(take_as_flag(&args(&["--as", "a", "--as", "b"])).is_err());
    }

    #[test]
    fn history_path_keys_on_caller_and_target() {
        let p = history_path("claude", "alice", "gemini");
        assert!(p.ends_with("claude__alice.gemini.bin"));
        // Distinct caller or target → distinct file (no cross-thread bleed).
        assert_ne!(
            history_path("claude", "alice", "gemini"),
            history_path("bob", "alice", "gemini")
        );
        assert_ne!(
            history_path("claude", "alice", "gemini"),
            history_path("claude", "bob", "gemini")
        );
        // Lives under a hidden dir so it doesn't clutter the working tree.
        assert!(p.starts_with(".localharness"));
    }

    #[test]
    fn format_whoami_unregistered_is_one_line() {
        let info = WhoamiInfo {
            name: "ghost".into(),
            owner: None,
            token_id: 0,
            tba: None,
            has_persona: false,
            public_face: None,
        };
        assert_eq!(format_whoami(&info), "ghost is unregistered");
    }

    #[test]
    fn format_whoami_full_profile() {
        let info = WhoamiInfo {
            name: "claude".into(),
            owner: Some("0xabc".into()),
            token_id: 8,
            tba: Some("0xdef".into()),
            has_persona: true,
            public_face: Some("app".into()),
        };
        let out = format_whoami(&info);
        assert!(out.starts_with("claude.localharness.xyz\n"));
        assert!(out.contains("owner    0xabc"));
        assert!(out.contains("tokenId  8"));
        assert!(out.contains("wallet   0xdef  (token-bound account)"));
        assert!(out.contains("persona  published"));
        assert!(out.contains("face     app"));
    }

    #[test]
    fn format_whoami_absent_persona_and_face() {
        let info = WhoamiInfo {
            name: "bare".into(),
            owner: Some("0x1".into()),
            token_id: 3,
            tba: None,
            has_persona: false,
            public_face: None,
        };
        let out = format_whoami(&info);
        assert!(out.contains("persona  none"));
        assert!(out.contains("face     unset (directory)"));
        assert!(out.contains("wallet   —"));
    }

    #[test]
    fn format_whoami_json_registered_roundtrips() {
        let info = WhoamiInfo {
            name: "claude".into(),
            owner: Some("0xabc".into()),
            token_id: 8,
            tba: Some("0xdef".into()),
            has_persona: true,
            public_face: Some("app".into()),
        };
        let v: serde_json::Value = serde_json::from_str(&format_whoami_json(&info)).unwrap();
        assert_eq!(v["name"], "claude");
        assert_eq!(v["registered"], true);
        assert_eq!(v["owner"], "0xabc");
        assert_eq!(v["tokenId"], 8);
        assert_eq!(v["wallet"], "0xdef");
        assert_eq!(v["persona"], true);
        assert_eq!(v["face"], "app");
    }

    #[test]
    fn format_whoami_json_unregistered_nulls() {
        let info = WhoamiInfo {
            name: "ghost".into(),
            owner: None,
            token_id: 0,
            tba: None,
            has_persona: false,
            public_face: None,
        };
        let v: serde_json::Value = serde_json::from_str(&format_whoami_json(&info)).unwrap();
        assert_eq!(v["registered"], false);
        assert!(v["owner"].is_null());
        assert!(v["wallet"].is_null());
        assert!(v["face"].is_null());
        assert_eq!(v["persona"], false);
    }

    #[test]
    fn hint_for_call_error_classifies_common_failures() {
        // Payment / session / credits → the $LH hint.
        for s in [
            "HTTP 402 Payment Required",
            "proxy: no session for 0xabc",
            "insufficient credit",
        ] {
            assert!(
                hint_for_call_error(s).unwrap().contains("$LH"),
                "expected $LH hint for {s:?}"
            );
        }
        // Auth → the signature hint.
        for s in ["401 Unauthorized", "bad signature", "403 Forbidden"] {
            assert!(
                hint_for_call_error(s).unwrap().contains("signature"),
                "expected auth hint for {s:?}"
            );
        }
        // Rate limit.
        assert!(hint_for_call_error("429 Too Many Requests")
            .unwrap()
            .contains("rate limited"));
    }

    #[test]
    fn hint_for_call_error_is_case_insensitive_and_silent_on_unknown() {
        assert!(hint_for_call_error("PAYMENT REQUIRED").is_some());
        // An unrecognised error gets no hint (caller still prints the raw text).
        assert_eq!(hint_for_call_error("connection reset by peer"), None);
        assert_eq!(hint_for_call_error("some unrelated parse error"), None);
    }

    #[test]
    fn rustlite_compiles_a_minimal_cartridge() {
        // Uses only primitives proven in the live claude-app.rl face.
        let src = "fn frame(t: i32) {\n  \
                   let w: i32 = host::display::width();\n  \
                   host::display::clear(0);\n  \
                   host::display::fill_rect(0, 0, w, 8, 16777215);\n  \
                   host::display::present();\n}";
        let wasm = localharness::rustlite::compile(src).expect("minimal cartridge compiles");
        assert_eq!(&wasm[0..4], b"\0asm", "valid wasm magic header");
        assert!(wasm.len() <= PUBLISH_CAP);
    }

    #[test]
    fn rustlite_rejects_garbage() {
        assert!(localharness::rustlite::compile("this is not rustlite").is_err());
    }

    #[test]
    fn cartridge_entry_detection() {
        // A real frame() cartridge exports the entry the loader calls.
        let with =
            localharness::rustlite::compile("fn frame(t: i32) { host::display::present(); }")
                .unwrap();
        assert!(cartridge_has_entry(&with), "frame() must be detected");

        // Compiles, but only a helper — no entry → would render nothing.
        let without = localharness::rustlite::compile("fn helper(n: i32) -> i32 { n + 1 }").unwrap();
        assert!(!cartridge_has_entry(&without), "no entry must be rejected");

        // The shipped bitmask cartridge has an entry.
        let bitmask = localharness::rustlite::compile(include_str!("../../bitmask.rl")).unwrap();
        assert!(cartridge_has_entry(&bitmask));

        // Malformed / truncated bytes never panic and report no entry.
        assert!(!cartridge_has_entry(b""));
        assert!(!cartridge_has_entry(b"\0asm")); // header only
        assert!(!cartridge_has_entry(b"\0asm\x01\0\0\0\x07\xff")); // bogus section size
    }

    #[test]
    fn name_validation_matches_registry_rule() {
        assert!(name_is_valid("alice"));
        assert!(name_is_valid("a-1-b"));
        assert!(!name_is_valid("Alice")); // uppercase
        assert!(!name_is_valid("a_b")); // underscore
        assert!(!name_is_valid("")); // empty
        assert!(name_is_valid(&"a".repeat(63)));
        assert!(!name_is_valid(&"a".repeat(64))); // too long
        assert!(!name_is_valid("🤖")); // emoji (non-ascii) — already caught
        assert!(!name_is_valid("-foo")); // leading hyphen — not a valid DNS label
        assert!(!name_is_valid("foo-")); // trailing hyphen
        assert!(!name_is_valid("-")); // only a hyphen
        assert!(name_is_valid("a-b-c")); // internal hyphens are fine
    }

    #[test]
    fn usage_documents_every_command() {
        // Every dispatchable subcommand must appear in the help text, so a new
        // command can't ship undocumented for beta testers reading `help`.
        for cmd in [
            "create", "compile", "publish", "face", "persona", "call", "list",
            "feedback", "probe", "triage", "threads", "forget", "whoami", "invite",
            "bounty", "colony", "reputation", "guild", "vote", "tba",
        ] {
            assert!(
                USAGE.contains(cmd),
                "`{cmd}` is dispatchable but missing from the help/USAGE text"
            );
        }
    }

    #[test]
    fn parse_work_ref_handles_none_hex_and_bounty_id() {
        // None → the zero ref.
        assert_eq!(parse_work_ref(None), Ok([0u8; 32]));
        assert_eq!(parse_work_ref(Some("   ")), Ok([0u8; 32]));
        // A bare integer (or #N) → bounty id left-padded into the low 8 bytes — the
        // SAME bytes `bounty_work_ref` produces (what the colony [7/7] step uses).
        assert_eq!(parse_work_ref(Some("7")).unwrap(), bounty_work_ref(7));
        assert_eq!(parse_work_ref(Some("#42")).unwrap(), bounty_work_ref(42));
        let r7 = parse_work_ref(Some("7")).unwrap();
        assert_eq!(&r7[24..32], &7u64.to_be_bytes());
        assert!(r7[..24].iter().all(|&b| b == 0));
        // A 0x hex ref is right-aligned (left-padded with zeros).
        let r = parse_work_ref(Some("0xabcd")).unwrap();
        assert_eq!(r[30], 0xab);
        assert_eq!(r[31], 0xcd);
        assert!(r[..30].iter().all(|&b| b == 0));
        // A full 32-byte hex ref is preserved as-is.
        let full = "0x".to_string() + &"cd".repeat(32);
        assert_eq!(parse_work_ref(Some(&full)).unwrap(), [0xcd; 32]);
        // Rejects: over-long hex, non-hex, and a non-integer non-hex token.
        assert!(parse_work_ref(Some(&("0x".to_string() + &"ab".repeat(33)))).is_err());
        assert!(parse_work_ref(Some("0xzz")).is_err());
        assert!(parse_work_ref(Some("notanid")).is_err());
    }

    #[test]
    fn format_work_ref_renders_bounty_id_and_zero() {
        // Zero ref → no note.
        assert_eq!(format_work_ref(&format!("0x{}", "0".repeat(64))), "");
        // A bounty-id ref (high 24 bytes zero) → "(work #N)".
        let id_ref = format!("0x{}{:016x}", "0".repeat(48), 9u64);
        assert_eq!(format_work_ref(&id_ref), "  (work #9)");
        // A ref with non-zero high bytes → a truncated 0x note.
        let mixed = format!("0xcd{}", "0".repeat(62));
        assert_eq!(format_work_ref(&mixed), "  (ref 0xcd000000…)");
        // Malformed length → no note (no panic).
        assert_eq!(format_work_ref("0xdead"), "");
    }

    #[test]
    fn take_tba_flag_extracts_target_from_anywhere() {
        // No flag → all positionals, no override (default = caller's main TBA).
        let (t, rest) = take_tba_flag(&args(&["0xabc", "0", "--data", "0x01"])).unwrap();
        assert_eq!(t, None);
        assert_eq!(rest, args(&["0xabc", "0", "--data", "0x01"]));
        // --tba <name> at the front — positionals preserved in order.
        let (t, rest) = take_tba_flag(&args(&["--tba", "guildb", "0xdiamond", "0"])).unwrap();
        assert_eq!(t.as_deref(), Some("guildb"));
        assert_eq!(rest, args(&["0xdiamond", "0"]));
        // --tba <0xaddr> in the middle, alongside an untouched --data (left for the
        // later take_data_flag pass) — only --tba is consumed here.
        let (t, rest) =
            take_tba_flag(&args(&["0xdiamond", "0", "--tba", "0xfeed", "--data", "0xbeef"]))
                .unwrap();
        assert_eq!(t.as_deref(), Some("0xfeed"));
        assert_eq!(rest, args(&["0xdiamond", "0", "--data", "0xbeef"]));
        // Dangling / doubled → error.
        assert!(take_tba_flag(&args(&["--tba"])).is_err());
        assert!(take_tba_flag(&args(&["--tba", "a", "--tba", "b"])).is_err());
    }

    #[test]
    fn take_data_flag_extracts_hex_from_anywhere() {
        // No flag → all positionals, no data.
        let (d, rest) = take_data_flag(&args(&["alice", "5"])).unwrap();
        assert_eq!(d, None);
        assert_eq!(rest, args(&["alice", "5"]));
        // --data at the end.
        let (d, rest) = take_data_flag(&args(&["0xabc", "0", "--data", "0xdeadbeef"])).unwrap();
        assert_eq!(d.as_deref(), Some("0xdeadbeef"));
        assert_eq!(rest, args(&["0xabc", "0"]));
        // --data in the middle — positionals preserved in order.
        let (d, rest) = take_data_flag(&args(&["--data", "0x01", "bob", "2"])).unwrap();
        assert_eq!(d.as_deref(), Some("0x01"));
        assert_eq!(rest, args(&["bob", "2"]));
        // Dangling / doubled → error.
        assert!(take_data_flag(&args(&["--data"])).is_err());
        assert!(take_data_flag(&args(&["--data", "0x01", "--data", "0x02"])).is_err());
    }

    #[test]
    fn decode_hex_arg_accepts_prefix_and_rejects_malformed() {
        // 0x-prefixed and bare both decode the same.
        assert_eq!(decode_hex_arg("0xdeadbeef").unwrap(), vec![0xDE, 0xAD, 0xBE, 0xEF]);
        assert_eq!(decode_hex_arg("deadbeef").unwrap(), vec![0xDE, 0xAD, 0xBE, 0xEF]);
        // Case-insensitive.
        assert_eq!(decode_hex_arg("0xAaBb").unwrap(), vec![0xAA, 0xBB]);
        // Empty (or bare 0x) → no bytes (a value-only call).
        assert!(decode_hex_arg("").unwrap().is_empty());
        assert!(decode_hex_arg("0x").unwrap().is_empty());
        // Odd length / non-hex → clean error, never a panic.
        assert!(decode_hex_arg("0xabc").is_err());
        assert!(decode_hex_arg("0xzz").is_err());
    }

    #[test]
    fn parse_list_flags_handles_as_and_json_any_order() {
        assert_eq!(parse_list_flags(&args(&[])).unwrap(), (None, false));
        assert_eq!(parse_list_flags(&args(&["--json"])).unwrap(), (None, true));
        let (c, j) = parse_list_flags(&args(&["--as", "bob", "--json"])).unwrap();
        assert_eq!((c.as_deref(), j), (Some("bob"), true));
        let (c, j) = parse_list_flags(&args(&["--json", "--as", "bob"])).unwrap();
        assert_eq!((c.as_deref(), j), (Some("bob"), true));
        assert!(parse_list_flags(&args(&["--as"])).is_err()); // dangling --as
        assert!(parse_list_flags(&args(&["alice"])).is_err()); // no positionals
    }

    #[test]
    fn format_owned_text_and_json() {
        let toks = vec![
            registry::OwnedToken { token_id: 8, name: "claude".into(), tba: Some("0xabc".into()) },
            registry::OwnedToken { token_id: 3, name: "alice".into(), tba: None },
        ];
        let text = format_owned("0xowner", &toks, false);
        assert!(text.contains("2 subdomain"));
        assert!(text.contains("claude  (tokenId 8)  0xabc"));
        assert!(text.contains("alice  (tokenId 3)  —"));

        let v: serde_json::Value =
            serde_json::from_str(&format_owned("0xowner", &toks, true)).unwrap();
        assert_eq!(v["count"], 2);
        assert_eq!(v["owner"], "0xowner");
        assert_eq!(v["subdomains"][0]["name"], "claude");
        assert_eq!(v["subdomains"][0]["tokenId"], 8);
        assert!(v["subdomains"][1]["wallet"].is_null());
    }

    #[test]
    fn read_file_clean_maps_not_found_without_leaking_os_error() {
        // Closes on-chain QA finding #1: "os error 2" must not reach the user.
        let err = read_file_clean("definitely-nonexistent-file-xyz123.rl").unwrap_err();
        assert!(err.contains("file not found"), "got: {err}");
        assert!(err.contains("definitely-nonexistent-file-xyz123.rl"), "got: {err}");
        assert!(!err.contains("os error"), "must not leak raw OS error: {err}");
    }

    #[test]
    fn looks_like_path_distinguishes_files_from_prose() {
        // Path-shaped: separators or known source/text extensions.
        assert!(looks_like_path("persona.txt"));
        assert!(looks_like_path("prompts/agent.md"));
        assert!(looks_like_path("C:\\agents\\bob.prompt"));
        assert!(looks_like_path("./x.rl"));
        // Plain prose persona text is NOT a path.
        assert!(!looks_like_path("You are bob, a helpful agent"));
        assert!(!looks_like_path("bob"));
    }

    #[test]
    fn resolve_persona_arg_literal_text_passthrough() {
        // A non-path-shaped, unreadable string is the persona text verbatim —
        // it must NOT touch the filesystem error path.
        let p = resolve_persona_arg("You are bob, answer tersely").unwrap();
        assert_eq!(p, "You are bob, answer tersely");
    }

    #[test]
    fn resolve_persona_arg_missing_file_is_clean_error() {
        // A path-shaped arg that doesn't exist → clean error, no raw OS error,
        // and NOT silently used as literal text.
        let err = resolve_persona_arg("definitely-nonexistent-xyz123.txt").unwrap_err();
        assert!(err.contains("file not found"), "got: {err}");
        assert!(!err.contains("os error"), "must not leak raw OS error: {err}");
    }

    #[test]
    fn qa_checks_pass_on_a_healthy_platform() {
        // The probe's deterministic invariants must hold against the shipped
        // rustlite + entry detector. If this fails, the probe would (correctly)
        // file an on-chain bug — so it doubles as a platform-health assertion.
        let fails = run_qa_checks();
        assert!(fails.is_empty(), "probe found issues on a healthy build: {fails:?}");
    }

    #[test]
    fn triage_dedups_and_ranks_by_recurrence() {
        let bodies = vec![
            "Compile leaks OS error".to_string(),
            "compile leaks os error".to_string(),       // same modulo case
            "  Compile   leaks OS error ".to_string(),  // same modulo whitespace
            "whoami is slow".to_string(),
        ];
        let ranked = triage_findings(&bodies);
        assert_eq!(ranked.len(), 2, "two distinct issues after dedup");
        assert_eq!(ranked[0].1, 3, "the recurring one ranks first with count 3");
        assert!(ranked[0].0.to_lowercase().contains("compile leaks"));
        assert_eq!(ranked[1].1, 1);
    }

    #[test]
    fn triage_skips_empty_bodies() {
        let bodies = vec!["".to_string(), "   ".to_string(), "real bug".to_string()];
        let ranked = triage_findings(&bodies);
        assert_eq!(ranked, vec![("real bug".to_string(), 1)]);
    }

    #[test]
    fn parse_qa_envelope_accepts_valid_rejects_others() {
        let env =
            parse_qa_envelope("qa/v1 source=qa-probe v0.20.0: compile leaked os error").unwrap();
        assert_eq!(env.source, "qa-probe");
        assert_eq!(env.version, "0.20.0");
        assert!(env.body.contains("compile leaked"));
        // Not a fleet envelope → rejected (triage won't consume its body).
        assert!(parse_qa_envelope("just some human feedback").is_none());
        assert!(parse_qa_envelope("qa/v1 source=x v1.0.0:   ").is_none()); // empty body
        assert!(parse_qa_envelope("qa/v1 no source or colon").is_none());
        assert!(parse_qa_envelope("qa/v1 source=x vNOTVERSION: body").is_none());
    }

    #[test]
    fn feedback_json_emits_fields_and_fleet_envelope() {
        let entries = vec![
            registry::FeedbackEntry {
                sender: "0xabc".into(),
                timestamp: 100,
                text: "[BUG] something broke".into(),
            },
            registry::FeedbackEntry {
                sender: "0xdef".into(),
                timestamp: 200,
                text: "qa/v1 source=qa-probe v0.20.0: a real bug".into(),
            },
        ];
        let v: serde_json::Value = serde_json::from_str(&feedback_json(&entries)).unwrap();
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        // plain entry: dedup-key fields + raw text, no fleet fields
        assert_eq!(arr[0]["sender"], "0xabc");
        assert_eq!(arr[0]["timestamp"], 100);
        assert_eq!(arr[0]["text"], "[BUG] something broke");
        assert!(arr[0].get("fleet_source").is_none());
        // qa/v1 envelope: gets fleet_source + decoded body
        assert_eq!(arr[1]["fleet_source"], "qa-probe");
        assert!(arr[1]["body"].as_str().unwrap().contains("a real bug"));
        // empty log → valid empty array
        let empty: serde_json::Value = serde_json::from_str(&feedback_json(&[])).unwrap();
        assert_eq!(empty.as_array().unwrap().len(), 0);
    }

    #[test]
    fn format_feedback_tags_fleet_envelopes_only() {
        let entries = vec![
            registry::FeedbackEntry {
                sender: "0x1".into(),
                timestamp: 1,
                text: "qa/v1 source=qa-probe v0.20.0: a real bug".into(),
            },
            registry::FeedbackEntry {
                sender: "0x2".into(),
                timestamp: 2,
                text: "a human note".into(),
            },
        ];
        let out = format_feedback(&entries);
        assert!(out.contains("[fleet:qa-probe]"));
        assert!(
            out.lines().any(|l| l.contains("0x2") && !l.contains("[fleet")),
            "human feedback must not be tagged as fleet"
        );
    }

    #[test]
    fn format_feedback_empty_and_entries() {
        assert!(format_feedback(&[]).contains("no on-chain feedback"));
        let entries = vec![
            registry::FeedbackEntry {
                sender: "0xabc".into(),
                timestamp: 1700000000,
                text: "create flow worked\nbut whoami was slow".into(),
            },
        ];
        let out = format_feedback(&entries);
        assert!(out.contains("1 on-chain feedback"));
        assert!(out.contains("0xabc"));
        // Newlines collapsed so one entry stays one block.
        assert!(out.contains("create flow worked but whoami was slow"));
    }

    #[test]
    fn format_owned_empty() {
        assert!(format_owned("0xo", &[], false).contains("no subdomains"));
        let v: serde_json::Value = serde_json::from_str(&format_owned("0xo", &[], true)).unwrap();
        assert_eq!(v["count"], 0);
        assert!(v["subdomains"].as_array().unwrap().is_empty());
    }

    #[test]
    fn sponsor_key_is_valid_and_derives_documented_address() {
        // The embedded SPONSOR_KEY pays fees for EVERY sponsored CLI op
        // (create/publish/persona). If it's stale or mistyped, all onboarding
        // silently fails. Guard that it parses and derives the documented
        // sponsor address (the dedicated low-budget key, rotated 2026-05-25) —
        // so a future rotation that forgets the bin won't ship broken.
        let signer = wallet::from_private_key_hex(SPONSOR_KEY).expect("SPONSOR_KEY must parse");
        let addr = format!("0x{}", to_hex(&wallet::address(&signer)));
        assert_eq!(
            addr.to_ascii_lowercase(),
            "0x0aff88ad13ef24cac5befd0f9dc3a05df79a922c",
            "SPONSOR_KEY no longer derives the documented sponsor address"
        );
    }

    #[test]
    fn llms_txt_publishes_canonical_onchain_constants() {
        // The agent-facing spec is read by agents to orient on-chain. It must
        // not drift from the code's source of truth — stale addresses would
        // send an agent to the wrong chain/contract. Automates the manual
        // "audit llms.txt vs registry.rs" pass.
        let spec = include_str!("../../web/llms.txt");
        assert!(
            spec.contains(registry::REGISTRY_ADDRESS),
            "llms.txt missing canonical diamond address {}",
            registry::REGISTRY_ADDRESS
        );
        assert!(
            spec.contains(registry::LOCALHARNESS_TOKEN_ADDRESS),
            "llms.txt missing the $LH token address {}",
            registry::LOCALHARNESS_TOKEN_ADDRESS
        );
        assert!(
            spec.contains(registry::RPC_URL),
            "llms.txt missing the RPC URL {}",
            registry::RPC_URL
        );
        assert!(
            spec.contains(&registry::CHAIN_ID.to_string()),
            "llms.txt missing chain id {}",
            registry::CHAIN_ID
        );
    }

    #[test]
    fn parse_addr20_roundtrips_registry_address() {
        let a = parse_addr20(registry::REGISTRY_ADDRESS).expect("valid registry addr");
        assert_eq!(a.len(), 20);
        // Case-insensitive, 0x-optional.
        assert_eq!(parse_addr20("0x00"), None); // wrong length
        assert!(parse_addr20(&"0".repeat(40)).is_some());
    }

    #[test]
    fn parse_interval_units_and_floor() {
        // Suffix units scale to seconds.
        assert_eq!(parse_interval("60s"), Ok(60));
        assert_eq!(parse_interval("5m"), Ok(300));
        assert_eq!(parse_interval("1h"), Ok(3600));
        assert_eq!(parse_interval("2h"), Ok(7200));
        // Bare number = seconds; case + whitespace tolerant.
        assert_eq!(parse_interval(" 90 "), Ok(90));
        assert_eq!(parse_interval("5M"), Ok(300));
        // Below the 60s minimum is rejected (the facet reverts on it).
        assert!(parse_interval("59s").is_err());
        assert!(parse_interval("0m").is_err());
        assert!(parse_interval("30").is_err());
        // Non-numeric / empty / overflow are errors, never a tx.
        assert!(parse_interval("abc").is_err());
        assert!(parse_interval("").is_err());
        assert!(parse_interval("m").is_err());
        assert!(parse_interval(&format!("{}h", u64::MAX)).is_err());
    }

    #[test]
    fn fmt_interval_compact() {
        assert_eq!(fmt_interval(60), "1m");
        assert_eq!(fmt_interval(300), "5m");
        assert_eq!(fmt_interval(3600), "1h");
        assert_eq!(fmt_interval(90), "90s");
        assert_eq!(fmt_interval(5400), "1h30m");
        assert_eq!(fmt_interval(0), "0s");
    }

    #[test]
    fn parse_schedule_args_full_and_defaults() {
        let p = parse_schedule_args(&args(&[
            "oracle", "check", "the", "price", "--every", "5m", "--budget", "1", "--runs", "50",
        ]))
        .unwrap();
        assert_eq!(p.target, "oracle");
        assert_eq!(p.task, "check the price"); // joined multi-word task
        assert_eq!(p.interval_secs, 300);
        assert_eq!(p.budget_wei, 1_000_000_000_000_000_000); // 1 $LH in wei
        assert_eq!(p.max_runs, 50);

        // --runs defaults; flags may precede the task; fractional budget.
        let p = parse_schedule_args(&args(&[
            "bot", "--every", "1h", "--budget", "0.5", "ping",
        ]))
        .unwrap();
        assert_eq!(p.target, "bot");
        assert_eq!(p.task, "ping");
        assert_eq!(p.interval_secs, 3600);
        assert_eq!(p.budget_wei, 500_000_000_000_000_000); // 0.5 $LH
        assert_eq!(p.max_runs, SCHEDULE_DEFAULT_RUNS);
    }

    #[test]
    fn parse_schedule_args_rejects_bad_input() {
        // Missing required flags.
        assert!(parse_schedule_args(&args(&["t", "task"])).is_err());
        assert!(parse_schedule_args(&args(&["t", "task", "--every", "5m"])).is_err());
        // No task (only the target positional).
        assert!(parse_schedule_args(&args(&["t", "--every", "5m", "--budget", "1"])).is_err());
        // Zero / non-numeric budget + bad runs.
        assert!(parse_schedule_args(&args(&["t", "x", "--every", "5m", "--budget", "0"])).is_err());
        assert!(parse_schedule_args(&args(&["t", "x", "--every", "5m", "--budget", "nope"])).is_err());
        assert!(
            parse_schedule_args(&args(&["t", "x", "--every", "5m", "--budget", "1", "--runs", "0"]))
                .is_err()
        );
        // Sub-minute interval bubbles up from parse_interval.
        assert!(parse_schedule_args(&args(&["t", "x", "--every", "10s", "--budget", "1"])).is_err());
    }

    #[test]
    fn format_job_row_contains_key_fields() {
        let job = registry::ScheduledJob {
            owner: "0xowner".into(),
            interval: 300,
            status: 0,
            next_run: 1_000 + 120, // 2m out from `now`
            budget_wei: 1_000_000_000_000_000_000,
            runs_left: 42,
            target_id: 7,
        };
        let row = format_job_row(3, "oracle", &job, "check\nthe price", 1_000);
        assert!(row.contains("#3"));
        assert!(row.contains("oracle"));
        assert!(row.contains("every 5m"));
        assert!(row.contains("next in 2m"));
        assert!(row.contains("runs-left 42"));
        assert!(row.contains("[active]"));
        assert!(row.contains("check the price")); // newline flattened
    }

    #[test]
    fn format_job_row_terminal_and_due() {
        // next_run == 0 (terminal) → em-dash; status label flows through.
        let job = registry::ScheduledJob {
            owner: "0x0".into(),
            interval: 60,
            status: 3,
            next_run: 0,
            budget_wei: 0,
            runs_left: 0,
            target_id: 1,
        };
        let row = format_job_row(1, "bot", &job, "", 5_000);
        assert!(row.contains("next —"));
        assert!(row.contains("[exhausted]"));
        // Due-now: next_run in the past.
        let mut due = job.clone();
        due.status = 0;
        due.next_run = 100;
        let row = format_job_row(2, "bot", &due, "", 5_000);
        assert!(row.contains("next due now"));
    }

    #[test]
    fn parse_ttl_units_and_bounds() {
        // Suffix units scale to seconds.
        assert_eq!(parse_ttl("1h"), Ok(3600));
        assert_eq!(parse_ttl("7d"), Ok(7 * 86_400));
        assert_eq!(parse_ttl("90d"), Ok(90 * 86_400));
        assert_eq!(parse_ttl("90m"), Ok(5400));
        // Bare number = seconds; case + whitespace tolerant.
        assert_eq!(parse_ttl(" 3600 "), Ok(3600));
        assert_eq!(parse_ttl("1H"), Ok(3600));
        assert_eq!(parse_ttl("2D"), Ok(2 * 86_400));
        // Below 1h is rejected; the exact 1h boundary is allowed.
        assert_eq!(parse_ttl("3600s"), Ok(3600));
        assert!(parse_ttl("59m").is_err());
        assert!(parse_ttl("3599").is_err());
        assert!(parse_ttl("0d").is_err());
        // Above 90d is rejected; the exact 90d boundary is allowed.
        assert!(parse_ttl("91d").is_err());
        assert!(parse_ttl("100d").is_err());
        // Non-numeric / empty / overflow are errors, never a tx.
        assert!(parse_ttl("abc").is_err());
        assert!(parse_ttl("").is_err());
        assert!(parse_ttl("d").is_err());
        assert!(parse_ttl(&format!("{}d", u64::MAX)).is_err());
    }

    #[test]
    fn fmt_ttl_compact() {
        assert_eq!(fmt_ttl(3600), "1h");
        assert_eq!(fmt_ttl(7 * 86_400), "7d");
        assert_eq!(fmt_ttl(90 * 86_400), "90d");
        assert_eq!(fmt_ttl(5400), "90m");
        assert_eq!(fmt_ttl(36 * 3600), "36h"); // 1.5d isn't a whole day → hours
    }

    #[test]
    fn gen_invite_code_is_link_safe_prefixed_and_unique() {
        let a = gen_invite_code("100");
        let b = gen_invite_code("100");
        // Shape: inv-<amount>-<10 chars>.
        assert!(a.starts_with("inv-100-"), "got {a}");
        let tail = a.strip_prefix("inv-100-").unwrap();
        assert_eq!(tail.len(), 10);
        // Link-safe: lowercase ASCII alnum only (no padding, no URL-reserved
        // chars), so `?invite=<code>` needs no escaping AND `bytes(code)` is
        // exactly what the facet keccaks.
        assert!(
            a.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-'),
            "non-link-safe char in {a}"
        );
        // CSPRNG tail → two codes (essentially) never collide.
        assert_ne!(a, b);
        // The amount label flows into the prefix verbatim.
        assert!(gen_invite_code("10.5").starts_with("inv-10.5-"));
    }

    #[test]
    fn invite_code_hash_matches_redeem_style_keccak() {
        // The CLI hashes the code the SAME way the facet's acceptInvite(string)
        // recomputes it: keccak256(bytes(code)). Empty-string vector pins it.
        let h = registry::invite_code_hash("");
        let hex: String = h.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
        );
        // A generated code round-trips deterministically.
        let code = gen_invite_code("100");
        assert_eq!(registry::invite_code_hash(&code), registry::invite_code_hash(&code));
    }

    #[test]
    fn parse_invite_create_args_full_and_defaults() {
        // Full: explicit amount + ttl.
        let p = parse_invite_create_args(&args(&["--amount", "100", "--ttl", "30d"])).unwrap();
        assert_eq!(p.amount_label, "100");
        assert_eq!(p.amount_wei, 100 * 1_000_000_000_000_000_000);
        assert_eq!(p.ttl_secs, 30 * 86_400);
        // --ttl defaults to 7d; flags order-independent; fractional amount.
        let p = parse_invite_create_args(&args(&["--amount", "10.5"])).unwrap();
        assert_eq!(p.amount_wei, 10_500_000_000_000_000_000);
        assert_eq!(p.ttl_secs, INVITE_DEFAULT_TTL_SECS);
    }

    #[test]
    fn parse_invite_create_args_rejects_bad_input() {
        // Missing --amount.
        assert!(parse_invite_create_args(&args(&["--ttl", "7d"])).is_err());
        // Zero / non-numeric amount.
        assert!(parse_invite_create_args(&args(&["--amount", "0"])).is_err());
        assert!(parse_invite_create_args(&args(&["--amount", "nope"])).is_err());
        // Out-of-range ttl bubbles up from parse_ttl.
        assert!(parse_invite_create_args(&args(&["--amount", "10", "--ttl", "30m"])).is_err());
        assert!(parse_invite_create_args(&args(&["--amount", "10", "--ttl", "91d"])).is_err());
        // Unknown flag.
        assert!(parse_invite_create_args(&args(&["--amount", "10", "--bogus"])).is_err());
    }

    // ---- bounty arg parsing + row formatting --------------------------------

    #[test]
    fn parse_bounty_post_args_full_and_defaults() {
        // Full: multi-word task joins; explicit reward + ttl.
        let p = parse_bounty_post_args(&args(&[
            "audit", "my", "contract", "--reward", "5", "--ttl", "30d",
        ]))
        .unwrap();
        assert_eq!(p.task, "audit my contract"); // joined positional remainder
        assert_eq!(p.reward_wei, 5 * 1_000_000_000_000_000_000); // 5 $LH in wei
        assert_eq!(p.ttl_secs, 30 * 86_400);

        // --ttl defaults to 7d; flags may precede the task; fractional reward.
        let p = parse_bounty_post_args(&args(&["--reward", "0.5", "fix", "the", "bug"])).unwrap();
        assert_eq!(p.task, "fix the bug");
        assert_eq!(p.reward_wei, 500_000_000_000_000_000); // 0.5 $LH
        assert_eq!(p.ttl_secs, INVITE_DEFAULT_TTL_SECS);
    }

    #[test]
    fn parse_bounty_post_args_rejects_bad_input() {
        // No task.
        assert!(parse_bounty_post_args(&args(&["--reward", "5"])).is_err());
        // Missing --reward.
        assert!(parse_bounty_post_args(&args(&["do", "a", "thing"])).is_err());
        // Zero / non-numeric reward.
        assert!(parse_bounty_post_args(&args(&["task", "--reward", "0"])).is_err());
        assert!(parse_bounty_post_args(&args(&["task", "--reward", "nope"])).is_err());
        // Out-of-range ttl bubbles up from parse_ttl.
        assert!(parse_bounty_post_args(&args(&["task", "--reward", "5", "--ttl", "30m"])).is_err());
        assert!(parse_bounty_post_args(&args(&["task", "--reward", "5", "--ttl", "91d"])).is_err());
    }

    #[test]
    fn parse_bounty_id_accepts_hash_and_bare() {
        assert_eq!(parse_bounty_id("7"), Ok(7));
        assert_eq!(parse_bounty_id("#42"), Ok(42));
        assert_eq!(parse_bounty_id("  #3  "), Ok(3));
        assert!(parse_bounty_id("nope").is_err());
        assert!(parse_bounty_id("").is_err());
    }

    #[test]
    fn parse_guild_id_accepts_hash_and_bare() {
        assert_eq!(parse_guild_id("7"), Ok(7));
        assert_eq!(parse_guild_id("#42"), Ok(42));
        assert_eq!(parse_guild_id("  #3  "), Ok(3));
        assert!(parse_guild_id("nope").is_err());
        assert!(parse_guild_id("").is_err());
    }

    /// The `guild role` arg parses to the on-chain `uint8`; `none` and garbage
    /// are rejected (a role must be member/officer/admin).
    #[test]
    fn guild_role_arg_parses_to_u8() {
        assert_eq!(registry::GuildRole::parse("member").unwrap().as_u8(), 1);
        assert_eq!(registry::GuildRole::parse("Officer").unwrap().as_u8(), 2);
        assert_eq!(registry::GuildRole::parse("  ADMIN ").unwrap().as_u8(), 3);
        assert!(registry::GuildRole::parse("none").is_err());
        assert!(registry::GuildRole::parse("owner").is_err());
    }

    /// `guild invite alice` (a name) classifies as a Name (→ owner lookup);
    /// a raw 0x address classifies as an Address (used as-is). The pure half of
    /// `resolve_member_address` — the async owner lookup needs the chain.
    #[test]
    fn guild_member_arg_classification() {
        use localharness::encoding::{classify_recipient, Recipient};
        // A 40-hex address is used verbatim.
        let addr = "0x1111111111111111111111111111111111111111";
        assert_eq!(
            classify_recipient(addr).unwrap(),
            Recipient::Address(addr.to_string())
        );
        // A bare name is lowercased and resolved to its owner downstream.
        assert_eq!(
            classify_recipient("Alice").unwrap(),
            Recipient::Name("alice".to_string())
        );
        // Empty / zero-address are rejected up front (no member to invite).
        assert!(classify_recipient("").is_err());
        assert!(classify_recipient("0x0000000000000000000000000000000000000000").is_err());
    }

    #[test]
    fn format_bounty_row_contains_key_fields() {
        let b = registry::Bounty {
            poster: "0xposter".into(),
            reward_wei: 5_000_000_000_000_000_000, // 5 $LH
            expiry: 1_000 + 300,                   // 5m out from `now`
            status: 0,
            claimant_token_id: 0,
        };
        let row = format_bounty_row(7, &b, "audit\nthe vault", 1_000);
        assert!(row.contains("#7"));
        assert!(row.contains("reward 5.00 LH"));
        assert!(row.contains("expires in 5m"));
        assert!(row.contains("[open]"));
        assert!(row.contains("audit the vault")); // newline flattened
    }

    #[test]
    fn format_bounty_row_expired_and_no_expiry() {
        let mut b = registry::Bounty {
            poster: "0x0".into(),
            reward_wei: 0,
            expiry: 0, // unset → em-dash
            status: 3, // paid
            claimant_token_id: 9,
        };
        let row = format_bounty_row(1, &b, "", 5_000);
        assert!(row.contains("expires —"));
        assert!(row.contains("[paid]"));
        // An expiry in the past reads EXPIRED.
        b.expiry = 100;
        b.status = 0;
        let row = format_bounty_row(2, &b, "", 5_000);
        assert!(row.contains("expires EXPIRED"));
    }

    #[test]
    fn parse_proposal_id_accepts_hash_and_bare() {
        assert_eq!(parse_proposal_id("7"), Ok(7));
        assert_eq!(parse_proposal_id("#42"), Ok(42));
        assert_eq!(parse_proposal_id("  #3  "), Ok(3));
        assert!(parse_proposal_id("nope").is_err());
        assert!(parse_proposal_id("").is_err());
    }

    /// The ballot arg parses for/against (and common synonyms), case-insensitive;
    /// garbage is rejected. This bool is the on-chain `support` flag.
    #[test]
    fn parse_vote_support_maps_for_against() {
        for raw in ["for", "FOR", "yes", " Y ", "aye", "support"] {
            assert_eq!(parse_vote_support(raw), Ok(true), "{raw}");
        }
        for raw in ["against", "AGAINST", "no", " N ", "nay", "oppose"] {
            assert_eq!(parse_vote_support(raw), Ok(false), "{raw}");
        }
        assert!(parse_vote_support("maybe").is_err());
        assert!(parse_vote_support("").is_err());
    }

    /// The voting period clamps to VotingFacet's 1h…30d (the facet would revert
    /// `BadVotingPeriod` outside that). 1h passes; a 90d (valid for invites) is
    /// rejected for a vote; sub-1h is rejected by the shared `parse_ttl`.
    #[test]
    fn parse_voting_period_bounds_to_30d() {
        assert_eq!(parse_voting_period("1h"), Ok(3600));
        assert_eq!(parse_voting_period("7d"), Ok(7 * 86_400));
        assert_eq!(parse_voting_period("30d"), Ok(30 * 86_400));
        assert!(parse_voting_period("31d").is_err()); // over MAX_VOTING_PERIOD
        assert!(parse_voting_period("90d").is_err()); // invite-valid, vote-invalid
        assert!(parse_voting_period("30m").is_err()); // under the 1h minimum
    }

    /// `vote propose` parsing: required positionals (guildId/to/amount), an
    /// optional `--period`, and a multi-word memo from the positional remainder.
    /// `--period` may sit anywhere; default period is 7d (within 1h…30d).
    #[test]
    fn parse_vote_propose_args_positionals_and_period() {
        // guildId + to + amount + multi-word memo, no --period → default 7d.
        let p = parse_vote_propose_args(&args(&["5", "alice", "2.5", "q3", "grant"])).unwrap();
        assert_eq!(p.guild_id, 5);
        assert_eq!(p.to, "alice");
        assert_eq!(p.amount_wei, 2_500_000_000_000_000_000); // 2.5 $LH
        assert_eq!(p.period_secs, INVITE_DEFAULT_TTL_SECS);
        assert_eq!(p.memo, "q3 grant");

        // --period anywhere; memo can be empty.
        let p = parse_vote_propose_args(&args(&["3", "--period", "1h", "0x1111111111111111111111111111111111111111", "1"])).unwrap();
        assert_eq!(p.guild_id, 3);
        assert_eq!(p.to, "0x1111111111111111111111111111111111111111");
        assert_eq!(p.amount_wei, 1_000_000_000_000_000_000);
        assert_eq!(p.period_secs, 3600);
        assert_eq!(p.memo, "");

        // Missing positionals / bad amount / out-of-range period are errors.
        assert!(parse_vote_propose_args(&args(&["5", "alice"])).is_err()); // no amount
        assert!(parse_vote_propose_args(&args(&["5", "alice", "0"])).is_err()); // zero amount
        assert!(parse_vote_propose_args(&args(&["5", "alice", "1", "--period", "90d"])).is_err());
    }

    /// `format_proposal_row` shows id, status, tally, deadline (relative), the
    /// passing flag, and a flattened memo snippet.
    #[test]
    fn format_proposal_row_contains_key_fields() {
        let p = registry::Proposal {
            guild_id: 5,
            proposer: "0xproposer".into(),
            to: "0xrecipient".into(),
            amount: 2_000_000_000_000_000_000,
            deadline: 1_000 + 3600, // 1h out from `now`
            status: 0,              // active
            for_votes: 2,
            against_votes: 1,
        };
        let t = registry::Tally { for_votes: 2, against_votes: 1, quorum: 2, votes_cast: 3, passing: true };
        let row = format_proposal_row(9, &p, &t, "fund\nthe audit", 1_000);
        assert!(row.contains("#9"));
        assert!(row.contains("[active]"));
        assert!(row.contains("for 2 / against 1"));
        assert!(row.contains("quorum 2"));
        assert!(row.contains("closes in 1h"));
        assert!(row.contains("(passing)"));
        assert!(row.contains("fund the audit")); // newline flattened
    }

    /// A CLOSED (deadline past) proposal reads CLOSED + the not-passing label
    /// when the tally hasn't met quorum/majority.
    #[test]
    fn format_proposal_row_closed_and_not_passing() {
        let p = registry::Proposal {
            guild_id: 1,
            proposer: "0x0".into(),
            to: "0x0".into(),
            amount: 0,
            deadline: 100, // in the past
            status: 2,     // failed
            for_votes: 0,
            against_votes: 0,
        };
        let t = registry::Tally { for_votes: 0, against_votes: 0, quorum: 1, votes_cast: 0, passing: false };
        let row = format_proposal_row(2, &p, &t, "", 5_000);
        assert!(row.contains("[failed]"));
        assert!(row.contains("closes CLOSED"));
        assert!(row.contains("(not passing)"));
    }
}
