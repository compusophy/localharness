#[allow(unused_imports)]
use crate::*;

pub(crate) const INVITE_USAGE: &str = "\
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
pub(crate) fn parse_ttl(raw: &str) -> Result<u64, String> {
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
pub(crate) fn fmt_ttl(secs: u64) -> String {
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
pub(crate) fn gen_invite_code(amount_label: &str) -> String {
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
pub(crate) struct ParsedInviteCreate {
    amount_label: String,
    amount_wei: u128,
    ttl_secs: u64,
}

pub(crate) fn parse_invite_create_args(rest: &[String]) -> Result<ParsedInviteCreate, String> {
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
pub(crate) async fn invite(caller: Option<&str>, rest: &[String]) -> i32 {
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
pub(crate) async fn invite_create(caller: Option<&str>, rest: &[String]) -> i32 {
    let ParsedInviteCreate { amount_label, amount_wei, ttl_secs } = match parse_invite_create_args(rest) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
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
pub(crate) async fn invite_accept(caller: Option<&str>, code: &str) -> i32 {
    let code = code.trim();
    if code.is_empty() {
        eprintln!("invite accept: empty code");
        return 2;
    }
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
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
pub(crate) async fn invite_reclaim(caller: Option<&str>, code: &str) -> i32 {
    let code = code.trim();
    if code.is_empty() {
        eprintln!("invite reclaim: empty code");
        return 2;
    }
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
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
pub(crate) async fn invite_list(caller: Option<&str>) -> i32 {
    let signer = match load_signer(caller) {
        Ok(s) => s,
        Err(code) => return code,
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
