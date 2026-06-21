use crate::{bytes_to_hex_str, ensure_wallet_covers, fmt_lh, load_signer_and_sponsor, parse_id, registry, wallet};

// ---- validation (ValidationFacet: ERC-8004-style validation STAKING) --------
//
// The money-backed half of reputation (ReputationFacet attestations are the
// free-signal half). A VALIDATOR escrows `$LH` behind a verdict ("this work is
// valid" / "invalid") about a subject's work — referenced by the bounty whose
// result is being judged (workRef = bytes32(bountyId)). A CHALLENGER
// counter-stakes the OPPOSITE verdict with the same amount. The work's bounty
// POSTER (or the diamond owner as arbiter fallback) RESOLVES, and the winner
// takes both stakes. Unchallenged stakes RECLAIM after the challenge window;
// an unresolved challenge DRAWS (both refunded) after the resolve window.
// Escrow-conservation throughout (supply-neutral). Mirrors `registry::*` and
// the sponsored-write + caller-resolution shape of `bounty` / `party` / `guild`.

pub(crate) const VALIDATION_USAGE: &str = "\
usage: localharness validation <stake|challenge|resolve|reclaim|draw|show|count> ...
  validation stake [--as <me>] <subject> <bountyId> <valid|invalid> <amount>
                                          escrow $LH behind a verdict on <subject>'s work
                                          for bounty <bountyId> (subject: name or #id)
  validation challenge [--as <me>] <id>   counter-stake the OPPOSITE verdict (same amount,
                                          read from the validation; only while Open)
  validation resolve [--as <me>] <id> <validator|challenger>
                                          rule a challenged validation (resolver-only: the
                                          bounty poster, or the diamond owner) — winner takes both
  validation reclaim [--as <me>] <id>     refund an UNCHALLENGED stake after the window (to the validator)
  validation draw [--as <me>] <id>        refund BOTH sides of a challenged-but-unresolved validation
  validation show <id>                    the validation record (validator, challenger, verdict, stakes, status)
  validation count                        total validations ever staked";

/// Human label for the ABI status enum (0 Open … 5 Drawn).
pub(crate) fn validation_status_label(status: u8) -> &'static str {
    match status {
        0 => "open",
        1 => "challenged",
        2 => "reclaimed",
        3 => "validator won",
        4 => "challenger won",
        5 => "drawn",
        _ => "unknown",
    }
}

/// `localharness validation <subcommand>` — the validation-staking router.
pub(crate) async fn validation(caller: Option<&str>, rest: &[String]) -> i32 {
    match rest.first().map(String::as_str) {
        Some("stake") => match (rest.get(1), rest.get(2), rest.get(3), rest.get(4)) {
            (Some(subject), Some(bounty), Some(verdict), Some(amount)) => {
                validation_stake(caller, subject, bounty, verdict, amount).await
            }
            _ => {
                eprintln!("usage: localharness validation stake [--as <me>] <subject> <bountyId> <valid|invalid> <amount>");
                2
            }
        },
        Some("challenge") => match rest.get(1) {
            Some(id) => validation_challenge(caller, id).await,
            None => {
                eprintln!("usage: localharness validation challenge [--as <me>] <id>");
                2
            }
        },
        Some("resolve") => match (rest.get(1), rest.get(2)) {
            (Some(id), Some(side)) => validation_resolve(caller, id, side).await,
            _ => {
                eprintln!("usage: localharness validation resolve [--as <me>] <id> <validator|challenger>");
                2
            }
        },
        Some("reclaim") => match rest.get(1) {
            Some(id) => validation_reclaim(caller, id, false).await,
            None => {
                eprintln!("usage: localharness validation reclaim [--as <me>] <id>");
                2
            }
        },
        Some("draw") => match rest.get(1) {
            Some(id) => validation_reclaim(caller, id, true).await,
            None => {
                eprintln!("usage: localharness validation draw [--as <me>] <id>");
                2
            }
        },
        Some("show") => match rest.get(1) {
            Some(id) => validation_show(id).await,
            None => {
                eprintln!("usage: localharness validation show <id>");
                2
            }
        },
        Some("count") => match registry::validation_count().await {
            Ok(n) => {
                println!("{n} validation(s) staked");
                0
            }
            Err(e) => {
                eprintln!("validation count: {e}");
                1
            }
        },
        _ => {
            eprintln!("{VALIDATION_USAGE}");
            2
        }
    }
}

