#[allow(unused_imports)]
use crate::*;

// ---- reputation (attestation-based on-chain agent reputation) -------------
//
// A peer-attestation reputation primitive over ReputationFacet: `reputation
// show <agent>` reads an agent's running `(count, sum)` + recent attestations;
// `reputation attest <agent> <rating> [--ref ...]` records a 1-5 rating about a
// piece of work (a bounty id or a 0x ref). The colony engine's [7/7] step
// auto-attests the worker, so the demand flywheel keeps reputation flowing.

pub(crate) const REPUTATION_USAGE: &str = "\
usage: localharness reputation <show|attest> ...   (alias: rep)
  reputation show <agent>                              an agent's count, avg rating, recent attestations
  reputation attest [--as <me>] <agent> <rating 1-5> [--ref <hex|bountyId>]
                                                       attest to an agent you've worked with (1-5)
  --ref tags the work: a bounty id (left-padded to bytes32) or a 0x… 32-byte ref;
  it defaults to a zero ref. You can't attest to yourself or re-attest the same
  (agent, ref) pair.";

/// Turn a `--ref` argument into a `bytes32` workRef: a `0x…` value is parsed as a
/// raw 32-byte ref (left-padded if shorter); a bare integer is treated as a bounty
/// id and left-padded big-endian into the low 8 bytes (the SAME `bytes32(bountyId)`
/// the colony [7/7] step uses). `None` → the zero ref. Pure + testable.
pub(crate) fn parse_work_ref(raw: Option<&str>) -> Result<[u8; 32], String> {
    let mut out = [0u8; 32];
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(out); // default: zero ref
    };
    if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
        if hex.is_empty() || hex.len() > 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("--ref hex must be 1..32 bytes of hex, got '{raw}'"));
        }
        // Right-align the supplied bytes (left-pad with zeros) into the 32-byte word.
        let bytes = hex_to_bytes_padded(hex)?;
        out[32 - bytes.len()..].copy_from_slice(&bytes);
        return Ok(out);
    }
    // A bare integer → bounty id, left-padded big-endian into the low 8 bytes.
    match raw.trim_start_matches('#').parse::<u64>() {
        Ok(id) => {
            out[24..32].copy_from_slice(&id.to_be_bytes());
            Ok(out)
        }
        Err(_) => Err(format!(
            "--ref must be a 0x… hex ref or a bounty id (integer), got '{raw}'"
        )),
    }
}

/// `bytes32(bountyId)` — a bounty id left-padded big-endian into the low 8 bytes
/// of a 32-byte word, the canonical workRef the colony [7/7] step attests with.
pub(crate) fn bounty_work_ref(bounty_id: u64) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[24..32].copy_from_slice(&bounty_id.to_be_bytes());
    out
}

/// `localharness reputation <subcommand>` — the reputation router.
pub(crate) async fn reputation(caller: Option<&str>, rest: &[String]) -> i32 {
    match rest.first().map(String::as_str) {
        Some("show") => match rest.get(1) {
            Some(agent) => reputation_show(agent).await,
            None => {
                eprintln!("usage: localharness reputation show <agent>");
                2
            }
        },
        Some("attest") => reputation_attest(caller, &rest[1..]).await,
        _ => {
            eprintln!("{REPUTATION_USAGE}");
            2
        }
    }
}

/// `reputation show <agent>` — resolve the name→tokenId, then print its
/// attestation count, average rating (sum/count), and recent attestations.
/// Read-only, no `$LH`.
pub(crate) async fn reputation_show(agent: &str) -> i32 {
    let token_id = match registry::id_of_name(agent).await {
        Ok(0) | Err(_) => {
            eprintln!("reputation show: '{agent}' is not a registered agent (check the name)");
            return 1;
        }
        Ok(id) => id,
    };
    let (count, sum) = match registry::reputation_of(token_id).await {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("RPC error reading reputation: {e}");
            return 1;
        }
    };
    println!("reputation of {agent} (token #{token_id}):");
    if count == 0 {
        println!("  no attestations yet — be the first with `reputation attest {agent} <1-5>`");
        return 0;
    }
    // Average to 2 dp without floats: (sum*100)/count rounded.
    let avg_x100 = (sum * 100 + count / 2) / count;
    println!("  attestations: {count}");
    println!("  average rating: {}.{:02} / 5  (sum {sum})", avg_x100 / 100, avg_x100 % 100);
    // Recent attestations (the head of the list).
    match registry::attestations_of(token_id, 0, REPUTATION_SHOW_LIMIT).await {
        Ok(rows) if !rows.is_empty() => {
            println!("  recent attestations:");
            for (attester, rating, work_ref) in rows {
                // Surface a bounty-id workRef compactly when the high bytes are 0.
                let ref_note = format_work_ref(&work_ref);
                println!("    {rating}★  by {attester}{ref_note}");
            }
        }
        Ok(_) => {}
        Err(e) => println!("  (could not list attestations: {e})"),
    }
    0
}

/// How many recent attestations `reputation show` lists. A small page head — the
/// list is small at launch scale.
pub(crate) const REPUTATION_SHOW_LIMIT: u64 = 10;

