// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Test} from "forge-std/Test.sol";
import {MultiSignerAccount} from "../src/erc6551/MultiSignerAccount.sol";

/// Test harness: overrides `owner()` (normally read from the clone's
/// bytecode footer via the 6551 registry) with a settable address so the
/// epoch / signer-invalidation logic can be exercised in isolation.
contract Harness is MultiSignerAccount {
    address public ownerOverride;

    function setOwner(address o) external {
        ownerOverride = o;
    }

    function owner() public view override returns (address) {
        return ownerOverride;
    }
}

contract MultiSignerAccountTest is Test {
    Harness acct;
    address ownerA = address(0xA11CE);
    address ownerB = address(0xB0B);
    address deviceA = address(0xD7117CE);
    address deviceB = address(0xD7780B);

    function setUp() public {
        acct = new Harness();
        acct.setOwner(ownerA);
    }

    function test_owner_can_add_and_signer_is_authorized() public {
        vm.prank(ownerA);
        acct.addSigner(deviceA);
        assertTrue(acct.isAuthorizedSigner(deviceA));
        assertTrue(acct.isAuthorizedSigner(ownerA)); // holder always authorized
    }

    function test_non_owner_cannot_add_signer() public {
        // A linked device cannot enroll further devices.
        vm.prank(ownerA);
        acct.addSigner(deviceA);
        vm.prank(deviceA);
        vm.expectRevert("MultiSigner: only owner");
        acct.addSigner(deviceB);
    }

    function test_signers_invalidated_on_transfer() public {
        vm.prank(ownerA);
        acct.addSigner(deviceA);
        assertTrue(acct.isAuthorizedSigner(deviceA));

        // NFT changes hands → deviceA must lose authorization immediately,
        // with no explicit cleanup call.
        acct.setOwner(ownerB);
        assertFalse(acct.isAuthorizedSigner(deviceA), "stale signer survived transfer");
        assertTrue(acct.isAuthorizedSigner(ownerB), "new holder must be authorized");
    }

    function test_old_device_cannot_readd_after_transfer() public {
        vm.prank(ownerA);
        acct.addSigner(deviceA);
        acct.setOwner(ownerB);
        // The stale device can't re-enroll itself (only owner may add).
        vm.prank(deviceA);
        vm.expectRevert("MultiSigner: only owner");
        acct.addSigner(deviceA);
    }

    function test_new_owner_establishes_fresh_set() public {
        vm.prank(ownerA);
        acct.addSigner(deviceA);
        acct.setOwner(ownerB);
        vm.prank(ownerB);
        acct.addSigner(deviceB);
        assertTrue(acct.isAuthorizedSigner(deviceB), "new owner's signer authorized");
        assertFalse(acct.isAuthorizedSigner(deviceA), "old owner's signer still invalid");
    }

    function test_former_holders_signer_denied_while_others_hold() public {
        // The security requirement: a device enrolled by a past holder
        // must not be able to act while a DIFFERENT address holds the TBA.
        vm.prank(ownerA);
        acct.addSigner(deviceA);

        acct.setOwner(ownerB);
        assertFalse(acct.isAuthorizedSigner(deviceA), "denied under B");

        acct.setOwner(address(0xC0FFEE));
        assertFalse(acct.isAuthorizedSigner(deviceA), "denied under C");

        // A re-acquires the NFT → A's own device is legitimately live again
        // (authorization always tracks the *current* holder's enrollments).
        acct.setOwner(ownerA);
        assertTrue(acct.isAuthorizedSigner(deviceA), "A's device live again when A re-owns");
    }

    function test_owner_can_remove_signer() public {
        vm.startPrank(ownerA);
        acct.addSigner(deviceA);
        assertTrue(acct.isAuthorizedSigner(deviceA));
        acct.removeSigner(deviceA);
        assertFalse(acct.isAuthorizedSigner(deviceA));
        vm.stopPrank();
    }
}
