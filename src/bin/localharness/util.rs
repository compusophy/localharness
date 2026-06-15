#[allow(unused_imports)]
use crate::*;

/// Extract a single `<flag> <value>` pair from ANYWHERE in the arg list and
/// return `(value, remaining_args_without_the_pair)`. The remainder is owned so
/// the pair can be removed from the middle — position-fragile parsing was a real
/// bug (`probe --deep --as fleet` missed a non-leading `--as`). A repeated flag
/// is an error ("{flag} given more than once"); a dangling flag errs with the
/// caller-supplied `usage` line. The shared engine behind
/// [`take_as_flag`] / [`take_tba_flag`] / [`take_data_flag`].
pub(crate) fn take_value_flag(
    args: &[String],
    flag: &str,
    usage: &str,
) -> Result<(Option<String>, Vec<String>), String> {
    let mut value: Option<String> = None;
    let mut rest: Vec<String> = Vec::with_capacity(args.len());
    let mut i = 0;
    while i < args.len() {
        if args[i] == flag {
            if value.is_some() {
                return Err(format!("{flag} given more than once"));
            }
            match args.get(i + 1) {
                Some(v) => {
                    value = Some(v.clone());
                    i += 2;
                }
                None => return Err(usage.to_string()),
            }
        } else {
            rest.push(args[i].clone());
            i += 1;
        }
    }
    Ok((value, rest))
}

/// Extract a `--as <name>` flag (the acting identity) from anywhere in the args.
pub(crate) fn take_as_flag(args: &[String]) -> Result<(Option<String>, Vec<String>), String> {
    take_value_flag(args, "--as", "usage: --as <name> requires a name")
}

/// Format `$LH` wei as a 2-decimal LH string. A nonzero amount below 0.01
/// renders as `<0.01 LH` — truncating dust to a false "0.00 LH" let a 1-wei
/// invite print as escrowing nothing (the fleet's dust-invite lie). Every
/// command's `$LH` display flows through here, so they all get the fix.
pub(crate) fn fmt_lh(wei: u128) -> String {
    const ONE_LH: u128 = 1_000_000_000_000_000_000;
    const ONE_CENT: u128 = ONE_LH / 100; // 0.01 $LH
    if wei > 0 && wei < ONE_CENT {
        return "<0.01 LH".to_string();
    }
    let whole = wei / ONE_LH;
    let cents = (wei % ONE_LH) / ONE_CENT;
    format!("{whole}.{cents:02} LH")
}

/// Render a duration in SECONDS as a compact human string: `57m` / `1h 2m` /
/// `2d 3h` / `45s`. The shared duration renderer for everywhere a `$LH` op
/// prints a TTL / expiry / cadence (on-chain feedback #82/#83: raw `3422s` and
/// `--ttl 1h → seconds` were unreadable). Two units max, coarsest first, the
/// finer unit dropped once it would read zero (e.g. exactly 2 days = `2d`, not
/// `2d 0h`). Pure + testable.
pub(crate) fn fmt_duration(secs: u64) -> String {
    const MIN: u64 = 60;
    const HOUR: u64 = 3600;
    const DAY: u64 = 86_400;
    if secs == 0 {
        return "0s".to_string();
    }
    if secs < MIN {
        return format!("{secs}s");
    }
    if secs < HOUR {
        let (m, s) = (secs / MIN, secs % MIN);
        return if s == 0 { format!("{m}m") } else { format!("{m}m {s}s") };
    }
    if secs < DAY {
        let (h, m) = (secs / HOUR, (secs % HOUR) / MIN);
        return if m == 0 { format!("{h}h") } else { format!("{h}h {m}m") };
    }
    let (d, h) = (secs / DAY, (secs % DAY) / HOUR);
    if h == 0 { format!("{d}d") } else { format!("{d}d {h}h") }
}

