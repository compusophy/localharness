//! Single source of truth for the drift-prone FACTS that are mirrored across
//! the managed docs (`web/skill.md`, `web/llms.txt`). The top-level `README.md`
//! is hand-written and minimal — deliberately not generated.
//!
//! These facts — chain addresses, the crate version, `$LH` pricing, the agent
//! tool list, the CLI command list — used to be hand-copied into each doc and
//! went stale silently. This module DERIVES them from canonical code (the
//! `registry::chain` consts, `env!("CARGO_PKG_VERSION")`) or holds the one
//! canonical copy (pricing, tool/CLI lists) and renders the exact text block
//! each doc embeds between `<!-- GEN:<key> -->` markers.
//!
//! The pipeline:
//! 1. Facts live HERE (this module).
//! 2. `cargo run --bin gen-docs` fills every `GEN` block from [`render`].
//! 3. A `cargo test` drift-test ([`tests::no_doc_drift`]) fails if any block
//!    in the committed docs differs from the freshly-rendered block.
//! 4. `scripts/release.{sh,ps1}` run `gen-docs -- --check` in pre-flight, so a
//!    version bump CANNOT ship with stale docs.
//!
//! NEVER hand-edit text inside a `GEN` block — the generator owns it and the
//! drift gate will reject your edit. Change the FACT here, then regenerate.
//!
//! Gated on `feature = "wallet"` because it references `crate::registry::chain`
//! (the registry module is wallet-gated). `gen-docs` and the drift-test run
//! under `wallet`; see `docs/SOP-doc-integrity.md`.

use crate::registry::chain::{self, ChainConfig};

/// The crate version, from Cargo at compile time. The single source for every
/// `version` block in the managed docs.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

// ---------------------------------------------------------------------------
// Pricing
// ---------------------------------------------------------------------------

/// Doc copy for `$LH` pricing. The RUNTIME source of truth is the proxy's
/// per-model table (`proxy/api/_prices.ts`); this is the DOC source. Keep them
/// in sync by hand — both are deliberately small, env-overridable tables.
//
// SoT mirror: proxy/api/_prices.ts — keep in sync.
pub const PRICING_SUMMARY: &str =
    "1 $LH per message on the default model; premium models are tiered \
     (Haiku/Sonnet/Opus = 1 / 5 / 20 $LH; GPT nano/mini = 1, gpt-5.1 = 5, \
     gpt-5-pro = 20). Fiat on-ramp mints on the GROSS charged amount at \
     $1 = 100 $LH. $LH is a flat usage credit decoupled from the dollar, \
     NOT a stablecoin.";

// ---------------------------------------------------------------------------
// Agent tool list + CLI command list (single-sourced HERE)
// ---------------------------------------------------------------------------

/// The canonical agent tool surface, grouped by family. Single-sourced here so
/// every doc renders the SAME list. (A future enhancement can derive these from
/// the builtin/platform registries; for now single-sourcing the LIST is the
/// win.) Each entry is `(group, [tool, ...])`.
pub const AGENT_TOOLS: &[(&str, &[&str])] = &[
    (
        "Filesystem (OPFS sandbox)",
        &[
            "list_directory",
            "view_file",
            "find_file",
            "search_directory",
            "create_file",
            "edit_file",
            "delete_file",
            "rename_file",
        ],
    ),
    (
        "Platform / subdomains",
        &[
            "create_subdomain",
            "batch_create_subdomains",
            "create_and_publish_app",
            "publish_public_face",
            "list_subdomains",
            "release_subdomain",
            "bulk_release_subdomains",
        ],
    ),
    (
        "Agents / orchestration",
        &[
            "call_agent",
            "discover_agents",
            "consult_model",
            "start_subagent",
            "spawn_recursive_subagent",
            "schedule_task",
        ],
    ),
    (
        "Payments / economy",
        &[
            "send_lh",
            "batch_send_lh",
            "check_balances",
            "query_balance",
            "post_bounty",
            "claim_bounty",
            "submit_result",
            "accept_result",
            "discover_bounties",
            "create_guild",
            "invite_to_guild",
            "fund_guild",
            "spend_treasury",
            "propose_measure",
            "cast_vote",
            "execute_proposal",
            "list_proposals",
        ],
    ),
    (
        "Self-edit / learning",
        &[
            "set_persona",
            "record_lesson",
            "consolidate_lessons",
            "set_lessons",
            "create_skill",
            "list_skills",
            "delete_skill",
        ],
    ),
    (
        "Build / run",
        &[
            "compile_rustlite",
            "run_cartridge",
            "render_html",
            "run_wasm_cli",
            "execute_script",
            "generate_image",
        ],
    ),
    (
        "Multi-chain reads",
        &["evm_chains", "evm_balance", "resolve_ens", "evm_call"],
    ),
    (
        "Grounding / I/O",
        &[
            "web_fetch",
            "notify",
            "list_notifications",
            "clear_notifications",
            "submit_feedback",
            "read_self_docs",
            "current_time",
            "ask_question",
            "finish",
            "dwell",
            "clear_context",
            "compact_context",
        ],
    ),
];