/// workRef = `bytes32(bountyId)` — the same coupling the facet's resolver uses
/// (the poster of `uint256(workRef)` is the on-chain resolver).
fn work_ref_of_bounty(bounty_id: u64) -> [u8; 32] {
    let mut wr = [0u8; 32];
    wr[24..].copy_from_slice(&bounty_id.to_be_bytes());
    wr
}

/// Parse a verdict word into the staked bool. valid/true/yes → true.
fn parse_verdict(raw: &str) -> Result<bool, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "valid" | "true" | "yes" | "ok" => Ok(true),
        "invalid" | "false" | "no" => Ok(false),
        other => Err(format!("verdict must be 'valid' or 'invalid', got '{other}'")),
    }
}

/// `validation stake <subject> <bountyId> <valid|invalid> <amount>` — escrow a
/// verdict (approve + stakeValidation in one sponsored tx). Reads the new id
/// back from `validation_count()` after mining.
pub(crate) async fn validation_stake(
    caller: Option<&str>,
    subject: &str,
    bounty_arg: &str,
    verdict_arg: &str,
    amount: &str,
) -> i32 {
    let subject_id = match crate::party::resolve_member_token_id(subject).await {
        Ok(id) => id,
        Err(e) => {
            eprintln!("validation stake: {e}");
            return 1;
        }
    };
    let bounty_id = match parse_id(bounty_arg, "bounty") {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let valid = match parse_verdict(verdict_arg) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("validation stake: {e}");
            return 2;
        }
    };
    let amount_wei = match localharness::encoding::parse_token_amount(amount) {
        Some(w) if w > 0 => w,
        _ => {
            eprintln!("validation stake: invalid amount '{amount}' (expected a positive number of $LH)");
            return 2;
        }
    };
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    let from_hex = bytes_to_hex_str(&wallet::address(&signer));
    if let Err(code) = ensure_wallet_covers(&signer, &from_hex, amount_wei).await {
        return code;
    }
    println!(
        "staking {} that {subject} (token #{subject_id})'s work for bounty #{bounty_id} is {} …",
        fmt_lh(amount_wei),
        if valid { "VALID" } else { "INVALID" }
    );
    match registry::stake_validation_sponsored(
        &signer,
        &sponsor,
        work_ref_of_bounty(bounty_id),
        subject_id,
        valid,
        amount_wei,
        registry::ALPHA_USD_ADDRESS(),
    )
    .await
    {
        Ok(tx) => {
            let id_note = match registry::validation_count().await {
                Ok(n) if n > 0 => format!(" (validation #{})", n - 1),
                _ => String::new(),
            };
            println!("✓ verdict staked{id_note}  tx: {tx}");
            println!("  it reclaims after the challenge window, or doubles/forfeits on resolution.");
            0
        }
        Err(e) => {
            eprintln!("validation stake failed: {e}");
            1
        }
    }
}

