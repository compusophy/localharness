// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Test} from "forge-std/Test.sol";
import {ReputationFacet} from "../src/facets/ReputationFacet.sol";
import {LibReputationStorage} from "../src/libraries/LibReputationStorage.sol";
import {LibRegistryStorage} from "../src/libraries/LibRegistryStorage.sol";

/// Test harness: ReputationFacet + a setter that writes the SHARED diamond-
/// storage slot a real diamond populates via the registry facet (ownerOfId on
/// `register`). Because every `Lib*Storage.load()` resolves against THIS
/// contract's storage, writing `ownerOfId` here IS the cross-facet storage
/// sharing the diamond provides — the facet reads it identically to
/// production. The facet has NO escrow / payout, so the harness needs no token
/// or TBA resolver (unlike BountyHarness).
contract ReputationHarness is ReputationFacet {
    function _registerIdentity(uint256 id, address owner) external {
        LibRegistryStorage.load().ownerOfId[id] = owner;
    }
}

contract ReputationFacetTest is Test {
    ReputationHarness r;

    // --- identities (subjects of attestation) ---
    uint256 constant SUBJECT = 7; // the worker whose reputation accrues
    uint256 constant SUBJECT_B = 9; // a second subject (multi-subject tests)
    address constant SUBJECT_OWNER = address(0xCAFE); // controls SUBJECT
    address constant SUBJECT_B_OWNER = address(0xBEAD); // controls SUBJECT_B

    // --- attesters (the peers leaving signals) ---
    address constant ALICE = address(0xA11CE);
    address constant BOB = address(0xB0B);
    address constant CAROL = address(0xCAC0);

    // --- work references (opaque hashes / pointers) ---
    bytes32 constant WORK_1 = keccak256("ipfs://bafy.../fix-the-rustlite-bug");
    bytes32 constant WORK_2 = keccak256("ipfs://bafy.../the-other-patch");
    bytes32 constant WORK_3 = keccak256("git:deadbeef");

    function setUp() public {
        r = new ReputationHarness();
        // Register the subjects (their tokenIds get non-zero owners).
        r._registerIdentity(SUBJECT, SUBJECT_OWNER);
        r._registerIdentity(SUBJECT_B, SUBJECT_B_OWNER);
    }

    // =====================================================================
    // attest: happy path — record + aggregate
    // =====================================================================

    function test_attest_records_and_aggregates() public {
        vm.prank(ALICE);
        r.attest(SUBJECT, 5, WORK_1);

        (uint256 count, uint256 sum) = r.reputationOf(SUBJECT);
        assertEq(count, 1, "one attestation");
        assertEq(sum, 5, "rating summed");
        assertTrue(r.hasAttested(ALICE, SUBJECT, WORK_1), "dedup flag set");

        // The trail carries the exact record.
        (address[] memory atts, uint8[] memory rts, bytes32[] memory refs, uint256 cur) =
            r.attestationsOf(SUBJECT, 0, 10);
        assertEq(atts.length, 1);
        assertEq(atts[0], ALICE, "attester recorded");
        assertEq(rts[0], 5, "rating recorded");
        assertEq(refs[0], WORK_1, "workRef recorded");
        assertEq(cur, 1, "cursor advanced to length");
    }

    function test_attest_emits_event() public {
        vm.expectEmit(true, true, true, true);
        emit ReputationFacet.Attested(SUBJECT, ALICE, 4, WORK_1);
        vm.prank(ALICE);
        r.attest(SUBJECT, 4, WORK_1);
    }

    function test_attest_accepts_rating_bounds_1_and_5() public {
        vm.prank(ALICE);
        r.attest(SUBJECT, 1, WORK_1); // min valid
        vm.prank(BOB);
        r.attest(SUBJECT, 5, WORK_2); // max valid
        (uint256 count, uint256 sum) = r.reputationOf(SUBJECT);
        assertEq(count, 2);
        assertEq(sum, 6, "1 + 5");
    }

    // =====================================================================
    // attest: every revert
    // =====================================================================

    function test_attest_reverts_rating_zero() public {
        vm.prank(ALICE);
        vm.expectRevert(ReputationFacet.BadRating.selector);
        r.attest(SUBJECT, 0, WORK_1);
    }

    function test_attest_reverts_rating_six() public {
        vm.prank(ALICE);
        vm.expectRevert(ReputationFacet.BadRating.selector);
        r.attest(SUBJECT, 6, WORK_1);
    }

    function test_attest_reverts_rating_high() public {
        vm.prank(ALICE);
        vm.expectRevert(ReputationFacet.BadRating.selector);
        r.attest(SUBJECT, 255, WORK_1);
    }

    function test_attest_reverts_unknown_subject() public {
        vm.prank(ALICE);
        vm.expectRevert(ReputationFacet.UnknownSubject.selector);
        r.attest(4242, 5, WORK_1); // tokenId 4242 not registered
    }

    function test_attest_reverts_self_attestation() public {
        // The subject's OWNER attests its own identity → rejected.
        vm.prank(SUBJECT_OWNER);
        vm.expectRevert(ReputationFacet.SelfAttestation.selector);
        r.attest(SUBJECT, 5, WORK_1);
    }

    function test_attest_reverts_already_attested_same_workref() public {
        vm.prank(ALICE);
        r.attest(SUBJECT, 5, WORK_1);
        // Same attester, same subject, same workRef → dedup revert.
        vm.prank(ALICE);
        vm.expectRevert(ReputationFacet.AlreadyAttested.selector);
        r.attest(SUBJECT, 3, WORK_1);
    }

    // --- revert ORDERING / side-effect checks ---------------------------

    function test_bad_rating_writes_nothing() public {
        vm.prank(ALICE);
        vm.expectRevert(ReputationFacet.BadRating.selector);
        r.attest(SUBJECT, 0, WORK_1);
        (uint256 count, uint256 sum) = r.reputationOf(SUBJECT);
        assertEq(count, 0, "no aggregate written on a bad rating");
        assertEq(sum, 0);
        assertFalse(r.hasAttested(ALICE, SUBJECT, WORK_1), "no dedup flag on revert");
    }

    function test_already_attested_does_not_double_count() public {
        vm.prank(ALICE);
        r.attest(SUBJECT, 5, WORK_1);
        vm.prank(ALICE);
        vm.expectRevert(ReputationFacet.AlreadyAttested.selector);
        r.attest(SUBJECT, 2, WORK_1);
        (uint256 count, uint256 sum) = r.reputationOf(SUBJECT);
        assertEq(count, 1, "still exactly one");
        assertEq(sum, 5, "sum not bumped by the rejected re-attest");
    }

    function test_self_attestation_writes_nothing() public {
        vm.prank(SUBJECT_OWNER);
        vm.expectRevert(ReputationFacet.SelfAttestation.selector);
        r.attest(SUBJECT, 5, WORK_1);
        (uint256 count,) = r.reputationOf(SUBJECT);
        assertEq(count, 0, "self-attest left no record");
    }

    // =====================================================================
    // distinct works / multiple attesters — accumulation
    // =====================================================================

    function test_same_attester_distinct_works_allowed() public {
        // One attester may attest the SAME subject for DIFFERENT works.
        vm.prank(ALICE);
        r.attest(SUBJECT, 5, WORK_1);
        vm.prank(ALICE);
        r.attest(SUBJECT, 4, WORK_2);
        vm.prank(ALICE);
        r.attest(SUBJECT, 3, WORK_3);

        (uint256 count, uint256 sum) = r.reputationOf(SUBJECT);
        assertEq(count, 3, "three distinct works from one attester");
        assertEq(sum, 12, "5 + 4 + 3");
        assertTrue(r.hasAttested(ALICE, SUBJECT, WORK_1));
        assertTrue(r.hasAttested(ALICE, SUBJECT, WORK_2));
        assertTrue(r.hasAttested(ALICE, SUBJECT, WORK_3));
    }

    function test_multiple_attesters_accumulate() public {
        vm.prank(ALICE);
        r.attest(SUBJECT, 5, WORK_1);
        vm.prank(BOB);
        r.attest(SUBJECT, 4, WORK_1); // SAME workRef, DIFFERENT attester — allowed
        vm.prank(CAROL);
        r.attest(SUBJECT, 3, WORK_1);

        (uint256 count, uint256 sum) = r.reputationOf(SUBJECT);
        assertEq(count, 3, "three attesters accumulated");
        assertEq(sum, 12, "5 + 4 + 3");
    }

    function test_dedup_is_per_workref_not_global() public {
        // Bob attesting WORK_1 must NOT block Bob attesting WORK_2.
        vm.prank(BOB);
        r.attest(SUBJECT, 5, WORK_1);
        assertTrue(r.hasAttested(BOB, SUBJECT, WORK_1));
        assertFalse(r.hasAttested(BOB, SUBJECT, WORK_2), "WORK_2 still open for Bob");
        vm.prank(BOB);
        r.attest(SUBJECT, 2, WORK_2); // must succeed
        (uint256 count,) = r.reputationOf(SUBJECT);
        assertEq(count, 2);
    }

    function test_dedup_is_per_subject() public {
        // Alice attesting SUBJECT/WORK_1 must NOT block SUBJECT_B/WORK_1.
        vm.prank(ALICE);
        r.attest(SUBJECT, 5, WORK_1);
        assertFalse(r.hasAttested(ALICE, SUBJECT_B, WORK_1), "different subject is independent");
        vm.prank(ALICE);
        r.attest(SUBJECT_B, 5, WORK_1); // must succeed
        (uint256 cA,) = r.reputationOf(SUBJECT);
        (uint256 cB,) = r.reputationOf(SUBJECT_B);
        assertEq(cA, 1);
        assertEq(cB, 1, "subject B accrues independently");
    }

    function test_subjects_are_independent() public {
        vm.prank(ALICE);
        r.attest(SUBJECT, 5, WORK_1);
        vm.prank(BOB);
        r.attest(SUBJECT, 4, WORK_2);
        // SUBJECT_B untouched.
        (uint256 cB, uint256 sB) = r.reputationOf(SUBJECT_B);
        assertEq(cB, 0);
        assertEq(sB, 0);
    }

    // =====================================================================
    // reputationOf on an un-attested / unknown id
    // =====================================================================

    function test_reputationOf_unattested_is_zero() public view {
        (uint256 count, uint256 sum) = r.reputationOf(SUBJECT_B);
        assertEq(count, 0);
        assertEq(sum, 0);
    }

    function test_reputationOf_unknown_id_is_zero() public view {
        // An id that was never registered — still zero, no revert (a read).
        (uint256 count, uint256 sum) = r.reputationOf(123456);
        assertEq(count, 0);
        assertEq(sum, 0);
    }

    function test_hasAttested_false_when_none() public view {
        assertFalse(r.hasAttested(ALICE, SUBJECT, WORK_1));
    }

    // =====================================================================
    // attestationsOf paging
    // =====================================================================

    function _fill(uint256 n) internal {
        // n distinct attesters each attest SUBJECT once for a distinct work.
        for (uint256 i = 0; i < n; i++) {
            address attester = address(uint160(0x1000 + i));
            bytes32 work = keccak256(abi.encodePacked("work", i));
            vm.prank(attester);
            r.attest(SUBJECT, uint8(1 + (i % 5)), work);
        }
    }

    function test_attestationsOf_pages_in_windows() public {
        _fill(5);
        (uint256 count,) = r.reputationOf(SUBJECT);
        assertEq(count, 5);

        // Full scan.
        (address[] memory all,,, uint256 cur) = r.attestationsOf(SUBJECT, 0, 100);
        assertEq(all.length, 5, "full scan returns all");
        assertEq(cur, 5, "cursor at length");

        // Window [0,2).
        (address[] memory p1,,, uint256 c1) = r.attestationsOf(SUBJECT, 0, 2);
        assertEq(p1.length, 2);
        assertEq(c1, 2);
        assertEq(p1[0], address(uint160(0x1000)));
        assertEq(p1[1], address(uint160(0x1001)));

        // Window [2,4).
        (address[] memory p2,,, uint256 c2) = r.attestationsOf(SUBJECT, c1, 2);
        assertEq(p2.length, 2);
        assertEq(c2, 4);
        assertEq(p2[0], address(uint160(0x1002)));

        // Window [4,5) — last partial page (limit overshoots length).
        (address[] memory p3,,, uint256 c3) = r.attestationsOf(SUBJECT, c2, 2);
        assertEq(p3.length, 1, "partial last page clamps to length");
        assertEq(c3, 5);
        assertEq(p3[0], address(uint160(0x1004)));

        // Past the end — empty, cursor clamps to length.
        (address[] memory p4,,, uint256 c4) = r.attestationsOf(SUBJECT, c3, 2);
        assertEq(p4.length, 0);
        assertEq(c4, 5, "cursor clamps to length past the end");
    }

    function test_attestationsOf_zero_limit_is_empty() public {
        _fill(3);
        (address[] memory atts,,, uint256 cur) = r.attestationsOf(SUBJECT, 0, 0);
        assertEq(atts.length, 0, "zero limit returns empty");
        assertEq(cur, 3, "cursor reports the full length");
    }

    function test_attestationsOf_empty_trail() public view {
        (address[] memory atts, uint8[] memory rts, bytes32[] memory refs, uint256 cur) =
            r.attestationsOf(SUBJECT, 0, 10);
        assertEq(atts.length, 0);
        assertEq(rts.length, 0);
        assertEq(refs.length, 0);
        assertEq(cur, 0, "empty trail length is 0");
    }

    function test_attestationsOf_parallel_arrays_aligned() public {
        vm.prank(ALICE);
        r.attest(SUBJECT, 2, WORK_1);
        vm.prank(BOB);
        r.attest(SUBJECT, 4, WORK_2);

        (address[] memory atts, uint8[] memory rts, bytes32[] memory refs,) =
            r.attestationsOf(SUBJECT, 0, 10);
        assertEq(atts[0], ALICE);
        assertEq(rts[0], 2);
        assertEq(refs[0], WORK_1);
        assertEq(atts[1], BOB);
        assertEq(rts[1], 4);
        assertEq(refs[1], WORK_2);
    }

    // =====================================================================
    // CONSERVATION / monotonicity — count == # of successful attests, and
    // count always == the trail length, sum always == Σ ratings.
    // =====================================================================

    function test_conservation_count_equals_successful_attests() public {
        // 4 successful + 2 reverts (one dedup, one self-attest) → count == 4.
        vm.prank(ALICE);
        r.attest(SUBJECT, 5, WORK_1); // ok 1
        vm.prank(BOB);
        r.attest(SUBJECT, 3, WORK_1); // ok 2 (diff attester, same work)
        vm.prank(ALICE);
        vm.expectRevert(ReputationFacet.AlreadyAttested.selector);
        r.attest(SUBJECT, 1, WORK_1); // REVERT (dedup)
        vm.prank(SUBJECT_OWNER);
        vm.expectRevert(ReputationFacet.SelfAttestation.selector);
        r.attest(SUBJECT, 5, WORK_2); // REVERT (self)
        vm.prank(ALICE);
        r.attest(SUBJECT, 4, WORK_2); // ok 3
        vm.prank(CAROL);
        r.attest(SUBJECT, 2, WORK_3); // ok 4

        (uint256 count, uint256 sum) = r.reputationOf(SUBJECT);
        assertEq(count, 4, "count == number of SUCCESSFUL attests only");
        assertEq(sum, 14, "5 + 3 + 4 + 2");

        // count must equal the actual trail length.
        (address[] memory all,,, uint256 cur) = r.attestationsOf(SUBJECT, 0, 1000);
        assertEq(all.length, count, "trail length == aggregate count");
        assertEq(cur, count);
    }

    function testFuzz_conservation(uint256 seedRaw) public {
        // Drive a random sequence of attests across a fixed attester set and a
        // small work set; assert after every step that count == trail length
        // and sum == Σ of the recorded ratings. Reverts (dedup / self / bad
        // rating) must NEVER move the aggregate.
        uint256 seed = seedRaw;
        address[4] memory attesters = [ALICE, BOB, CAROL, SUBJECT_OWNER];
        bytes32[3] memory works = [WORK_1, WORK_2, WORK_3];

        for (uint256 i = 0; i < 30; i++) {
            seed = uint256(keccak256(abi.encode(seed, i)));
            address attester = attesters[seed % 4];
            uint8 rating = uint8(seed % 7); // 0..6 — includes invalid ratings
            bytes32 work = works[(seed >> 8) % 3];

            (uint256 countBefore, uint256 sumBefore) = r.reputationOf(SUBJECT);

            bool willRevert = (rating == 0 || rating > 5) // BadRating
                || (attester == SUBJECT_OWNER) // SelfAttestation
                || r.hasAttested(attester, SUBJECT, work); // AlreadyAttested

            vm.prank(attester);
            if (willRevert) {
                vm.expectRevert();
                r.attest(SUBJECT, rating, work);
            } else {
                r.attest(SUBJECT, rating, work);
            }

            (uint256 countAfter, uint256 sumAfter) = r.reputationOf(SUBJECT);
            if (willRevert) {
                assertEq(countAfter, countBefore, "revert never moves count");
                assertEq(sumAfter, sumBefore, "revert never moves sum");
            } else {
                assertEq(countAfter, countBefore + 1, "success bumps count by one");
                assertEq(sumAfter, sumBefore + rating, "success adds the rating");
            }

            // Trail length must always track the aggregate count.
            (address[] memory trail,,,) = r.attestationsOf(SUBJECT, 0, 1000);
            assertEq(trail.length, countAfter, "trail length == count, always");
        }
    }
}
