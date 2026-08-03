//! Relay/signer allowlist parity guard (design/relay-allowlist-gaps.md "Root
//! cause / prevention"): every diamond write a registry `*_sponsored` fn can
//! submit must appear in BOTH `src/app/signer.rs diamond_signable_selectors()`
//! and `proxy/api/sponsor.ts DIAMOND_WRITE_SIGS`, or on mainnet the relay 403s
//! `LH_RELAY_SELECTOR` / the signer refuses — the drift that stranded
//! party/validation escrow for weeks. Perfect mechanical selector extraction
//! isn't reachable from source text (calldata goes through per-facet encoder
//! fns), so `SPONSORED` PINS the fn→signature table; the enumeration guard
//! fails on any new/removed `*_sponsored` fn in `src/registry/`, forcing the
//! pin — and therefore both allowlists — to be updated deliberately.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// What a registry `*_sponsored` fn submits.
enum Submits {
    /// Diamond calls with these EXACT ABI signature strings → must be in both allowlists.
    Diamond(&'static [&'static str]),
    /// No diamond-selector call of its own ($LH-token target, TBA-account execute,
    /// or a generic calls/CREATE primitive) — gated by other policy, not these lists.
    NonDiamond,
    /// A diamond selector DELIBERATELY absent from both allowlists (role-gated;
    /// both files document the absence) — asserted to STAY absent.
    AbsentByDesign(&'static str),
}
use Submits::*;

