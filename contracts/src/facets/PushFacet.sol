// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibPushStorage} from "../libraries/LibPushStorage.sol";

/// @title PushFacet
/// @notice Web Push subscriptions keyed by the SUBSCRIBER'S ADDRESS — the fix
///         for cross-device notifications that never reached devices without a
///         registered MAIN identity. Any device self-registers its own push
///         subscription (`setPushSub`, `msg.sender`-keyed); the credit proxy
///         resolves a subscriber address → subscription directly (no
///         mainOf → tokenId → metadata hop), so even a bare device key that only
///         tapped "subscribe" can be buzzed.
///
///         The subscription JSON is a bearer capability (the endpoint URL can be
///         pushed to by anyone holding it); payloads are still E2E-encrypted to
///         the device's p256dh/auth keys, so the exposure is spam, not content.
///         Bounded so one write can't blow gas.
contract PushFacet {
    event PushSubSet(address indexed who, uint256 length);
    event PushSubCleared(address indexed who);

    error EmptyPushSub();
    error PushSubTooLong(uint256 length);

    /// Register (or replace) the caller's Web Push subscription JSON. Self-keyed:
    /// a device can only ever set ITS OWN subscription.
    function setPushSub(bytes calldata sub) external {
        uint256 len = sub.length;
        if (len == 0) revert EmptyPushSub();
        if (len > 4096) revert PushSubTooLong(len);
        LibPushStorage.load().sub[msg.sender] = sub;
        emit PushSubSet(msg.sender, len);
    }

    /// Drop the caller's subscription (e.g. on unsubscribe / device removal).
    function clearPushSub() external {
        delete LibPushStorage.load().sub[msg.sender];
        emit PushSubCleared(msg.sender);
    }

    /// Read a device's push subscription JSON (empty bytes if none). The proxy
    /// calls this per feed subscriber to fan a broadcast out.
    function pushSubOf(address who) external view returns (bytes memory) {
        return LibPushStorage.load().sub[who];
    }

    /// `true` iff `who` has a non-empty push subscription registered.
    function hasPushSub(address who) external view returns (bool) {
        return LibPushStorage.load().sub[who].length != 0;
    }
}
