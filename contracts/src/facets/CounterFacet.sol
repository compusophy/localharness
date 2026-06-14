// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibCounterStorage} from "../libraries/LibCounterStorage.sol";

/// @title CounterFacet
/// @notice A minimal per-caller counter — the deploy/cut target for SolidityLite
///         Installment 0 (design/soliditylite.md §3). It exercises every v1
///         primitive of the future Solidity-subset compiler: a `mapping` + a
///         scalar in keccak-namespaced diamond storage, an `event` with an
///         `indexed` param, two bounded `require`s, `msg.sender`, compound
///         arithmetic, and `view` reads. Hand-written here as the curated
///         template the compiler will eventually auto-synthesize.
///
///         No constructor: a facet is DELEGATECALL'd from the diamond, so all
///         state lives in the diamond's storage via `LibCounterStorage.s()`
///         (`msg.sender` is the real caller, `address(this)` the diamond).
///
///         CUTTING IT (diamond owner; mirror script/Add*.s.sol): deploy +
///         diamondCut Add [increment(), incrementBy(uint256), countOf(address),
///         totalCount()].
contract CounterFacet {
    /// Emitted on every write with the caller's new per-address count and the
    /// new global total.
    event Incremented(address indexed who, uint256 newCount, uint256 newTotal);

    /// Bump the caller's count (and the global total) by one.
    function increment() external {
        LibCounterStorage.Storage storage st = LibCounterStorage.s();
        uint256 newCount = st.count[msg.sender] + 1;
        st.count[msg.sender] = newCount;
        st.total += 1;
        emit Incremented(msg.sender, newCount, st.total);
    }

    /// Bump the caller's count (and the global total) by `n`, bounded to
    /// `1..=100` so a single call can't run the count away.
    function incrementBy(uint256 n) external {
        require(n > 0, "zero");
        require(n <= 100, "too big");
        LibCounterStorage.Storage storage st = LibCounterStorage.s();
        uint256 newCount = st.count[msg.sender] + n;
        st.count[msg.sender] = newCount;
        st.total += n;
        emit Incremented(msg.sender, newCount, st.total);
    }

    /// The increment count recorded for `who`. View — no gas, no tx.
    function countOf(address who) external view returns (uint256) {
        return LibCounterStorage.s().count[who];
    }

    /// The running sum of every increment across all callers. View.
    function totalCount() external view returns (uint256) {
        return LibCounterStorage.s().total;
    }
}
