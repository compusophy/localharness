// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Test} from "forge-std/Test.sol";
import {LocalharnessRegistryFacet} from "../src/facets/LocalharnessRegistryFacet.sol";

/// Defense-in-depth: `register()` must reject names that are not valid DNS
/// labels BEFORE any mint / state write, EXACTLY matching the CLI's
/// `name_is_valid` (1-63 bytes of lowercase a-z / 0-9 / hyphen, no
/// leading/trailing hyphen). The facet is exercised in isolation — its
/// diamond-storage `load()` resolves to the deployed contract's own
/// storage, so `register` mints into this test instance directly.
contract RegistryNameValidationTest is Test {
    LocalharnessRegistryFacet registry;

    function setUp() public {
        registry = new LocalharnessRegistryFacet();
    }

    // --- rejections: every byte that would brick DNS routing ------------

    function test_rejects_oversize_64_chars() public {
        string memory n = _repeat("a", 64); // 64 > 63 max
        vm.expectRevert(abi.encodeWithSelector(LocalharnessRegistryFacet.InvalidName.selector, n));
        registry.register(n);
    }

    function test_rejects_uppercase() public {
        string memory n = "Alice";
        vm.expectRevert(abi.encodeWithSelector(LocalharnessRegistryFacet.InvalidName.selector, n));
        registry.register(n);
    }

    function test_rejects_underscore() public {
        string memory n = "a_b";
        vm.expectRevert(abi.encodeWithSelector(LocalharnessRegistryFacet.InvalidName.selector, n));
        registry.register(n);
    }

    function test_rejects_emoji_non_ascii() public {
        string memory n = unicode"🤖"; // multi-byte UTF-8 fails the per-byte range
        vm.expectRevert(abi.encodeWithSelector(LocalharnessRegistryFacet.InvalidName.selector, n));
        registry.register(n);
    }

    function test_rejects_leading_hyphen() public {
        string memory n = "-foo";
        vm.expectRevert(abi.encodeWithSelector(LocalharnessRegistryFacet.InvalidName.selector, n));
        registry.register(n);
    }

    function test_rejects_trailing_hyphen() public {
        string memory n = "foo-";
        vm.expectRevert(abi.encodeWithSelector(LocalharnessRegistryFacet.InvalidName.selector, n));
        registry.register(n);
    }

    function test_rejects_empty() public {
        string memory n = "";
        vm.expectRevert(abi.encodeWithSelector(LocalharnessRegistryFacet.InvalidName.selector, n));
        registry.register(n);
    }

    function test_rejects_only_hyphen() public {
        string memory n = "-";
        vm.expectRevert(abi.encodeWithSelector(LocalharnessRegistryFacet.InvalidName.selector, n));
        registry.register(n);
    }

    // --- acceptances: valid DNS labels mint cleanly ---------------------

    function test_accepts_simple_name() public {
        uint256 id = registry.register("alice");
        assertEq(id, 1, "first mint is tokenId 1");
        assertEq(registry.ownerOfName("alice"), address(this));
    }

    function test_accepts_internal_hyphens() public {
        uint256 id = registry.register("a-b-c");
        assertEq(registry.ownerOfName("a-b-c"), address(this));
        assertEq(registry.idOfName("a-b-c"), id);
    }

    function test_accepts_digits_and_max_length() public {
        // Single char (min) and full 63-char label (max) both valid.
        registry.register("a");
        registry.register(_repeat("z", 63));
        assertEq(registry.ownerOfName(_repeat("z", 63)), address(this));
    }

    // --- no ghost left behind on a rejected register --------------------

    function test_rejected_name_writes_no_state() public {
        string memory n = "Bad_Name";
        vm.expectRevert(abi.encodeWithSelector(LocalharnessRegistryFacet.InvalidName.selector, n));
        registry.register(n);
        // The revert is atomic: nothing minted, counter untouched.
        assertEq(registry.idOfName(n), 0, "rejected name must not be registered");
        assertEq(registry.nextId(), 0, "nextId must not advance on a rejected mint");
    }

    function _repeat(string memory ch, uint256 n) internal pure returns (string memory) {
        bytes memory out = new bytes(n);
        bytes1 c = bytes(ch)[0];
        for (uint256 i = 0; i < n; i++) out[i] = c;
        return string(out);
    }
}