/// The canonical `localharness` CLI command list, one line each. Single-sourced
/// here so the docs never drift from the binary's dispatch surface.
/// `(command, one-line description)`.
pub const CLI_COMMANDS: &[(&str, &str)] = &[
    ("create", "claim <name>.localharness.xyz (sponsored); scaffolds ./app.rl"),
    ("onboard", "get a brand-new identity its first $LH via an invite (the terminal onboarding entry)"),
    ("compile", "compile-check a rustlite cartridge locally (no on-chain write)"),
    ("sh", "run a bashlite script: fs + lh-* commands + `run` composition; value moves (lh-send) need --confirm"),
    ("publish", "publish a public face (.rl app or .html page; auto-claims if needed)"),
    ("face", "set the public face: directory | app | html"),
    ("persona", "publish the agent's on-chain system prompt"),
    ("price", "advertise a per-call $LH price (or `clear`)"),
    ("call", "headless agent turn AS a target via the proxy (no key, no tab)"),
    ("discover", "find agents by capability (read-only, free)"),
    ("whoami", "profile of a name: owner, wallet, persona, advertised price"),
    ("status", "read-only economy dashboard (identity, balances, jobs, …)"),
    ("list", "the subdomains you own"),
    ("models", "list the valid --model ids"),
    ("redeem", "mint $LH from a one-time bootstrap code"),
    ("send", "transfer $LH to a 0x address or a name's owner"),
    ("buy", "buy $LH with a card (fiat on-ramp)"),
    ("onramp", "fund $LH with USDC.e via the Tempo MPP on-ramp (autonomous, no card)"),
    ("credits", "show meter + wallet balances"),
    ("topup", "deposit wallet $LH into the per-call meter"),
    ("invite", "escrow $LH behind a refundable bearer onboarding code"),
    ("link", "adopt a funded web wallet's seed into a terminal identity (QR seed-adoption)"),
    ("bounty", "post/list/claim/submit/accept paid work (BountyFacet)"),
    ("colony", "run one autonomous post→work→judge→pay economy cycle"),
    ("reputation", "attestation-based on-chain agent trust (alias: rep)"),
    ("guild", "durable on-chain orgs with a pooled treasury"),
    ("party", "ad-hoc squads with an escrowed, pre-agreed split"),
    ("validation", "ERC-8004 validation staking on a workRef"),
    ("vote", "guild DAO governance over the treasury"),
    ("tba", "act through a token-bound account (show/deploy/exec)"),
    ("room", "encrypted on-chain shared key/value state (SessionRoomFacet)"),
    ("schedule", "escrow $LH, run an agent on an interval, no tab"),
    ("goal", "ralph-style GOAL loop: self-cancels + refunds when done"),
    ("jobs", "list your scheduled jobs"),
    ("unschedule", "cancel a job; refunds its remaining budget"),
    ("keeper", "one decentralized-keeper tick: poke all due jobs"),
    ("notify", "Web Push to your device (or --to <agent>)"),
    ("threads", "list your saved per-(caller,target) conversations"),
    ("forget", "drop saved conversation threads"),
    ("feedback", "submit on-chain feedback, or read all (no text)"),
    ("facet", "SolidityLite: deploy/cut your own on-chain facets"),
    ("mcp", "serve a call_agent tool over stdio MCP"),
    ("mcp-call", "true x402 MCP-over-HTTP call to a target agent"),
    ("release", "DESTRUCTIVE: burn an owned name (--confirm <name>)"),
];

// ---------------------------------------------------------------------------
// Marker scheme
// ---------------------------------------------------------------------------

/// The opening marker for a generated block: `<!-- GEN:<key> -->`.
/// HTML comments are inert in markdown (`skill.md`, `README.md`) and read as
/// clear, non-rendering delimiters in the plain-text `llms.txt`, so ONE marker
/// style covers all three managed docs.
pub fn open_marker(key: &str) -> String {
    format!("<!-- GEN:{key} -->")
}

