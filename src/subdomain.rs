//! Pure subdomain-name validation — the single source of truth for the browser
//! create tools, kept in sync with the on-chain
//! `LocalharnessRegistryFacet._isValidName` rule. Native-tested (this is why it
//! lives at the crate root, not inside the wasm-only `app` module).
//!
//! The bug this closes (GitHub #66/#60): the create path used to `sanitize()` a
//! requested name by silently DROPPING any char outside `[a-z0-9-]`, so asking
//! to register `café-shop` quietly minted `caf-shop` — a DIFFERENT name than
//! requested — and a leading/trailing hyphen sailed past the client only to
//! revert on-chain. `validate` instead REJECTS a name that isn't already a
//! valid DNS-safe label, returning a human-readable reason the caller (the
//! AGENT, via a tool error) can act on, rather than guessing.

/// Is `name` a routable DNS label (the registry/DNS-gateway invariant)?
///
/// THE canonical rule, shared by every mint path so no caller can spend
/// sponsored gas on an unroutable "zombie" name (on-chain feedback, juno-qa:
/// the registry minted labels >63 chars that the DNS gateway then silently
/// choked on). A valid label is **1–63 chars** of `[a-z0-9-]` with no
/// leading/trailing hyphen and ASCII only — RFC 1035, matching the contract's
/// `_isValidName`. Takes the name AS-IS (no normalization): a caller wanting
/// "normalize-or-reject" semantics uses [`validate`], which is strictly
/// tighter (it also caps at 32 and lowercases first). The CLI's
/// `name_is_valid` delegates here so the binary and the browser agree.
pub fn is_valid_subdomain_label(name: &str) -> bool {
    let len = name.len(); // ASCII past the all-ascii char check, so byte len == char len
    (1..=63).contains(&len)
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Validate + normalize a requested subdomain label.
///
/// Lowercases and trims (unsurprising normalization), then requires the result
/// to be a valid label: 3–32 chars, `[a-z0-9-]` only, no leading/trailing
/// hyphen, ASCII only. Returns the normalized name, or `Err(reason)` describing
/// the first violation (surfaced to the agent as a tool error — NOT painted as
/// form text). The 3–32 bound matches the app's existing create range; the
/// character/hyphen rule matches the contract's `_isValidName` and the
/// canonical [`is_valid_subdomain_label`] (which this is strictly tighter than).
pub fn validate(input: &str) -> Result<String, String> {
    let name = input.trim().to_ascii_lowercase();
    if !name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
        return Err(
            "use only lowercase letters, digits, and hyphens — no spaces, dots, or accented/unicode characters"
                .to_string(),
        );
    }
    // char count == byte count here (all-ASCII past the check above), but count
    // chars for a correct message regardless.
    let len = name.chars().count();
    if !(3..=32).contains(&len) {
        return Err(format!("name must be 3–32 characters (got {len})"));
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err("name can't start or end with a hyphen".to_string());
    }
    Ok(name)
}

/// Normalise a requested name to the character set the on-chain registry
/// enforces: trim, lowercase, DROP anything outside `[a-z0-9-]`, trim edge
/// hyphens. THE one cleaner (hoisted from `app::tenant`, which now delegates
/// here) so pure cores — `batch_apps`' duplicate rejection — share it instead
/// of forking the filter. Silently-mangling by design (the human claim form);
/// programmatic callers use [`validate`], which REJECTS instead.
pub fn sanitize(input: &str) -> String {
    let s: String = input
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    s.trim_matches('-').to_string()
}

/// What a batch `register` must do about the up-front `$LH` allowance —
/// the decision hoisted out of the wasm-only `app::events::subdomains` so
/// `cargo test` covers it (the `turn_flow`/`batch_apps` hoisting pattern).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive] // pub SDK surface — a new outcome must not be semver-breaking
pub enum BatchApproval {
    /// `registrationCost()` READ as zero — registration is genuinely free;
    /// submit the registers with no `approve` (the historical free-claim path).
    NoApproveNeeded,
    /// Prepend ONE `approve(diamond, amount)` (cumulative: cost × names).
    Approve(u128),
    /// Do NOT submit anything; surface this reason to the caller instead.
    Abort(String),
}