/// Truncate `text` to at most `max` characters at a WORD BOUNDARY, appending an
/// explicit `…` ellipsis when anything was cut. Newlines collapse to spaces
/// first (a one-line preview). Cutting mid-word was an accessibility complaint
/// (on-chain feedback #93/#95: a screen reader can't parse a clipped token), so
/// we back up to the last whitespace within the budget; if a single word is
/// longer than `max` we hard-cut it (rather than emit nothing). Pure + testable.
pub(crate) fn truncate_words(text: &str, max: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    // Take `max` chars, then back up to the last space so we never split a word.
    let head: String = flat.chars().take(max).collect();
    let cut = match head.rfind(' ') {
        Some(idx) if idx > 0 => head[..idx].trim_end().to_string(),
        // No interior space (one long word) → hard cut at the char budget.
        _ => head.trim_end().to_string(),
    };
    format!("{cut}…")
}

/// Reject an empty / whitespace-only user text BEFORE any identity or meter
/// work — a blank `call <name> ""` used to run (and BILL) a full metered turn.
/// `label` names the offending input in the one-line error (e.g. "call:
/// message"). Pure + testable; callers print the `Err` and exit 1.
pub(crate) fn non_blank(text: &str, label: &str) -> Result<(), String> {
    if text.trim().is_empty() {
        Err(format!("{label} is empty — nothing to send"))
    } else {
        Ok(())
    }
}

/// Parse a numeric `id` argument (`#7` or `7`) for the given `noun` (the error
/// reads "invalid {noun} id '{raw}'"). Pure + testable — the shared engine
/// behind [`parse_bounty_id`] / [`parse_guild_id`] / [`parse_proposal_id`].
pub(crate) fn parse_id(raw: &str, noun: &str) -> Result<u64, String> {
    raw.trim()
        .trim_start_matches('#')
        .parse::<u64>()
        .map_err(|_| format!("invalid {noun} id '{raw}'"))
}

/// Parse a bounty `id` argument (`#7` or `7`).
pub(crate) fn parse_bounty_id(raw: &str) -> Result<u64, String> {
    parse_id(raw, "bounty")
}

/// Load the caller's identity signer alone, mapping any failure to a process
/// exit code (resolve failure = 2, unparseable key = 1). The shared front-half
/// of every READ-ONLY command that still needs a local key (credits / status /
/// list / jobs / `* mine` / probe) — the sponsored-write twin is
/// [`load_signer_and_sponsor`].
pub(crate) fn load_signer(caller: Option<&str>) -> Result<k256::ecdsa::SigningKey, i32> {
    let (key_file, key_hex) = resolve_caller_key(caller).map_err(|e| {
        eprintln!("{e}");
        2
    })?;
    wallet::from_private_key_hex(&key_hex).map_err(|e| {
        eprintln!("bad key in {key_file}: {e}");
        1
    })
}

/// Load the embedded sponsor key (exit 1 on a parse failure — never happens in
/// practice; the const is guarded by a unit test).
pub(crate) fn load_sponsor() -> Result<k256::ecdsa::SigningKey, i32> {
    wallet::from_private_key_hex(SPONSOR_KEY).map_err(|e| {
        eprintln!("sponsor key error: {e}");
        1
    })
}

/// Load `<name>`'s identity signer from its key file (cwd or config home),
/// mapping any failure to exit 1. The NAME-keyed flavor `face`/`persona` use
/// (vs the caller-keyed [`load_signer`]): a missing key here is a "run create
/// first" error, not a resolve/usage error.
pub(crate) fn load_name_signer(name: &str) -> Result<k256::ecdsa::SigningKey, i32> {
    let Some(key_file) = resolve_key_read_path(name) else {
        eprintln!("no identity key for {name} — run `localharness create {name}` first");
        return Err(1);
    };
    let key_hex = match std::fs::read_to_string(&key_file) {
        Ok(s) => s.trim().to_string(),
        Err(_) => {
            eprintln!("no identity key at {key_file} — run `localharness create {name}` first");
            return Err(1);
        }
    };
    wallet::from_private_key_hex(&key_hex).map_err(|e| {
        eprintln!("bad key in {key_file}: {e}");
        1
    })
}

