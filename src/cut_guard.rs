//! Static safety lint for agent-authored facet cuts — design `soliditylite.md`
//! §7 Layer 1, the "immune system" first line.
//!
//! PURE + native-testable (no chain, no deps): given the facet's new selectors,
//! the target diamond's existing selectors, and whether the cut carries an
//! `_init` delegatecall, decide whether the cut is SAFE to submit — BEFORE
//! spending gas (a selector clash reverts the whole cut) and BEFORE an agent can
//! brick or seize a diamond. Wired into `localharness facet cut` as a pre-flight;
//! reused by the (future) browser cut tool. This is advisory off-chain defense —
//! the on-chain registrar must re-enforce the reserved-selector + `_init==0`
//! rules for cuts an agent signs directly (§7).

/// Diamond CORE selectors an agent cut must NEVER Add/Replace/Remove: the cut
/// facet, ownership, and the full loupe set. Adding/replacing these is how a
/// malicious facet seizes ownership, swaps the cut logic, or blinds the loupe.
pub const RESERVED_SELECTORS: [[u8; 4]; 8] = [
    [0x1f, 0x93, 0x1c, 0x1c], // diamondCut((address,uint8,bytes4[])[],address,bytes)
    [0xf2, 0xfd, 0xe3, 0x8b], // transferOwnership(address)
    [0x8d, 0xa5, 0xcb, 0x5b], // owner()
    [0x7a, 0x0e, 0xd6, 0x27], // facets()
    [0xad, 0xfc, 0xa1, 0x5e], // facetFunctionSelectors(address)
    [0x52, 0xef, 0x6b, 0x2c], // facetAddresses()
    [0xcd, 0xff, 0xac, 0xc6], // facetAddress(bytes4)
    [0x01, 0xff, 0xc9, 0xa7], // supportsInterface(bytes4)
];

/// `0x`-prefixed 4-byte selector, for error messages.
fn hex4(s: &[u8; 4]) -> String {
    format!("0x{:02x}{:02x}{:02x}{:02x}", s[0], s[1], s[2], s[3])
}

/// Lint an `Add` cut of `new` selectors into a diamond whose `existing`
/// selectors are given, with `init_is_zero` = whether the cut's `_init` is
/// `address(0)`. Returns `Err(reasons)` listing EVERY problem at once (so the
/// agent can fix them in one pass), or `Ok(())` if the cut is safe.
///
/// Rejections:
/// - a new selector is RESERVED → the facet could seize cut/ownership or blind
///   the loupe ([`RESERVED_SELECTORS`]);
/// - a new selector already EXISTS on the diamond → an `Add` would revert
///   (`LibDiamondCut: Can't add function that already exists`) after burning gas;
/// - the cut carries a non-zero `_init` → an init delegatecall runs arbitrary
///   code in the diamond's storage context (the §7 highest-severity vector);
/// - a selector is declared twice in the facet (the cut would revert mid-loop).
pub fn check_cut(new: &[[u8; 4]], existing: &[[u8; 4]], init_is_zero: bool) -> Result<(), Vec<String>> {
    let mut errs = Vec::new();
    if !init_is_zero {
        errs.push(
            "cut `_init` must be address(0): an init delegatecall runs arbitrary code in the \
             diamond's storage context (can overwrite owner/credits) — forbidden for agent cuts"
                .to_string(),
        );
    }
    for s in new {
        if RESERVED_SELECTORS.contains(s) {
            errs.push(format!(
                "refusing to cut reserved diamond selector {} (cut/ownership/loupe) — a facet must \
                 not be able to seize the diamond",
                hex4(s)
            ));
        }
        if existing.contains(s) {
            errs.push(format!(
                "selector {} already exists on the diamond — an Add cut would revert; remove it \
                 from the facet or Replace deliberately",
                hex4(s)
            ));
        }
    }
    for (i, s) in new.iter().enumerate() {
        if new[i + 1..].contains(s) {
            errs.push(format!("selector {} is declared twice in the facet", hex4(s)));
        }
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: [u8; 4] = [0xaa, 0xbb, 0xcc, 0xdd];
    const B: [u8; 4] = [0x11, 0x22, 0x33, 0x44];
    const DIAMOND_CUT: [u8; 4] = [0x1f, 0x93, 0x1c, 0x1c];
    const OWNER: [u8; 4] = [0x8d, 0xa5, 0xcb, 0x5b];

    #[test]
    fn clean_cut_passes() {
        assert!(check_cut(&[A, B], &[], true).is_ok());
        // existing, but no overlap → still fine.
        assert!(check_cut(&[A], &[B], true).is_ok());
    }

    #[test]
    fn clash_is_rejected() {
        let e = check_cut(&[A, B], &[B], true).unwrap_err();
        assert_eq!(e.len(), 1);
        assert!(e[0].contains("already exists"), "{e:?}");
    }

    #[test]
    fn reserved_selectors_are_rejected() {
        let e = check_cut(&[DIAMOND_CUT], &[], true).unwrap_err();
        assert!(e[0].contains("reserved"), "{e:?}");
        // owner() too.
        assert!(check_cut(&[OWNER], &[], true).is_err());
        // every reserved selector is caught.
        for s in RESERVED_SELECTORS {
            assert!(check_cut(&[s], &[], true).is_err(), "missed reserved {s:?}");
        }
    }

    #[test]
    fn nonzero_init_is_rejected() {
        let e = check_cut(&[A], &[], false).unwrap_err();
        assert!(e.iter().any(|r| r.contains("_init")), "{e:?}");
    }

    #[test]
    fn duplicate_selector_in_facet_is_rejected() {
        let e = check_cut(&[A, A], &[], true).unwrap_err();
        assert!(e.iter().any(|r| r.contains("twice")), "{e:?}");
    }

    #[test]
    fn all_problems_reported_at_once() {
        // reserved + clash + nonzero init in one call → 3 reasons.
        let e = check_cut(&[DIAMOND_CUT, A], &[A], false).unwrap_err();
        assert!(e.len() >= 3, "expected >=3 reasons, got {e:?}");
    }
}
