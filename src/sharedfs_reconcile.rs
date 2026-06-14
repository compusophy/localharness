//! Pure, deterministic CONVERGENT reconcile for the cross-device shared-folder
//! sync (`crate::app::sharedfs_sync` — cfg-gated out of non-browser doc
//! builds, so plain code formatting here, not links).
//!
//! ## The bug this fixes
//! The v1 sync merged two devices' folders by FILENAME only: each peer
//! announced its list of names, requested the names it lacked, and wrote them.
//! When two devices held the SAME filename with DIFFERENT content, neither side
//! ever re-requested it — the folders DIVERGED silently and never healed (no
//! content hash, no last-write-wins). Not corruption, but a permanent split.
//!
//! ## Why content-hash + conflict-copy (NOT last-write-wins)
//! LWW needs a clock. Our data model has NONE: a shared file is just
//! `{name, bytes}` — `crate::app::shared_fs::SharedEntry` carries only
//! `name + size`, OPFS [`crate::filesystem::Metadata`] carries only `kind +
//! size`, and the seal format stores no timestamp. There is no mtime/version to
//! compare, so LWW is unimplementable here without inventing (and trusting) a
//! per-file clock across devices that never talk except during a sync.
//!
//! So resolution is driven by the ONE identity a file does have: a hash of its
//! content. The rule is **deterministic + symmetric**:
//!   - Same name, same content (equal hash) → no-op (one copy survives).
//!   - Same name, DIFFERENT content → the file whose content hash is
//!     **lexicographically greater** keeps the plain name (the "winner"); the
//!     other is preserved as a CONFLICT COPY named `name.conflict-<shorthash>`
//!     (the short hash of the LOSER's content, [`CONFLICT_HASH_HEX_LEN`](crate::sharedfs_reconcile::CONFLICT_HASH_HEX_LEN) hex
//!     chars). No edit is silently lost.
//!   - Distinct names → union, exactly as before.
//!
//! Both devices compute the same content hashes over the same bytes, so both
//! independently pick the same winner AND derive the same conflict-copy name —
//! the merged SET is identical on both sides. That is the convergence property:
//! `reconcile(A, B)` and `reconcile(B, A)` yield the same set of
//! `{name -> content}` entries. After one exchange the two devices hold byte-
//! identical folders, and a re-sync is a pure no-op (idempotent).
//!
//! ## Pure by construction
//! This module is target-independent and dependency-free: it never touches
//! OPFS, WebRTC, or the seal key, and it does NOT hash — the caller supplies
//! each file's content hash (the wasm app computes keccak256 via `sha3`, which
//! it already depends on; the tests pass arbitrary bytes). That keeps the
//! convergence logic unit-testable under a plain native `cargo test`, the same
//! way [`crate::encoding`] was hoisted out of `app::events`. The wasm sync path
//! (`crate::app::sharedfs_sync`) calls
//! [`plan_pulls`](crate::sharedfs_reconcile::plan_pulls) to decide what to
//! fetch from a peer and what conflict-copies to write.

/// Number of hex chars of the loser's content hash appended to a conflict copy
/// (`name.conflict-<shorthash>`). 8 hex = 32 bits of the hash — enough to make
/// the name content-derived (so both devices generate the IDENTICAL conflict
/// name and the set still converges) without bloating the file name. The suffix
/// is purely a deterministic, collision-resistant-enough label, not a security
/// boundary.
pub const CONFLICT_HASH_HEX_LEN: usize = 8;

/// The fixed literal a conflict copy inserts between the base name and the short
/// hash: `<name>` + `CONFLICT_SEP` + `<shorthash>`. Pinned so the name guard in
/// `crate::app::shared_fs` can recognise (and size-budget for) a well-formed
/// conflict name without re-deriving the format.
pub const CONFLICT_SEP: &str = ".conflict-";