/// fn name → the diamond signature(s) it can submit. `*_bridged` / cost-bridge
/// variants include `withdrawCredits(uint256)` (the meter→wallet bridge leg
/// prepended into the same tx). The `approve(address,uint256)` escrow leg
/// targets the $LH TOKEN, not the diamond, so it is not listed here.
const SPONSORED: &[(&str, Submits)] = &[
    // bounty.rs
    ("post_bounty_sponsored", Diamond(&["postBounty(bytes,uint128,uint64)"])),
    ("post_bounty_sponsored_bridged", Diamond(&["postBounty(bytes,uint128,uint64)", "withdrawCredits(uint256)"])),
    ("claim_bounty_sponsored", Diamond(&["claimBounty(uint256,uint256)"])),
    ("submit_result_sponsored", Diamond(&["submitResult(uint256,bytes)"])),
    ("accept_result_sponsored", Diamond(&["acceptResult(uint256)"])),
    ("cancel_bounty_sponsored", Diamond(&["cancelBounty(uint256)"])),
    ("reclaim_expired_sponsored", Diamond(&["reclaimExpired(uint256)"])),
    // credits.rs
    ("redeem_sponsored", Diamond(&["redeem(string)"])),
    ("deposit_credits_sponsored", Diamond(&["depositCredits(uint256)"])),
    ("withdraw_credits_sponsored", Diamond(&["withdrawCredits(uint256)"])),
    ("approve_lh_sponsored", NonDiamond),  // $LH token approve
    ("transfer_lh_sponsored", NonDiamond), // $LH token transfer
    // guild.rs (createGuild's cost path bridges like claim_and_maybe_set_main)
    ("create_guild_sponsored", Diamond(&["createGuild(string)", "withdrawCredits(uint256)"])),
    ("invite_to_guild_sponsored", Diamond(&["inviteToGuild(uint256,address)"])),
    ("accept_guild_invite_sponsored", Diamond(&["acceptGuildInvite(uint256)"])),
    ("leave_guild_sponsored", Diamond(&["leaveGuild(uint256)"])),
    ("set_role_sponsored", Diamond(&["setRole(uint256,address,uint8)"])),
    ("fund_guild_sponsored", Diamond(&["fundGuild(uint256,uint256)"])),
    ("fund_guild_sponsored_bridged", Diamond(&["fundGuild(uint256,uint256)", "withdrawCredits(uint256)"])),
    ("spend_treasury_sponsored", Diamond(&["spendTreasury(uint256,address,uint256,bytes)"])),
    // invite.rs
    ("create_invite_sponsored", Diamond(&["createInvite(bytes32,uint256,uint64)"])),
    ("create_invite_sponsored_bridged", Diamond(&["createInvite(bytes32,uint256,uint64)", "withdrawCredits(uint256)"])),
    ("accept_invite_sponsored", Diamond(&["acceptInvite(string)"])),
    ("reclaim_invite_sponsored", Diamond(&["reclaimInvite(bytes32)"])),
    // mint_gate.rs — issuer-signed fiat mint; must never be relay/signer-listed
    ("mint_from_fiat_sponsored", AbsentByDesign("mintFromFiat(address,uint256,bytes32,uint256,bytes)")),
    // party.rs
    ("form_party_sponsored", Diamond(&["formParty(uint256[],uint16[],uint64)"])),
    ("join_party_sponsored", Diamond(&["joinParty(uint256)"])),
    ("fund_party_sponsored", Diamond(&["fundParty(uint256,uint128)"])),
    ("fund_party_sponsored_bridged", Diamond(&["fundParty(uint256,uint128)", "withdrawCredits(uint256)"])),
    ("complete_party_sponsored", Diamond(&["completeParty(uint256)"])),
    ("disband_party_sponsored", Diamond(&["disbandParty(uint256)"])),
    // reputation.rs
    ("attest_sponsored", Diamond(&["attest(uint256,uint8,bytes32)"])),
    // sessionroom.rs
    ("create_room_sponsored", Diamond(&["createRoom()"])),
    ("room_add_member_sponsored", Diamond(&["roomAddMember(uint256,address)"])),
    ("append_op_sponsored", Diamond(&["appendOp(uint256,bytes)"])),
    ("clear_room_sponsored", Diamond(&["clearRoom(uint256)"])),
    // signaling.rs
    ("announce_sponsored", Diamond(&["announce(bytes32,address,address,bytes,bytes)"])),
    ("post_signal_sponsored", Diamond(&["postSignal(address,bytes)"])),
    // subscribe.rs
    ("subscribe_sponsored", Diamond(&["subscribe(uint256)"])),
    ("unsubscribe_sponsored", Diamond(&["unsubscribe(uint256)"])),
    // tba.rs (execute legs target the TBA ACCOUNT address, not the diamond)
    ("register_main_sponsored", Diamond(&["registerMain(uint256)"])),
    ("tba_execute_batch_sponsored", Diamond(&["createTokenBoundAccount(uint256)"])),
    ("release_name_sponsored", Diamond(&["releaseName(uint256)"])),
    ("tba_execute_call_sponsored", NonDiamond),
    ("create_token_bound_account_sponsored", Diamond(&["createTokenBoundAccount(uint256)"])),
    ("tba_send_lh_sponsored", NonDiamond),
    ("claim_and_maybe_set_main_sponsored", Diamond(&["register(string)", "withdrawCredits(uint256)"])),
    // tithe.rs
    ("collect_tithe_sponsored", Diamond(&["collectTithe(address)"])),
    // tx.rs — the generic submit/CREATE primitives (callers own the calldata)
    ("submit_tempo_sponsored", NonDiamond),
    ("create_sponsored", NonDiamond),
    // validation.rs
    ("stake_validation_sponsored", Diamond(&["stakeValidation(bytes32,uint256,bool,uint256)"])),
    ("challenge_validation_sponsored", Diamond(&["challengeValidation(uint256)"])),
    ("resolve_validation_sponsored", Diamond(&["resolveValidation(uint256,bool)"])),
    ("reclaim_stake_sponsored", Diamond(&["reclaimStake(uint256)"])),
    ("reclaim_unresolved_sponsored", Diamond(&["reclaimUnresolved(uint256)"])),
    // voting.rs
    ("propose_sponsored", Diamond(&["propose(uint256,address,uint256,bytes,uint64)"])),
    ("vote_sponsored", Diamond(&["vote(uint256,bool)"])),
    ("execute_proposal_sponsored", Diamond(&["execute(uint256)"])),
    // weighted_voting.rs
    ("set_shares_sponsored", Diamond(&["setShares(uint256,address,uint256)"])),
    ("propose_weighted_sponsored", Diamond(&["proposeWeighted(uint256,address,uint256,uint256,string)"])),
    ("vote_weighted_sponsored", Diamond(&["voteWeighted(uint256,bool)"])),
    ("execute_weighted_proposal_sponsored", Diamond(&["executeWeighted(uint256)"])),
    // x402.rs
    ("settle_x402_sponsored", Diamond(&["settle(address,address,uint256,uint256,uint256,bytes32,bytes)"])),
];

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Every `src/registry/*.rs` source, `//` line comments stripped.
fn registry_sources(root: &Path) -> Vec<(PathBuf, String)> {
    let dir = root.join("src").join("registry");
    let mut out = Vec::new();
    for entry in fs::read_dir(&dir).unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display())) {
        let path = entry.expect("dir entry").path();
        if path.extension().is_some_and(|e| e == "rs") {
            let text = strip_line_comments(&read(&path));
            out.push((path, text));
        }
    }
    assert!(!out.is_empty(), "no .rs files under src/registry");
    out
}