/// Decide the batch allowance from the `registrationCost()` READ RESULT.
///
/// The bug this closes: the call site did
/// `registration_cost().await.unwrap_or(0)`, so ONE flaky `eth_call` read as
/// "registration is free" — the cumulative approve was skipped and every
/// `register` in the sponsored batch then reverted on `transferFrom` with no
/// allowance, burning the sponsor gas. A failed read is NOT a zero price, so
/// it aborts BEFORE anything is submitted, naming the price read.
///
/// - `Err(e)` → [`BatchApproval::Abort`] (message mentions the price read and
///   that nothing was submitted).
/// - `Ok(0)` → [`BatchApproval::NoApproveNeeded`] — a genuine free config keeps
///   working exactly as before.
/// - `Ok(cost)` with `cost > 0` → [`BatchApproval::Approve`]`(cost × count)`.
/// - `count == 0` → [`BatchApproval::NoApproveNeeded`] at any price: an empty
///   batch pulls nothing, so `approve(0)` would be a wasted call.
/// - `cost × count` overflowing `u128` → [`BatchApproval::Abort`]. The previous
///   `saturating_mul` would have approved a saturated `u128::MAX` — an amount
///   nobody asked for and nobody holds — so failing honestly beats guessing.
pub fn plan_batch_approval(cost: Result<u128, String>, count: usize) -> BatchApproval {
    let cost = match cost {
        Ok(c) => c,
        Err(e) => {
            return BatchApproval::Abort(format!(
                "couldn't read the registration price (registrationCost): {e} — \
                 nothing was submitted and no gas was spent; try again"
            ))
        }
    };
    if cost == 0 || count == 0 {
        return BatchApproval::NoApproveNeeded;
    }
    match cost.checked_mul(count as u128) {
        Some(total) => BatchApproval::Approve(total),
        None => BatchApproval::Abort(format!(
            "the registration price read back as {cost} wei — × {count} names \
             overflows the allowance; nothing was submitted"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_labels() {
        assert_eq!(validate("alice").unwrap(), "alice");
        assert_eq!(validate("foo-bar").unwrap(), "foo-bar");
        assert_eq!(validate("a1b2c3").unwrap(), "a1b2c3");
        assert_eq!(validate(&"a".repeat(32)).unwrap(), "a".repeat(32));
    }

    #[test]
    fn normalizes_case_and_whitespace_unsurprisingly() {
        assert_eq!(validate("  Alice  ").unwrap(), "alice");
        assert_eq!(validate("MyAgent2").unwrap(), "myagent2");
    }

    #[test]
    fn rejects_unicode_instead_of_silently_mangling() {
        // The #66 repro: this used to mint "caf-shop"; now it's a clear error.
        assert!(validate("café-shop").is_err());
        assert!(validate("日本").is_err());
        assert!(validate("über").is_err());
    }

    #[test]
    fn rejects_spaces_and_dots() {
        assert!(validate("my cool app").is_err());
        assert!(validate("a.b.c").is_err());
        assert!(validate("under_score").is_err());
    }

    #[test]
    fn rejects_bad_length() {
        assert!(validate("ab").is_err()); // too short
        assert!(validate(&"a".repeat(33)).is_err()); // too long
        assert!(validate("").is_err());
    }

    #[test]
    fn rejects_edge_hyphens() {
        assert!(validate("-alice").is_err());
        assert!(validate("alice-").is_err());
        assert!(validate("--").is_err());
    }

    #[test]
    fn label_rule_blocks_unroutable_names() {
        // The juno-qa bug: a >63-char label is unroutable; the canonical rule
        // (1–63) must reject it BEFORE any mint spends sponsored gas.
        assert!(is_valid_subdomain_label("alice"));
        assert!(is_valid_subdomain_label("a")); // single char is a valid label
        assert!(is_valid_subdomain_label("a-b-c"));
        assert!(is_valid_subdomain_label(&"a".repeat(63))); // exactly the cap
        assert!(!is_valid_subdomain_label(&"a".repeat(64))); // the zombie — too long
        assert!(!is_valid_subdomain_label("")); // empty
        assert!(!is_valid_subdomain_label("Alice")); // uppercase
        assert!(!is_valid_subdomain_label("a_b")); // underscore
        assert!(!is_valid_subdomain_label("café")); // non-ascii
        assert!(!is_valid_subdomain_label("-foo")); // leading hyphen
        assert!(!is_valid_subdomain_label("foo-")); // trailing hyphen
        assert!(!is_valid_subdomain_label("-")); // only a hyphen
    }

    #[test]
    fn failed_price_read_aborts_instead_of_reading_as_free() {
        // THE bug: one flaky eth_call used to unwrap_or(0) into "free", the
        // approve was skipped, and every register reverted on transferFrom.
        let plan = plan_batch_approval(Err("rpc error: timeout".into()), 3);
        let BatchApproval::Abort(reason) = plan else {
            panic!("a failed price read must abort, got {plan:?}");
        };
        // Honest reason: names the price read AND the underlying error, and
        // says nothing was submitted (no gas burned).
        assert!(reason.contains("registrationCost"), "{reason}");
        assert!(reason.contains("price"), "{reason}");
        assert!(reason.contains("rpc error: timeout"), "{reason}");
        assert!(reason.contains("nothing was submitted"), "{reason}");
    }

    #[test]
    fn genuine_zero_still_registers_with_no_approve() {
        // A real free config must behave exactly as before — no approve call.
        assert_eq!(plan_batch_approval(Ok(0), 5), BatchApproval::NoApproveNeeded);
        assert_eq!(plan_batch_approval(Ok(0), 1), BatchApproval::NoApproveNeeded);
        // An empty batch pulls nothing, so approve(0) would be a wasted call.
        assert_eq!(plan_batch_approval(Ok(7), 0), BatchApproval::NoApproveNeeded);
    }

    #[test]
    fn paid_registration_approves_the_cumulative_total() {
        // The allowance is CUMULATIVE — cost × names covers the whole batch.
        let one_lh = 1_000_000_000_000_000_000u128;
        assert_eq!(plan_batch_approval(Ok(one_lh), 1), BatchApproval::Approve(one_lh));
        assert_eq!(plan_batch_approval(Ok(one_lh), 7), BatchApproval::Approve(one_lh * 7));
        assert_eq!(plan_batch_approval(Ok(3), 28), BatchApproval::Approve(84));
    }

    #[test]
    fn overflowing_total_aborts_rather_than_approving_a_saturated_amount() {
        // The old saturating_mul would have approved u128::MAX here — an
        // amount nobody asked for. Fail honestly instead.
        let plan = plan_batch_approval(Ok(u128::MAX), 2);
        let BatchApproval::Abort(reason) = plan else {
            panic!("an overflowing total must abort, got {plan:?}");
        };
        assert!(reason.contains("overflows"), "{reason}");
        assert!(reason.contains("nothing was submitted"), "{reason}");
        // The exact boundary still approves: max/2 × 2 fits.
        assert_eq!(
            plan_batch_approval(Ok(u128::MAX / 2), 2),
            BatchApproval::Approve((u128::MAX / 2) * 2)
        );
    }

    #[test]
    fn validate_is_strictly_tighter_than_the_label_rule() {
        // Anything `validate` accepts is a routable label (the mint invariant);
        // `validate` additionally caps at 32 and lowercases first.
        for ok in ["alice", "foo-bar", &"a".repeat(32)] {
            let normalized = validate(ok).unwrap();
            assert!(
                is_valid_subdomain_label(&normalized),
                "validate accepted an unroutable label: {normalized}"
            );
        }
        // 33–63 chars pass the label rule but `validate` rejects (the 32 cap).
        assert!(is_valid_subdomain_label(&"a".repeat(40)));
        assert!(validate(&"a".repeat(40)).is_err());
    }
}