/// Load the caller's identity signer + the embedded sponsor in one shot, mapping
/// any failure to a process exit code. The shared front-half of every sponsored
/// write (bounty / guild / vote / invite / schedule / credits / …).
pub(crate) fn load_signer_and_sponsor(
    caller: Option<&str>,
) -> Result<(k256::ecdsa::SigningKey, k256::ecdsa::SigningKey), i32> {
    Ok((load_signer(caller)?, load_sponsor()?))
}

/// Ensure the caller's WALLET holds at least `needed_wei` `$LH` before a
/// `transferFrom`-pulling write (x402 `settle`, or any escrow: bounty post /
/// invite create / schedule / goal / guild fund). When the wallet is short,
/// the AUTO-BRIDGE pulls the shortfall back out of the caller's unspent
/// chat-meter credits via a sponsored `withdrawCredits` (the meter and the
/// wallet are one balance in practice — on-chain feedback #63). Prints its
/// own messages; `Err` carries the process exit code. A balance-read failure
/// only falls through — the on-chain pull is the authoritative gate.
pub(crate) async fn ensure_wallet_covers(
    signer: &k256::ecdsa::SigningKey,
    from_hex: &str,
    needed_wei: u128,
) -> Result<(), i32> {
    let Ok(wallet_wei) = registry::token_balance_of(from_hex).await else {
        return Ok(());
    };
    if wallet_wei >= needed_wei {
        return Ok(());
    }
    let shortfall = needed_wei - wallet_wei;
    let meter_wei = registry::credit_balance_of(from_hex).await.unwrap_or(0);
    if meter_wei < shortfall {
        eprintln!(
            "this needs {} but your wallet holds {} and your chat meter {} — \
             fund up with `localharness redeem <code>` or a $LH transfer first",
            fmt_lh(needed_wei),
            fmt_lh(wallet_wei),
            fmt_lh(meter_wei),
        );
        return Err(1);
    }
    println!(
        "wallet is short {} — pulling it from your unspent chat credits …",
        fmt_lh(shortfall)
    );
    let sponsor = load_sponsor()?;
    match registry::withdraw_credits_sponsored(
        signer,
        &sponsor,
        shortfall,
        registry::ALPHA_USD_ADDRESS,
    )
    .await
    {
        Ok(tx) => {
            println!("  withdrawn (tx {tx})");
            Ok(())
        }
        Err(e) => {
            eprintln!("could not withdraw credits automatically: {e}");
            Err(1)
        }
    }
}

