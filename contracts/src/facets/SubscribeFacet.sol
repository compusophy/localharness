// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibSubscribeStorage} from "../libraries/LibSubscribeStorage.sol";

/// @title SubscribeFacet
/// @notice Per-subdomain notification subscriber sets — the on-chain half
///         of the "Ready Up" feed (cartridge `host::agent::subscribe` /
///         `broadcast`). PERMISSIONLESS: any identity may subscribe to any
///         subdomain's feed (`subscribe(targetId)`), which is exactly the
///         point — a visitor opts in to a host's pings. The push delivery
///         itself is off-chain (the credit proxy reads `subscribersOf`,
///         looks up each subscriber's on-chain push subscription, and sends
///         Web Push). This facet stores ONLY the membership set; it moves no
///         value and gates nothing on ownership.
///
///         Sybil note: `msg.sender` is an on-chain identity, so a subscriber
///         set is identity-gated by construction — a cartridge that only
///         acts for subscribers is sybil-resistant to the cost of minting
///         identities.
contract SubscribeFacet {
    event Subscribed(uint256 indexed targetId, address indexed who);
    event Unsubscribed(uint256 indexed targetId, address indexed who);

    /// Subscribe `msg.sender` to `targetId`'s feed. Idempotent (a repeat is
    /// a no-op, not a revert) so a client can call it without first reading.
    function subscribe(uint256 targetId) external {
        LibSubscribeStorage.Storage storage s = LibSubscribeStorage.load();
        if (s.indexOf[targetId][msg.sender] != 0) return; // already subscribed
        s.subscribers[targetId].push(msg.sender);
        s.indexOf[targetId][msg.sender] = s.subscribers[targetId].length; // 1-based
        emit Subscribed(targetId, msg.sender);
    }

    /// Unsubscribe `msg.sender` from `targetId`'s feed. Idempotent. O(1)
    /// swap-remove keeps the array dense for `subscribersOf`.
    function unsubscribe(uint256 targetId) external {
        LibSubscribeStorage.Storage storage s = LibSubscribeStorage.load();
        uint256 idx = s.indexOf[targetId][msg.sender];
        if (idx == 0) return; // not subscribed
        address[] storage arr = s.subscribers[targetId];
        uint256 len = arr.length;
        if (idx != len) {
            address moved = arr[len - 1];
            arr[idx - 1] = moved;
            s.indexOf[targetId][moved] = idx;
        }
        arr.pop();
        s.indexOf[targetId][msg.sender] = 0;
        emit Unsubscribed(targetId, msg.sender);
    }

    // --- Views ----------------------------------------------------------

    function isSubscribed(uint256 targetId, address who) external view returns (bool) {
        return LibSubscribeStorage.load().indexOf[targetId][who] != 0;
    }

    function subscriberCount(uint256 targetId) external view returns (uint256) {
        return LibSubscribeStorage.load().subscribers[targetId].length;
    }

    /// The full subscriber list. The proxy's broadcast route reads this,
    /// then looks up each subscriber's push subscription off-chain. Unbounded
    /// in principle; for very large feeds a paged accessor is a follow-up.
    function subscribersOf(uint256 targetId) external view returns (address[] memory) {
        return LibSubscribeStorage.load().subscribers[targetId];
    }
}
