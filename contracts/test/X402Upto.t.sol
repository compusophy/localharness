// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Test} from "forge-std/Test.sol";
import {X402Facet} from "../src/facets/X402Facet.sol";
import {LibCreditsStorage} from "../src/libraries/LibCreditsStorage.sol";

/// $LH-shaped TIP-20 mock — the surface X402Facet pulls through.
contract MockLH {
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    function mint(address to, uint256 amt) external {
        balanceOf[to] += amt;
    }

    function approve(address spender, uint256 amt) external returns (bool) {
        allowance[msg.sender][spender] = amt;
        return true;
    }

    function transferFrom(address from, address to, uint256 amt) external returns (bool) {
        uint256 a = allowance[from][msg.sender];
        require(a >= amt, "allowance");
        require(balanceOf[from] >= amt, "balance");
        allowance[from][msg.sender] = a - amt;
        balanceOf[from] -= amt;
        balanceOf[to] += amt;
        return true;
    }
}

contract X402Harness is X402Facet {
    function _setCreditsToken(address t) external {
        LibCreditsStorage.load().creditsToken = t;
    }
}

/// Coverage for the x402 "Upto" rail (sign-max / settle-actual) — the
/// token-metering settlement path. The payer signs a MAX; the facilitator
/// reports the actual; `settleUpto` moves `min(actual, max)` and never more.
contract X402UptoTest is Test {
    X402Harness x;
    MockLH lh;

    uint256 payerPk = 0xA11CE;
    address payer;
    address payee = address(0xBEEF);

    uint256 constant MAXV = 10 ether;
    bytes32 constant NONCE = bytes32(uint256(1));
    bytes32 constant PAYMENT_TYPEHASH = keccak256(
        "PaymentAuthorization(address from,address to,uint256 value,uint256 validAfter,uint256 validBefore,bytes32 nonce)"
    );

    function setUp() public {
        x = new X402Harness();
        lh = new MockLH();
        x._setCreditsToken(address(lh));
        payer = vm.addr(payerPk);
        lh.mint(payer, 1000 ether);
        vm.prank(payer);
        lh.approve(address(x), type(uint256).max);
        vm.warp(1_000_000);
    }

    /// Sign a PaymentAuthorization whose `value` is the MAX the payer authorizes.
    function _sign(uint256 maxValue, uint256 validAfter, uint256 validBefore, bytes32 nonce)
        internal
        view
        returns (bytes memory)
    {
        bytes32 structHash = keccak256(
            abi.encode(PAYMENT_TYPEHASH, payer, payee, maxValue, validAfter, validBefore, nonce)
        );
        bytes32 digest = keccak256(abi.encodePacked("\x19\x01", x.x402DomainSeparator(), structHash));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(payerPk, digest);
        return abi.encodePacked(r, s, v);
    }

    /// The happy path: a max-auth, settled at a LOWER actual, moves the actual.
    function test_upto_settles_actual_below_max() public {
        bytes memory sig = _sign(MAXV, 0, block.timestamp + 1 days, NONCE);
        x.settleUpto(payer, payee, MAXV, 3 ether, 0, block.timestamp + 1 days, NONCE, sig);
        assertEq(lh.balanceOf(payee), 3 ether, "payee got the actual");
        assertEq(lh.balanceOf(payer), 997 ether, "payer paid only the actual");
        assertTrue(x.authorizationState(payer, NONCE), "nonce consumed");
    }

    /// actual == max is the boundary — the full signed amount moves.
    function test_upto_at_max_boundary() public {
        bytes memory sig = _sign(MAXV, 0, block.timestamp + 1 days, NONCE);
        x.settleUpto(payer, payee, MAXV, MAXV, 0, block.timestamp + 1 days, NONCE, sig);
        assertEq(lh.balanceOf(payee), MAXV, "full max moved at the boundary");
    }

    /// A facilitator can NEVER charge above the signed ceiling — and the nonce is
    /// NOT consumed on the revert, so an honest retry still works.
    function test_upto_rejects_over_max() public {
        bytes memory sig = _sign(MAXV, 0, block.timestamp + 1 days, NONCE);
        vm.expectRevert(X402Facet.AmountExceedsMax.selector);
        x.settleUpto(payer, payee, MAXV, MAXV + 1, 0, block.timestamp + 1 days, NONCE, sig);
        assertFalse(x.authorizationState(payer, NONCE), "nonce untouched after over-max revert");
        // honest retry at a valid actual still settles
        x.settleUpto(payer, payee, MAXV, 2 ether, 0, block.timestamp + 1 days, NONCE, sig);
        assertEq(lh.balanceOf(payee), 2 ether, "retry settles");
    }

    /// One-shot: the same authorization can't be settled twice.
    function test_upto_replay_reverts() public {
        bytes memory sig = _sign(MAXV, 0, block.timestamp + 1 days, NONCE);
        x.settleUpto(payer, payee, MAXV, 1 ether, 0, block.timestamp + 1 days, NONCE, sig);
        vm.expectRevert(X402Facet.AuthAlreadyUsed.selector);
        x.settleUpto(payer, payee, MAXV, 1 ether, 0, block.timestamp + 1 days, NONCE, sig);
    }

    /// A 0-cost settle moves nothing but BURNS the nonce (no later reuse).
    function test_upto_zero_actual_consumes_nonce() public {
        bytes memory sig = _sign(MAXV, 0, block.timestamp + 1 days, NONCE);
        x.settleUpto(payer, payee, MAXV, 0, 0, block.timestamp + 1 days, NONCE, sig);
        assertEq(lh.balanceOf(payee), 0, "no transfer at 0 actual");
        assertTrue(x.authorizationState(payer, NONCE), "nonce still consumed");
        vm.expectRevert(X402Facet.AuthAlreadyUsed.selector);
        x.settleUpto(payer, payee, MAXV, 0, 0, block.timestamp + 1 days, NONCE, sig);
    }

    /// The signature binds the MAX: passing a different maxValue than was signed
    /// fails verification (no charging against a forged ceiling).
    function test_upto_bad_sig_on_mismatched_max() public {
        bytes memory sig = _sign(MAXV, 0, block.timestamp + 1 days, NONCE); // signed max = 10
        vm.expectRevert(X402Facet.BadSignature.selector);
        x.settleUpto(payer, payee, 20 ether, 5 ether, 0, block.timestamp + 1 days, NONCE, sig);
    }

    /// The validity window is enforced exactly as `settle`.
    function test_upto_expired_reverts() public {
        uint256 vb = block.timestamp + 100;
        bytes memory sig = _sign(MAXV, 0, vb, NONCE);
        vm.warp(vb + 1);
        vm.expectRevert(X402Facet.AuthExpired.selector);
        x.settleUpto(payer, payee, MAXV, 1 ether, 0, vb, NONCE, sig);
    }

    function test_upto_not_yet_valid_reverts() public {
        uint256 va = block.timestamp + 100;
        bytes memory sig = _sign(MAXV, va, block.timestamp + 1 days, NONCE);
        vm.expectRevert(X402Facet.AuthNotYetValid.selector);
        x.settleUpto(payer, payee, MAXV, 1 ether, va, block.timestamp + 1 days, NONCE, sig);
    }

    /// The one-shot nonce is SHARED with `settle`: a max-auth settled exactly via
    /// `settle` can't then be re-drained via `settleUpto`, and vice versa.
    function test_upto_shares_nonce_with_exact_settle() public {
        bytes memory sig = _sign(MAXV, 0, block.timestamp + 1 days, NONCE);
        // Drain it via the exact path first (moves the full signed value).
        x.settle(payer, payee, MAXV, 0, block.timestamp + 1 days, NONCE, sig);
        assertEq(lh.balanceOf(payee), MAXV, "exact settle moved the value");
        // The Upto path now sees the consumed nonce.
        vm.expectRevert(X402Facet.AuthAlreadyUsed.selector);
        x.settleUpto(payer, payee, MAXV, 1 ether, 0, block.timestamp + 1 days, NONCE, sig);
    }
}
