use crate::{bytes_to_hex_str, collect_flags, ensure_wallet_covers, fmt_duration, fmt_lh, load_signer, load_signer_and_sponsor, parse_id, parse_ttl, registry, resolve_address_label, wallet, INVITE_DEFAULT_TTL_SECS};

// ---- party (PartyFacet: ad-hoc squads with an escrowed, pre-agreed split) ----
//
// Rung 2 of the coordination ladder (bounty → PARTY → guild → DAO). A party
// is an EPHEMERAL squad of agent identities formed around one objective: the
// creator proposes members + a bps split (fixed at formation), each member's
// owner CONSENTS (`party join`), anyone FUNDS the pot, and the creator
// COMPLETES — the pot splits to the member TBAs by shares, then the party
// dissolves. `party disband` (creator any time; anyone after the ttl)
// refunds every funder exactly. Mirrors `registry::*_party_*`; the same
// sponsored-write + caller-resolution shape as `bounty`/`guild`.

pub(crate) const PARTY_USAGE: &str = "\
usage: localharness party <form|join|fund|complete|disband|show|list|mine> ...
  party form [--as <me>] [--ttl <dur>] <member[:bps]>...   propose a squad around one goal:
                                                       members are names or token ids; :bps
                                                       fixes each share (must sum to 10000) —
                                                       omit ALL bps for an equal split
  party join     [--as <me>] <partyId>                 consent to membership (your identity's
                                                       seats; the last consent activates it)
  party fund     [--as <me>] <partyId> <amount>        escrow $LH into the party pot
  party complete [--as <me>] <partyId>                 split the pot to member TBAs by shares
                                                       and dissolve (creator only)
  party disband  [--as <me>] <partyId>                 dissolve + refund every funder exactly
                                                       (creator any time; anyone after expiry)
  party show <partyId>                                 members, shares, consents, pot, funders
  party list                                           live (forming/active) parties
  party mine [--as <me>]                               parties you formed
  member: a subdomain name or a token id (#7)   dur: 1h / 7d / 30d (1h … 90d, default 7d)";

/// How many parties `party list` scans from the board's head — the same
/// launch-scale page bound as `BOUNTY_LIST_SCAN`.
pub(crate) const PARTY_LIST_SCAN: u64 = 100;

/// One parsed `party form` member spec: the raw member (name or `#id`) and
/// its optional explicit share. Pure parsing; resolution hits the chain.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct MemberSpec {
    pub member: String,
    pub bps: Option<u16>,
}

/// Parse `party form`'s positional `<member[:bps]>...` specs. Either EVERY
/// member carries an explicit `:bps` (which must sum to 10000) or NONE does
/// (equal split, remainder to the FIRST member). Mixing is rejected — a
/// half-specified split is always a mistake. Pure + testable.
pub(crate) fn parse_member_specs(positional: &[String]) -> Result<Vec<(String, u16)>, String> {
    if positional.is_empty() {
        return Err(format!("party form needs at least one <member>\n{PARTY_USAGE}"));
    }
    let mut specs: Vec<MemberSpec> = Vec::with_capacity(positional.len());
    for raw in positional {
        let (member, bps) = match raw.rsplit_once(':') {
            Some((m, b)) => {
                let bps = b
                    .parse::<u16>()
                    .ok()
                    .filter(|&v| v > 0 && v <= 10_000)
                    .ok_or_else(|| format!("invalid share '{b}' in '{raw}' (1..10000 bps)"))?;
                (m.to_string(), Some(bps))
            }
            None => (raw.clone(), None),
        };
        if member.trim().is_empty() {
            return Err(format!("empty member in '{raw}'"));
        }
        specs.push(MemberSpec { member: member.trim().to_string(), bps });
    }
    let explicit = specs.iter().filter(|s| s.bps.is_some()).count();
    if explicit == 0 {
        // Equal split; the FIRST member takes the rounding remainder.
        let n = specs.len() as u16;
        let base = 10_000 / n;
        let remainder = 10_000 - base * n;
        return Ok(specs
            .into_iter()
            .enumerate()
            .map(|(i, s)| (s.member, if i == 0 { base + remainder } else { base }))
            .collect());
    }
    if explicit != specs.len() {
        return Err("give EVERY member a :bps share, or NONE (equal split)".to_string());
    }
    let sum: u32 = specs.iter().map(|s| s.bps.unwrap() as u32).sum();
    if sum != 10_000 {
        return Err(format!("shares must sum to 10000 bps, got {sum}"));
    }
    Ok(specs.into_iter().map(|s| (s.member, s.bps.unwrap())).collect())
}

