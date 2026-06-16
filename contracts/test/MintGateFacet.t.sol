// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Test} from "forge-std/Test.sol";
import {Diamond} from "../src/Diamond.sol";
import {MintGateFacet} from "../src/facets/MintGateFacet.sol";
import {CreditMeterFacet} from "../src/facets/CreditMeterFacet.sol";
import {CreditsFacet} from "../src/facets/CreditsFacet.sol";
import {LocalharnessCredits} from "../src/LocalharnessCredits.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";

/// A SECOND ISSUER_ROLE holder — stands in for "any other facet (or an
/// owner-cut malicious facet) that can mint as the diamond". Proves the C1 cap
/// lives in the TOKEN (global), not per-facet.
contract MaliciousMinter {
    LocalharnessCredits public token;

    constructor(LocalharnessCredits t) {
        token = t;
    }

    function drain(address to, uint256 amount) external {
        token.mint(to, amount);
    }
}

/// Real-diamond money-safety suite: MintGate + CreditMeter are cut into an
/// actual EIP-2535 diamond, so every call is the production delegatecall path
/// with real owner gating and shared storage. The diamond is the escrow holder
/// and the ISSUER on `LocalharnessCredits`.
contract MintGateFacetTest is Test {
    MintGateFacet gate; // typed view of the diamond
    CreditMeterFacet cm; // typed view of the diamond
    LocalharnessCredits lh;
    MaliciousMinter evil;
    address diamond;

    uint256 constant ISSUER_PK = 0xA11CE;
    address issuer;
    address owner = address(0x0000000000000000000000000000000000000FEE);
    address clawbacker = address(0xC1A4);
    address proxyMeter = address(0xBEEF);
    address buyer = address(0xB0B);
    address attacker = address(0xBAD);

    uint256 constant SECP_N =
        0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141;
    uint256 constant LOCK = 90 days;

    function setUp() public {
        issuer = vm.addr(ISSUER_PK);
        lh = new LocalharnessCredits(type(uint256).max, address(this));

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](3);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(new MintGateFacet()),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: _gateSelectors()
        });
        cuts[1] = IDiamond.FacetCut({
            facetAddress: address(new CreditMeterFacet()),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: _meterSelectors()
        });
        cuts[2] = IDiamond.FacetCut({
            facetAddress: address(new CreditsFacet()),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: _creditsSelectors()
        });

        diamond = address(new Diamond(owner, cuts));
        gate = MintGateFacet(diamond);
        cm = CreditMeterFacet(diamond);
        evil = new MaliciousMinter(lh);

        // The diamond mints into its own escrow + self-burns on clawback; the
        // test funds buyers directly; the rogue facet stands in for a 2nd issuer.
        lh.grantRole(lh.ISSUER_ROLE(), diamond);
        lh.grantRole(lh.ISSUER_ROLE(), address(this));
        lh.grantRole(lh.ISSUER_ROLE(), address(evil));

        vm.startPrank(owner);
        CreditsFacet(diamond).setCreditsToken(address(lh));
        cm.setMeter(proxyMeter);
        gate.setFiatIssuerSigner(issuer);
        gate.setClawbacker(clawbacker);
        gate.setFiatLockSecs(LOCK);
        vm.stopPrank();
    }

    function _gateSelectors() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](17);
        s[0] = MintGateFacet.mintFromFiat.selector;
        s[1] = MintGateFacet.clawbackFiatMint.selector;
        s[2] = MintGateFacet.setFiatIssuerSigner.selector;
        s[3] = MintGateFacet.setClawbacker.selector;
        s[4] = MintGateFacet.setPerReceiptMaxWei.selector;
        s[5] = MintGateFacet.setFiatLockSecs.selector;
        s[6] = MintGateFacet.setFiatMintWindow.selector;
        s[7] = MintGateFacet.fiatIssuerSigner.selector;
        s[8] = MintGateFacet.clawbacker.selector;
        s[9] = MintGateFacet.perReceiptMaxWei.selector;
        s[10] = MintGateFacet.fiatLockSecs.selector;
        s[11] = MintGateFacet.fiatLockedOf.selector;
        s[12] = MintGateFacet.receiptUsed.selector;
        s[13] = MintGateFacet.receiptInfo.selector;
        s[14] = MintGateFacet.fiatMintWindow.selector;
        s[15] = MintGateFacet.circulatingSupply.selector;
        s[16] = MintGateFacet.fiatMintDomainSeparator.selector;
    }

    function _meterSelectors() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](7);
        s[0] = CreditMeterFacet.depositCredits.selector;
        s[1] = CreditMeterFacet.withdrawCredits.selector;
        s[2] = CreditMeterFacet.meter.selector;
        s[3] = CreditMeterFacet.setMeter.selector;
        s[4] = CreditMeterFacet.creditOf.selector;
        s[5] = CreditMeterFacet.meterAddress.selector;
        s[6] = CreditMeterFacet.withdrawableOf.selector;
    }

    function _creditsSelectors() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](2);
        s[0] = CreditsFacet.setCreditsToken.selector;
        s[1] = CreditsFacet.creditsToken.selector;
    }

    // --- signing helpers -------------------------------------------------

    function _digest(address to, uint256 amount, bytes32 receiptId, uint256 validBefore)
        internal
        view
        returns (bytes32)
    {
        bytes32 domain = keccak256(
            abi.encode(
                keccak256(
                    "EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"
                ),
                keccak256(bytes("localharness-mintgate")),
                keccak256(bytes("1")),
                block.chainid,
                diamond
            )
        );
        bytes32 structHash = keccak256(
            abi.encode(
                keccak256("FiatMint(address to,uint256 amount,bytes32 receiptId,uint256 validBefore)"),
                to,
                amount,
                receiptId,
                validBefore
            )
        );
        return keccak256(abi.encodePacked("\x19\x01", domain, structHash));
    }

    function _sign(uint256 pk, bytes32 digest) internal pure returns (bytes memory) {
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(pk, digest);
        return abi.encodePacked(r, s, v);
    }

    function _mint(uint256 pk, address to, uint256 amount, bytes32 receiptId) internal {
        uint256 vb = block.timestamp + 1 hours;
        bytes memory sig = _sign(pk, _digest(to, amount, receiptId, vb));
        gate.mintFromFiat(to, amount, receiptId, vb, sig);
    }

    // --- mint: escrow + lock + spendability ------------------------------

    function test_mint_lands_in_escrow_and_locks() public {
        _mint(ISSUER_PK, buyer, 10 ether, "r1");

        assertEq(cm.creditOf(buyer), 10 ether, "spendable credit");
        (uint256 amt, uint256 unlockAt) = gate.fiatLockedOf(buyer);
        assertEq(amt, 10 ether, "locked amount");
        assertEq(unlockAt, block.timestamp + LOCK, "unlockAt");
        assertEq(lh.balanceOf(diamond), 10 ether, "escrow held by diamond");
        assertEq(lh.totalSupply(), 10 ether, "minted supply");
        assertEq(gate.circulatingSupply(), 0, "nothing circulating yet");
    }

    function test_locked_credit_is_spendable_on_compute() public {
        _mint(ISSUER_PK, buyer, 10 ether, "r1");
        vm.prank(proxyMeter);
        cm.meter(buyer, 3 ether);
        assertEq(cm.creditOf(buyer), 7 ether);
        (uint256 amt,) = gate.fiatLockedOf(buyer);
        assertEq(amt, 7 ether, "spend shrinks clawable lock");
    }

    // --- C2: lock-aware withdraw ----------------------------------------

    function test_withdraw_reverts_while_locked() public {
        _mint(ISSUER_PK, buyer, 10 ether, "r1");
        vm.prank(buyer);
        vm.expectRevert(CreditMeterFacet.InsufficientCredits.selector);
        cm.withdrawCredits(1);
        assertEq(cm.withdrawableOf(buyer), 0);
    }

    function test_withdraw_succeeds_after_unlock() public {
        _mint(ISSUER_PK, buyer, 10 ether, "r1");
        vm.warp(block.timestamp + LOCK + 1);
        assertEq(cm.withdrawableOf(buyer), 10 ether);
        vm.prank(buyer);
        cm.withdrawCredits(10 ether);
        assertEq(lh.balanceOf(buyer), 10 ether);
        assertEq(lh.balanceOf(diamond), 0);
        (uint256 amt,) = gate.fiatLockedOf(buyer);
        assertEq(amt, 0, "lock clamped to remaining balance after withdraw");
    }

    function test_deposit_funds_withdrawable_despite_lock() public {
        // Buyer's OWN deposited $LH must not be trapped by a later fiat lock.
        lh.mint(buyer, 5 ether);
        vm.prank(buyer);
        lh.approve(diamond, type(uint256).max);
        vm.prank(buyer);
        cm.depositCredits(5 ether);

        _mint(ISSUER_PK, buyer, 10 ether, "r1");
        assertEq(cm.creditOf(buyer), 15 ether);
        assertEq(cm.withdrawableOf(buyer), 5 ether, "only the unlocked deposit");

        vm.prank(buyer);
        cm.withdrawCredits(5 ether);
        assertEq(lh.balanceOf(buyer), 5 ether);

        vm.prank(buyer);
        vm.expectRevert(CreditMeterFacet.InsufficientCredits.selector);
        cm.withdrawCredits(1); // the fiat 10 is still locked
    }

    // --- clawback --------------------------------------------------------

    function test_clawback_burns_full_when_unspent() public {
        _mint(ISSUER_PK, buyer, 10 ether, "r1");
        vm.prank(clawbacker);
        uint256 recovered = gate.clawbackFiatMint("r1", 0);
        assertEq(recovered, 10 ether);
        assertEq(cm.creditOf(buyer), 0);
        assertEq(lh.totalSupply(), 0, "escrow burned");
        assertEq(lh.balanceOf(diamond), 0);
        (, , , bool clawed, ) = gate.receiptInfo("r1");
        assertTrue(clawed);
    }

    function test_clawback_after_partial_spend_recovers_remainder() public {
        _mint(ISSUER_PK, buyer, 10 ether, "r1");
        vm.prank(proxyMeter);
        cm.meter(buyer, 4 ether); // final/non-clawable spend
        vm.prank(clawbacker);
        uint256 recovered = gate.clawbackFiatMint("r1", 0);
        assertEq(recovered, 6 ether, "only still-locked recoverable");
        assertEq(cm.creditOf(buyer), 0);
        assertEq(lh.balanceOf(diamond), 4 ether, "spent stays as revenue");
        assertEq(lh.totalSupply(), 4 ether);
    }

    function test_clawback_after_withdraw_recovers_nothing() public {
        // The accepted, lock-window-bounded residual (red-team H1).
        _mint(ISSUER_PK, buyer, 10 ether, "r1");
        vm.warp(block.timestamp + LOCK + 1);
        vm.prank(buyer);
        cm.withdrawCredits(10 ether);
        vm.prank(clawbacker);
        uint256 recovered = gate.clawbackFiatMint("r1", 0);
        assertEq(recovered, 0);
        assertEq(lh.totalSupply(), 10 ether, "already escaped to wallet");
    }

    function test_clawback_only_clawbacker_or_owner() public {
        _mint(ISSUER_PK, buyer, 10 ether, "r1");
        vm.prank(attacker);
        vm.expectRevert(bytes("LibDiamond: not owner"));
        gate.clawbackFiatMint("r1", 0);
        vm.prank(owner);
        gate.clawbackFiatMint("r1", 0);
    }

    function test_clawback_double_reverts() public {
        _mint(ISSUER_PK, buyer, 10 ether, "r1");
        vm.prank(clawbacker);
        gate.clawbackFiatMint("r1", 0);
        vm.prank(clawbacker);
        vm.expectRevert(MintGateFacet.AlreadyClawed.selector);
        gate.clawbackFiatMint("r1", 0);
    }

    function test_partial_clawback_is_amount_aware_and_cumulative() public {
        _mint(ISSUER_PK, buyer, 10 ether, "r1");
        // A $3-equivalent partial refund claws only 3, not the whole receipt.
        vm.prank(clawbacker);
        uint256 r1 = gate.clawbackFiatMint("r1", 3 ether);
        assertEq(r1, 3 ether, "partial claws only the refunded amount");
        assertEq(cm.creditOf(buyer), 7 ether, "rest of the credit survives");
        (, , bool used, bool clawed, uint256 clawedWei) = gate.receiptInfo("r1");
        assertTrue(used);
        assertFalse(clawed, "not fully clawed after a partial");
        assertEq(clawedWei, 3 ether);

        // A later refund whose CUMULATIVE total is 4 claws only the +1 delta.
        vm.prank(clawbacker);
        uint256 r2 = gate.clawbackFiatMint("r1", 4 ether);
        assertEq(r2, 1 ether, "cumulative target claws the delta only");
        assertEq(cm.creditOf(buyer), 6 ether);

        // Re-submitting the same cumulative target is rejected (idempotent guard).
        vm.prank(clawbacker);
        vm.expectRevert(MintGateFacet.AlreadyClawed.selector);
        gate.clawbackFiatMint("r1", 4 ether);

        // A full clawback (maxWei=0) finishes the rest.
        vm.prank(clawbacker);
        uint256 r3 = gate.clawbackFiatMint("r1", 0);
        assertEq(r3, 6 ether);
        assertEq(cm.creditOf(buyer), 0);
        (, , , bool clawedFinal, ) = gate.receiptInfo("r1");
        assertTrue(clawedFinal);
        assertEq(lh.totalSupply(), 0, "all escrow burned across the partials");
    }

    function test_clawback_unknown_receipt_reverts() public {
        vm.prank(clawbacker);
        vm.expectRevert(MintGateFacet.UnknownReceipt.selector);
        gate.clawbackFiatMint("never", 0);
    }

    // --- idempotency + signature safety ---------------------------------

    function test_replayed_receipt_reverts() public {
        _mint(ISSUER_PK, buyer, 10 ether, "r1");
        uint256 vb = block.timestamp + 1 hours;
        bytes memory sig = _sign(ISSUER_PK, _digest(buyer, 10 ether, "r1", vb));
        vm.expectRevert(MintGateFacet.ReceiptUsed.selector);
        gate.mintFromFiat(buyer, 10 ether, "r1", vb, sig);
    }

    function test_forged_signature_reverts() public {
        uint256 vb = block.timestamp + 1 hours;
        bytes memory badSig = _sign(0xBADBAD, _digest(buyer, 10 ether, "r1", vb));
        vm.expectRevert(MintGateFacet.BadSignature.selector);
        gate.mintFromFiat(buyer, 10 ether, "r1", vb, badSig);
    }

    function test_tampered_amount_reverts() public {
        uint256 vb = block.timestamp + 1 hours;
        bytes memory sig = _sign(ISSUER_PK, _digest(buyer, 10 ether, "r1", vb));
        vm.expectRevert(MintGateFacet.BadSignature.selector);
        gate.mintFromFiat(buyer, 100 ether, "r1", vb, sig);
    }

    function test_high_s_signature_reverts() public {
        uint256 vb = block.timestamp + 1 hours;
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(ISSUER_PK, _digest(buyer, 10 ether, "r1", vb));
        bytes32 highS = bytes32(SECP_N - uint256(s));
        uint8 flipped = v == 27 ? 28 : 27;
        bytes memory mal = abi.encodePacked(r, highS, flipped);
        vm.expectRevert(MintGateFacet.BadSignature.selector);
        gate.mintFromFiat(buyer, 10 ether, "r1", vb, mal);
    }

    function test_expired_validBefore_reverts() public {
        uint256 vb = block.timestamp; // block.timestamp >= validBefore → expired
        bytes memory sig = _sign(ISSUER_PK, _digest(buyer, 10 ether, "r1", vb));
        vm.expectRevert(MintGateFacet.AuthExpired.selector);
        gate.mintFromFiat(buyer, 10 ether, "r1", vb, sig);
    }

    function test_signer_rotation() public {
        uint256 newPk = 0xF00D;
        vm.prank(owner);
        gate.setFiatIssuerSigner(vm.addr(newPk));
        uint256 vb = block.timestamp + 1 hours;
        bytes memory oldSig = _sign(ISSUER_PK, _digest(buyer, 10 ether, "r1", vb));
        vm.expectRevert(MintGateFacet.BadSignature.selector);
        gate.mintFromFiat(buyer, 10 ether, "r1", vb, oldSig);
        _mint(newPk, buyer, 10 ether, "r2");
        assertEq(cm.creditOf(buyer), 10 ether);
    }

    // --- caps: per-receipt + fiat window --------------------------------

    function test_per_receipt_cap_enforced() public {
        vm.prank(owner);
        gate.setPerReceiptMaxWei(5 ether);
        uint256 vb = block.timestamp + 1 hours;
        bytes memory sig = _sign(ISSUER_PK, _digest(buyer, 6 ether, "r1", vb));
        vm.expectRevert(MintGateFacet.PerReceiptCapExceeded.selector);
        gate.mintFromFiat(buyer, 6 ether, "r1", vb, sig);
        _mint(ISSUER_PK, buyer, 5 ether, "r2");
    }

    function test_fiat_window_cap_and_roll() public {
        vm.prank(owner);
        gate.setFiatMintWindow(10 ether, 1 days);
        _mint(ISSUER_PK, buyer, 10 ether, "r1");
        uint256 vb = block.timestamp + 1 hours;
        bytes memory sig = _sign(ISSUER_PK, _digest(buyer, 1 ether, "r2", vb));
        vm.expectRevert(MintGateFacet.FiatWindowCapExceeded.selector);
        gate.mintFromFiat(buyer, 1 ether, "r2", vb, sig);
        vm.warp(block.timestamp + 1 days + 1);
        _mint(ISSUER_PK, buyer, 1 ether, "r3");
    }

    // --- C1: the cap lives in the TOKEN, bounds every minter -------------

    function test_global_cap_bounds_malicious_second_facet() public {
        lh.tightenMintWindow(50 ether, 1 days); // test is token owner

        _mint(ISSUER_PK, buyer, 40 ether, "r1"); // window 40/50

        vm.expectRevert(LocalharnessCredits.MintWindowCapExceeded.selector);
        evil.drain(attacker, 20 ether);

        evil.drain(attacker, 10 ether); // 50/50
        vm.expectRevert(LocalharnessCredits.MintWindowCapExceeded.selector);
        evil.drain(attacker, 1);

        vm.warp(block.timestamp + 1 days + 1);
        evil.drain(attacker, 5 ether);
        assertEq(lh.balanceOf(attacker), 15 ether);
    }

    // The cap is a FIXED/tumbling window: an attacker can mint the full cap at
    // the end of one window and again at the start of the next (≤2x cap per
    // windowSecs across a boundary), but never >cap WITHIN a window. Documents
    // the real bound (size the cap at half the tolerable per-interval loss).
    function test_global_cap_tumbling_window_bounds_2x_across_boundary() public {
        lh.tightenMintWindow(50 ether, 100); // 50 LH / 100s

        _mint(ISSUER_PK, buyer, 50 ether, "r1"); // fills window 1
        // within the same window, one wei more reverts
        uint256 vb = block.timestamp + 1 hours;
        bytes memory sig = _sign(ISSUER_PK, _digest(buyer, 1, "r2", vb));
        vm.expectRevert(LocalharnessCredits.MintWindowCapExceeded.selector);
        gate.mintFromFiat(buyer, 1, "r2", vb, sig);

        // cross the boundary: a second full cap succeeds → 2x cap total
        vm.warp(block.timestamp + 100);
        _mint(ISSUER_PK, buyer, 50 ether, "r3");
        assertEq(cm.creditOf(buyer), 100 ether, "2x cap across the boundary");

        // …but still capped WITHIN the new window
        uint256 vb2 = block.timestamp + 1 hours;
        bytes memory sig2 = _sign(ISSUER_PK, _digest(buyer, 1, "r4", vb2));
        vm.expectRevert(LocalharnessCredits.MintWindowCapExceeded.selector);
        gate.mintFromFiat(buyer, 1, "r4", vb2, sig2);
    }

    // --- M: cap RAISE is time-locked ------------------------------------

    function test_cap_loosen_is_timelocked() public {
        lh.tightenMintWindow(50 ether, 1 days);

        vm.expectRevert(LocalharnessCredits.NotTightening.selector);
        lh.tightenMintWindow(1000 ether, 1 days);
        vm.expectRevert(LocalharnessCredits.NotTightening.selector);
        lh.tightenMintWindow(0, 1 days);

        lh.proposeLoosenMintWindow(1000 ether, 1 days);
        vm.expectRevert(LocalharnessCredits.TimelockNotElapsed.selector);
        lh.applyLoosenMintWindow();

        vm.warp(block.timestamp + lh.CAP_LOOSEN_TIMELOCK());
        lh.applyLoosenMintWindow();
        assertEq(lh.mintWindowCapWei(), 1000 ether);
    }

    function test_cap_loosen_can_be_cancelled() public {
        lh.tightenMintWindow(50 ether, 1 days);
        lh.proposeLoosenMintWindow(1000 ether, 1 days);
        lh.cancelLoosenMintWindow();
        vm.warp(block.timestamp + lh.CAP_LOOSEN_TIMELOCK());
        vm.expectRevert(LocalharnessCredits.NothingPending.selector);
        lh.applyLoosenMintWindow();
    }

    // --- structural invariant: circulating == total − escrow ------------

    function testFuzz_circulating_identity_and_escrow_covers_lock(
        uint96 mintAmt,
        uint96 spendAmt,
        bool doClawback
    ) public {
        uint256 m = uint256(mintAmt) % 1_000 ether + 1;
        _mint(ISSUER_PK, buyer, m, "r1");

        uint256 spend = uint256(spendAmt) % (m + 1);
        if (spend > 0) {
            vm.prank(proxyMeter);
            cm.meter(buyer, spend);
        }
        if (doClawback) {
            vm.prank(clawbacker);
            gate.clawbackFiatMint("r1", 0);
        }

        assertEq(
            gate.circulatingSupply(),
            lh.totalSupply() - lh.balanceOf(diamond),
            "circulating == total - escrow"
        );
        (uint256 lockedAmt,) = gate.fiatLockedOf(buyer);
        assertGe(lh.balanceOf(diamond), lockedAmt, "escrow covers lock");
    }
}
