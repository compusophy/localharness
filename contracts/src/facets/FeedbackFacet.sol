// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibFeedbackStorage} from "../libraries/LibFeedbackStorage.sol";

/// @title FeedbackFacet
/// @notice Cuts feedback submission + paged reads into the diamond.
///         Anyone can call `submitFeedback`; the call BOTH emits an event
///         (cheap to stream) AND appends an entry to contract storage, so
///         the history can be read back via view functions even after the
///         RPC's log window has scrolled past it (Tempo caps `eth_getLogs`
///         to a 100k-block window). The append-only array can't be edited
///         or deleted, and users can leave multiple notes.
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
    /// Whatever the caller passes is both emitted in the event log and
    /// appended to storage verbatim; off-chain readers render the text as
    /// plain text only (NEVER as HTML).
    function submitFeedback(string calldata text) external {
        uint256 len = bytes(text).length;
        if (len == 0) revert EmptyFeedback();
        if (len > 2048) revert FeedbackTooLong(len);

        LibFeedbackStorage.load().entries.push(
            LibFeedbackStorage.Entry({
                sender: msg.sender,
                timestamp: uint64(block.timestamp),
                text: text
            })
        );

        emit FeedbackSubmitted(msg.sender, block.timestamp, text);
    }

    /// Total number of feedback submissions ever stored.
    function feedbackCount() external view returns (uint256) {
        return LibFeedbackStorage.load().entries.length;
    }

    /// Read a single entry by index (0-based, oldest first). Reverts if
    /// `i` is out of range via the array bounds check.
    function feedbackAt(uint256 i)
        external
        view
        returns (address sender, uint64 timestamp, string memory text)
    {
        LibFeedbackStorage.Entry storage e = LibFeedbackStorage.load().entries[i];
        return (e.sender, e.timestamp, e.text);
    }

    /// Page over the log: returns up to `count` entries starting at
    /// `start` (clamped to the array length). Three parallel arrays keep
    /// the ABI flat and easy to decode with `cast`.
    function feedbackRange(uint256 start, uint256 count)
        external
        view
        returns (
            address[] memory senders,
            uint64[] memory timestamps,
            string[] memory texts
        )
    {
        LibFeedbackStorage.Entry[] storage entries = LibFeedbackStorage.load().entries;
        uint256 total = entries.length;

        if (start >= total) {
            return (new address[](0), new uint64[](0), new string[](0));
        }

        uint256 end = start + count;
        if (end > total) {
            end = total;
        }
        uint256 n = end - start;

        senders = new address[](n);
        timestamps = new uint64[](n);
        texts = new string[](n);

        for (uint256 k = 0; k < n; k++) {
            LibFeedbackStorage.Entry storage e = entries[start + k];
            senders[k] = e.sender;
            timestamps[k] = e.timestamp;
            texts[k] = e.text;
        }
    }
}
