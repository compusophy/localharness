// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev EIP-2535: introspection for diamonds. "Loupe" because you
///      hold it up to the diamond to see all its facets.
interface IDiamondLoupe {
    struct Facet {
        address facetAddress;
        bytes4[] functionSelectors;
    }

    /// Gets all facets and their selectors.
    function facets() external view returns (Facet[] memory facets_);

    /// Gets all the function selectors supported by a specific facet.
    function facetFunctionSelectors(address _facet)
        external
        view
        returns (bytes4[] memory facetFunctionSelectors_);

    /// Get all the facet addresses used by a diamond.
    function facetAddresses() external view returns (address[] memory facetAddresses_);

    /// Gets the facet that supports the given selector.
    function facetAddress(bytes4 _functionSelector)
        external
        view
        returns (address facetAddress_);
}