/// Maximum number of bytes [`conflict_name`] appends to a base name:
/// `CONFLICT_SEP` (`.conflict-`) + up to [`CONFLICT_HASH_HEX_LEN`] hex chars.
/// The name-safety guard reserves exactly this much headroom so a base name that
/// passes the guard ALWAYS yields a conflict name that also passes — closing the
/// gap (#85) where a 111–128-char base produced a 129–146-char conflict name
/// that failed `path_is_safe`, silently dropping the loser's edit.
pub const CONFLICT_SUFFIX_MAX_LEN: usize = CONFLICT_SEP.len() + CONFLICT_HASH_HEX_LEN;

/// True iff `name` is a well-formed conflict copy: `<base><CONFLICT_SEP><short>`
/// where `<base>` is non-empty and `<short>` is 1..=[`CONFLICT_HASH_HEX_LEN`]
/// lowercase hex chars (exactly what [`conflict_name`] emits). Lets a name guard
/// admit a long-but-legitimate conflict name even when the plain-name cap (with
/// reserved headroom) would reject it, so the holder can still serve/store the
/// conflict copy. Does NOT vouch for traversal safety — the caller still applies
/// its own no-`/`/no-`..` checks.
pub fn is_conflict_name(name: &str) -> bool {
    let Some(idx) = name.rfind(CONFLICT_SEP) else {
        return false;
    };
    if idx == 0 {
        return false; // empty base
    }
    let short = &name[idx + CONFLICT_SEP.len()..];
    !short.is_empty()
        && short.len() <= CONFLICT_HASH_HEX_LEN
        && short.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// One file in a device's shared folder, reduced to what reconcile needs: its
/// flat `name` and a `hash` of its content. The hash is opaque to this module —
/// any deterministic function of the bytes works; equality means "same content"
/// and the lexicographic order of the hashes breaks same-name conflicts. The
/// real sync uses keccak256; tests use short byte strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMeta {
    /// Flat file name (no path), as listed by `shared_fs::apex_list`.
    pub name: String,
    /// Content hash. Two files are "the same" iff their hashes are equal; the
    /// LEXICOGRAPHICALLY GREATER hash wins a same-name conflict.
    pub hash: Vec<u8>,
}

impl FileMeta {
    /// Convenience constructor.
    pub fn new(name: impl Into<String>, hash: impl Into<Vec<u8>>) -> Self {
        Self {
            name: name.into(),
            hash: hash.into(),
        }
    }
}

/// Lowercase hex of `bytes`.
fn hex(bytes: &[u8]) -> String {
    const HEXD: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEXD[(b >> 4) as usize] as char);
        s.push(HEXD[(b & 0xf) as usize] as char);
    }
    s
}

/// The conflict-copy name for a file whose content hashed to `hash`:
/// `<name>.conflict-<shorthash>`. Deterministic from `name` + `hash`, so both
/// devices derive the SAME conflict name for the same losing content (which is
/// what keeps the merged set convergent). The short hash takes the FIRST
/// [`CONFLICT_HASH_HEX_LEN`] hex chars of the hash (or all of it, if shorter).
pub fn conflict_name(name: &str, hash: &[u8]) -> String {
    let full = hex(hash);
    let short = if full.len() >= CONFLICT_HASH_HEX_LEN {
        &full[..CONFLICT_HASH_HEX_LEN]
    } else {
        &full
    };
    format!("{name}.conflict-{short}")
}

/// What THIS device must do after seeing a peer's manifest, to converge with it.
///
/// Returned by [`plan_pulls`] and consumed by the wasm sync path:
///   - [`ReconcilePlan::want`] — file NAMES to request from the peer (the peer
///     holds bytes we don't, under either a new name or a conflict-copy name we
///     don't yet have).
///   - [`ReconcilePlan::rename_local`] — local files to COPY to a conflict name
///     because the peer's same-named file won the deterministic tiebreak: we
///     keep the peer's winner under the plain name (pulled via `want`) and keep
///     our own losing edit under `(from -> to)` so nothing is lost.
///
/// Applying a plan is monotonic (only adds/renames, never deletes the winner),
/// and BOTH devices' plans drive the same final set — see the module doc.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReconcilePlan {
    /// Names to pull from the peer (plain names + conflict-copy names).
    pub want: Vec<String>,
    /// `(from_name, to_conflict_name)` local copies to make so a locally-losing
    /// edit survives under a conflict name while the peer's winner takes the
    /// plain name.
    pub rename_local: Vec<(String, String)>,
}