/// Resolve a `member` spec to its registry tokenId: `#7`/`7` is an id used
/// as-is (existence is checked on-chain by formParty); a name resolves via
/// `idOfName` (0 = unregistered → a named error).
pub(crate) async fn resolve_member_token_id(member: &str) -> Result<u64, String> {
    let trimmed = member.trim().trim_start_matches('#');
    if !trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_digit()) {
        return match trimmed.parse::<u64>() {
            // Reject id 0 up front (mirrors the name path's zero-check) — it's never
            // a valid tokenId and formParty would otherwise revert with a cryptic error.
            Ok(0) => Err(format!("member id 0 is not valid (check the member '{member}')")),
            Ok(id) => Ok(id),
            Err(_) => Err(format!("invalid member id '{member}'")),
        };
    }
    match registry::id_of_name(&member.trim().to_ascii_lowercase()).await {
        Ok(0) => Err(format!("'{member}' is not registered")),
        Ok(id) => Ok(id),
        Err(e) => Err(format!("RPC error resolving '{member}': {e}")),
    }
}

/// `localharness party <subcommand>` — the party router.
pub(crate) async fn party(caller: Option<&str>, rest: &[String]) -> i32 {
    match rest.first().map(String::as_str) {
        Some("form") => party_form(caller, &rest[1..]).await,
        Some("join") => match rest.get(1) {
            Some(id) => party_join(caller, id).await,
            None => {
                eprintln!("usage: localharness party join [--as <me>] <partyId>");
                2
            }
        },
        Some("fund") => match (rest.get(1), rest.get(2)) {
            (Some(id), Some(amount)) => party_fund(caller, id, amount).await,
            _ => {
                eprintln!("usage: localharness party fund [--as <me>] <partyId> <amount>");
                2
            }
        },
        Some("complete") => match rest.get(1) {
            Some(id) => party_complete(caller, id).await,
            None => {
                eprintln!("usage: localharness party complete [--as <me>] <partyId>");
                2
            }
        },
        Some("disband") => match rest.get(1) {
            Some(id) => party_disband(caller, id).await,
            None => {
                eprintln!("usage: localharness party disband [--as <me>] <partyId>");
                2
            }
        },
        Some("show") => match rest.get(1) {
            Some(id) => party_show(id).await,
            None => {
                eprintln!("usage: localharness party show <partyId>");
                2
            }
        },
        Some("list") => party_list().await,
        Some("mine") => party_mine(caller).await,
        _ => {
            eprintln!("{PARTY_USAGE}");
            2
        }
    }
}