/// The closing marker for a generated block: `<!-- /GEN:<key> -->`.
pub fn close_marker(key: &str) -> String {
    format!("<!-- /GEN:{key} -->")
}

/// Every generated-block key the docs may embed. Stable identifiers; a doc
/// includes whichever it needs.
pub const KEYS: &[&str] = &["version", "chain", "pricing", "tools", "cli"];

// ---------------------------------------------------------------------------
// Render helpers
// ---------------------------------------------------------------------------

/// Render the markdown/text block for a given `GEN` key. The INNER content of
/// the block (without the markers themselves). Returns `None` for an unknown
/// key.
pub fn render(key: &str) -> Option<String> {
    match key {
        "version" => Some(render_version()),
        "chain" => Some(render_chain()),
        "pricing" => Some(render_pricing()),
        "tools" => Some(render_tools()),
        "cli" => Some(render_cli()),
        _ => None,
    }
}

fn render_version() -> String {
    format!(
        "**version:** {} (the crate version; the deployed web bundle matches \
         crates.io when current)",
        version()
    )
}

/// Render ONE chain's facts as a table row body. Emitted EXPLICITLY for both
/// chains so the docs state both correctly regardless of which feature the
/// reader's build used (never relies on `ACTIVE`, which flips by feature).
fn chain_row(role: &str, c: &ChainConfig) -> String {
    format!(
        "| {role} | {} | {} | `{}` | `{}` | `{}` |",
        c.name, c.chain_id, c.rpc_url, c.diamond, c.lh_token
    )
}

fn render_chain() -> String {
    let mut s = String::new();
    s.push_str(
        "Both the **live web platform** at `localharness.xyz` and the \
         **`localharness` CLI** run on **Tempo mainnet** (chain 4217) by default. \
         **Tempo Moderato** (chain 42431) is an opt-in, free-registration DEV \
         sandbox — the CLI selects it with `LH_CHAIN=testnet` (or `--dev`); an \
         unrecognized `LH_CHAIN` is an error, never a silent fallback. The web \
         bundle is pinned to mainnet at build (`--features mainnet`).\n\n",
    );
    s.push_str("| Role | Network | chain_id | RPC | Diamond | `$LH` token |\n");
    s.push_str("|---|---|---|---|---|---|\n");
    s.push_str(&chain_row("live platform + CLI default (mainnet)", &chain::MAINNET));
    s.push('\n');
    s.push_str(&chain_row("dev sandbox (opt-in: --dev)", &chain::MODERATO));
    s.push('\n');
    s.push_str(&format!(
        "\nSponsor fee token (NOT `$LH`): mainnet `{}`, testnet `{}`. The \
         diamond is the only durable address — per-facet addresses churn on \
         re-cut; query the live set via DiamondLoupeFacet.",
        chain::MAINNET.fee_token,
        chain::MODERATO.fee_token,
    ));
    s
}

fn render_pricing() -> String {
    PRICING_SUMMARY.to_string()
}

fn render_tools() -> String {
    let mut s = String::new();
    for (group, tools) in AGENT_TOOLS {
        s.push_str(&format!("- **{group}:** {}\n", tools.join(", ")));
    }
    // Trim the trailing newline so the block is byte-stable.
    s.pop();
    s
}

fn render_cli() -> String {
    let mut s = String::new();
    for (cmd, desc) in CLI_COMMANDS {
        s.push_str(&format!("- `localharness {cmd}` — {desc}\n"));
    }
    s.pop();
    s
}

// ---------------------------------------------------------------------------
// GEN-block fill / extract (the generator + drift core)
// ---------------------------------------------------------------------------

/// The result of filling/checking one document's GEN blocks.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct FillReport {
    /// Keys whose block content CHANGED (was stale, now fresh).
    pub changed: Vec<String>,
    /// Keys found and already fresh.
    pub fresh: Vec<String>,
}

impl FillReport {
    /// True if any block was stale (i.e. the doc differed from the manifest).
    pub fn drifted(&self) -> bool {
        !self.changed.is_empty()
    }
}

/// Major.minor of the crate version — the form a Cargo dependency pin uses
/// (`localharness = "0.50"`, which resolves to the latest 0.50.x).
fn major_minor() -> String {
    let v = version();
    let mut it = v.split('.');
    match (it.next(), it.next()) {
        (Some(a), Some(b)) => format!("{a}.{b}"),
        _ => v.to_string(),
    }
}