/// Decide what THIS device (manifest `local`) must pull from / write locally,
/// given the PEER's manifest `remote`, to make the two folders converge.
///
/// Deterministic and symmetric in the sense that matters: run on device A with
/// `(local=A, remote=B)` and on device B with `(local=B, remote=A)`, and after
/// each device applies its own plan (pull the wanted names from the peer, make
/// the conflict-copies) BOTH devices hold the identical merged set
/// `{name -> content}`. [`merged_set`] computes that fixed point directly for
/// testing the convergence property without a transport.
///
/// Rules, per file `name`:
///   - peer has it, we don't → `want` it (plain union).
///   - both have it, SAME hash → nothing (already identical).
///   - both have it, DIFFERENT hash → deterministic tiebreak on the hash:
///       * peer's hash GREATER (peer wins) → we will adopt the peer's bytes
///         under `name` (`want` it) AND copy our local loser to its conflict
///         name (`rename_local`), and also `want` the peer's loser-conflict-copy
///         (so we end up holding both sides' conflict copies too).
///       * our hash greater/equal-greater (we win) → we keep `name`; we still
///         `want` the peer's losing content under ITS conflict name so the loser
///         survives on our side as well.
///   - we have it, peer doesn't → nothing to pull (the peer will `want` it from
///     us via its own symmetric plan).
///
/// Names already present locally are never re-requested, so the plan is
/// idempotent: a second pass over converged folders yields an empty plan.
pub fn plan_pulls(local: &[FileMeta], remote: &[FileMeta]) -> ReconcilePlan {
    let mut plan = ReconcilePlan::default();
    let have_name = |n: &str| local.iter().any(|f| f.name == n);

    for r in remote {
        match local.iter().find(|l| l.name == r.name) {
            None => {
                // Peer-only name: plain union pull (unless we somehow already
                // hold that exact name, guarded above by `find`).
                plan.want.push(r.name.clone());
            }
            Some(l) if l.hash == r.hash => {
                // Identical file — no-op.
            }
            Some(l) => {
                // Same name, different content → deterministic content-hash
                // tiebreak. Both devices see the same pair of hashes and agree
                // on the winner, so this is symmetric.
                let peer_wins = r.hash > l.hash;
                // The peer's losing/own copy must survive on our side under its
                // content-derived conflict name. The conflict name is derived
                // from the LOSER's hash; whichever file loses, its conflict copy
                // exists on the peer (the peer makes it via its own plan), so we
                // pull it. Only pull if we don't already hold it.
                if peer_wins {
                    // Peer's bytes win `name`: adopt them, and preserve OUR
                    // losing edit as a local conflict copy.
                    plan.want.push(r.name.clone());
                    let our_conflict = conflict_name(&l.name, &l.hash);
                    if !have_name(&our_conflict) {
                        plan.rename_local
                            .push((l.name.clone(), our_conflict));
                    }
                } else {
                    // We win `name`; the peer's losing bytes survive on our side
                    // as a conflict copy pulled from the peer.
                    let peer_conflict = conflict_name(&r.name, &r.hash);
                    if !have_name(&peer_conflict) {
                        plan.want.push(peer_conflict);
                    }
                }
            }
        }
    }
    plan
}

