// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Test} from "forge-std/Test.sol";
import {CreditMeterFacet} from "../src/facets/CreditMeterFacet.sol";
import {LibCreditMeterStorage} from "../src/libraries/LibCreditMeterStorage.sol";
import {LibCreditsStorage} from "../src/libraries/LibCreditsStorage.sol";

/// Minimal `$LH`-shaped TIP-20 mock (same shape as InviteFacet.t.sol's).
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

    function transfer(address to, uint256 amt) external returns (bool) {
        require(balanceOf[msg.sender] >= amt, "balance");
        balanceOf[msg.sender] -= amt;
        balanceOf[to] += amt;
        return true;
    }
}

/// Harness: the facet + setters for the shared diamond-storage slots a real
/// diamond populates elsewhere (creditsToken via CreditsFacet, meter via
/// setMeter-as-owner). `address(this)` (the harness) IS the escrow holder,
/// exactly like the live diamond.
contract MeterHarness is CreditMeterFacet {
    function _setCreditsToken(address token) external {
        LibCreditsStorage.load().creditsToken = token;
    }

    function _setMeterRaw(address m) external {
        LibCreditMeterStorage.load().meter = m;
    }
}

contract CreditMeterFacetTest is Test {
    MeterHarness facet;
    MockLH lh;

    address user = address(0xA11CE);
    address proxyMeter = address(0xBEEF);

    event CreditsWithdrawn(address indexed user, uint256 amount, uint256 newBalance);

    function setUp() public {
        facet = new MeterHarness();
        lh = new MockLH();
        facet._setCreditsToken(address(lh));
        facet._setMeterRaw(proxyMeter);

        lh.mint(user, 100 ether);
        vm.prank(user);
        lh.approve(address(facet), type(uint256).max);
    }

    /// THE pot-merge invariant: deposit -> withdraw round-trips the full
    /// amount; ledger and wallet both end where they started.
    function test_withdraw_round_trips_unspent_credits() public {
        vm.prank(user);
        facet.depositCredits(10 ether);
        assertEq(facet.creditOf(user), 10 ether);
        assertEq(lh.balanceOf(user), 90 ether);

        vm.expectEmit(true, false, false, true);
        emit CreditsWithdrawn(user, 10 ether, 0);
        vm.prank(user);
        facet.withdrawCredits(10 ether);

        assertEq(facet.creditOf(user), 0);
        assertEq(lh.balanceOf(user), 100 ether);
        assertEq(lh.balanceOf(address(facet)), 0);
    }

    /// Only UNSPENT credits are withdrawable: metered spend is final, and
    /// the spent `$LH` stays in the diamond (platform revenue), never owed.
    function test_withdraw_after_meter_spend_caps_at_remainder() public {
        vm.prank(user);
        facet.depositCredits(10 ether);
        vm.prank(proxyMeter);
        facet.meter(user, 3 ether);

        vm.prank(user);
        vm.expectRevert(CreditMeterFacet.InsufficientCredits.selector);
        facet.withdrawCredits(8 ether);

        vm.prank(user);
        facet.withdrawCredits(7 ether);
        assertEq(facet.creditOf(user), 0);
        assertEq(lh.balanceOf(user), 97 ether);
        // The 3 ether of metered spend remains escrowed in the diamond.
        assertEq(lh.balanceOf(address(facet)), 3 ether);
    }

    function test_withdraw_zero_balance_reverts() public {
        vm.prank(user);
        vm.expectRevert(CreditMeterFacet.InsufficientCredits.selector);
        facet.withdrawCredits(1);
    }

    function test_withdraw_unconfigured_token_reverts() public {
        facet._setCreditsToken(address(0));
        vm.prank(user);
        vm.expectRevert(CreditMeterFacet.NotConfigured.selector);
        facet.withdrawCredits(1);
    }

    /// A withdraws cannot touch B's escrow: the ledger is per-user even
    /// though the diamond pools the tokens.
    function test_withdraw_cannot_take_another_users_escrow() public {
        address other = address(0xD00D);
        lh.mint(other, 5 ether);
        vm.prank(other);
        lh.approve(address(facet), type(uint256).max);
        vm.prank(other);
        facet.depositCredits(5 ether);

        // user never deposited; the pool holding other's 5 ether is not theirs.
        vm.prank(user);
        vm.expectRevert(CreditMeterFacet.InsufficientCredits.selector);
        facet.withdrawCredits(1 ether);
    }

    /// Conservation fuzz: any deposit / meter / withdraw interleaving keeps
    /// the diamond's token balance >= the total ledger (withdrawals always
    /// covered). Amounts are bounded; ops that would over-spend are skipped
    /// exactly like the facet reverts them.
    function testFuzz_escrow_always_covers_ledger(uint96[8] calldata amounts, uint8 opsMask)
        public
    {
        uint256 ledger;
        for (uint256 i = 0; i < 8; i++) {
            uint256 amt = uint256(amounts[i]) % 5 ether;
            if (amt == 0) continue;
            uint256 op = (uint256(opsMask) >> (i % 8)) % 3;
            if (op == 0) {
                if (lh.balanceOf(user) < amt) continue;
                vm.prank(user);
                facet.depositCredits(amt);
                ledger += amt;
            } else if (op == 1 && ledger >= amt) {
                vm.prank(proxyMeter);
                facet.meter(user, amt);
                ledger -= amt;
            } else if (op == 2 && ledger >= amt) {
                vm.prank(user);
                facet.withdrawCredits(amt);
                ledger -= amt;
            }
            assertEq(facet.creditOf(user), ledger);
            assertGe(lh.balanceOf(address(facet)), ledger);
        }
    }
}