/// Rewrite every `localharness = "X.Y…"` dependency pin in `doc` to the current
/// crate major.minor. These pins live inside ```toml fences, where the
/// HTML-comment GEN markers can't reach — yet a stale pin is the most
/// investor-visible drift there is (it tells people to depend on an OLD
/// version). So [`fill`] owns them too: `gen-docs` rewrites them and
/// `gen-docs --check` (the release pre-flight gate) catches the drift, exactly
/// like a GEN block.
fn rewrite_dep_pins(doc: &str) -> String {
    let target = major_minor();
    const NEEDLE: &str = "localharness = \"";
    let mut out = String::with_capacity(doc.len());
    let mut rest = doc;
    while let Some(i) = rest.find(NEEDLE) {
        out.push_str(&rest[..i + NEEDLE.len()]);
        let after = &rest[i + NEEDLE.len()..];
        match after.find('"') {
            Some(end) => {
                out.push_str(&target);
                rest = &after[end..]; // resume at the closing quote
            }
            None => rest = after,
        }
    }
    out.push_str(rest);
    out
}

/// Replace the INNER content of every `<!-- GEN:key -->...<!-- /GEN:key -->`
/// block in `doc` with the freshly-rendered block from the manifest, AND rewrite
/// any `localharness = "X.Y"` dependency pin to the current version. Returns the
/// rewritten document plus a report of which blocks changed (a rewritten pin is
/// reported as the synthetic key `dep-version`). IDEMPOTENT: running it on its
/// own output yields no further change.
///
/// An unknown key inside a marker pair is left untouched (and not reported) so
/// a doc can carry markers this version doesn't know about without data loss.
pub fn fill(doc: &str) -> (String, FillReport) {
    let mut out = String::with_capacity(doc.len() + 256);
    let mut report = FillReport::default();
    let mut rest = doc;

    const OPEN_PREFIX: &str = "<!-- GEN:";

    loop {
        // Find the next opening marker `<!-- GEN:`.
        let Some(open_abs) = rest.find(OPEN_PREFIX) else {
            out.push_str(rest);
            break;
        };

        // Parse the key: between `<!-- GEN:` and ` -->`.
        let after_prefix = &rest[open_abs + OPEN_PREFIX.len()..];
        let Some(key_end) = after_prefix.find(" -->") else {
            // Malformed opener (no ` -->`). Copy through the opener token and
            // keep scanning the remainder — one bad marker must NOT abort the
            // rest of the file.
            let consumed = open_abs + OPEN_PREFIX.len();
            out.push_str(&rest[..consumed]);
            rest = &rest[consumed..];
            continue;
        };
        let key = after_prefix[..key_end].trim().to_string();
        let close_marker = close_marker(&key);

        // Locate the matching close marker AFTER the open marker.
        let Some(close_rel) = rest[open_abs..].find(&close_marker) else {
            // No closing marker for this key. Copy through the opener token and
            // keep scanning — don't swallow the rest of the document.
            let consumed = open_abs + OPEN_PREFIX.len();
            out.push_str(&rest[..consumed]);
            rest = &rest[consumed..];
            continue;
        };
        let close_abs = open_abs + close_rel;
        let block_end = close_abs + close_marker.len();

        // Copy text before the block verbatim.
        out.push_str(&rest[..open_abs]);

        match render(&key) {
            Some(fresh) => {
                // Reconstruct: open marker + \n + fresh + \n + close marker.
                let new_block = format!("{}\n{fresh}\n{close_marker}", open_marker(&key));
                // The OLD block as committed (markers inclusive).
                let old_block = &rest[open_abs..block_end];
                if old_block == new_block {
                    report.fresh.push(key);
                } else {
                    report.changed.push(key);
                }
                out.push_str(&new_block);
            }
            None => {
                // Unknown key — leave the whole block untouched (forward-compat).
                out.push_str(&rest[open_abs..block_end]);
            }
        }
        rest = &rest[block_end..];
    }

    // Own the Cargo dependency pin(s) too — outside any GEN block (they sit in
    // ```toml fences). A rewritten pin is reported as `dep-version` so the drift
    // gate flags it just like a stale GEN block.
    let pinned = rewrite_dep_pins(&out);
    if pinned != out {
        report.changed.push("dep-version".to_string());
    }
    (pinned, report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// The managed (generated) docs, relative to the crate root. The top-level
    /// `README.md` is intentionally excluded — it is hand-written and minimal.
    const MANAGED_DOCS: &[&str] = &["web/skill.md", "web/llms.txt"];

    fn read_doc(rel: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{} must exist and be readable: {e}", path.display()))
    }

    /// THE drift gate (runs under `cargo test --lib --features wallet`). Every
    /// GEN block in every managed doc must EQUAL the freshly-rendered block. A
    /// failure means a fact changed in this module (or the doc) without the
    /// generator being run — the fix is one command.
    #[test]
    fn no_doc_drift() {
        let mut stale = Vec::new();
        for rel in MANAGED_DOCS {
            let doc = read_doc(rel);
            let (_filled, report) = fill(&doc);
            for key in &report.changed {
                stale.push(format!("  {rel}: GEN:{key}"));
            }
        }
        assert!(
            stale.is_empty(),
            "doc drift: the following GEN blocks are stale —\n{}\n\nrun `cargo run --bin gen-docs` to regenerate.",
            stale.join("\n")
        );
    }

    /// No managed doc may carry a STALE dependency pin (`localharness = "X.Y"`)
    /// — `fill` owns those now, so a drifted pin shows up as `dep-version` in the
    /// change report. This is the exact class that shipped a stale `0.47` past
    /// every other gate.
    #[test]
    fn no_stale_dep_pin() {
        let mm = major_minor();
        for rel in MANAGED_DOCS {
            let doc = read_doc(rel);
            for line in doc.lines().filter(|l| l.contains("localharness = \"")) {
                assert!(
                    line.contains(&format!("localharness = \"{mm}\"")),
                    "{rel}: stale dependency pin (`{}`) — should be `localharness = \"{mm}\"`; run gen-docs.",
                    line.trim()
                );
            }
        }
    }

    /// The AGENT-FACING default prompt must never HARDCODE the crate version —
    /// it derives facts from `env!`/self-docs (GEN-managed). A baked-in literal
    /// would go stale on the next bump and feed every agent a wrong fact. (The
    /// doc-integrity umbrella covers the system instructions too, not just the
    /// markdown docs.)
    #[test]
    fn system_prompt_has_no_hardcoded_version() {
        let v = version();
        for rel in ["src/app/chat/prompt.rs", "src/app/self_docs.rs"] {
            let src = read_doc(rel);
            assert!(
                !src.contains(v),
                "{rel} hardcodes the crate version {v:?} — derive it (env!/self_docs) so the agent's prompt can't go stale.",
            );
        }
    }

    /// Every managed doc must actually CONTAIN at least one GEN block (else the
    /// generator silently owns nothing and drift can never be caught).
    #[test]
    fn every_managed_doc_has_gen_blocks() {
        for rel in MANAGED_DOCS {
            let doc = read_doc(rel);
            assert!(
                doc.contains("<!-- GEN:"),
                "{rel} has no GEN blocks — the doc-integrity system can't manage its facts"
            );
        }
    }

    /// `fill` is idempotent: filling its own output yields no further change.
    #[test]
    fn fill_is_idempotent() {
        let sample = "intro\n<!-- GEN:version -->\nSTALE\n<!-- /GEN:version -->\noutro\n";
        let (once, r1) = fill(sample);
        assert!(r1.drifted(), "the STALE block should have been rewritten");
        let (twice, r2) = fill(&once);
        assert_eq!(once, twice, "second fill must be a no-op");
        assert!(!r2.drifted(), "second fill must report no drift");
    }

    /// An unknown GEN key is left untouched (forward-compat).
    #[test]
    fn unknown_key_untouched() {
        let sample = "<!-- GEN:bogus -->\nkeep me\n<!-- /GEN:bogus -->";
        let (out, report) = fill(sample);
        assert_eq!(out, sample);
        assert!(report.changed.is_empty() && report.fresh.is_empty());
    }

    /// The chain block is derived from `chain.rs` and must carry BOTH chains'
    /// real values (never `ACTIVE`-flipped), so the doc is correct on any build.
    #[test]
    fn chain_block_carries_both_chains() {
        let block = render_chain();
        assert!(block.contains("4217") && block.contains("42431"));
        assert!(block.contains(chain::MAINNET.diamond));
        assert!(block.contains(chain::MODERATO.diamond));
        assert!(block.contains(chain::MAINNET.lh_token));
        assert!(block.contains(chain::MODERATO.lh_token));
        assert!(block.contains("rpc.tempo.xyz"));
        assert!(block.contains("rpc.moderato.tempo.xyz"));
    }

    /// The version block must carry the live Cargo version.
    #[test]
    fn version_block_matches_cargo() {
        assert!(render_version().contains(env!("CARGO_PKG_VERSION")));
    }

    /// Every declared KEY renders.
    #[test]
    fn all_keys_render() {
        for k in KEYS {
            assert!(render(k).is_some(), "KEY {k} has no renderer");
        }
    }
}
