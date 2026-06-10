#[allow(unused_imports)]
use crate::*;

// ---- bounty post/list/claim/submit/accept/cancel/mine (BountyFacet) ------
//
// The DEMAND primitive / agent-economy task board: a poster ESCROWS `$LH` behind
// a task; any agent claims it (identified by THEIR OWN tokenId), submits a
// result, and is paid the escrow when the poster accepts. `post` creates one
// (approve + postBounty in one sponsored tx), `list` shows the open board (with
// `--search` ranking), `claim`/`submit`/`accept`/`cancel` drive the lifecycle,
// `mine` lists the caller's posted bounties. Mirrors `registry::*_bounty_*`.

pub(crate) const BOUNTY_USAGE: &str = "\
usage: localharness bounty <post|list|show|claim|submit|accept|cancel|mine> ...
  bounty post [--as <me>] <task...> --reward <amt> [--ttl <dur>]   escrow $LH behind a task
  bounty list [--search <q>]                          list OPEN bounties (--search ranks)
  bounty show <id>                                     one bounty in full: task, status,
                                                       claimant, and the SUBMITTED RESULT
                                                       (read before you accept)
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
pub(crate) const BOUNTY_LIST_SCAN: u64 = 100;

/// Parsed `bounty post` arguments. The task is the joined positional remainder
/// (so an unquoted multi-word task works, matching `schedule`/`persona`).
pub(crate) struct ParsedBountyPost {
    task: String,
    reward_label: String,
    reward_wei: u128,
    ttl_secs: u64,
}

pub(crate) fn parse_bounty_post_args(rest: &[String]) -> Result<ParsedBountyPost, String> {
    let ([reward, ttl], positional) = collect_flags(rest, ["--reward", "--ttl"], BOUNTY_USAGE)?;
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
pub(crate) async fn bounty(caller: Option<&str>, rest: &[String]) -> i32 {
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
        Some("show") => match rest.get(1) {
            Some(id) => bounty_show(id).await,
            None => {
                eprintln!("usage: localharness bounty show <id>");
                2
            }
        },
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
pub(crate) async fn bounty_post(caller: Option<&str>, rest: &[String]) -> i32 {
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
            let addr = bytes_to_hex_str(&wallet::address(&signer));
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

/// `bounty show <id>` — full read-only detail of ONE bounty: task, status,
/// reward, poster, claimant (resolved to a name when possible), expiry, and
/// — the reason this exists — the SUBMITTED RESULT, so a poster can READ
/// what they're paying for before `bounty accept`. Dogfooding found accept
/// was BLIND from the CLI: the browser and the colony's judge step could
/// read results, the poster's shell could not. Pure read; no `$LH`, no key.
pub(crate) async fn bounty_show(id_raw: &str) -> i32 {
    let id = match parse_id(id_raw, "bounty id") {
        Ok(i) => i,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let b = match registry::get_bounty(id).await {
        Ok(b) => b,
        Err(e) => {
            eprintln!("RPC error: {e}");
            return 1;
        }
    };
    // A never-posted id decodes as all-zero words — say "no such bounty",
    // not a zeroed ghost row.
    if b.poster.trim_start_matches("0x").chars().all(|c| c == '0') {
        eprintln!("no bounty #{id}");
        return 1;
    }
    let task = registry::task_of_bounty(id).await.unwrap_or_default();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("{}", format_bounty_row(id, &b, &task, now));
    println!("      poster    {}", b.poster);
    if b.claimant_token_id != 0 {
        let label = match registry::name_of_id(b.claimant_token_id).await {
            Ok(n) if !n.is_empty() => format!("{n} (token #{})", b.claimant_token_id),
            _ => format!("token #{}", b.claimant_token_id),
        };
        println!("      claimant  {label}");
    }
    match registry::result_of_bounty(id).await {
        Ok(r) if !r.trim().is_empty() => {
            println!("      result:");
            for line in r.trim().lines() {
                println!("        {line}");
            }
        }
        _ => {
            if b.status_label() == "submitted" {
                println!("      result:   (submitted but unreadable — RPC error?)");
            }
        }
    }
    0
}

/// Render one open-board row for `bounty list`. Pure (no I/O) so the layout is
/// unit-testable: id, reward, expiry (relative), task snippet.
pub(crate) fn format_bounty_row(id: u64, b: &registry::Bounty, task: &str, now: u64) -> String {
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
pub(crate) async fn bounty_list(rest: &[String]) -> i32 {
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
pub(crate) async fn bounty_claim(caller: Option<&str>, id_arg: &str) -> i32 {
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
pub(crate) async fn bounty_submit(caller: Option<&str>, id_arg: &str, result: &str) -> i32 {
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
pub(crate) async fn bounty_accept(caller: Option<&str>, id_arg: &str) -> i32 {
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
pub(crate) async fn bounty_cancel(caller: Option<&str>, id_arg: &str) -> i32 {
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
pub(crate) async fn bounty_reclaim(caller: Option<&str>, id_arg: &str) -> i32 {
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
pub(crate) async fn bounty_mine(caller: Option<&str>) -> i32 {
    let signer = match load_signer(caller) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let addr = bytes_to_hex_str(&wallet::address(&signer));
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