/// `validation challenge <id>` — counter-stake the opposite verdict. The
/// counter-stake MUST equal the validation's own stake, so we read it first.
pub(crate) async fn validation_challenge(caller: Option<&str>, id_arg: &str) -> i32 {
    let id = match parse_id(id_arg, "validation") {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let v = match registry::get_validation(id).await {
        Ok(Some(v)) => v,
        Ok(None) => {
            eprintln!("validation challenge: validation #{id} doesn't exist");
            return 1;
        }
        Err(e) => {
            eprintln!("validation challenge: {e}");
            return 1;
        }
    };
    if v.status != 0 {
        eprintln!(
            "validation challenge: validation #{id} is {} — only an OPEN validation can be challenged",
            validation_status_label(v.status)
        );
        return 1;
    }
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    let from_hex = bytes_to_hex_str(&wallet::address(&signer));
    if let Err(code) = ensure_wallet_covers(&signer, &from_hex, v.stake_wei).await {
        return code;
    }
    println!(
        "counter-staking {} that the work is {} (the opposite of the open verdict) …",
        fmt_lh(v.stake_wei),
        if v.verdict_valid { "INVALID" } else { "VALID" }
    );
    match registry::challenge_validation_sponsored(
        &signer,
        &sponsor,
        id,
        v.stake_wei,
        registry::ALPHA_USD_ADDRESS(),
    )
    .await
    {
        Ok(tx) => {
            println!("✓ challenged validation #{id} — the resolver now rules  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("validation challenge failed: {e}");
            1
        }
    }
}

/// `validation resolve <id> <validator|challenger>` — rule a challenged
/// validation (resolver-only on chain). The named side is paid BOTH stakes.
pub(crate) async fn validation_resolve(caller: Option<&str>, id_arg: &str, side: &str) -> i32 {
    let id = match parse_id(id_arg, "validation") {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let validator_wins = match side.trim().to_ascii_lowercase().as_str() {
        "validator" | "valid" => true,
        "challenger" | "invalid" => false,
        other => {
            eprintln!("validation resolve: winner must be 'validator' or 'challenger', got '{other}'");
            return 2;
        }
    };
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    println!(
        "resolving validation #{id} in favour of the {} …",
        if validator_wins { "VALIDATOR" } else { "CHALLENGER" }
    );
    match registry::resolve_validation_sponsored(
        &signer,
        &sponsor,
        id,
        validator_wins,
        registry::ALPHA_USD_ADDRESS(),
    )
    .await
    {
        Ok(tx) => {
            println!("✓ resolved — both stakes paid to the winner  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("validation resolve failed (resolver-only: the bounty poster or diamond owner): {e}");
            1
        }
    }
}

/// `validation reclaim <id>` (unchallenged) / `validation draw <id>` (challenged
/// but unresolved past the window). Both are permissionless pokes that refund
/// the rightful side(s).
pub(crate) async fn validation_reclaim(caller: Option<&str>, id_arg: &str, unresolved: bool) -> i32 {
    let id = match parse_id(id_arg, "validation") {
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
    let what = if unresolved { "draw (refund both sides of)" } else { "reclaim the unchallenged stake on" };
    println!("attempting to {what} validation #{id} …");
    let res = if unresolved {
        registry::reclaim_unresolved_sponsored(&signer, &sponsor, id, registry::ALPHA_USD_ADDRESS()).await
    } else {
        registry::reclaim_stake_sponsored(&signer, &sponsor, id, registry::ALPHA_USD_ADDRESS()).await
    };
    match res {
        Ok(tx) => {
            println!("✓ refunded validation #{id}  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("validation {} failed (is the window over?): {e}", if unresolved { "draw" } else { "reclaim" });
            1
        }
    }
}

/// `validation show <id>` — the decoded record.
pub(crate) async fn validation_show(id_arg: &str) -> i32 {
    let id = match parse_id(id_arg, "validation") {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    match registry::get_validation(id).await {
        Ok(Some(v)) => {
            println!("validation #{id}  [{}]", validation_status_label(v.status));
            println!("  subject token  #{}", v.subject_token_id);
            println!("  verdict        work is {}", if v.verdict_valid { "VALID" } else { "INVALID" });
            println!("  validator      {}", v.validator);
            let challenger_zero = v.challenger.trim_start_matches("0x").chars().all(|c| c == '0');
            println!("  challenger     {}", if challenger_zero { "none yet".to_string() } else { v.challenger.clone() });
            println!("  stake/side     {}", fmt_lh(v.stake_wei));
            println!("  workRef        0x{}", v.work_ref_hex.trim_start_matches("0x"));
            0
        }
        Ok(None) => {
            eprintln!("validation #{id} doesn't exist");
            1
        }
        Err(e) => {
            eprintln!("validation show: {e}");
            1
        }
    }
}