/// The fully-converged merged folder, as a sorted list of
/// `(name, content_hash)`, computed directly from two manifests. This is the
/// fixed point both devices reach after exchanging plans; the tests assert it is
/// symmetric (`merged_set(A,B) == merged_set(B,A)`), which is the convergence
/// guarantee. The wasm path doesn't call this — it applies [`plan_pulls`]
/// incrementally over the channel — but the two agree by construction.
///
/// For a same-name conflict, the winner (greater hash) keeps `name` and the
/// loser is emitted under [`conflict_name`]. Distinct names union. Identical
/// files collapse to one entry.
pub fn merged_set(a: &[FileMeta], b: &[FileMeta]) -> Vec<(String, Vec<u8>)> {
    use std::collections::BTreeMap;
    // name -> content hash. Insertion is order-independent because the conflict
    // resolution is symmetric: we always key the WINNER under the plain name and
    // the LOSER under its content-derived conflict name, regardless of which
    // side a file came from.
    let mut out: BTreeMap<String, Vec<u8>> = BTreeMap::new();

    // Collect all (name -> set of distinct hashes) across both sides.
    let mut by_name: BTreeMap<String, Vec<Vec<u8>>> = BTreeMap::new();
    for f in a.iter().chain(b.iter()) {
        let slot = by_name.entry(f.name.clone()).or_default();
        if !slot.contains(&f.hash) {
            slot.push(f.hash.clone());
        }
    }

    for (name, mut hashes) in by_name {
        if hashes.len() == 1 {
            out.insert(name, hashes.pop().unwrap());
            continue;
        }
        // Conflict: winner = max hash keeps `name`; every other distinct hash
        // becomes a conflict copy. (With two devices there's at most 2, but
        // handle N defensively.)
        hashes.sort();
        let winner = hashes.pop().unwrap(); // greatest
        for loser in &hashes {
            out.insert(conflict_name(&name, loser), loser.clone());
        }
        out.insert(name, winner);
    }

    out.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(name: &str, hash: &[u8]) -> FileMeta {
        FileMeta::new(name, hash.to_vec())
    }

    /// Same name / different content resolves to a DETERMINISTIC winner: the
    /// greater content hash keeps the plain name, the loser becomes a conflict
    /// copy. The pick does not depend on argument order.
    #[test]
    fn same_name_different_content_deterministic_winner() {
        let a = vec![f("notes.txt", b"\x01")];
        let b = vec![f("notes.txt", b"\x02")]; // 0x02 > 0x01 → b wins
        let m_ab = merged_set(&a, &b);
        let m_ba = merged_set(&b, &a);
        assert_eq!(m_ab, m_ba, "winner must not depend on order");
        // Winner (0x02) keeps the plain name.
        assert_eq!(
            m_ab.iter().find(|(n, _)| n == "notes.txt").unwrap().1,
            b"\x02".to_vec()
        );
        // Loser (0x01) survives as a conflict copy.
        let conflict = conflict_name("notes.txt", b"\x01");
        assert_eq!(
            m_ab.iter().find(|(n, _)| n == &conflict).unwrap().1,
            b"\x01".to_vec()
        );
    }

    /// THE convergence property: `merged_set(A,B)` equals `merged_set(B,A)` as a
    /// set, across a mix of distinct names, identical files, and conflicts. This
    /// is what guarantees two devices reach the same state after a sync.
    #[test]
    fn merge_is_symmetric_convergent() {
        let a = vec![
            f("only_a.txt", b"AA"),
            f("shared_same.txt", b"SAME"),
            f("conflict.txt", b"\x10\x00"),
        ];
        let b = vec![
            f("only_b.txt", b"BB"),
            f("shared_same.txt", b"SAME"),
            f("conflict.txt", b"\x20\x00"), // greater → b wins conflict.txt
        ];
        let m_ab = merged_set(&a, &b);
        let m_ba = merged_set(&b, &a);
        assert_eq!(m_ab, m_ba, "reconcile must be symmetric (convergence)");

        // Spot-check the converged contents.
        let get = |m: &Vec<(String, Vec<u8>)>, n: &str| {
            m.iter().find(|(name, _)| name == n).map(|(_, h)| h.clone())
        };
        assert_eq!(get(&m_ab, "only_a.txt"), Some(b"AA".to_vec()));
        assert_eq!(get(&m_ab, "only_b.txt"), Some(b"BB".to_vec()));
        assert_eq!(get(&m_ab, "shared_same.txt"), Some(b"SAME".to_vec()));
        // conflict.txt: greater hash (0x20..) wins; loser (0x10..) is a copy.
        assert_eq!(get(&m_ab, "conflict.txt"), Some(b"\x20\x00".to_vec()));
        let loser_copy = conflict_name("conflict.txt", b"\x10\x00");
        assert_eq!(get(&m_ab, &loser_copy), Some(b"\x10\x00".to_vec()));
    }

    /// Distinct names just union (the non-regressing v1 behaviour).
    #[test]
    fn distinct_names_union() {
        let a = vec![f("a.txt", b"a"), f("b.txt", b"b")];
        let b = vec![f("c.txt", b"c")];
        let m = merged_set(&a, &b);
        let names: Vec<&str> = m.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["a.txt", "b.txt", "c.txt"]);
        assert_eq!(merged_set(&a, &b), merged_set(&b, &a));
    }

    /// Identical folders are a no-op — one copy of each file, no conflict
    /// copies, and the plan is empty (idempotent re-sync).
    #[test]
    fn identical_files_are_noop() {
        let a = vec![f("x.txt", b"X"), f("y.txt", b"Y")];
        let b = a.clone();
        let m = merged_set(&a, &b);
        assert_eq!(m.len(), 2, "no conflict copies for identical content");
        assert_eq!(m, merged_set(&b, &a));
        // The incremental plan agrees: nothing to do.
        assert_eq!(plan_pulls(&a, &b), ReconcilePlan::default());
        assert_eq!(plan_pulls(&b, &a), ReconcilePlan::default());
    }

    /// The conflict copy preserves BOTH contents — the merged set contains the
    /// winner under the plain name AND the loser under the conflict name, so no
    /// edit is silently lost.
    #[test]
    fn conflict_copy_preserves_both_contents() {
        let a = vec![f("doc", b"\xaa")];
        let b = vec![f("doc", b"\xbb")]; // 0xbb > 0xaa
        let m = merged_set(&a, &b);
        let contents: std::collections::BTreeSet<Vec<u8>> =
            m.iter().map(|(_, h)| h.clone()).collect();
        assert!(contents.contains(b"\xaa".as_slice()), "loser preserved");
        assert!(contents.contains(b"\xbb".as_slice()), "winner preserved");
        assert_eq!(m.len(), 2);
    }

    /// An empty side: merging with nothing yields the non-empty side unchanged
    /// (and is symmetric).
    #[test]
    fn empty_side() {
        let a = vec![f("a.txt", b"a"), f("b.txt", b"b")];
        let empty: Vec<FileMeta> = Vec::new();
        let m = merged_set(&a, &empty);
        assert_eq!(m.len(), 2);
        assert_eq!(merged_set(&a, &empty), merged_set(&empty, &a));
        assert_eq!(merged_set(&empty, &empty), Vec::new());
    }

    /// The incremental [`plan_pulls`] each device runs drives convergence to the
    /// SAME set that [`merged_set`] computes directly. We simulate applying both
    /// devices' plans and assert the resulting folders are byte-identical and
    /// equal to `merged_set`.
    #[test]
    fn plans_converge_to_merged_set() {
        let a = vec![
            f("only_a", b"A"),
            f("shared", b"\x05"),
            f("dup", b"DUP"),
        ];
        let b = vec![
            f("only_b", b"B"),
            f("shared", b"\x09"), // greater → b wins `shared`
            f("dup", b"DUP"),
        ];

        // Apply A's plan: A pulls wanted names from B and makes its conflict
        // copies; same for B.
        let final_a = apply_plan(&a, &b);
        let final_b = apply_plan(&b, &a);

        assert_eq!(final_a, final_b, "both devices reach the same folder");
        assert_eq!(final_a, merged_set(&a, &b), "matches the direct fixed point");
    }

    /// The conflict suffix budget exactly covers what [`conflict_name`] appends:
    /// `.conflict-` plus the short hash. This is the headroom the name guard
    /// (`crate::app::shared_fs::path_is_safe`) must reserve so a base name that
    /// passes the guard ALWAYS yields a conflict name that fits too (#85).
    #[test]
    fn conflict_suffix_len_matches_what_conflict_name_appends() {
        // A full-length short hash (>= CONFLICT_HASH_HEX_LEN hex chars worth of
        // bytes) appends the maximum: `.conflict-` + CONFLICT_HASH_HEX_LEN hex.
        let max = conflict_name("x", &[0xab, 0xcd, 0xef, 0x01, 0x23]);
        assert_eq!(max.len() - "x".len(), CONFLICT_SUFFIX_MAX_LEN);
        // For ANY base + hash, the appended length never exceeds the budget.
        for base in ["a", "notes.txt", &"z".repeat(64)] {
            for hash in [&b""[..], &b"\x00"[..], &b"\xff\xff\xff\xff\xff\xff"[..]] {
                let cn = conflict_name(base, hash);
                assert!(
                    cn.len() <= base.len() + CONFLICT_SUFFIX_MAX_LEN,
                    "conflict suffix for {base:?}/{hash:?} overran the budget"
                );
            }
        }
    }

    /// A base name capped at `128 - CONFLICT_SUFFIX_MAX_LEN` (the reserved-
    /// headroom plain-name cap) yields a conflict name within the 128-byte cap —
    /// the property that fixes the silent loser-drop in #85.
    #[test]
    fn capped_base_yields_in_bounds_conflict_name() {
        const NAME_CAP: usize = 128;
        let base = "n".repeat(NAME_CAP - CONFLICT_SUFFIX_MAX_LEN); // 110 chars
        assert!(base.len() <= NAME_CAP - CONFLICT_SUFFIX_MAX_LEN);
        let cn = conflict_name(&base, &[0xde, 0xad, 0xbe, 0xef, 0x99]);
        assert!(
            cn.len() <= NAME_CAP,
            "conflict name {} > cap {NAME_CAP}",
            cn.len()
        );
        assert!(is_conflict_name(&cn), "must be recognised as a conflict name");
    }

    /// [`is_conflict_name`] accepts exactly what [`conflict_name`] emits and
    /// rejects plain names + malformed lookalikes.
    #[test]
    fn is_conflict_name_round_trips() {
        // Emitted conflict names are recognised, across hash lengths.
        assert!(is_conflict_name(&conflict_name("notes.txt", b"\x01")));
        assert!(is_conflict_name(&conflict_name("a", &[0xab, 0xcd, 0xef, 0x12])));
        assert!(is_conflict_name(&conflict_name(
            "doc",
            &[0xff, 0xff, 0xff, 0xff, 0xff]
        )));
        // Plain names are not conflict names.
        assert!(!is_conflict_name("notes.txt"));
        assert!(!is_conflict_name("a.b.c"));
        // Empty base or empty/over-long/non-hex/upper short hash is rejected.
        assert!(!is_conflict_name(".conflict-ab"));
        assert!(!is_conflict_name("x.conflict-"));
        assert!(!is_conflict_name("x.conflict-abcdef012")); // 9 > 8 hex
        assert!(!is_conflict_name("x.conflict-zz")); // non-hex
        assert!(!is_conflict_name("x.conflict-AB")); // uppercase
    }

    /// Test helper: simulate `local` applying its `plan_pulls(local, remote)` —
    /// pull each wanted name's content from `remote` (or, for a conflict-copy
    /// name, from the corresponding remote loser), and make the local conflict
    /// copies. Returns the resulting folder as a sorted `(name, hash)` set, the
    /// same shape `merged_set` returns.
    fn apply_plan(local: &[FileMeta], remote: &[FileMeta]) -> Vec<(String, Vec<u8>)> {
        use std::collections::BTreeMap;
        let plan = plan_pulls(local, remote);
        let mut folder: BTreeMap<String, Vec<u8>> =
            local.iter().map(|f| (f.name.clone(), f.hash.clone())).collect();

        // Local conflict copies: copy `from`'s current bytes to `to`.
        for (from, to) in &plan.rename_local {
            if let Some(h) = folder.get(from).cloned() {
                folder.insert(to.clone(), h);
            }
        }
        // Pull wanted names from the peer. A wanted name is either a plain name
        // the peer holds, or a conflict-copy name we derive from a peer file.
        for want in &plan.want {
            if let Some(rf) = remote.iter().find(|f| &f.name == want) {
                folder.insert(want.clone(), rf.hash.clone());
            } else if let Some(rf) = remote
                .iter()
                .find(|f| &conflict_name(&f.name, &f.hash) == want)
            {
                folder.insert(want.clone(), rf.hash.clone());
            }
        }
        folder.into_iter().collect()
    }
}
