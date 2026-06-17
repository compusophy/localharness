// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibValidationStorage} from "../libraries/LibValidationStorage.sol";
import {LibBountyStorage} from "../libraries/LibBountyStorage.sol";
import {LibCreditsStorage} from "../libraries/LibCreditsStorage.sol";
import {LibRegistryStorage} from "../libraries/LibRegistryStorage.sol";
import {LibDiamond} from "../libraries/LibDiamond.sol";

interface IERC20Min {
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
    function transfer(address to, uint256 amount) external returns (bool);
}

/// @title ValidationFacet
/// @notice ERC-8004-style VALIDATION STAKING — the money-backed half of the
///         reputation system (ReputationFacet's free attestations are the
///         other half). A VALIDATOR puts `$LH` where their mouth is: they
///         STAKE behind a verdict ("this work is valid" / "this work is
///         not") about a subject identity's `workRef`. Anyone who disagrees
///         CHALLENGES by counter-staking the SAME amount behind the opposite
///         verdict; a simple, safe v1 resolver — the work's bounty POSTER,
///         or the diamond owner as arbiter fallback — picks the winner, and
///         the loser's stake pays the winner. An unchallenged stake is
///         reclaimable after the window; an unresolved challenge auto-draws
///         after a second window (both refunded). Every wei staked is either
///         returned or paid out — never stranded, never minted (the escrow-
///         conservation fuzz in the test suite asserts it after every step).
///
///         REUSED PRIMITIVE — this is the InviteFacet / BountyFacet escrow
///         state-machine again, with TWO escrow legs instead of one:
///           • `transferFrom` staker→diamond on stake AND on challenge
///             (the diamond escrows; NO minting — supply-neutral exactly
///             like invites/bounties),
///           • CEI status-flips before EVERY payout/refund,
///           • disjoint windows so each fork of the lifecycle is XOR
///             (challenged XOR reclaimed; resolved XOR drawn),
///           • permissionless reclaim pokes whose refund ALWAYS goes to the
///             escrow's rightful owner, never `msg.sender`.
///
///         LIFECYCLE (the ABI-pinned Status enum):
///           stakeValidation     → Open       (validator escrows stakeWei)
///           challengeValidation → Challenged (equal counter-stake escrowed)
///           resolveValidation   → ValidatorWon | ChallengerWon
///                                 (winner is paid BOTH stakes)
///         plus the two no-strand refund exits:
///           reclaimStake      (anyone, Open + expired)      → Reclaimed
///           reclaimUnresolved (anyone, Challenged + expired) → Drawn
///
///         WHO RESOLVES (v1-simple, deliberately NOT an optimistic oracle):
///         the platform convention is `workRef = bytes32(bountyId)` (CLI
///         `bounty_work_ref`), so when `uint256(workRef)` is an existing
///         bounty, that bounty's POSTER — the work's natural oracle, the
///         same trust model as BountyFacet.acceptResult — may resolve. The
///         DIAMOND OWNER may ALWAYS resolve (the arbiter fallback, and the
///         only resolver for non-bounty workRefs). An AWOL resolver cannot
///         strand the stakes: past the resolve deadline the dispute draws
///         and both sides take their own stake back. Judge-and-party note:
///         a poster who is ALSO the validator/challenger still resolves —
///         documented, same poster-is-the-oracle trust as the bounty board;
///         the counterparty's hard stop is the draw refund (they can never
///         lose more than the time value of their stake to a hostile
///         resolver... a hostile poster CAN award themself the pot when they
///         are a disputant, which is exactly the bounty board's existing
///         "poster is the oracle" trust boundary — don't stake/challenge
///         against the oracle of a work you don't trust).
///
///         SELF-VALIDATION RULES (documented decisions):
///           • The SUBJECT's owner cannot STAKE about their own work
///             (mirrors ReputationFacet.SelfAttestation — staking "valid"
///             on yourself is the obvious self-pump).
///           • The SUBJECT's owner CAN CHALLENGE a validation of their work
///             (defending your own work with a counter-stake is the
///             intended move).
///           • The validator cannot challenge themself (a pointless wash).
///           • One verdict per (validator, subject, workRef), EVER — the
///             dedup mirrors AlreadyAttested and survives reclaim/loss, so
///             a loser can't re-stake the same claim and a flip-flopper
///             can't stake both sides serially.
///
///         GAS / STORAGE: a stake is a 5-slot record + three index pushes +
///         two counters; no unbounded blob (workRef is a fixed bytes32).
///         All views are O(1) or return caller-bounded arrays.
///
///         CUTTING IT (diamond owner; mirror script/AddBountyFacet): deploy
///         + diamondCut Add the 13 selectors in
///         script/AddValidationFacet.s.sol. No post-cut config — the credits
///         token, registry owners, bounty posters, and the diamond owner are
///         all read from shared diamond-storage slots already populated by
///         the live facets.
contract ValidationFacet {
    // --- Events (indexed for off-chain harvest / reputation indexers) ----

    event ValidationStaked(
        uint256 indexed id,
        address indexed validator,
        uint256 indexed subjectTokenId,
        bytes32 workRef,
        bool verdictValid,
        uint128 stakeWei,
        uint64 challengeDeadline
    );
    event ValidationChallenged(
        uint256 indexed id, address indexed challenger, uint128 stakeWei, uint64 resolveDeadline
    );
    event ValidationResolved(
        uint256 indexed id, address indexed winner, bool validatorWon, uint128 payoutWei
    );
    event StakeReclaimed(uint256 indexed id, address indexed validator, uint128 stakeWei);
    event ValidationDrawn(
        uint256 indexed id, address indexed validator, address indexed challenger, uint128 stakeWei
    );

    // --- Errors ---------------------------------------------------------

    error NotConfigured(); // credits token unset
    error ZeroStake(); // stakeWei == 0
    error StakeCapExceeded(); // stake > uint128 max, or an address past MAX_STAKED
    error TooManyActiveValidations(); // validator already at MAX_ACTIVE_PER_VALIDATOR
    error UnknownSubject(); // subjectTokenId is not a registered identity
    error SelfValidation(); // the subject's owner staking about their own work
    error AlreadyValidated(); // (validator, subject, workRef) already staked
    error UnknownValidation(); // no such id
    error NotOpen(); // challenge/reclaimStake on a non-Open validation
    error NotChallenged(); // resolve/reclaimUnresolved on a non-Challenged validation
    error ChallengeWindowClosed(); // challenge after the challenge deadline
    error ChallengeWindowStillOpen(); // reclaimStake before the challenge deadline
    error ResolveWindowClosed(); // resolve after the resolve deadline (it's a draw now)
    error ResolveWindowStillOpen(); // reclaimUnresolved before the resolve deadline
    error SelfChallenge(); // the validator challenging their own stake
    error NotResolver(); // resolver gate: not the work's poster nor the diamond owner
    error ResolverIsDisputant(); // the poster-resolver is also a disputant (self-deal)

    // --- Stake (permissionless; validator escrows their own $LH) --------

    /// Stake `stakeWei` `$LH` behind a verdict about `subjectTokenId`'s work
    /// `workRef` (`valid` = the claim "this work is valid"). ESCROWS the
    /// stake (`transferFrom` validator→diamond; approve the diamond first —
    /// the bundle batches approve + stake into one sponsored tx, exactly
    /// like `createInvite` / `postBounty`). The validation is challengeable
    /// for CHALLENGE_WINDOW, then reclaimable. Returns the new id.
    ///
    /// Rejects a zero stake, a stake past uint128 / the per-address
    /// MAX_STAKED cap, an unregistered subject, the subject's own owner
    /// (self-validation), a duplicate (validator, subject, workRef), and a
    /// validator already at MAX_ACTIVE_PER_VALIDATOR live validations.
    ///
    /// CEI: the WHOLE record + the dedup flag + the indexes + both counters
    /// land BEFORE the external `transferFrom`, so a failed pull reverts the
    /// whole tx and leaves NO ghost validation (and no consumed id).
    function stakeValidation(bytes32 workRef, uint256 subjectTokenId, bool valid, uint256 stakeWei)
        external
        returns (uint256 validationId)
    {
        // --- Checks ---
        if (stakeWei == 0) revert ZeroStake();
        if (stakeWei > type(uint128).max) revert StakeCapExceeded();

        // The subject must be a registered identity (same existence test as
        // ReputationFacet / BountyFacet) — no verdicts about phantom ids.
        address subjectOwner = LibRegistryStorage.load().ownerOfId[subjectTokenId];
        if (subjectOwner == address(0)) revert UnknownSubject();
        // The subject's controller can't stake about its own work (the
        // self-pump; see the contract-level self-validation rules).
        if (subjectOwner == msg.sender) revert SelfValidation();

        LibValidationStorage.Storage storage s = LibValidationStorage.load();

        // One verdict per (validator, subject, workRef), ever.
        bytes32 dedupKey = keccak256(abi.encodePacked(msg.sender, subjectTokenId, workRef));
        if (s.validated[dedupKey]) revert AlreadyValidated();

        if (s.activeOf[msg.sender] >= LibValidationStorage.MAX_ACTIVE_PER_VALIDATOR) {
            revert TooManyActiveValidations();
        }
        uint256 newStaked = s.stakedOf[msg.sender] + stakeWei;
        if (newStaked > LibValidationStorage.MAX_STAKED) revert StakeCapExceeded();

        address token = LibCreditsStorage.load().creditsToken;
        if (token == address(0)) revert NotConfigured();

        // --- Effects (everything BEFORE the escrow pull; CEI) ---
        s.validated[dedupKey] = true;
        s.activeOf[msg.sender] += 1;
        s.stakedOf[msg.sender] = newStaked;
        validationId = ++s.nextValidationId; // ids start at 1
        uint64 challengeDeadline =
            uint64(block.timestamp) + LibValidationStorage.CHALLENGE_WINDOW;
        s.validations[validationId] = LibValidationStorage.Validation({
            validator: msg.sender,
            challengeDeadline: challengeDeadline,
            status: LibValidationStorage.Status.Open,
            verdictValid: valid,
            stakeWei: uint128(stakeWei),
            challenger: address(0),
            resolveDeadline: 0,
            subjectTokenId: subjectTokenId,
            workRef: workRef
        });
        s.validationsOfWork[workRef].push(validationId);
        s.validationsOfValidator[msg.sender].push(validationId);

        // --- Interaction: escrow LAST. A failed pull reverts everything.
        require(
            IERC20Min(token).transferFrom(msg.sender, address(this), stakeWei),
            "validation: escrow failed"
        );

        emit ValidationStaked(
            validationId,
            msg.sender,
            subjectTokenId,
            workRef,
            valid,
            uint128(stakeWei),
            challengeDeadline
        );
    }

    // --- Challenge (counter-stake the opposite verdict) ------------------

    /// Challenge an Open validation while `now <= challengeDeadline` by
    /// counter-staking EXACTLY the validation's `stakeWei` behind the
    /// OPPOSITE verdict (the challenge's claim is implicit — it is always
    /// `!verdictValid`). Flips Open → Challenged and starts the
    /// RESOLUTION_WINDOW clock. Anyone but the validator may challenge —
    /// including the subject's owner defending their own work.
    ///
    /// CEI: status + challenger + resolveDeadline + the challenger's
    /// stakedOf land BEFORE the external `transferFrom`, so a failed pull
    /// reverts the flip and the validation stays cleanly Open.
    function challengeValidation(uint256 validationId) external {
        LibValidationStorage.Storage storage s = LibValidationStorage.load();
        LibValidationStorage.Validation storage v = s.validations[validationId];

        if (v.validator == address(0)) revert UnknownValidation();
        if (v.status != LibValidationStorage.Status.Open) revert NotOpen();
        if (block.timestamp > v.challengeDeadline) revert ChallengeWindowClosed();
        if (msg.sender == v.validator) revert SelfChallenge();

        uint256 stake = v.stakeWei;
        uint256 newStaked = s.stakedOf[msg.sender] + stake;
        if (newStaked > LibValidationStorage.MAX_STAKED) revert StakeCapExceeded();

        address token = LibCreditsStorage.load().creditsToken;
        if (token == address(0)) revert NotConfigured();

        // --- Effects (before the escrow pull; CEI) ---
        uint64 resolveDeadline =
            uint64(block.timestamp) + LibValidationStorage.RESOLUTION_WINDOW;
        v.status = LibValidationStorage.Status.Challenged;
        v.challenger = msg.sender;
        v.resolveDeadline = resolveDeadline;
        s.stakedOf[msg.sender] = newStaked;

        // --- Interaction: counter-stake escrow LAST.
        require(
            IERC20Min(token).transferFrom(msg.sender, address(this), stake),
            "validation: counter-stake failed"
        );

        emit ValidationChallenged(validationId, msg.sender, v.stakeWei, resolveDeadline);
    }

    // --- Resolve (the work's oracle picks the winner) ---------------------

    /// Resolve a Challenged validation while `now <= resolveDeadline`.
    /// RESOLVER-ONLY: the POSTER of bounty `uint256(workRef)` when that
    /// bounty exists (the work's natural oracle), or the DIAMOND OWNER
    /// (arbiter fallback; the only resolver for non-bounty workRefs).
    /// `validatorWins = true` sides with the staked verdict; the winner is
    /// paid BOTH stakes (their own back + the loser's). Flips Challenged →
    /// ValidatorWon / ChallengerWon.
    ///
    /// CEI: terminal status + both stakedOf decrements + the active-count
    /// decrement land BEFORE the payout `transfer`, so a re-entrant token
    /// re-reads `status != Challenged` and reverts (no double payout).
    function resolveValidation(uint256 validationId, bool validatorWins) external {
        LibValidationStorage.Storage storage s = LibValidationStorage.load();
        LibValidationStorage.Validation storage v = s.validations[validationId];

        if (v.validator == address(0)) revert UnknownValidation();
        if (v.status != LibValidationStorage.Status.Challenged) revert NotChallenged();
        if (block.timestamp > v.resolveDeadline) revert ResolveWindowClosed();
        if (msg.sender != _posterOf(v.workRef) && msg.sender != LibDiamond.contractOwner()) {
            revert NotResolver();
        }
        // Defense-in-depth: a disputant must never be their own judge. The legit
        // resolver is a NEUTRAL third party (the work's bounty-poster). If that
        // poster is also the validator/challenger it's a self-deal — force the
        // owner-arbiter / draw path instead. The diamond owner (the platform
        // arbiter of last resort) is exempt, as it is already fully trusted.
        if (
            msg.sender != LibDiamond.contractOwner()
                && (msg.sender == v.validator || msg.sender == v.challenger)
        ) {
            revert ResolverIsDisputant();
        }

        uint128 stake = v.stakeWei;
        address validator = v.validator;
        address challenger = v.challenger;
        address winner = validatorWins ? validator : challenger;

        // --- Effects (terminal state + escrow accounting BEFORE the payout).
        v.status = validatorWins
            ? LibValidationStorage.Status.ValidatorWon
            : LibValidationStorage.Status.ChallengerWon;
        s.stakedOf[validator] -= stake;
        s.stakedOf[challenger] -= stake;
        s.activeOf[validator] -= 1;

        address token = LibCreditsStorage.load().creditsToken;
        if (token == address(0)) revert NotConfigured();
        uint128 payout = stake * 2; // both stakes; stake <= 2^128-1 came in as uint256-checked
        require(IERC20Min(token).transfer(winner, payout), "validation: payout failed");

        emit ValidationResolved(validationId, winner, validatorWins, payout);
    }

    // --- Reclaim (unchallenged stake comes home after the window) --------

    /// Reclaim an Open validation past its challenge deadline. PERMISSION-
    /// LESS to call (anyone can poke it), but the refund ALWAYS goes to the
    /// VALIDATOR, never `msg.sender`. The unchallenged verdict simply
    /// stands — the stake comes home 100%. Flips Open → Reclaimed.
    ///
    /// CEI: status + stakedOf + active-count BEFORE the refund.
    function reclaimStake(uint256 validationId) external {
        LibValidationStorage.Storage storage s = LibValidationStorage.load();
        LibValidationStorage.Validation storage v = s.validations[validationId];

        if (v.validator == address(0)) revert UnknownValidation();
        if (v.status != LibValidationStorage.Status.Open) revert NotOpen();
        if (block.timestamp <= v.challengeDeadline) revert ChallengeWindowStillOpen();

        uint128 stake = v.stakeWei;
        address validator = v.validator;

        v.status = LibValidationStorage.Status.Reclaimed;
        s.stakedOf[validator] -= stake;
        s.activeOf[validator] -= 1;

        address token = LibCreditsStorage.load().creditsToken;
        if (token == address(0)) revert NotConfigured();
        require(IERC20Min(token).transfer(validator, stake), "validation: reclaim failed");

        emit StakeReclaimed(validationId, validator, stake);
    }

    // --- Draw (unresolved dispute auto-refunds both sides) ---------------

    /// Refund a Challenged validation whose resolver never showed: past the
    /// resolve deadline, anyone may poke it and BOTH sides take their OWN
    /// stake back (a draw — nobody profits, nothing strands). This is the
    /// AWOL-resolver hard stop; without it a dead poster + an absent owner
    /// would lock both stakes forever. Flips Challenged → Drawn.
    ///
    /// CEI: status + both stakedOf decrements + active-count BEFORE the two
    /// refund transfers; a re-entrant token re-reads `status != Challenged`
    /// and reverts.
    function reclaimUnresolved(uint256 validationId) external {
        LibValidationStorage.Storage storage s = LibValidationStorage.load();
        LibValidationStorage.Validation storage v = s.validations[validationId];

        if (v.validator == address(0)) revert UnknownValidation();
        if (v.status != LibValidationStorage.Status.Challenged) revert NotChallenged();
        if (block.timestamp <= v.resolveDeadline) revert ResolveWindowStillOpen();

        uint128 stake = v.stakeWei;
        address validator = v.validator;
        address challenger = v.challenger;

        v.status = LibValidationStorage.Status.Drawn;
        s.stakedOf[validator] -= stake;
        s.stakedOf[challenger] -= stake;
        s.activeOf[validator] -= 1;

        address token = LibCreditsStorage.load().creditsToken;
        if (token == address(0)) revert NotConfigured();
        require(IERC20Min(token).transfer(validator, stake), "validation: refund failed");
        require(IERC20Min(token).transfer(challenger, stake), "validation: refund failed");

        emit ValidationDrawn(validationId, validator, challenger, stake);
    }

    // --- Views (the discovery surface) ------------------------------------

    /// Full validation record by id. Returns zeros for an unknown id
    /// (validator == address(0)).
    function getValidation(uint256 validationId)
        external
        view
        returns (
            address validator,
            address challenger,
            uint256 subjectTokenId,
            bytes32 workRef,
            uint128 stakeWei,
            uint64 challengeDeadline,
            uint64 resolveDeadline,
            uint8 status,
            bool verdictValid
        )
    {
        LibValidationStorage.Validation storage v =
            LibValidationStorage.load().validations[validationId];
        return (
            v.validator,
            v.challenger,
            v.subjectTokenId,
            v.workRef,
            v.stakeWei,
            v.challengeDeadline,
            v.resolveDeadline,
            uint8(v.status),
            v.verdictValid
        );
    }

    /// The bounty-poster half of the resolver gate for a validation: the
    /// POSTER of bounty `uint256(workRef)`, or address(0) when the workRef
    /// is not an existing bounty (then ONLY the diamond owner can resolve).
    /// Surfaced so clients can show "who can resolve this dispute".
    function validationResolverOf(uint256 validationId) external view returns (address) {
        LibValidationStorage.Validation storage v =
            LibValidationStorage.load().validations[validationId];
        if (v.validator == address(0)) return address(0);
        return _posterOf(v.workRef);
    }

    /// Whether `validator` has already staked a verdict about
    /// (`subjectTokenId`, `workRef`) — the dedup predicate, queryable up
    /// front to skip a doomed write (mirrors `hasAttested`).
    function hasValidated(address validator, uint256 subjectTokenId, bytes32 workRef)
        external
        view
        returns (bool)
    {
        bytes32 dedupKey = keccak256(abi.encodePacked(validator, subjectTokenId, workRef));
        return LibValidationStorage.load().validated[dedupKey];
    }

    /// Every validation id ever staked about `workRef` (live + terminal) —
    /// the per-work discovery index.
    function validationsOfWork(bytes32 workRef) external view returns (uint256[] memory) {
        return LibValidationStorage.load().validationsOfWork[workRef];
    }

    /// Every validation id `validator` ever staked (live + terminal).
    function validationsOf(address validator) external view returns (uint256[] memory) {
        return LibValidationStorage.load().validationsOfValidator[validator];
    }

    /// Total validations ever staked (== highest id; ids are monotonic).
    function validationCount() external view returns (uint256) {
        return LibValidationStorage.load().nextValidationId;
    }

    /// Total `$LH` an address currently has locked in LIVE validation
    /// escrow (stakes + counter-stakes) — the MAX_STAKED cap key and the
    /// per-address conservation read.
    function validationStakedOf(address staker) external view returns (uint256) {
        return LibValidationStorage.load().stakedOf[staker];
    }

    /// The count of a validator's currently-LIVE (Open/Challenged)
    /// validations — the MAX_ACTIVE_PER_VALIDATOR cap key.
    function activeValidationCountOf(address validator) external view returns (uint256) {
        return LibValidationStorage.load().activeOf[validator];
    }

    // --- Internal ---------------------------------------------------------

    /// The work's natural oracle: the poster of bounty `uint256(workRef)`
    /// when one exists (the platform convention is workRef =
    /// bytes32(bountyId)), else address(0). A non-bounty workRef can never
    /// alias a poster — bounty ids are small monotonic integers, and an
    /// unset id reads poster == address(0) from the shared bounty slot.
    function _posterOf(bytes32 workRef) internal view returns (address) {
        return LibBountyStorage.load().bounties[uint256(workRef)].poster;
    }
}