/// Render a workRef for display: a zero ref shows nothing; a ref whose high 24
/// bytes are zero is shown as its low-8 bounty id; otherwise the full 0x-hex.
/// Pure (operates on the `0x…` string from `attestations_of`).
pub(crate) fn format_work_ref(work_ref_hex: &str) -> String {
    let hex = work_ref_hex.trim_start_matches("0x");
    if hex.len() != 64 || hex.chars().all(|c| c == '0') {
        return String::new(); // zero / malformed ref → no note
    }
    // High 48 nibbles (24 bytes) zero → a left-padded integer (bounty id).
    if hex[..48].chars().all(|c| c == '0') {
        if let Ok(id) = u64::from_str_radix(&hex[48..], 16) {
            return format!("  (work #{id})");
        }
    }
    format!("  (ref 0x{}…)", &hex[..8])
}

/// `reputation attest <agent> <rating 1-5> [--ref <hex|bountyId>]` — attest to an
/// agent you've worked with. Resolves the agent name→tokenId, signs `attest` as
/// the caller, and surfaces a duplicate/self/bad-rating revert clearly.
pub(crate) async fn reputation_attest(caller: Option<&str>, rest: &[String]) -> i32 {
    // Positional: <agent> <rating>; flag: --ref <value>.
    let mut positional: Vec<&str> = Vec::new();
    let mut work_ref_arg: Option<&str> = None;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--ref" => {
                match rest.get(i + 1) {
                    Some(v) => work_ref_arg = Some(v),
                    None => {
                        eprintln!("--ref needs a value\n{REPUTATION_USAGE}");
                        return 2;
                    }
                }
                i += 2;
            }
            other => {
                positional.push(other);
                i += 1;
            }
        }
    }
    let (agent, rating_arg) = match positional.as_slice() {
        [agent, rating] => (*agent, *rating),
        _ => {
            eprintln!("usage: localharness reputation attest [--as <me>] <agent> <rating 1-5> [--ref <hex|bountyId>]");
            return 2;
        }
    };
    let rating = match rating_arg.trim().parse::<u8>() {
        Ok(r) if (1..=5).contains(&r) => r,
        _ => {
            eprintln!("rating must be an integer 1-5, got '{rating_arg}'");
            return 2;
        }
    };
    let work_ref = match parse_work_ref(work_ref_arg) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };

    let token_id = match registry::id_of_name(agent).await {
        Ok(0) | Err(_) => {
            eprintln!("reputation attest: '{agent}' is not a registered agent (check the name)");
            return 1;
        }
        Ok(id) => id,
    };
    let (signer, sponsor) = match load_signer_and_sponsor(caller) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    println!("attesting {rating}★ to {agent} (token #{token_id}) …");
    match registry::attest_sponsored(
        &signer,
        &sponsor,
        token_id,
        rating,
        work_ref,
        registry::ALPHA_USD_ADDRESS,
    )
    .await
    {
        Ok(tx) => {
            println!("✓ attested {rating}★ to {agent} — it's now on-chain  tx: {tx}");
            println!("  see it with `reputation show {agent}`.");
            0
        }
        Err(e) => {
            eprintln!("reputation attest failed: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_work_ref_handles_none_hex_and_bounty_id() {
        // None → the zero ref.
        assert_eq!(parse_work_ref(None), Ok([0u8; 32]));
        assert_eq!(parse_work_ref(Some("   ")), Ok([0u8; 32]));
        // A bare integer (or #N) → bounty id left-padded into the low 8 bytes — the
        // SAME bytes `bounty_work_ref` produces (what the colony [7/7] step uses).
        assert_eq!(parse_work_ref(Some("7")).unwrap(), bounty_work_ref(7));
        assert_eq!(parse_work_ref(Some("#42")).unwrap(), bounty_work_ref(42));
        let r7 = parse_work_ref(Some("7")).unwrap();
        assert_eq!(&r7[24..32], &7u64.to_be_bytes());
        assert!(r7[..24].iter().all(|&b| b == 0));
        // A 0x hex ref is right-aligned (left-padded with zeros).
        let r = parse_work_ref(Some("0xabcd")).unwrap();
        assert_eq!(r[30], 0xab);
        assert_eq!(r[31], 0xcd);
        assert!(r[..30].iter().all(|&b| b == 0));
        // A full 32-byte hex ref is preserved as-is.
        let full = "0x".to_string() + &"cd".repeat(32);
        assert_eq!(parse_work_ref(Some(&full)).unwrap(), [0xcd; 32]);
        // Rejects: over-long hex, non-hex, and a non-integer non-hex token.
        assert!(parse_work_ref(Some(&("0x".to_string() + &"ab".repeat(33)))).is_err());
        assert!(parse_work_ref(Some("0xzz")).is_err());
        assert!(parse_work_ref(Some("notanid")).is_err());
    }

    #[test]
    fn format_work_ref_renders_bounty_id_and_zero() {
        // Zero ref → no note.
        assert_eq!(format_work_ref(&format!("0x{}", "0".repeat(64))), "");
        // A bounty-id ref (high 24 bytes zero) → "(work #N)".
        let id_ref = format!("0x{}{:016x}", "0".repeat(48), 9u64);
        assert_eq!(format_work_ref(&id_ref), "  (work #9)");
        // A ref with non-zero high bytes → a truncated 0x note.
        let mixed = format!("0xcd{}", "0".repeat(62));
        assert_eq!(format_work_ref(&mixed), "  (ref 0xcd000000…)");
        // Malformed length → no note (no panic).
        assert_eq!(format_work_ref("0xdead"), "");
    }
}
