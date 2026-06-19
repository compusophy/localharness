#[allow(unused_imports)]
use crate::*;

pub(crate) const INVITE_USAGE: &str = "\
usage: localharness invite <create|accept|reclaim|list> ...
  invite create [--as <me>] [--amount <X>] [--ttl <dur>]  escrow X $LH behind a fresh
                                                          code; prints the share link
  invite accept [--as <me>] <code>                        accept an invite (paid to you)
  invite reclaim [--as <me>] <code>                       refund an EXPIRED invite
  invite list [--as <me>]                                 your total escrowed $LH
  dur: 1h / 7d / 30d   (1h … 90d, default 7d)   amount: $LH (default 2, range 0.01–10)";

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

/// The directory holding the CLIENT-SIDE invite-code records (`invite create`
/// appends here so `invite list` can show per-invite rows). Sits beside the key
/// home (`<home>/.localharness/invites`), falling back to a cwd dir when no home
/// dir resolves — same precedence as the key store. The CHAIN is the source of
/// truth for an invite's STATE; this file only remembers which codes you made
/// (the plaintext is never on-chain). Keyed per funder address so multiple
/// identities don't collide.
pub(crate) fn invite_records_path(funder_hex: &str) -> std::path::PathBuf {
    let dir = key_home_dir()
        .and_then(|d| d.parent().map(|p| p.join("invites")))
        .unwrap_or_else(|| std::path::Path::new(".localharness").join("invites"));
    dir.join(format!("{}.tsv", funder_hex.to_ascii_lowercase()))
}

/// One locally-remembered invite a funder created: the bearer code, the amount
/// escrowed (wei), and the absolute unix expiry. The on-chain STATE is read
/// fresh per row (open/claimed/expired) — this is only the "which codes did I
/// make" memory the MVP facet doesn't index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InviteRecord {
    pub code: String,
    pub amount_wei: u128,
    pub expiry: u64,
}

/// Serialize an [`InviteRecord`] as one TSV line (`code\tamount_wei\texpiry`).
/// Pure + testable — the on-disk wire shape `parse_invite_records` reads back.
pub(crate) fn format_invite_record_line(r: &InviteRecord) -> String {
    format!("{}\t{}\t{}\n", r.code, r.amount_wei, r.expiry)
}

/// Parse the records file body (newline-separated `code\tamount\texpiry` lines)
/// into [`InviteRecord`]s, skipping any malformed/blank line rather than
/// failing the whole listing. Pure + testable.
pub(crate) fn parse_invite_records(body: &str) -> Vec<InviteRecord> {
    body.lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let code = parts.next()?.trim();
            let amount_wei = parts.next()?.trim().parse::<u128>().ok()?;
            let expiry = parts.next()?.trim().parse::<u64>().ok()?;
            if code.is_empty() {
                return None;
            }
            Some(InviteRecord { code: code.to_string(), amount_wei, expiry })
        })
        .collect()
}

/// Append a freshly-created invite to the funder's local records (best-effort —
/// a write failure only loses the `invite list` convenience, never the on-chain
/// escrow). Creates the records dir on demand.
pub(crate) fn record_invite(funder_hex: &str, r: &InviteRecord) {
    let path = invite_records_path(funder_hex);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut existing) = std::fs::read_to_string(&path) {
        if !existing.is_empty() && !existing.ends_with('\n') {
            existing.push('\n');
        }
        existing.push_str(&format_invite_record_line(r));
        let _ = std::fs::write(&path, existing);
    } else {
        let _ = std::fs::write(&path, format_invite_record_line(r));
    }
}

/// Read the funder's locally-remembered invites (empty when none / unreadable).
pub(crate) fn read_invite_records(funder_hex: &str) -> Vec<InviteRecord> {
    std::fs::read_to_string(invite_records_path(funder_hex))
        .map(|b| parse_invite_records(&b))
        .unwrap_or_default()
}

/// A short, non-secret hint of a bearer code for a listing row: keep the
/// `inv-<amount>-` prefix (already public in the share link) and the last 4
/// chars of the random tail, masking the middle. Never prints the full bearer
/// secret in a list. Pure + testable.
pub(crate) fn code_hint(code: &str) -> String {
    // Tail = everything after the second hyphen (the random part).
    match code.rsplit_once('-') {
        Some((prefix, tail)) if tail.len() > 4 => {
            format!("{prefix}-…{}", &tail[tail.len() - 4..])
        }
        _ => code.to_string(),
    }
}