/// `party form [--ttl <dur>] <member[:bps]>...` — propose the squad + split
/// (`formParty`). Members the caller's address owns consent automatically;
/// the rest run `party join`. Reads the new partyId back from
/// `partiesOf(creator)`.
pub(crate) async fn party_form(caller: Option<&str>, rest: &[String]) -> i32 {
    let ([ttl], positional) = match collect_flags(rest, ["--ttl"], PARTY_USAGE) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let ttl_secs = match ttl {
        None => INVITE_DEFAULT_TTL_SECS,
        Some(raw) => match parse_ttl(&raw) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("{e}");
                return 2;
            }
        },
    };
    let specs = match parse_member_specs(&positional) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    // Resolve every member to its tokenId BEFORE signing anything.
    let mut member_ids: Vec<u64> = Vec::with_capacity(specs.len());
    let mut shares: Vec<u16> = Vec::with_capacity(specs.len());
    let mut labels: Vec<String> = Vec::with_capacity(specs.len());
    for (member, bps) in &specs {
        match resolve_member_token_id(member).await {
            Ok(id) => {
                member_ids.push(id);
                shares.push(*bps);
                labels.push(format!("{member} (token #{id}, {:.2}%)", *bps as f64 / 100.0));
            }
            Err(e) => {
                eprintln!("party form: {e}");
                return 1;
            }
        }
    }
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    println!("forming a {}-member party (expires in {}):", member_ids.len(), fmt_duration(ttl_secs));
    for l in &labels {
        println!("  {l}");
    }
    match registry::form_party_sponsored(
        &signer,
        &sponsor,
        &member_ids,
        &shares,
        ttl_secs,
        registry::ALPHA_USD_ADDRESS(),
    )
    .await
    {
        Ok(tx) => {
            // The new partyId is the last entry in the creator's partiesOf index.
            let addr = bytes_to_hex_str(&wallet::address(&signer));
            let id_note = match registry::parties_of(&addr).await {
                Ok(ids) if !ids.is_empty() => Some(ids[ids.len() - 1]),
                _ => None,
            };
            match id_note {
                Some(id) => {
                    println!("✓ party #{id} formed — members consent with `party join {id}`");
                    println!("  fund the pot:   party fund {id} <amount>");
                    println!("  settle + split: party complete {id}");
                }
                None => println!("✓ party formed — see it with `party mine`"),
            }
            println!("  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("party form failed: {e}");
            1
        }
    }
}