/// Drop everything from `//` to end-of-line (good enough for these sources —
/// no sig literal contains `//`).
fn strip_line_comments(src: &str) -> String {
    src.lines()
        .map(|l| l.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Mechanically enumerate fn identifiers containing `_sponsored` — the
/// count/name guard that fires when a new sponsored write appears.
fn sponsored_fns(sources: &[(PathBuf, String)]) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for (_, text) in sources {
        let bytes = text.as_bytes();
        let mut i = 0;
        while let Some(pos) = text[i..].find("fn ") {
            let at = i + pos;
            i = at + 3;
            // require a word boundary before `fn`
            if at > 0 && (bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_') {
                continue;
            }
            let ident: String = text[i..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if ident.contains("_sponsored") {
                names.insert(ident);
            }
        }
    }
    names
}

/// String literals quoted by `quote` in `region`, kept only when shaped like an
/// ABI signature (`name(args)`).
fn quoted_sigs(region: &str, quote: char) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = region;
    while let Some(start) = rest.find(quote) {
        let after = &rest[start + 1..];
        let Some(len) = after.find(quote) else { break };
        let lit = &after[..len];
        if lit.contains('(') && lit.ends_with(')') {
            out.insert(lit.to_string());
        }
        rest = &after[len + 1..];
    }
    out
}

/// The `diamond_signable_selectors()` array in `src/app/signer.rs`, as sig strings.
fn signer_sigs(root: &Path) -> BTreeSet<String> {
    let src = read(&root.join("src").join("app").join("signer.rs"));
    let start = src
        .find("fn diamond_signable_selectors()")
        .expect("src/app/signer.rs: fn diamond_signable_selectors() not found — if it moved/renamed, update tests/relay_allowlist_parity.rs");
    let end = src[start..]
        .find(".iter()")
        .map(|o| start + o)
        .expect("signer.rs: diamond_signable_selectors() array end (.iter()) not found");
    let sigs = quoted_sigs(&strip_line_comments(&src[start..end]), '"');
    assert!(sigs.len() > 30, "signer.rs selector extraction broke (got {})", sigs.len());
    sigs
}

/// The `DIAMOND_WRITE_SIGS` array in `proxy/api/sponsor.ts`, as sig strings.
fn relay_sigs(proxy_ts: &Path) -> BTreeSet<String> {
    let src = read(proxy_ts);
    let start = src
        .find("const DIAMOND_WRITE_SIGS = [")
        .expect("sponsor.ts: DIAMOND_WRITE_SIGS not found — if it moved/renamed, update tests/relay_allowlist_parity.rs");
    let end = src[start..]
        .find("];")
        .map(|o| start + o)
        .expect("sponsor.ts: DIAMOND_WRITE_SIGS array end not found");
    let sigs = quoted_sigs(&strip_line_comments(&src[start..end]), '\'');
    assert!(sigs.len() > 30, "sponsor.ts sig extraction broke (got {})", sigs.len());
    sigs
}

/// The core drift test: registry `*_sponsored` diamond writes ⊆ BOTH allowlists.
#[test]
fn registry_sponsored_selectors_are_allowlisted_in_signer_and_relay() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let sources = registry_sources(root);

    // 1. Enumeration guard (both directions): the pinned table must cover
    //    EXACTLY the `*_sponsored` fns in src/registry — a new one fails here.
    let found = sponsored_fns(&sources);
    let pinned: BTreeSet<&str> = SPONSORED.iter().map(|(n, _)| *n).collect();
    for name in &found {
        assert!(
            pinned.contains(name.as_str()),
            "new registry sponsored fn `{name}` is not classified in \
             tests/relay_allowlist_parity.rs::SPONSORED. Add a row with the exact \
             diamond signature(s) it submits, AND add any new signature to BOTH \
             src/app/signer.rs diamond_signable_selectors() and proxy/api/sponsor.ts \
             DIAMOND_WRITE_SIGS (then `cd proxy && vercel --prod`) — a one-sided add \
             403s LH_RELAY_SELECTOR on mainnet and strands escrow \
             (design/relay-allowlist-gaps.md)."
        );
    }
    for name in &pinned {
        assert!(
            found.contains(*name),
            "SPONSORED pins `{name}` but no such fn exists in src/registry — remove the stale row"
        );
    }

    // 2. Pin sanity: every pinned signature is a literal in registry source, so
    //    the table itself can't drift from what the encoders actually emit.
    let all_registry: String = sources.iter().map(|(_, t)| t.as_str()).collect();
    let signer = signer_sigs(root);
    let proxy_ts = root.join("proxy").join("api").join("sponsor.ts");
    let relay = proxy_ts.exists().then(|| relay_sigs(&proxy_ts)); // absent in a packaged crate
    if relay.is_none() {
        eprintln!("skip relay half: proxy/api/sponsor.ts not present");
    }

    for (name, submits) in SPONSORED {
        match submits {
            NonDiamond => {}
            AbsentByDesign(sig) => {
                let quoted = format!("\"{sig}\"");
                assert!(all_registry.contains(&quoted), "`{name}`: pinned sig {sig} not found in src/registry — fix the pin");
                assert!(
                    !signer.contains(*sig),
                    "{sig} is role-gated and must stay OFF signer.rs diamond_signable_selectors()"
                );
                if let Some(relay) = &relay {
                    assert!(
                        !relay.contains(*sig),
                        "{sig} is role-gated and must stay OFF sponsor.ts DIAMOND_WRITE_SIGS"
                    );
                }
            }
            Diamond(sigs) => {
                for sig in *sigs {
                    let quoted = format!("\"{sig}\"");
                    assert!(all_registry.contains(&quoted), "`{name}`: pinned sig {sig} not found in src/registry — fix the pin");
                    assert!(
                        signer.contains(*sig),
                        "`{name}` submits `{sig}` but it is MISSING from src/app/signer.rs \
                         diamond_signable_selectors() — the browser signer refuses the call \
                         (add it there AND ensure sponsor.ts DIAMOND_WRITE_SIGS has it)"
                    );
                    if let Some(relay) = &relay {
                        assert!(
                            relay.contains(*sig),
                            "`{name}` submits `{sig}` but it is MISSING from proxy/api/sponsor.ts \
                             DIAMOND_WRITE_SIGS — on mainnet the relay 403s LH_RELAY_SELECTOR and \
                             escrow strands (add it there, then `cd proxy && vercel --prod`)"
                        );
                    }
                }
            }
        }
    }
}

/// signer.rs documents its list as MIRRORING sponsor.ts (the L15 defense in
/// depth) — hold the two sets byte-equal so a one-sided add fails loudly.
#[test]
fn signer_and_relay_diamond_allowlists_mirror_exactly() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let proxy_ts = root.join("proxy").join("api").join("sponsor.ts");
    if !proxy_ts.exists() {
        eprintln!("skip: proxy/api/sponsor.ts not present");
        return;
    }
    let signer = signer_sigs(root);
    let relay = relay_sigs(&proxy_ts);
    let only_signer: Vec<_> = signer.difference(&relay).collect();
    let only_relay: Vec<_> = relay.difference(&signer).collect();
    assert!(
        only_signer.is_empty() && only_relay.is_empty(),
        "diamond allowlists drifted — signer.rs-only: {only_signer:?}; sponsor.ts-only: \
         {only_relay:?}. Add the missing side (signer.rs diamond_signable_selectors() / \
         sponsor.ts DIAMOND_WRITE_SIGS + proxy deploy), or delete from both."
    );
}
