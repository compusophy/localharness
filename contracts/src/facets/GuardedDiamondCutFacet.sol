// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {IDiamondCut} from "../interfaces/IDiamondCut.sol";
import {LibDiamond} from "../libraries/LibDiamond.sol";

/// @dev A drop-in replacement for {DiamondCutFacet}, seeded into agent-owned
///      CHILD diamonds (SolidityLite design §7, the trust-critical on-chain
///      half). An agent OWNS its child diamond, so it can call `diamondCut`
///      freely — this facet makes that safe by construction. It occupies the
///      SAME selector (`0x1f931c1c`) and is therefore the only `diamondCut`
///      entry point, and it refuses to:
///
///        - touch any RESERVED selector (cut / ownership / loupe / ERC-165) in
///          ANY action. Because `diamondCut` itself is reserved, the guard
///          cannot be replaced or removed — the constraint is PERMANENT. The
///          owner controls (`transferOwnership`/`owner`) and loupe stay bound
///          to their genesis facets and can't be hijacked or blinded.
///        - run an `_init` delegatecall: `_init` must be `address(0)` and
///          `_calldata` empty. An init delegatecall runs arbitrary code in the
///          diamond's storage context (it could overwrite the owner slot) — the
///          §7 highest-severity vector.
///
///      Everything else — adding/replacing/removing the agent's OWN facets —
///      works exactly like the standard cut. This is the on-chain twin of the
///      off-chain `cut_guard` lint (`src/cut_guard.rs`): the lint catches
///      mistakes before gas, this binds the invariant even for a raw,
///      CLI-bypassing, agent-signed `diamondCut`.
contract GuardedDiamondCutFacet is IDiamondCut {
    // Diamond CORE selectors no agent cut may add/replace/remove. Kept in sync
    // with `cut_guard::RESERVED_SELECTORS` on the Rust side.
    bytes4 private constant DIAMOND_CUT = 0x1f931c1c; // diamondCut((address,uint8,bytes4[])[],address,bytes)
    bytes4 private constant TRANSFER_OWNERSHIP = 0xf2fde38b; // transferOwnership(address)
    bytes4 private constant OWNER = 0x8da5cb5b; // owner()
    bytes4 private constant FACETS = 0x7a0ed627; // facets()
    bytes4 private constant FACET_FUNCTION_SELECTORS = 0xadfca15e; // facetFunctionSelectors(address)
    bytes4 private constant FACET_ADDRESSES = 0x52ef6b2c; // facetAddresses()
    bytes4 private constant FACET_ADDRESS = 0xcdffacc6; // facetAddress(bytes4)
    bytes4 private constant SUPPORTS_INTERFACE = 0x01ffc9a7; // supportsInterface(bytes4)

    function diamondCut(
        FacetCut[] calldata _diamondCut,
        address _init,
        bytes calldata _calldata
    ) external override {
        LibDiamond.enforceIsContractOwner();
        require(_init == address(0), "Guarded: init delegatecall forbidden");
        require(_calldata.length == 0, "Guarded: init calldata forbidden");
        for (uint256 i; i < _diamondCut.length; i++) {
            bytes4[] calldata sels = _diamondCut[i].functionSelectors;
            for (uint256 j; j < sels.length; j++) {
                require(!_isReserved(sels[j]), "Guarded: reserved selector");
            }
        }
        // Always pass a zeroed init — guaranteed no delegatecall.
        LibDiamond.diamondCut(_diamondCut, address(0), "");
    }

    function _isReserved(bytes4 s) private pure returns (bool) {
        return
            s == DIAMOND_CUT ||
            s == TRANSFER_OWNERSHIP ||
            s == OWNER ||
            s == FACETS ||
            s == FACET_FUNCTION_SELECTORS ||
            s == FACET_ADDRESSES ||
            s == FACET_ADDRESS ||
            s == SUPPORTS_INTERFACE;
    }
}
