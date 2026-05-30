// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @title PairingFacet
/// @notice Device-pairing rendezvous for adding a second device as a
///         signer WITHOUT copying a 0x address between machines.
///
///         The desktop (which holds the MAIN's master wallet) picks a
///         short one-time code and shows it. The phone opens the user's
///         own subdomain at `?pair=<code>`, generates a fresh device
///         keypair, and calls `announcePairing(keccak256(code))` as a
///         sponsored Tempo tx — so the phone's device key is `msg.sender`
///         and pays nothing. The desktop filters `PairingAnnounced` logs
///         by that exact code hash, learns the device address from the
///         indexed `device` topic, and enrolls it via `addSigner` on the
///         TBA.
///
///         Two-way challenge with zero copying:
///           1. Only a holder of the device key can produce a tx whose
///              recovered sender is `device` (Tempo sender signature).
///           2. Only someone who saw the desktop's code can produce the
///              matching `codeHash`.
///
///         Event-only, no storage (mirrors FeedbackFacet): the block log
///         IS the rendezvous channel. Anyone can call; the code hash is
///         single-use in practice (the desktop stops listening after the
///         first match) and reveals nothing — it's a hash of a random
///         short-lived code.
contract PairingFacet {
    event PairingAnnounced(
        bytes32 indexed codeHash,
        address indexed device,
        uint256 timestamp
    );

    /// Announce that `msg.sender` is a device wishing to pair, keyed by
    /// the hash of the one-time code shown on the initiating device.
    function announcePairing(bytes32 codeHash) external {
        emit PairingAnnounced(codeHash, msg.sender, block.timestamp);
    }
}