/// `party join <partyId>` — consent to every member seat the caller's
/// address owns (`joinParty`). The last consent flips the party Active.
pub(crate) async fn party_join(caller: Option<&str>, id_arg: &str) -> i32 {
    let party_id = match parse_id(id_arg, "party") {
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
    println!("consenting to party #{party_id} …");
    match registry::join_party_sponsored(&signer, &sponsor, party_id, registry::ALPHA_USD_ADDRESS())
        .await
    {
        Ok(tx) => {
            let note = match registry::get_party(party_id).await {
                Ok(p) if p.status == 1 => " — the party is now ACTIVE",
                Ok(p) => {
                    println!(
                        "✓ consented to party #{party_id} ({}/{} seats in)  tx: {tx}",
                        p.accepted_count, p.member_count
                    );
                    return 0;
                }
                _ => "",
            };
            println!("✓ consented to party #{party_id}{note}  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("party join failed: {e}");
            1
        }
    }
}

/// `party fund <partyId> <amount>` — escrow `$LH` from the caller's wallet
/// into the party pot (approve + fundParty in one sponsored tx). Refunded
/// exactly on disband/expiry; split to the members on complete.
pub(crate) async fn party_fund(caller: Option<&str>, id_arg: &str, amount: &str) -> i32 {
    let party_id = match parse_id(id_arg, "party") {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let amount_wei = match localharness::encoding::parse_token_amount(amount) {
        Some(w) if w > 0 => w,
        _ => {
            eprintln!("party fund: invalid amount '{amount}' (expected a positive number of $LH)");
            return 2;
        }
    };
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    // The contribution pulls from the WALLET pot — auto-bridge any shortfall
    // out of the chat meter first (the guild-fund precedent).
    let from_hex = bytes_to_hex_str(&wallet::address(&signer));
    if let Err(code) = ensure_wallet_covers(&signer, &from_hex, amount_wei).await {
        return code;
    }
    println!("funding party #{party_id} with {} …", fmt_lh(amount_wei));
    match registry::fund_party_sponsored(
        &signer,
        &sponsor,
        party_id,
        amount_wei,
        registry::ALPHA_USD_ADDRESS(),
    )
    .await
    {
        Ok(tx) => {
            println!("✓ {} escrowed into party #{party_id}'s pot  tx: {tx}", fmt_lh(amount_wei));
            0
        }
        Err(e) => {
            eprintln!("party fund failed: {e}");
            1
        }
    }
}

/// `party complete <partyId>` — the creator settles: the pot splits to each
/// member's TBA by the agreed shares and the party dissolves
/// (`completeParty`).
pub(crate) async fn party_complete(caller: Option<&str>, id_arg: &str) -> i32 {
    let party_id = match parse_id(id_arg, "party") {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    // Read-only preflight: name the revert cause before burning sponsor gas
    // (the bounty-preflight precedent).
    if let Ok(p) = registry::get_party(party_id).await {
        if let Err(msg) = party_preflight(party_id, &p, "complete") {
            eprintln!("{msg}");
            return 1;
        }
    }
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    println!("completing party #{party_id} (splitting the pot to member TBAs) …");
    match registry::complete_party_sponsored(&signer, &sponsor, party_id, registry::ALPHA_USD_ADDRESS())
        .await
    {
        Ok(tx) => {
            println!("✓ party #{party_id} completed — the pot is split to the members' TBAs  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!("party complete failed: {e}");
            1
        }
    }
}

/// `party disband <partyId>` — dissolve the party and refund every funder
/// their exact contribution (`disbandParty`; creator any time, anyone after
/// expiry).
pub(crate) async fn party_disband(caller: Option<&str>, id_arg: &str) -> i32 {
    let party_id = match parse_id(id_arg, "party") {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    if let Ok(p) = registry::get_party(party_id).await {
        if let Err(msg) = party_preflight(party_id, &p, "disband") {
            eprintln!("{msg}");
            return 1;
        }
    }
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    println!("disbanding party #{party_id} (refunding its funders) …");
    match registry::disband_party_sponsored(&signer, &sponsor, party_id, registry::ALPHA_USD_ADDRESS())
        .await
    {
        Ok(tx) => {
            println!("✓ party #{party_id} disbanded — every funder is refunded exactly  tx: {tx}");
            0
        }
        Err(e) => {
            eprintln!(
                "party disband failed: {e}\n  \
                 (only the creator may disband a live party before its ttl expires; \
                 after expiry anyone may)"
            );
            1
        }
    }
}

/// The READ-ONLY preflight verdict for a party WRITE — names the revert
/// cause (nonexistent / wrong state / expired window) BEFORE broadcasting a
/// tx that burns sponsor gas. Pure + testable.
pub(crate) fn party_preflight(id: u64, p: &registry::Party, action: &str) -> Result<(), String> {
    // A never-formed id decodes as the zero record (creator all-zero).
    if p.creator.trim_start_matches("0x").chars().all(|c| c == '0') {
        return Err(format!("party #{id} doesn't exist"));
    }
    match action {
        "complete" => match p.status {
            1 => Ok(()),
            0 => Err(format!(
                "party #{id} is still forming ({}/{} seats consented) — members run `party join {id}` first",
                p.accepted_count, p.member_count
            )),
            _ => Err(format!("party #{id} is already {} — nothing to settle", p.status_label())),
        },
        "disband" => match p.status {
            0 | 1 => Ok(()),
            _ => Err(format!("party #{id} is already {} — nothing to disband", p.status_label())),
        },
        _ => Ok(()),
    }
}

/// Render one board row for `party list`/`party mine`. Pure (no I/O) so the
/// layout is unit-testable: id, status, consent tally, pot, expiry.
pub(crate) fn format_party_row(id: u64, p: &registry::Party, now: u64) -> String {
    let when = if p.expiry == 0 {
        "—".to_string()
    } else if p.expiry <= now {
        "EXPIRED".to_string()
    } else {
        format!("in {}", fmt_duration(p.expiry - now))
    };
    format!(
        "  #{id}  [{status}]  {acc}/{n} consented  pot {pot}  expires {when}",
        status = p.status_label(),
        acc = p.accepted_count,
        n = p.member_count,
        pot = fmt_lh(p.escrow_wei),
    )
}

/// `party show <partyId>` — full read-only detail: members (resolved to
/// names) + shares + consents, the pot, and the funder roster. No `$LH`.
pub(crate) async fn party_show(id_arg: &str) -> i32 {
    let party_id = match parse_id(id_arg, "party") {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let p = match registry::get_party(party_id).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("RPC error: {e}");
            return 1;
        }
    };
    if p.creator.trim_start_matches("0x").chars().all(|c| c == '0') {
        eprintln!("no party #{party_id}");
        return 1;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("{}", format_party_row(party_id, &p, now));
    println!("      creator   {}", resolve_address_label(&p.creator).await);
    let members = registry::party_members_of(party_id).await.unwrap_or_default();
    let shares = registry::party_shares_of(party_id).await.unwrap_or_default();
    for (i, token_id) in members.iter().enumerate() {
        let name = registry::name_of_id(*token_id).await.unwrap_or_default();
        let label = if name.is_empty() {
            format!("token #{token_id}")
        } else {
            format!("{name} (token #{token_id})")
        };
        let bps = shares.get(i).copied().unwrap_or(0);
        let consented = registry::party_consent_of(party_id, *token_id).await.unwrap_or(false);
        println!(
            "      member    {label}  {:.2}%  [{}]",
            bps as f64 / 100.0,
            if consented { "consented" } else { "pending" }
        );
    }
    let funders = registry::party_funders_of(party_id).await.unwrap_or_default();
    for f in funders {
        let amt = registry::party_contribution_of(party_id, &f).await.unwrap_or(0);
        println!("      funder    {f}  {}", fmt_lh(amt));
    }
    0
}

/// `party list` — the live (forming/active, unexpired) squad board.
/// Read-only, no `$LH`.
pub(crate) async fn party_list() -> i32 {
    let ids = match registry::live_parties(0, PARTY_LIST_SCAN).await {
        Ok(ids) => ids,
        Err(e) => {
            eprintln!("party list failed: {e}");
            return 1;
        }
    };
    if ids.is_empty() {
        println!("no live parties — form one with `party form <member[:bps]>...`");
        return 0;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("{} live party(ies):", ids.len());
    for id in ids {
        match registry::get_party(id).await {
            Ok(p) => println!("{}", format_party_row(id, &p, now)),
            Err(e) => println!("  #{id}  (could not read: {e})"),
        }
    }
    0
}

/// `party mine [--as <me>]` — list the parties the caller FORMED
/// (`partiesOf` + a `getParty` per id). Read-only, no `$LH`.
pub(crate) async fn party_mine(caller: Option<&str>) -> i32 {
    let signer = match load_signer(caller) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let addr = bytes_to_hex_str(&wallet::address(&signer));
    let ids = match registry::parties_of(&addr).await {
        Ok(ids) => ids,
        Err(e) => {
            eprintln!("RPC error: {e}");
            return 1;
        }
    };
    if ids.is_empty() {
        println!("no parties formed by {addr} — form one with `party form <member[:bps]>...`");
        return 0;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("{} party(ies) formed by {addr}:", ids.len());
    for id in ids {
        match registry::get_party(id).await {
            Ok(p) => println!("{}", format_party_row(id, &p, now)),
            Err(e) => println!("  #{id}  (could not read: {e})"),
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args;

    #[test]
    fn parse_member_specs_explicit_shares_must_sum() {
        // Every member carries a :bps and they sum to 10000.
        let got = parse_member_specs(&args(&["alice:6000", "bob:4000"])).unwrap();
        assert_eq!(got, vec![("alice".to_string(), 6000), ("bob".to_string(), 4000)]);
        // Wrong sum is rejected with the actual total named.
        let e = parse_member_specs(&args(&["alice:6000", "bob:3999"])).unwrap_err();
        assert!(e.contains("9999"), "got: {e}");
        let e = parse_member_specs(&args(&["alice:6000", "bob:4001"])).unwrap_err();
        assert!(e.contains("10001"), "got: {e}");
    }

    #[test]
    fn parse_member_specs_equal_split_with_remainder_to_first() {
        // No :bps anywhere → equal split; 3 members: 3334 + 3333 + 3333.
        let got = parse_member_specs(&args(&["a", "b", "c"])).unwrap();
        assert_eq!(
            got,
            vec![("a".to_string(), 3334), ("b".to_string(), 3333), ("c".to_string(), 3333)]
        );
        assert_eq!(got.iter().map(|(_, b)| *b as u32).sum::<u32>(), 10_000, "always sums to 100%");
        // A single member takes everything.
        assert_eq!(parse_member_specs(&args(&["solo"])).unwrap(), vec![("solo".to_string(), 10_000)]);
    }

    #[test]
    fn parse_member_specs_rejects_mixed_and_bad_input() {
        // Mixing explicit + implicit shares is always a mistake.
        assert!(parse_member_specs(&args(&["a:5000", "b"])).is_err());
        // Zero / oversized / non-numeric shares.
        assert!(parse_member_specs(&args(&["a:0", "b:10000"])).is_err());
        assert!(parse_member_specs(&args(&["a:10001"])).is_err());
        assert!(parse_member_specs(&args(&["a:half"])).is_err());
        // Empty member name / empty list.
        assert!(parse_member_specs(&args(&[":5000"])).is_err());
        assert!(parse_member_specs(&args(&[])).is_err());
    }

    #[test]
    fn party_preflight_names_the_revert_cause() {
        let mk = |status: u8, acc: u64| registry::Party {
            creator: "0xabc".into(),
            expiry: 0,
            status,
            escrow_wei: 0,
            member_count: 2,
            accepted_count: acc,
        };
        // A never-formed id decodes as the zero record → "doesn't exist".
        let ghost = registry::Party {
            creator: "0x0000000000000000000000000000000000000000".into(),
            expiry: 0,
            status: 0,
            escrow_wei: 0,
            member_count: 0,
            accepted_count: 0,
        };
        assert_eq!(
            party_preflight(999, &ghost, "complete"),
            Err("party #999 doesn't exist".to_string())
        );
        // complete: only Active passes; Forming coaches the join step.
        assert!(party_preflight(1, &mk(1, 2), "complete").is_ok());
        let e = party_preflight(1, &mk(0, 1), "complete").unwrap_err();
        assert!(e.contains("1/2 seats consented"), "got: {e}");
        assert!(party_preflight(1, &mk(2, 2), "complete").is_err());
        assert!(party_preflight(1, &mk(3, 2), "complete").is_err());
        // disband: live (Forming/Active) passes; terminal is named.
        assert!(party_preflight(1, &mk(0, 0), "disband").is_ok());
        assert!(party_preflight(1, &mk(1, 2), "disband").is_ok());
        let e = party_preflight(1, &mk(3, 2), "disband").unwrap_err();
        assert!(e.contains("disbanded"), "got: {e}");
    }

    #[test]
    fn format_party_row_contains_key_fields() {
        let p = registry::Party {
            creator: "0xabc".into(),
            expiry: 1_000 + 300, // 5m out from `now`
            status: 1,
            escrow_wei: 5_000_000_000_000_000_000, // 5 $LH
            member_count: 3,
            accepted_count: 3,
        };
        let row = format_party_row(7, &p, 1_000);
        assert!(row.contains("#7"));
        assert!(row.contains("[active]"));
        assert!(row.contains("3/3 consented"));
        assert!(row.contains("pot 5.00 LH"));
        assert!(row.contains("expires in 5m"));
        // Expired + unset expiry render distinctly.
        let mut q = p.clone();
        q.expiry = 100;
        assert!(format_party_row(1, &q, 1_000).contains("expires EXPIRED"));
        q.expiry = 0;
        assert!(format_party_row(1, &q, 1_000).contains("expires —"));
    }
}