/// Map an on-chain invite `status` (0 Open, 1 Accepted, 2 Reclaimed) + its
/// `expiry` to a one-word listing state, distinguishing an Open-but-expired
/// invite (reclaimable) from a live one. Pure + testable.
pub(crate) fn invite_row_state(status: u8, expiry: u64, now: u64) -> &'static str {
    match status {
        1 => "claimed",
        2 => "reclaimed",
        _ if expiry != 0 && expiry <= now => "expired",
        _ => "open",
    }
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
    // `--amount` is OPTIONAL: omitted → the STANDARD 2 $LH onboarding gift
    // (enough to claim a 1-$LH subdomain + ~1 $LH starting credit).
    let (amount_label, amount_wei) = match amount {
        None => ("2".to_string(), INVITE_DEFAULT_AMOUNT_WEI),
        Some(label) => {
            let wei = match localharness::encoding::parse_token_amount(&label) {
                Some(w) if w > 0 => w,
                _ => return Err(format!("--amount must be a positive $LH amount, got '{label}'")),
            };
            (label, wei)
        }
    };
    // Dust guard: below 0.01 $LH the escrow is real but every display rounds
    // it away — reject it outright rather than create a worthless invite.
    if amount_wei < INVITE_MIN_AMOUNT_WEI {
        return Err(format!(
            "--amount must be at least 0.01 $LH, got '{amount_label}'"
        ));
    }
    // Ceiling: invites are STANDARDIZED onboarding gifts, not bulk transfers —
    // cap at 10 $LH (use `send` for larger $LH moves). Kills the unbounded
    // "1000 LH invite".
    if amount_wei > INVITE_MAX_AMOUNT_WEI {
        return Err(format!(
            "--amount must be at most 10 $LH (invites are onboarding gifts; use `send` for larger transfers), got '{amount_label}'"
        ));
    }
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
    // The escrow pulls the amount from the WALLET pot — auto-bridge any
    // shortfall out of the chat meter first (on-chain feedback #63).
    let from_hex = bytes_to_hex_str(&wallet::address(&signer));
    if let Err(code) = ensure_wallet_covers(&signer, &from_hex, amount_wei).await {
        return code;
    }

    let code = gen_invite_code(&amount_label);
    let code_hash = registry::invite_code_hash(&code);
    println!(
        "creating invite for {} (expires in {}) …",
        fmt_lh(amount_wei),
        fmt_duration(ttl_secs)
    );
    match registry::create_invite_sponsored(
        &signer,
        &sponsor,
        code_hash,
        amount_wei,
        ttl_secs,
        registry::ALPHA_USD_ADDRESS(),
    )
    .await
    {
        Ok(tx) => {
            // Remember this code locally so `invite list` can show a per-invite
            // row (the chain stays the source of truth for STATE; the MVP facet
            // doesn't index invites by funder). Best-effort.
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            record_invite(
                &from_hex,
                &InviteRecord { code: code.clone(), amount_wei, expiry: now + ttl_secs },
            );
            println!("✓ invite created — {} escrowed, expires in {}", fmt_lh(amount_wei), fmt_duration(ttl_secs));
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

/// The READ-ONLY preflight verdict for `invite accept`. `getInvite` returns the
/// zero record (funder all-zero) for a code that was never created — the chain
/// would revert generically, so we name "no such invite", "already claimed",
/// and "expired" BEFORE broadcasting a tx that burns sponsor gas (feedback #85).
/// `funder` / `amount` / `expiry` / `status` are the decoded `getInvite` tuple
/// (status 0=Open, 1=Accepted, 2=Reclaimed); `now` is the current unix time.
/// Pure + testable.
pub(crate) fn invite_accept_preflight(
    funder: &str,
    expiry: u64,
    status: u8,
    now: u64,
) -> Result<(), String> {
    if funder.trim_start_matches("0x").chars().all(|c| c == '0') {
        return Err("no such invite — check the code (it's case-sensitive)".to_string());
    }
    match status {
        1 => return Err("this invite has already been claimed".to_string()),
        2 => return Err("this invite was reclaimed by its funder (expired unclaimed)".to_string()),
        _ => {}
    }
    if expiry != 0 && expiry <= now {
        return Err(
            "this invite has expired — its funder can reclaim it, but it can't be accepted"
                .to_string(),
        );
    }
    Ok(())
}

/// `invite accept <code>` — accept an invite; the escrowed `$LH` is paid to the
/// caller. The plaintext `code` is hashed on-chain to find the invite.
pub(crate) async fn invite_accept(caller: Option<&str>, code: &str) -> i32 {
    let code = code.trim();
    if code.is_empty() {
        eprintln!("invite accept: empty code");
        return 2;
    }
    // Read-only preflight: a fake / already-claimed / expired code reverts
    // on-chain and burns sponsor gas naming nothing (feedback #85). A failed
    // read is non-fatal — it falls through to the write.
    if let Ok((funder, _amount, expiry, status)) =
        registry::get_invite(registry::invite_code_hash(code)).await
    {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if let Err(msg) = invite_accept_preflight(&funder, expiry, status, now) {
            eprintln!("{msg}");
            return 1;
        }
    }
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    match registry::accept_invite_sponsored(&signer, &sponsor, code, registry::ALPHA_USD_ADDRESS()).await {
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
    match registry::reclaim_invite_sponsored(&signer, &sponsor, code_hash, registry::ALPHA_USD_ADDRESS()).await {
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

/// `invite list` — the caller's invites. Prints the on-chain outstanding-escrow
/// total (`escrowedOf`) AND, for each code this CLI remembers creating (the
/// MVP facet doesn't index invites by funder — `record_invite` keeps a local
/// list), a per-invite row: code hint, amount, expiry, and its live on-chain
/// state (open/claimed/expired). The chain is the source of truth for STATE;
/// the local file just remembers which codes you made (feedback #78/#81).
pub(crate) async fn invite_list(caller: Option<&str>) -> i32 {
    let signer = match load_signer(caller) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let addr = bytes_to_hex_str(&wallet::address(&signer));
    let escrowed = match registry::escrowed_of(&addr).await {
        Ok(e) => e,
        Err(e) => {
            eprintln!("invite list failed: {e}");
            return 1;
        }
    };
    println!("{addr}");
    println!("  escrowed  {}   <- $LH locked in your pending (Open) invites", fmt_lh(escrowed));

    // Per-invite rows from the local record (read each code's live on-chain
    // state). A row whose read fails shows "?" rather than sinking the listing.
    let records = read_invite_records(&addr);
    if records.is_empty() {
        if escrowed == 0 {
            println!("  no outstanding invites.");
        } else {
            println!("  reclaim an expired one with `invite reclaim <code>` to get its $LH back.");
        }
        println!("  (codes created on another device aren't listed here — keep those codes.)");
        return 0;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("  {} invite(s) you created from this device:", records.len());
    for r in &records {
        let state = match registry::get_invite(registry::invite_code_hash(&r.code)).await {
            Ok((_funder, _amount, expiry, status)) => invite_row_state(status, expiry, now),
            Err(_) => "?",
        };
        let expires = if r.expiry == 0 || r.expiry <= now {
            "—".to_string()
        } else {
            format!("in {}", fmt_duration(r.expiry - now))
        };
        println!(
            "    {hint}  {amount}  expires {expires}  [{state}]",
            hint = code_hint(&r.code),
            amount = fmt_lh(r.amount_wei),
        );
    }
    println!("  reclaim an expired one with `invite reclaim <code>` to get its $LH back.");
    0
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
        // Full: explicit amount + ttl (within the 0.01–10 range).
        let p = parse_invite_create_args(&args(&["--amount", "5", "--ttl", "30d"])).unwrap();
        assert_eq!(p.amount_label, "5");
        assert_eq!(p.amount_wei, 5 * 1_000_000_000_000_000_000);
        assert_eq!(p.ttl_secs, 30 * 86_400);
        // Fractional amount; --ttl defaults to 7d.
        let p = parse_invite_create_args(&args(&["--amount", "1.5"])).unwrap();
        assert_eq!(p.amount_wei, 1_500_000_000_000_000_000);
        assert_eq!(p.ttl_secs, INVITE_DEFAULT_TTL_SECS);
        // --amount is OPTIONAL → the STANDARD 2 $LH onboarding gift.
        let p = parse_invite_create_args(&args(&[])).unwrap();
        assert_eq!(p.amount_label, "2");
        assert_eq!(p.amount_wei, INVITE_DEFAULT_AMOUNT_WEI);
        let p = parse_invite_create_args(&args(&["--ttl", "30d"])).unwrap();
        assert_eq!(p.amount_wei, INVITE_DEFAULT_AMOUNT_WEI);
    }

    #[test]
    fn parse_invite_create_args_rejects_bad_input() {
        // Above the 10 $LH ceiling is rejected; the exact 10 is allowed.
        assert!(parse_invite_create_args(&args(&["--amount", "11"])).is_err());
        assert!(parse_invite_create_args(&args(&["--amount", "10.01"])).is_err());
        let p = parse_invite_create_args(&args(&["--amount", "10"])).unwrap();
        assert_eq!(p.amount_wei, INVITE_MAX_AMOUNT_WEI);
        // Zero / non-numeric amount.
        assert!(parse_invite_create_args(&args(&["--amount", "0"])).is_err());
        assert!(parse_invite_create_args(&args(&["--amount", "nope"])).is_err());
        // Dust below the 0.01 $LH minimum (1 wei used to escrow as "0.00 LH").
        let e = parse_invite_create_args(&args(&["--amount", "0.000000000000000001"]))
            .err()
            .expect("dust amount must be rejected");
        assert!(e.contains("at least 0.01"), "got: {e}");
        assert!(parse_invite_create_args(&args(&["--amount", "0.009"])).is_err());
        // The exact minimum is allowed.
        let p = parse_invite_create_args(&args(&["--amount", "0.01"])).unwrap();
        assert_eq!(p.amount_wei, INVITE_MIN_AMOUNT_WEI);
        // Out-of-range ttl bubbles up from parse_ttl.
        assert!(parse_invite_create_args(&args(&["--amount", "10", "--ttl", "30m"])).is_err());
        assert!(parse_invite_create_args(&args(&["--amount", "10", "--ttl", "91d"])).is_err());
        // Unknown flag.
        assert!(parse_invite_create_args(&args(&["--amount", "10", "--bogus"])).is_err());
    }

    #[test]
    fn invite_records_roundtrip_and_skip_malformed() {
        let r1 = InviteRecord { code: "inv-100-abcdefghij".into(), amount_wei: 100_000_000_000_000_000_000, expiry: 1_700_000_000 };
        let r2 = InviteRecord { code: "inv-0.5-zyxwvutsrq".into(), amount_wei: 500_000_000_000_000_000, expiry: 0 };
        let body = format!("{}{}", format_invite_record_line(&r1), format_invite_record_line(&r2));
        let parsed = parse_invite_records(&body);
        assert_eq!(parsed, vec![r1, r2]);
        // Malformed / blank lines are skipped, not fatal.
        let messy = "inv-1-aaaa\t1\t100\nnonsense\n\ninv-2-bbbb\tnotanumber\t9\n";
        let parsed = parse_invite_records(messy);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].code, "inv-1-aaaa");
    }

    #[test]
    fn code_hint_masks_the_secret_tail() {
        // Keeps the public prefix + last 4 of the random tail; masks the middle.
        assert_eq!(code_hint("inv-100-abcdefghij"), "inv-100-…ghij");
        assert_eq!(code_hint("inv-10.5-zyxwvutsrq"), "inv-10.5-…tsrq");
        // A short / odd code is returned whole rather than over-masked.
        assert_eq!(code_hint("inv-1-ab"), "inv-1-ab");
        assert_eq!(code_hint("weird"), "weird");
    }

    #[test]
    fn invite_row_state_distinguishes_open_expired_terminal() {
        let now = 1_000u64;
        // Open + future expiry → open; Open + past → expired (reclaimable).
        assert_eq!(invite_row_state(0, now + 100, now), "open");
        assert_eq!(invite_row_state(0, now - 1, now), "expired");
        // Unset expiry never reads expired.
        assert_eq!(invite_row_state(0, 0, now), "open");
        // Terminal states win regardless of expiry.
        assert_eq!(invite_row_state(1, now - 1, now), "claimed");
        assert_eq!(invite_row_state(2, now + 100, now), "reclaimed");
    }

    #[test]
    fn invite_accept_preflight_names_the_revert_cause() {
        let real = "0xabc0000000000000000000000000000000000001";
        let zero = "0x0000000000000000000000000000000000000000";
        let now = 1_000_000u64;
        // Open + unexpired → accept may proceed.
        assert!(invite_accept_preflight(real, now + 3600, 0, now).is_ok());
        // No funder → "no such invite".
        let e = invite_accept_preflight(zero, 0, 0, now).unwrap_err();
        assert!(e.contains("no such invite"), "got: {e}");
        // Already accepted / reclaimed are named distinctly.
        assert!(invite_accept_preflight(real, now + 3600, 1, now)
            .unwrap_err()
            .contains("already been claimed"));
        assert!(invite_accept_preflight(real, 0, 2, now)
            .unwrap_err()
            .contains("reclaimed"));
        // Open but past its expiry → can't be accepted.
        let e = invite_accept_preflight(real, now - 1, 0, now).unwrap_err();
        assert!(e.contains("expired"), "got: {e}");
        // expiry == 0 (unset) never trips the expiry branch.
        assert!(invite_accept_preflight(real, 0, 0, now).is_ok());
    }

    // ---- bounty arg parsing + row formatting --------------------------------
}
