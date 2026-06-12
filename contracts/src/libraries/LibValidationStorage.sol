// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev Isolated storage for the ValidationFacet — the ERC-8004-flavored
///      VALIDATION-STAKING half of the reputation system (ReputationFacet
///      attestations are the free-signal half; this is the money-backed
///      half). Diamond storage pattern: a fresh slot, no collision with the
///      registry / TBA / invite / schedule / credits / bounty / reputation
///      storage already cut into the diamond. Add new fields ONLY at the end
///      of the struct, and ONLY at the end of `Validation` (diamond storage
///      layout is positional and immutable — the "append-only rule").
///
///      FINANCIAL — this IS a money facet (escrow / payout / refund), so it
///      inherits the full discipline the bounty / invite escrows were
///      hardened with: custom errors, CEI status-flips before every token
///      transfer, disjoint windows, and the escrow-conservation invariant
///      (every wei staked is either returned or paid out, never stranded or
///      minted — the fuzz in the test suite asserts it after every step).
///
///      DATA MODEL. A validation is one money-backed verdict from one
///      VALIDATOR address about one subject identity's work:
///        {validator, verdictValid (true="the work is valid"), workRef,
///         stakeWei}.
///      `workRef` is the SAME notion as ReputationFacet's: a hash /
///      off-chain pointer whose bytes are opaque — EXCEPT for the resolver
///      coupling (below), which interprets `uint256(workRef)` as a bounty id
///      when one exists. The platform convention (CLI `bounty_work_ref`) is
///      `workRef = bytes32(bountyId)`, so bounty-backed work gets its
///      natural oracle for free.
///
///      LIFECYCLE (the ABI-pinned Status enum):
///        stakeValidation     → Open       (validator escrows stakeWei)
///        challengeValidation → Challenged (a challenger counter-stakes the
///                                          SAME amount behind the OPPOSITE
///                                          verdict, while now <= challenge
///                                          deadline)
///        resolveValidation   → ValidatorWon | ChallengerWon
///                              (the work's bounty poster, or the diamond
///                               owner as arbiter fallback, picks the
///                               winner; the winner is paid BOTH stakes)
///      plus two refund exits (the no-strand guarantees):
///        reclaimStake (anyone, Open + past the challenge deadline)
///                              → Reclaimed (validator refunded 100%)
///        reclaimUnresolved (anyone, Challenged + past the resolve deadline)
///                              → Drawn (BOTH sides refunded their own stake)
///
///      DISJOINT WINDOWS (the invite/bounty hardening, applied twice):
///        challenge window  = now <= challengeDeadline (Open only)
///        reclaim window    = now >  challengeDeadline (Open only)
///        resolve window    = now <= resolveDeadline   (Challenged only)
///        draw window       = now >  resolveDeadline   (Challenged only)
///      so a validation is challenged XOR reclaimed, and resolved XOR drawn —
///      never both. No state ever leaves escrow locked forever: an AWOL
///      resolver costs each side only their own stake's time value, never the
///      stake itself.
///
///      WHY THE WINDOWS ARE FIXED CONSTANTS, not caller-supplied TTLs (the
///      one place this deliberately DIVERGES from invite/bounty): the
///      validator is the party a challenge runs AGAINST — letting them pick
///      the challenge window lets them pick "1 second" and make their stake
///      unchallengeable (free reputation-by-stake). A protocol-fixed window
///      removes that knob. Same for the resolve window: neither disputant
///      should control how long the arbiter has.
///
///      SELF-VALIDATION RULE (documented decision, mirrors ReputationFacet's
///      SelfAttestation): the SUBJECT token's owner cannot STAKE a validation
///      about their own work (staking "valid" on yourself is the obvious
///      self-pump; staking "invalid" on yourself is a wash-trade lever). The
///      subject's owner CAN however CHALLENGE someone else's validation of
///      their work — defending your own work with a counter-stake is the
///      legitimate, intended move. The validator cannot challenge themself
///      (a wash that would only grief the resolver).
///
///      V1-SIMPLE RESOLUTION (deliberately NOT an optimistic oracle): the
///      resolver is the work's natural oracle — the POSTER of the bounty
///      `uint256(workRef)` when that bounty exists — with the DIAMOND OWNER
///      as the always-available arbiter fallback (also the only resolver for
///      non-bounty workRefs). The poster-as-oracle is the SAME trust model as
///      BountyFacet's acceptResult. Noted follow-ups, additive cuts later:
///      staked third-party juries, reputation-weighted resolution, and
///      slashing a fraction to the resolver as a fee. The seam is the
///      `resolveValidation` gate, not this storage shape.
library LibValidationStorage {
    bytes32 constant POSITION = keccak256("localharness.validation.storage.v1");

    // --- Bounds (anti-grief circuit-breakers, mirroring invite/bounty) --
    /// How long an Open validation can be challenged (fixed — see the
    /// library doc for why the validator must NOT pick this).
    uint64 internal constant CHALLENGE_WINDOW = 3 days;
    /// How long after a challenge the resolver has to pick a winner before
    /// the dispute auto-draws (both sides refunded).
    uint64 internal constant RESOLUTION_WINDOW = 7 days;
    /// Per-validator cap on simultaneously-LIVE validations (anti-sybil row
    /// bound, mirrors BountyFacet's MAX_ACTIVE_PER_POSTER).
    uint256 internal constant MAX_ACTIVE_PER_VALIDATOR = 64;
    /// Per-address cap on total `$LH` locked in live validation escrow
    /// (validator stakes + challenger counter-stakes combined), mirroring
    /// InviteFacet's MAX_ESCROWED testnet circuit-breaker.
    uint256 internal constant MAX_STAKED = 1_000_000 ether;

    /// Validation lifecycle (the ABI-pinned enum — Open=0 … Drawn=5).
    /// Open → Challenged → {ValidatorWon, ChallengerWon, Drawn}, OR
    /// Open → Reclaimed. Reclaimed / ValidatorWon / ChallengerWon / Drawn
    /// are terminal.
    enum Status {
        Open, // 0 — staked; challengeable while now <= challengeDeadline, reclaimable after
        Challenged, // 1 — counter-staked; resolvable while now <= resolveDeadline, drawable after
        Reclaimed, // 2 — unchallenged; validator refunded 100%; terminal
        ValidatorWon, // 3 — resolver sided with the validator; validator paid 2x; terminal
        ChallengerWon, // 4 — resolver sided with the challenger; challenger paid 2x; terminal
        Drawn // 5 — challenged but never resolved; both refunded their own stake; terminal
    }

    /// One validation record, keyed by a monotonic `uint256 id`. Scalars
    /// packed to minimise cold SSTOREs:
    ///   slot 0: validator(160) + challengeDeadline(64) + status(8) +
    ///           verdictValid(8) = 240 bits
    ///   slot 1: stakeWei(128)
    ///   slot 2: challenger(160) + resolveDeadline(64) = 224 bits
    ///   slot 3: subjectTokenId(256)
    ///   slot 4: workRef(256)
    /// Append fields ONLY at the end.
    struct Validation {
        address validator; // who staked; the Reclaimed/Drawn/ValidatorWon recipient
        uint64 challengeDeadline; // unix seconds; the challenge/reclaim window boundary
        Status status; // Open | Challenged | Reclaimed | ValidatorWon | ChallengerWon | Drawn
        bool verdictValid; // the validator's claim: true = "this work is valid"
        uint128 stakeWei; // $LH each side escrows (18-dec wei); $LH supply << 2^128
        address challenger; // who counter-staked; address(0) until challenged
        uint64 resolveDeadline; // unix seconds; the resolve/draw window boundary; 0 until challenged
        uint256 subjectTokenId; // the identity whose work is being validated
        bytes32 workRef; // the work pointer (bytes32(bountyId) couples the resolver)
    }

    struct Storage {
        /// validationId -> validation record. Monotonic id from
        /// `nextValidationId`. A non-zero `validator` means the id is live
        /// (the unknown-validation guard). ids start at 1; 0 = no validation.
        mapping(uint256 => Validation) validations;
        /// Monotonic validation id counter (ids start at 1).
        uint256 nextValidationId;
        /// workRef -> every validation id staked about that work — the
        /// discovery index ("what verdicts exist about this work?").
        /// Append-only.
        mapping(bytes32 => uint256[]) validationsOfWork;
        /// validator -> every validation id they staked (the "my stakes"
        /// surface). Append-only; keyed by the STAKER, not the challenger.
        mapping(address => uint256[]) validationsOfValidator;
        /// dedup key -> already-staked flag. Key =
        /// keccak256(validator, subjectTokenId, workRef) — one validator
        /// stakes at most ONE verdict per (subject, work), ever (mirrors
        /// ReputationFacet's AlreadyAttested; a losing validator can't
        /// re-stake the same claim, and a reclaimed stake doesn't reopen
        /// the slot — one verdict is one verdict).
        mapping(bytes32 => bool) validated;
        /// address -> total `$LH` it currently has locked in LIVE validation
        /// escrow (its own stakes while Open/Challenged + its counter-stakes
        /// while Challenged). Maintained on every escrow/exit so MAX_STAKED
        /// is enforceable without iterating, and so the conservation
        /// invariant is auditable in one read.
        mapping(address => uint256) stakedOf;
        /// validator -> count of their currently-LIVE (Open/Challenged)
        /// validations — the MAX_ACTIVE_PER_VALIDATOR cap key.
        mapping(address => uint256) activeOf;
    }

    function load() internal pure returns (Storage storage s) {
        bytes32 position = POSITION;
        assembly {
            s.slot := position
        }
    }
}
