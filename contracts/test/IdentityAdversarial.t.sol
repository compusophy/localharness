// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Test} from "forge-std/Test.sol";

import {Diamond} from "../src/Diamond.sol";
import {DiamondInit} from "../src/upgradeInitializers/DiamondInit.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";

import {LocalharnessRegistryFacet} from "../src/facets/LocalharnessRegistryFacet.sol";
import {ERC721Facet} from "../src/facets/ERC721Facet.sol";
import {MainIdentityFacet} from "../src/facets/MainIdentityFacet.sol";
import {TbaFacet} from "../src/facets/TbaFacet.sol";
import {DeviceRegistryFacet} from "../src/facets/DeviceRegistryFacet.sol";
import {ReleaseFacet} from "../src/facets/ReleaseFacet.sol";
import {ERC6551Registry} from "../src/erc6551/ERC6551Registry.sol";
import {MultiSignerAccount} from "../src/erc6551/MultiSignerAccount.sol";

/// Adversarial review of the LIVE identity / registry facet set —
/// the surface that governs name ownership, the NFTs, token-bound
/// accounts, MAIN identity, linked devices, and on-chain metadata
/// (an agent's public face / persona). A bug here = identity theft.
///
/// Built as a REAL diamond with every identity facet cut in over ONE
/// shared storage layer, so cross-facet flows that DON'T exist in
/// single-facet isolation (transfer→MAIN, transfer→TBA signer authority,
/// the release MAIN guard) are exercised exactly as on-chain.
///
/// Aggregate ABI of all identity facets, so we can call the diamond
/// through one typed handle.
interface IIdentity {
    // registry
    function register(string calldata name) external returns (uint256);
    function setMetadata(uint256 agentId, bytes32 key, bytes calldata value) external;
    function metadata(uint256 agentId, bytes32 key) external view returns (bytes memory);
    function ownerOfName(string calldata name) external view returns (address);
    function ownerOfId(uint256 id) external view returns (address);
    function idOfName(string calldata name) external view returns (uint256);
    function isTaken(string calldata name) external view returns (bool);
    function setRegistrationCost(uint256 c) external;
    // erc721
    function balanceOf(address owner) external view returns (uint256);
    function ownerOf(uint256 tokenId) external view returns (address);
    function approve(address to, uint256 tokenId) external;
    function getApproved(uint256 tokenId) external view returns (address);
    function setApprovalForAll(address operator, bool approved) external;
    function transferFrom(address from, address to, uint256 tokenId) external;
    function tokenURI(uint256 tokenId) external view returns (string memory);
    // main identity
    function registerMain(uint256 tokenId) external;
    function clearMain() external;
    function mainOf(address holder) external view returns (uint256);
    function isMain(uint256 tokenId) external view returns (bool);
    // tba
    function setTbaConfig(address registry, address impl) external;
    function tokenBoundAccount(uint256 tokenId) external view returns (address);
    function createTokenBoundAccount(uint256 tokenId) external returns (address);
    // device registry
    function linkDevice(uint256 mainId, address device) external;
    function unlinkDevice(uint256 mainId, address device) external;
    function devicesOf(uint256 mainId) external view returns (address[] memory);
    function isDeviceLinked(uint256 mainId, address device) external view returns (bool);
    // release
    function releaseName(uint256 tokenId) external;
    function adminBurnNames(uint256[] calldata tokenIds) external;
    function adminResetAll() external;
}

