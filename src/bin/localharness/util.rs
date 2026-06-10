#[allow(unused_imports)]
use crate::*;

/// Extract a `--as <name>` flag from ANYWHERE in the arg list (not just the
/// first position) and return `(caller, remaining_args_without_the_flag)`. The
/// remainder is owned so the flag can be removed from the middle. Position-
/// fragile parsing was a real bug: `probe --deep --as fleet` failed because
/// `--as` wasn't first, so the fleet name was never resolved and the call
/// errored with "multiple identities". A second `--as` is an error.
pub(crate) fn take_as_flag(args: &[String]) -> Result<(Option<String>, Vec<String>), String> {
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

/// Format `$LH` wei as a 2-decimal LH string.
pub(crate) fn fmt_lh(wei: u128) -> String {
    let whole = wei / 1_000_000_000_000_000_000u128;
    let cents = (wei % 1_000_000_000_000_000_000u128) / 10_000_000_000_000_000u128;
    format!("{whole}.{cents:02} LH")
}

/// Parse a bounty `id` argument (`#7` or `7`). Pure + testable.
pub(crate) fn parse_bounty_id(raw: &str) -> Result<u64, String> {
    raw.trim()
        .trim_start_matches('#')
        .parse::<u64>()
        .map_err(|_| format!("invalid bounty id '{raw}'"))
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

/// Resolve the caller's OWN registered tokenId — the `claimantTokenId` that earns
/// a bounty reward. Resolution order (each a `name.localharness.xyz` NFT the
/// caller controls):
///   1. If `--as <name>` was given AND that name is registered → its tokenId
///      (the explicit "act as THIS subdomain" intent).
///   2. Else the caller's MAIN identity (`mainOf(address)`), their primary NFT.
///   3. Else their single owned token (if they hold exactly one).
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

/// Extract an optional `--tba <name-or-0xaddr>` flag (from anywhere) and return
/// `(Option<value>, remaining)`. A second `--tba` is an error. Pure + testable;
/// `tba exec` uses it to OVERRIDE the acting TBA (default = caller's-main) with
/// an arbitrary owned TBA — a name (→ `tokenBoundAccountByName`) or a raw 0x addr.
pub(crate) fn take_tba_flag(args: &[String]) -> Result<(Option<String>, Vec<String>), String> {
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
pub(crate) fn take_data_flag(args: &[String]) -> Result<(Option<String>, Vec<String>), String> {
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

/// Parse a guild `id` argument (`#7` or `7`). Pure + testable (mirrors
/// `parse_bounty_id`).
pub(crate) fn parse_guild_id(raw: &str) -> Result<u64, String> {
    raw.trim()
        .trim_start_matches('#')
        .parse::<u64>()
        .map_err(|_| format!("invalid guild id '{raw}'"))
}

/// Parse a proposal `id` argument (`#7` or `7`). Pure + testable (mirrors
/// `parse_bounty_id` / `parse_guild_id`).
pub(crate) fn parse_proposal_id(raw: &str) -> Result<u64, String> {
    raw.trim()
        .trim_start_matches('#')
        .parse::<u64>()
        .map_err(|_| format!("invalid proposal id '{raw}'"))
}

/// Registry name rule = a valid DNS label: 1-63 chars, lowercase a-z / 0-9 /
/// hyphen, and NO leading/trailing hyphen (RFC 1035 — a label like `-foo` or
/// `foo-` is a dead-on-arrival subdomain). Surfaced by the test-user fleet
/// (juno-qa) — emoji/oversized were already caught, the hyphen edge was not.
pub(crate) fn name_is_valid(name: &str) -> bool {
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
