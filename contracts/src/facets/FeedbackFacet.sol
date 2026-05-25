// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @title FeedbackFacet
/// @notice Cuts a single function into the diamond: `submitFeedback`.
///         Anyone can call it; the call emits an event the developer
///         can harvest off-chain via `eth_getLogs`. No storage — the
///         block log IS the database, which keeps gas low and avoids
///         a `mapping(address => string)` that would prevent users
///         from leaving multiple notes.
///
///         The text is bounded so a single submission can't blow gas
///         to the moon, but the limit is loose enough for real notes
///         (2048 bytes ≈ a long paragraph of feedback).
contract FeedbackFacet {
    event FeedbackSubmitted(
        address indexed sender,
        uint256 timestamp,
        string text
    );

    error EmptyFeedback();
    error FeedbackTooLong(uint256 length);

    /// Submit a feedback note. Plain string in, no parsing on-chain.
    /// Whatever the caller passes ends up in the event log verbatim;
    /// off-chain harvesting reads logs as bytes and renders them as
    /// text (NEVER as HTML — render as plain text only).
    function submitFeedback(string calldata text) external {
        uint256 len = bytes(text).length;
        if (len == 0) revert EmptyFeedback();
        if (len > 2048) revert FeedbackTooLong(len);
        emit FeedbackSubmitted(msg.sender, block.timestamp, text);
    }
}