contract IdentityAdversarialTest is Test {
    IIdentity id;
    Diamond diamond;
    ERC6551Registry tbaRegistry;
    MultiSignerAccount accountImpl;

    address admin = address(0xADD1); // diamond (EIP-173) owner
    address alice = address(0xA11CE);
    address bob = address(0xB0B);
    address mallory = address(0x4A110); // the attacker
    address deviceA = address(0xD7117CE);

    bytes32 constant K_PERSONA = keccak256("localharness.persona");
    bytes32 constant K_FACE = keccak256("localharness.public_face");

    function setUp() public {
        // Deploy facets.
        LocalharnessRegistryFacet regF = new LocalharnessRegistryFacet();
        ERC721Facet ercF = new ERC721Facet();
        MainIdentityFacet mainF = new MainIdentityFacet();
        TbaFacet tbaF = new TbaFacet();
        DeviceRegistryFacet devF = new DeviceRegistryFacet();
        ReleaseFacet relF = new ReleaseFacet();
        DiamondInit initc = new DiamondInit();

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](6);
        cuts[0] = _cut(address(regF), _registrySelectors());
        cuts[1] = _cut(address(ercF), _erc721Selectors());
        cuts[2] = _cut(address(mainF), _mainSelectors());
        cuts[3] = _cut(address(tbaF), _tbaSelectors());
        cuts[4] = _cut(address(devF), _deviceSelectors());
        cuts[5] = _cut(address(relF), _releaseSelectors());

        // Deploy diamond owned by `admin`. nextId is seeded lazily by
        // register's `if (nextId==0) nextId=1` guard (DiamondInit does the
        // same on the live deploy), so token ids start at 1 — never 0.
        initc; // DiamondInit deployed above; not needed for these unit tests.
        vm.prank(admin);
        diamond = new Diamond(admin, cuts);
        id = IIdentity(address(diamond));

        // 6551 infra + TBA config (owner-only).
        tbaRegistry = new ERC6551Registry();
        accountImpl = new MultiSignerAccount();
        vm.prank(admin);
        id.setTbaConfig(address(tbaRegistry), address(accountImpl));
    }

    // ====================================================================
    // setMetadata — the agent-hijack surface
    // ====================================================================

    /// A NON-owner of a name must NOT be able to overwrite its metadata
    /// (persona / public_face / app.wasm). This is the agent-hijack guard:
    /// `setMetadata` is gated on `ownerOfId[agentId] == msg.sender`.
    function test_nonOwner_cannot_setMetadata() public {
        vm.prank(alice);
        uint256 tok = id.register("alice");

        // Mallory tries to repaint Alice's persona.
        vm.prank(mallory);
        vm.expectRevert("not owner");
        id.setMetadata(tok, K_PERSONA, bytes("i am evil now"));

        // Untouched.
        assertEq(id.metadata(tok, K_PERSONA).length, 0, "persona must be untouched");
    }

    /// Even an ERC-721-APPROVED spender (token approval) is NOT a metadata
    /// authority — setMetadata checks ownerOfId only, not approvals. Approval
    /// is for transfer, not for impersonation.
    function test_approvedSpender_cannot_setMetadata() public {
        vm.prank(alice);
        uint256 tok = id.register("alice");
        vm.prank(alice);
        id.approve(mallory, tok); // mallory may transfer, but not impersonate

        vm.prank(mallory);
        vm.expectRevert("not owner");
        id.setMetadata(tok, K_FACE, bytes("app"));
    }

    /// The owner CAN set their own metadata (positive control).
    function test_owner_can_setMetadata() public {
        vm.prank(alice);
        uint256 tok = id.register("alice");
        vm.prank(alice);
        id.setMetadata(tok, K_PERSONA, bytes("hello"));
        assertEq(id.metadata(tok, K_PERSONA), bytes("hello"));
    }

    /// After a transfer, metadata authority follows the NFT: the OLD owner
    /// loses write access and the NEW owner gains it. (setMetadata reads
    /// ownerOfId fresh, so no stale authority survives the transfer.)
    function test_setMetadata_authority_follows_transfer() public {
        vm.prank(alice);
        uint256 tok = id.register("alice");
        vm.prank(alice);
        id.transferFrom(alice, bob, tok);

        // Alice can no longer write.
        vm.prank(alice);
        vm.expectRevert("not owner");
        id.setMetadata(tok, K_PERSONA, bytes("alice still here"));

        // Bob can.
        vm.prank(bob);
        id.setMetadata(tok, K_PERSONA, bytes("bob owns this now"));
        assertEq(id.metadata(tok, K_PERSONA), bytes("bob owns this now"));
    }

    // ====================================================================
    // register — name theft
    // ====================================================================

    /// You cannot re-register a taken name to steal it.
    function test_cannot_register_taken_name() public {
        vm.prank(alice);
        id.register("alice");

        vm.prank(mallory);
        vm.expectRevert("name taken");
        id.register("alice");

        assertEq(id.ownerOfName("alice"), alice, "owner unchanged");
    }

    /// Token ids start at 1; id 0 is the "free" sentinel. A direct register
    /// can never mint token 0 (which would read as unclaimed and be stealable).
    function test_first_token_id_is_one_not_zero() public {
        vm.prank(alice);
        uint256 tok = id.register("alice");
        assertEq(tok, 1, "first mint must be id 1, never 0");
    }

    // ====================================================================
    // ERC-721 transfer / approve authorization
    // ====================================================================

    /// A stranger cannot move someone else's NFT (the name).
    function test_stranger_cannot_transfer() public {
        vm.prank(alice);
        uint256 tok = id.register("alice");

        vm.prank(mallory);
        vm.expectRevert("ERC721: not approved");
        id.transferFrom(alice, mallory, tok);

        assertEq(id.ownerOf(tok), alice);
    }

    /// A stranger cannot approve themselves on someone else's NFT.
    function test_stranger_cannot_approve() public {
        vm.prank(alice);
        uint256 tok = id.register("alice");

        vm.prank(mallory);
        vm.expectRevert("ERC721: not approved");
        id.approve(mallory, tok);
    }

    /// `transferFrom` with the wrong `from` reverts (can't spoof source).
    function test_transfer_wrong_from_reverts() public {
        vm.prank(alice);
        uint256 tok = id.register("alice");
        vm.prank(alice);
        // alice is owner, but claims `from = bob`.
        vm.expectRevert("ERC721: wrong from");
        id.transferFrom(bob, mallory, tok);
    }

    /// A per-token approval is CLEARED by a transfer — the old approved
    /// spender can't yank the token back from the new owner.
    function test_approval_cleared_on_transfer() public {
        vm.prank(alice);
        uint256 tok = id.register("alice");
        vm.prank(alice);
        id.approve(mallory, tok); // mallory approved while alice owns

        vm.prank(alice);
        id.transferFrom(alice, bob, tok); // now bob owns

        assertEq(id.getApproved(tok), address(0), "stale approval must be cleared");

        // Mallory's old approval is dead — can't steal from bob.
        vm.prank(mallory);
        vm.expectRevert("ERC721: not approved");
        id.transferFrom(bob, mallory, tok);
    }

    /// Balances move correctly on transfer (no balance inflation / underflow).
    function test_balances_move_on_transfer() public {
        vm.prank(alice);
        uint256 tok = id.register("alice");
        assertEq(id.balanceOf(alice), 1);
        assertEq(id.balanceOf(bob), 0);

        vm.prank(alice);
        id.transferFrom(alice, bob, tok);
        assertEq(id.balanceOf(alice), 0);
        assertEq(id.balanceOf(bob), 1);
    }

    /// Operator (approval-for-all) can move the token; a revoked operator cannot.
    function test_operator_transfer_and_revocation() public {
        vm.prank(alice);
        uint256 tok = id.register("alice");
        vm.prank(alice);
        id.setApprovalForAll(bob, true);

        vm.prank(bob);
        id.transferFrom(alice, bob, tok); // operator moves it to self
        assertEq(id.ownerOf(tok), bob);

        // Revoke; bob (no longer owner anyway) can't move it again.
        vm.prank(bob);
        id.transferFrom(bob, alice, tok); // bob is owner, moves back
        vm.prank(alice);
        id.setApprovalForAll(bob, false);
        vm.prank(bob);
        vm.expectRevert("ERC721: not approved");
        id.transferFrom(alice, bob, tok);
    }

    // ====================================================================
    // MAIN identity
    // ====================================================================

    /// You cannot set a token you don't own as your MAIN.
    function test_cannot_set_unowned_as_main() public {
        vm.prank(alice);
        uint256 tok = id.register("alice");

        vm.prank(mallory);
        vm.expectRevert(
            abi.encodeWithSelector(MainIdentityFacet.NotOwner.selector, tok, mallory)
        );
        id.registerMain(tok);
    }

    /// registerMain only writes the CALLER's own slot; you cannot set
    /// another address's MAIN (there's no `holder` parameter — it's always
    /// msg.sender, and the token must be owned by msg.sender).
    function test_main_is_caller_scoped() public {
        vm.prank(alice);
        uint256 tok = id.register("alice");
        vm.prank(alice);
        id.registerMain(tok);

        assertEq(id.mainOf(alice), tok);
        assertEq(id.mainOf(mallory), 0, "attacker has no MAIN set on their behalf");
        assertTrue(id.isMain(tok));
    }

    /// `isMain` self-heals on transfer: it reads ownerOfId fresh and compares
    /// to mainOf[currentOwner]. After Alice transfers her MAIN token to Bob,
    /// isMain(tok) is false (Bob hasn't claimed it as MAIN) — no stale "this
    /// is alice's main" assertion grants control.
    function test_isMain_selfheals_on_transfer() public {
        vm.prank(alice);
        uint256 tok = id.register("alice");
        vm.prank(alice);
        id.registerMain(tok);
        assertTrue(id.isMain(tok));

        vm.prank(alice);
        id.transferFrom(alice, bob, tok);

        // isMain follows the live owner, not the stale mainOf[alice] pointer.
        assertFalse(id.isMain(tok), "stale MAIN must not assert post-transfer");
    }

    // ====================================================================
    // Release / burn
    // ====================================================================

    /// releaseName is owner-only — a stranger can't burn your name.
    function test_stranger_cannot_release() public {
        vm.prank(alice);
        uint256 tok = id.register("alice");

        vm.prank(mallory);
        vm.expectRevert(ReleaseFacet.NotOwner.selector);
        id.releaseName(tok);

        assertEq(id.ownerOf(tok), alice, "name not burned");
    }

    /// releaseName REFUSES to burn the caller's MAIN (would orphan identity).
    function test_cannot_release_main() public {
        vm.prank(alice);
        uint256 tok = id.register("alice");
        vm.prank(alice);
        id.registerMain(tok);

        vm.prank(alice);
        vm.expectRevert(ReleaseFacet.CannotReleaseMain.selector);
        id.releaseName(tok);
    }

    /// A NON-main name CAN be released by its owner, and the burn frees the
    /// name + clears exactly what register wrote (name re-registers cleanly,
    /// to a NEW id; the freed name is no longer owned).
    function test_release_frees_name_for_reregistration() public {
        vm.prank(alice);
        uint256 tok = id.register("alice");
        // Make a second name the MAIN so "alice" isn't the protected MAIN.
        vm.prank(alice);
        uint256 mainTok = id.register("alicemain");
        vm.prank(alice);
        id.registerMain(mainTok);

        vm.prank(alice);
        id.releaseName(tok);

        assertEq(id.ownerOfName("alice"), address(0), "name freed");
        assertFalse(id.isTaken("alice"), "name not taken after burn");

        // Mallory can now grab the freed name — it's genuinely free, and the
        // new mint is a fresh id (no inherited metadata/owner).
        vm.prank(mallory);
        uint256 tok2 = id.register("alice");
        assertTrue(tok2 != tok, "re-register mints a NEW id");
        assertEq(id.ownerOfName("alice"), mallory);
        assertEq(id.metadata(tok2, K_PERSONA).length, 0, "no inherited metadata");
    }

    /// The burn clears the MAIN pointer when the burned id WAS the MAIN —
    /// but releaseName refuses MAIN, so this is only reachable via admin
    /// burn. Verify adminBurnNames clears the MAIN pointer (no dangling
    /// primary-identity reference).
    function test_adminBurn_clears_main_pointer() public {
        vm.prank(alice);
        uint256 tok = id.register("alice");
        vm.prank(alice);
        id.registerMain(tok);
        assertEq(id.mainOf(alice), tok);

        uint256[] memory ids = new uint256[](1);
        ids[0] = tok;
        vm.prank(admin);
        id.adminBurnNames(ids);

        assertEq(id.mainOf(alice), 0, "MAIN pointer must be cleared on burn");
        assertEq(id.ownerOfName("alice"), address(0));
    }

    // ====================================================================
    // Admin reset (EIP-173) — strictly diamond-owner-gated
    // ====================================================================

    /// adminBurnNames is diamond-owner-only — a stranger can't force-burn.
    function test_nonOwner_cannot_adminBurn() public {
        vm.prank(alice);
        uint256 tok = id.register("alice");
        uint256[] memory ids = new uint256[](1);
        ids[0] = tok;

        vm.prank(mallory);
        vm.expectRevert("LibDiamond: not owner");
        id.adminBurnNames(ids);

        assertEq(id.ownerOf(tok), alice, "name not burned by non-owner");
    }

    /// adminResetAll is diamond-owner-only.
    function test_nonOwner_cannot_adminResetAll() public {
        vm.prank(alice);
        id.register("alice");

        vm.prank(mallory);
        vm.expectRevert("LibDiamond: not owner");
        id.adminResetAll();

        assertEq(id.ownerOfName("alice"), alice);
    }

    /// The legitimate diamond owner CAN reset (positive control), and it
    /// wipes the whole 1..nextId range.
    function test_owner_can_adminResetAll() public {
        vm.prank(alice);
        id.register("alice");
        vm.prank(bob);
        id.register("bob");

        vm.prank(admin);
        id.adminResetAll();

        assertEq(id.ownerOfName("alice"), address(0));
        assertEq(id.ownerOfName("bob"), address(0));
    }

    // ====================================================================
    // TBA — only the right config can be set; lookups are owner-agnostic
    // but deploys can't grant control of an unowned token
    // ====================================================================

    /// setTbaConfig is diamond-owner-only — a stranger can't repoint the
    /// 6551 registry/impl (which would change every agent's wallet address).
    function test_nonOwner_cannot_setTbaConfig() public {
        vm.prank(mallory);
        vm.expectRevert("LibDiamond: not owner");
        id.setTbaConfig(address(0xdead), address(0xbeef));
    }

    /// tokenBoundAccount reverts for a nonexistent token (can't derive a TBA
    /// for an unregistered/burned name).
    function test_tba_reverts_for_nonexistent_token() public {
        vm.expectRevert("TBA: nonexistent token");
        id.tokenBoundAccount(999);
    }

    /// Deploying a TBA is permissionless BY DESIGN (6551), but the deployer
    /// gains NO control: the account's `owner()` resolves to the NFT holder,
    /// not the deployer. Mallory deploying Alice's TBA doesn't make Mallory
    /// a signer.
    function test_tba_deploy_grants_deployer_no_authority() public {
        vm.prank(alice);
        uint256 tok = id.register("alice");

        address predicted = id.tokenBoundAccount(tok);

        // Mallory deploys it — permissionless, idempotent.
        vm.prank(mallory);
        address deployed = id.createTokenBoundAccount(tok);
        assertEq(deployed, predicted, "deploy lands at counterfactual address");

        // The account's owner is the NFT holder (alice), NOT the deployer.
        MultiSignerAccount acct = MultiSignerAccount(payable(deployed));
        assertEq(acct.owner(), alice, "TBA owner is the NFT holder, not deployer");
        assertTrue(acct.isAuthorizedSigner(alice), "holder authorized");
        assertFalse(acct.isAuthorizedSigner(mallory), "deployer is NOT a signer");

        // Mallory cannot enroll themselves as a signer.
        vm.prank(mallory);
        vm.expectRevert("MultiSigner: only owner");
        acct.addSigner(mallory);
    }

    /// TBA signer authority follows the NFT: after Alice transfers her name
    /// to Bob, Alice's enrolled device signer goes dormant and Bob is the
    /// authority — no stale device retains control of the wallet.
    function test_tba_signer_authority_follows_nft_transfer() public {
        vm.prank(alice);
        uint256 tok = id.register("alice");
        address tba = id.createTokenBoundAccount(tok);
        MultiSignerAccount acct = MultiSignerAccount(payable(tba));

        vm.prank(alice);
        acct.addSigner(deviceA);
        assertTrue(acct.isAuthorizedSigner(deviceA), "device live under alice");

        // Transfer the NFT to bob.
        vm.prank(alice);
        id.transferFrom(alice, bob, tok);

        assertEq(acct.owner(), bob, "TBA owner follows NFT");
        assertFalse(acct.isAuthorizedSigner(deviceA), "alice's device dormant after transfer");
        assertFalse(acct.isAuthorizedSigner(alice), "former holder no longer authorized");
        assertTrue(acct.isAuthorizedSigner(bob), "new holder authorized");
    }

    // ====================================================================
    // DeviceRegistry — link/unlink gated to the identity's current holder
    // ====================================================================

    /// Only the MAIN tokenId's current NFT holder may link a device — a
    /// stranger can't inject a device into someone else's identity index.
    function test_stranger_cannot_linkDevice() public {
        vm.prank(alice);
        uint256 tok = id.register("alice");

        vm.prank(mallory);
        vm.expectRevert(DeviceRegistryFacet.NotIdentityOwner.selector);
        id.linkDevice(tok, deviceA);

        assertFalse(id.isDeviceLinked(tok, deviceA));
    }

    /// The owner can link/unlink; the enumerable index stays consistent.
    function test_owner_link_unlink_roundtrip() public {
        vm.prank(alice);
        uint256 tok = id.register("alice");

        vm.prank(alice);
        id.linkDevice(tok, deviceA);
        assertTrue(id.isDeviceLinked(tok, deviceA));
        assertEq(id.devicesOf(tok).length, 1);

        vm.prank(alice);
        id.unlinkDevice(tok, deviceA);
        assertFalse(id.isDeviceLinked(tok, deviceA));
        assertEq(id.devicesOf(tok).length, 0);
    }

    /// After an NFT transfer, the OLD holder can no longer manage the device
    /// index (linkDevice resolves the owner fresh via ownerOfId) — and the
    /// NEW holder can. No residual device-management authority survives.
    function test_device_management_follows_transfer() public {
        vm.prank(alice);
        uint256 tok = id.register("alice");
        vm.prank(alice);
        id.transferFrom(alice, bob, tok);

        // Alice (former holder) is locked out.
        vm.prank(alice);
        vm.expectRevert(DeviceRegistryFacet.NotIdentityOwner.selector);
        id.linkDevice(tok, deviceA);

        // Bob (new holder) can manage.
        vm.prank(bob);
        id.linkDevice(tok, deviceA);
        assertTrue(id.isDeviceLinked(tok, deviceA));
    }

    // --- cut helpers -----------------------------------------------------

    function _cut(address facet, bytes4[] memory sels)
        internal
        pure
        returns (IDiamond.FacetCut memory)
    {
        return IDiamond.FacetCut({
            facetAddress: facet,
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: sels
        });
    }

    function _registrySelectors() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](7);
        s[0] = LocalharnessRegistryFacet.register.selector;
        s[1] = LocalharnessRegistryFacet.setMetadata.selector;
        s[2] = LocalharnessRegistryFacet.metadata.selector;
        s[3] = LocalharnessRegistryFacet.ownerOfName.selector;
        s[4] = LocalharnessRegistryFacet.ownerOfId.selector;
        s[5] = LocalharnessRegistryFacet.idOfName.selector;
        s[6] = LocalharnessRegistryFacet.isTaken.selector;
    }

    function _erc721Selectors() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](7);
        s[0] = ERC721Facet.balanceOf.selector;
        s[1] = ERC721Facet.ownerOf.selector;
        s[2] = ERC721Facet.approve.selector;
        s[3] = ERC721Facet.getApproved.selector;
        s[4] = ERC721Facet.setApprovalForAll.selector;
        s[5] = ERC721Facet.transferFrom.selector;
        s[6] = ERC721Facet.tokenURI.selector;
    }

    function _mainSelectors() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](4);
        s[0] = MainIdentityFacet.registerMain.selector;
        s[1] = MainIdentityFacet.clearMain.selector;
        s[2] = MainIdentityFacet.mainOf.selector;
        s[3] = MainIdentityFacet.isMain.selector;
    }

    function _tbaSelectors() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](3);
        s[0] = TbaFacet.setTbaConfig.selector;
        s[1] = TbaFacet.tokenBoundAccount.selector;
        s[2] = TbaFacet.createTokenBoundAccount.selector;
    }

    function _deviceSelectors() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](4);
        s[0] = DeviceRegistryFacet.linkDevice.selector;
        s[1] = DeviceRegistryFacet.unlinkDevice.selector;
        s[2] = DeviceRegistryFacet.devicesOf.selector;
        s[3] = DeviceRegistryFacet.isDeviceLinked.selector;
    }

    function _releaseSelectors() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](3);
        s[0] = ReleaseFacet.releaseName.selector;
        s[1] = ReleaseFacet.adminBurnNames.selector;
        s[2] = ReleaseFacet.adminResetAll.selector;
    }
}