/// Resolve the caller's OWN registered tokenId — the `claimantTokenId` that earns
/// a bounty reward. Resolution order (each a `name.localharness.xyz` NFT the
/// caller controls):
///   1. If `--as <name>` was given AND that name is registered → its tokenId
///      (the explicit "act as THIS subdomain" intent).
///   2. Else the caller's MAIN identity (`mainOf(address)`), their primary NFT.
///   3. Else their single owned token (if they hold exactly one).
///
/// A caller with NO registered identity can't claim — they must `create <name>`
/// first (the reward needs an on-chain identity to be paid to).
pub(crate) async fn resolve_own_token_id(
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
    let addr = bytes_to_hex_str(&wallet::address(signer));
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

/// Best-effort reverse-resolve a 0x ADDRESS to a display label: its MAIN
/// identity's name (`main_of` → `name_of_id`) when it has one, else the bare
/// address. So a poster / counterparty shown as a raw `0x…` gets the same
/// human name a claimant does (on-chain feedback #82/#83). All reads degrade to
/// the address on any failure — never sinks the caller.
pub(crate) async fn resolve_address_label(addr_hex: &str) -> String {
    if let Ok(main_id) = registry::main_of(addr_hex).await {
        if main_id != 0 {
            if let Ok(name) = registry::name_of_id(main_id).await {
                if !name.is_empty() {
                    return format!("{name} ({addr_hex})");
                }
            }
        }
    }
    addr_hex.to_string()
}

/// Extract an optional `--tba <name-or-0xaddr>` flag (from anywhere). `tba exec`
/// uses it to OVERRIDE the acting TBA (default = caller's-main) with an arbitrary
/// owned TBA — a name (→ `tokenBoundAccountByName`) or a raw 0x addr.
pub(crate) fn take_tba_flag(args: &[String]) -> Result<(Option<String>, Vec<String>), String> {
    take_value_flag(args, "--tba", "usage: --tba <name-or-0xaddr> requires a value")
}

/// Extract an optional `--data <hex>` flag (from anywhere).
pub(crate) fn take_data_flag(args: &[String]) -> Result<(Option<String>, Vec<String>), String> {
    take_value_flag(args, "--data", "usage: --data <hex> requires a value")
}

/// Walk an arg list ONCE, extracting the values of the named `flags` (returned
/// in the same order; a repeated flag's LAST value wins, matching the old
/// per-command loops) and collecting everything else as positionals in order.
/// A flag missing its value errs with the caller's `usage` line — exactly the
/// `ok_or(USAGE)?` each loop used. The shared flag-walk skeleton behind
/// `parse_schedule_args` / `parse_colony_run_args` / `parse_bounty_post_args` /
/// `parse_vote_propose_args`. (NOT used by `parse_invite_create_args`, which
/// REJECTS unknown args instead of collecting them, nor by the leading-flag
/// parsers `parse_call_args` / `parse_mcp_call_args`, which stop at the first
/// positional.)
pub(crate) fn collect_flags<const N: usize>(
    rest: &[String],
    flags: [&str; N],
    usage: &str,
) -> Result<([Option<String>; N], Vec<String>), String> {
    let mut values: [Option<String>; N] = std::array::from_fn(|_| None);
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        if let Some(slot) = flags.iter().position(|f| *f == rest[i]) {
            values[slot] = Some(rest.get(i + 1).ok_or(usage)?.clone());
            i += 2;
        } else {
            positional.push(rest[i].clone());
            i += 1;
        }
    }
    Ok((values, positional))
}

