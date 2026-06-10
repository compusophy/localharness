#[allow(unused_imports)]
use crate::*;

// ---- vote (VotingFacet: DAO governance — Rung 4 of the coordination ladder) --
//
// A guild MEMBER proposes a treasury spend, members VOTE one-member-one-vote,
// and a passed measure EXECUTES from the guild's pooled treasury (the same
// `LibGuildStorage` ledger `guild spend` debits, gated on a vote not the Admin
// role). Mirrors the `registry::*_proposal_*` / `propose`/`vote`/`execute`
// helpers; the same sponsored-write + caller-resolution shape as `guild`/`bounty`.
// A `to` arg given as a NAME resolves to its on-chain OWNER address (the
// `resolve_member_address` split), or accepts a raw `0x…` address.

pub(crate) const VOTE_USAGE: &str = "\
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
pub(crate) const VOTE_MAX_PERIOD_SECS: u64 = 30 * 24 * 3600;

/// How many of a guild's proposals `vote list` scans from the head. A sane page
/// bound mirroring `BOUNTY_LIST_SCAN`; bump when a cursor walk is worth it.
pub(crate) const VOTE_LIST_SCAN: u64 = 100;

/// Parse a `for`/`against` (or `yes`/`no`) ballot argument to the on-chain
/// `support` bool. Pure + testable; case-insensitive.
pub(crate) fn parse_vote_support(raw: &str) -> Result<bool, String> {
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
pub(crate) fn parse_voting_period(raw: &str) -> Result<u64, String> {
    let secs = parse_ttl(raw)?;
    if secs > VOTE_MAX_PERIOD_SECS {
        return Err(format!("voting period '{raw}' exceeds the 30d maximum"));
    }
    Ok(secs)
}

/// Parsed `vote propose` arguments. `to`/`amount` are required positionals; the
/// memo is the joined positional remainder (so an unquoted multi-word memo works,
/// matching `guild spend`/`bounty post`).
pub(crate) struct ParsedVotePropose {
    guild_id: u64,
    to: String,
    amount_wei: u128,
    period_secs: u64,
    memo: String,
}

pub(crate) fn parse_vote_propose_args(rest: &[String]) -> Result<ParsedVotePropose, String> {
    let ([period], positional) = collect_flags(rest, ["--period"], VOTE_USAGE)?;
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
pub(crate) async fn vote(caller: Option<&str>, rest: &[String]) -> i32 {
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
pub(crate) async fn vote_propose(caller: Option<&str>, rest: &[String]) -> i32 {
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
pub(crate) async fn vote_cast(caller: Option<&str>, id_arg: &str, ballot: &str) -> i32 {
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
pub(crate) async fn vote_execute(caller: Option<&str>, id_arg: &str) -> i32 {
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
pub(crate) fn format_proposal_row(id: u64, p: &registry::Proposal, t: &registry::Tally, memo: &str, now: u64) -> String {
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
pub(crate) async fn vote_list(id_arg: &str) -> i32 {
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
pub(crate) async fn vote_show(id_arg: &str) -> i32 {
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

#[cfg(test)]
mod tests {
    use super::*;

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