/// Decode a `--data` hex argument into bytes. Accepts an optional `0x` prefix;
/// rejects odd-length / non-hex with a clear message (never panics). Empty
/// (`""` / `0x`) decodes to no bytes — a value-only call.
pub(crate) fn decode_hex_arg(raw: &str) -> Result<Vec<u8>, String> {
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

/// Parse a guild `id` argument (`#7` or `7`).
pub(crate) fn parse_guild_id(raw: &str) -> Result<u64, String> {
    parse_id(raw, "guild")
}

/// Parse a proposal `id` argument (`#7` or `7`).
pub(crate) fn parse_proposal_id(raw: &str) -> Result<u64, String> {
    parse_id(raw, "proposal")
}

/// Registry name rule = a valid DNS label: 1-63 chars, lowercase a-z / 0-9 /
/// hyphen, and NO leading/trailing hyphen (RFC 1035 — a label like `-foo` or
/// `foo-` is a dead-on-arrival subdomain). Surfaced by the test-user fleet
/// (juno-qa) — emoji/oversized were already caught, the hyphen edge was not.
/// Delegates to the library's canonical `subdomain::is_valid_subdomain_label`
/// so the CLI and the browser mint paths enforce the SAME rule (one source of
/// truth — the rule used to be forked here, drifting from the app).
pub(crate) fn name_is_valid(name: &str) -> bool {
    localharness::subdomain::is_valid_subdomain_label(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_lh_never_shows_a_false_zero() {
        // True zero stays "0.00 LH" …
        assert_eq!(fmt_lh(0), "0.00 LH");
        // … but nonzero dust below a cent must NOT print as nothing: a 1-wei
        // invite printed "0.00 LH" while really escrowing value (fleet bug).
        assert_eq!(fmt_lh(1), "<0.01 LH");
        assert_eq!(fmt_lh(9_999_999_999_999_999), "<0.01 LH"); // 1 wei under a cent
        // From exactly one cent up, the 2-decimal display is unchanged.
        assert_eq!(fmt_lh(10_000_000_000_000_000), "0.01 LH");
        assert_eq!(fmt_lh(1_000_000_000_000_000_000), "1.00 LH");
        assert_eq!(fmt_lh(10_500_000_000_000_000_000), "10.50 LH");
        // Sub-cent residue on a non-dust amount still truncates (not false-zero).
        assert_eq!(fmt_lh(1_005_000_000_000_000_000), "1.00 LH");
    }

    #[test]
    fn fmt_duration_renders_compact_two_unit_spans() {
        assert_eq!(fmt_duration(0), "0s");
        assert_eq!(fmt_duration(45), "45s");
        // The exact complaint: a bare "3422s" must read as "57m 2s".
        assert_eq!(fmt_duration(3422), "57m 2s");
        assert_eq!(fmt_duration(60), "1m");
        assert_eq!(fmt_duration(90), "1m 30s");
        assert_eq!(fmt_duration(3600), "1h");
        assert_eq!(fmt_duration(3720), "1h 2m");
        assert_eq!(fmt_duration(86_400), "1d");
        assert_eq!(fmt_duration(183_600), "2d 3h"); // 2d 3h exactly
        // Coarsest-first, finer unit dropped at a clean boundary.
        assert_eq!(fmt_duration(2 * 86_400), "2d");
        assert_eq!(fmt_duration(7 * 86_400), "7d");
    }

    #[test]
    fn truncate_words_cuts_at_word_boundary_with_ellipsis() {
        // Under the budget → unchanged (just flattened).
        assert_eq!(truncate_words("short task", 50), "short task");
        // Newlines collapse to a single-line preview.
        assert_eq!(truncate_words("line one\n  line two", 50), "line one line two");
        // Over budget → cut at the last full word, never mid-word, with an ellipsis.
        let out = truncate_words("audit the solidity contract for reentrancy bugs", 20);
        assert_eq!(out, "audit the solidity…");
        assert!(!out.contains("contrac"), "must not split a word: {out}");
        // A single oversized word hard-cuts (better than emitting nothing).
        let long = truncate_words("supercalifragilisticexpialidocious", 10);
        assert_eq!(long, "supercalif…");
        // Exactly at the budget → no ellipsis.
        assert_eq!(truncate_words("12345", 5), "12345");
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
    fn non_blank_rejects_empty_and_whitespace_only() {
        // The billed-empty-call bug: `call <name> ""` ran a full metered turn.
        assert!(non_blank("hi", "call: message").is_ok());
        assert_eq!(
            non_blank("", "call: message"),
            Err("call: message is empty — nothing to send".to_string())
        );
        assert!(non_blank("   ", "mcp-call: message").is_err());
        assert!(non_blank("\t\n", "schedule: task").is_err());
        // Real content surrounded by whitespace is fine.
        assert!(non_blank("  x  ", "goal: task").is_ok());
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
    fn parse_addr20_roundtrips_registry_address() {
        // The CLI parses addresses via the crate-root `encoding::parse_address`
        // (the old local `parse_addr20` duplicate is gone); pin the behavior the
        // call sites rely on against the canonical registry address.
        let a = parse_address(registry::REGISTRY_ADDRESS).expect("valid registry addr");
        assert_eq!(a.len(), 20);
        // Case-insensitive, 0x-optional.
        assert!(parse_address("0x00").is_err()); // wrong length
        assert!(parse_address(&"0".repeat(40)).is_ok());
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

    #[test]
    fn parse_proposal_id_accepts_hash_and_bare() {
        assert_eq!(parse_proposal_id("7"), Ok(7));
        assert_eq!(parse_proposal_id("#42"), Ok(42));
        assert_eq!(parse_proposal_id("  #3  "), Ok(3));
        assert!(parse_proposal_id("nope").is_err());
        assert!(parse_proposal_id("").is_err());
    }
}
