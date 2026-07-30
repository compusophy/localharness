// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {Diamond} from "../src/Diamond.sol";
import {DiamondInit} from "../src/upgradeInitializers/DiamondInit.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";
import {IDiamondLoupe} from "../src/interfaces/IDiamondLoupe.sol";

import {DiamondCutFacet} from "../src/facets/DiamondCutFacet.sol";
import {DiamondLoupeFacet} from "../src/facets/DiamondLoupeFacet.sol";
import {OwnershipFacet} from "../src/facets/OwnershipFacet.sol";
import {LocalharnessRegistryFacet} from "../src/facets/LocalharnessRegistryFacet.sol";
import {ERC721Facet} from "../src/facets/ERC721Facet.sol";
import {TbaFacet} from "../src/facets/TbaFacet.sol";
import {MainIdentityFacet} from "../src/facets/MainIdentityFacet.sol";
import {DeviceRegistryFacet} from "../src/facets/DeviceRegistryFacet.sol";
import {ReleaseFacet} from "../src/facets/ReleaseFacet.sol";
import {CreditsFacet} from "../src/facets/CreditsFacet.sol";
import {RedeemFacet} from "../src/facets/RedeemFacet.sol";
import {InviteFacet} from "../src/facets/InviteFacet.sol";
import {CreditMeterFacet} from "../src/facets/CreditMeterFacet.sol";
import {MintGateFacet} from "../src/facets/MintGateFacet.sol";
import {X402Facet} from "../src/facets/X402Facet.sol";
import {TitheFacet} from "../src/facets/TitheFacet.sol";
import {BountyFacet} from "../src/facets/BountyFacet.sol";
import {PartyFacet} from "../src/facets/PartyFacet.sol";
import {GuildFacet} from "../src/facets/GuildFacet.sol";
import {VotingFacet} from "../src/facets/VotingFacet.sol";
import {WeightedVotingFacet} from "../src/facets/WeightedVotingFacet.sol";
import {ReputationFacet} from "../src/facets/ReputationFacet.sol";
import {ValidationFacet} from "../src/facets/ValidationFacet.sol";
import {SessionRoomFacet} from "../src/facets/SessionRoomFacet.sol";
import {SignalingFacet} from "../src/facets/SignalingFacet.sol";
import {SubscribeFacet} from "../src/facets/SubscribeFacet.sol";
import {MessageFacet} from "../src/facets/MessageFacet.sol";
import {GuardedDiamondCutFacet} from "../src/facets/GuardedDiamondCutFacet.sol";

import {LocalharnessCredits} from "../src/LocalharnessCredits.sol";
import {ERC6551Registry} from "../src/erc6551/ERC6551Registry.sol";
import {MultiSignerAccount} from "../src/erc6551/MultiSignerAccount.sol";

/// RESET GENESIS — deploy a FRESH, COMPLETE localharness diamond in one run.
///
/// Replaces the historical DeployDiamond + 14 one-shot patch-cut scripts: the
/// selector surface here is the FULL CURRENT one, extracted from the facet
/// SOURCES (which include every later patch: adminBurnNames/adminResetAll,
/// setRegistrationCost/treasury, withdrawableOf, settleUpto, lastRun-free
/// schedule-less world, name validation, guild-guarded release, ...).
///
/// 27 facets / 215 selectors. Deliberately DROPPED from the live-diamond
/// surface: SessionFacet (6), ScheduleFacet (18), TeamFacet (11) — retired.
/// Deliberately ADDED over live: X402Facet.settleUpto (source ahead of chain).
/// See design/reset-genesis.md for the audit table.
///
/// Also deploys the ecosystem contracts a reset needs:
///   - LocalharnessCredits ($LH) + ISSUER_ROLE grant to the diamond
///   - ERC6551Registry + MultiSignerAccount impl, wired via setTbaConfig
///   - a standalone GuardedDiamondCutFacet (child-diamond genesis seed —
///     re-pin it in src/registry/chain.rs together with the logged loupe +
///     ownership facet addresses)
///
/// Env (EVM_PRIVATE_KEY required; the rest optional, defaults mirror the
/// 2026-07-30 live mainnet config):
///   SUPPLY_CAP_WEI          $LH global supply cap        (default 1e27)
///   MINT_WINDOW_CAP_WEI     C1 rolling mint cap          (default 0 = OFF; set on mainnet!)
///   MINT_WINDOW_SECS        C1 window                    (default 1 days)
///   INITIAL_DAILY_ALLOWANCE faucet                       (default 0 = DISABLED, sybil hole)
///   REGISTRATION_COST_WEI   register() price             (default 1e18 = live value)
///   MAIN_COST_WEI           registerMain() price         (default 0)
///   METER_ADDR              proxy metering key           (default 0 = configure later)
///   FIAT_ISSUER_SIGNER      Stripe-webhook mint signer   (default 0 = configure later)
///   CLAWBACKER              refund clawback key          (default 0 = configure later)
///   FIAT_LOCK_SECS          fiat-mint lock               (default 0 = live value)
///   PER_RECEIPT_MAX_WEI     per-receipt mint cap         (default 0 = uncapped)
///   FIAT_WINDOW_CAP_WEI     fiat-mint rolling cap        (default 0 = OFF)
///   FIAT_WINDOW_SECS        fiat-mint window             (default 1 days)
///
/// Run with:
///   EVM_PRIVATE_KEY=0x... \
///   forge script script/ResetGenesis.s.sol --rpc-url tempo_mainnet --broadcast
contract ResetGenesis is Script {
    /// Everything a reset day needs to re-pin, in one bag (also keeps
    /// `run()`'s stack flat — 27 facet deploys blow the local budget).
    struct Genesis {
        address deployer;
        address diamond;
        address credits;
        address tbaRegistry;
        address accountImpl;
        address guardedCut;
    }

    function run() external returns (address diamondAddr) {
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");
        Genesis memory g;
        g.deployer = vm.addr(pk);

        vm.startBroadcast(pk);
        _deployDiamond(g); //   1-2. proxy + the three batched cuts + inits
        _wireTba(g); //         3.   ERC-6551 registry + MultiSignerAccount
        _wireCredits(g); //     4.   $LH token + ISSUER_ROLE + faucet knob
        _configurePlatform(g); // 5. pricing + meter + mint-gate keys
        g.guardedCut = address(new GuardedDiamondCutFacet()); // 6. child seed
        vm.stopBroadcast();

        _postFlight(g); //      7.   smoke asserts + the re-pin log
        diamondAddr = g.diamond;
    }

    // ── 1-2. Diamond + cuts ──────────────────────────────────────────────

    function _deployDiamond(Genesis memory g) internal {
        DiamondCutFacet cutFacet = new DiamondCutFacet();
        IDiamond.FacetCut[] memory initialCut = new IDiamond.FacetCut[](1);
        bytes4[] memory cutSel = new bytes4[](1);
        cutSel[0] = IDiamondCut.diamondCut.selector;
        initialCut[0] = IDiamond.FacetCut(address(cutFacet), IDiamond.FacetCutAction.Add, cutSel);
        g.diamond = address(new Diamond(g.deployer, initialCut));

        DiamondInit init = new DiamondInit();
        IDiamondCut(g.diamond).diamondCut(
            _coreCuts(), address(init), abi.encodeWithSelector(DiamondInit.init.selector)
        );
        IDiamondCut(g.diamond).diamondCut(
            _economyCuts(), address(init), abi.encodeWithSelector(DiamondInit.initErc721.selector)
        );
        IDiamondCut(g.diamond).diamondCut(_coordinationCuts(), address(0), "");
    }

    // ── 3. ERC-6551 wiring ───────────────────────────────────────────────

    function _wireTba(Genesis memory g) internal {
        g.tbaRegistry = address(new ERC6551Registry());
        g.accountImpl = address(new MultiSignerAccount());
        TbaFacet(g.diamond).setTbaConfig(g.tbaRegistry, g.accountImpl);
    }

    // ── 4. $LH token + roles ─────────────────────────────────────────────

    function _wireCredits(Genesis memory g) internal {
        LocalharnessCredits credits = new LocalharnessCredits(
            vm.envOr("SUPPLY_CAP_WEI", uint256(1_000_000_000 ether)), g.deployer
        );
        g.credits = address(credits);
        uint256 mintCap = vm.envOr("MINT_WINDOW_CAP_WEI", uint256(0));
        if (mintCap != 0) {
            credits.tightenMintWindow(mintCap, vm.envOr("MINT_WINDOW_SECS", uint256(1 days)));
        } else {
            console.log("WARNING: MINT_WINDOW_CAP_WEI=0 -> global mint cap DISABLED (set before mainnet)");
        }
        credits.grantRole(credits.ISSUER_ROLE(), g.diamond);
        CreditsFacet(g.diamond).setCreditsToken(g.credits);
        uint256 daily = vm.envOr("INITIAL_DAILY_ALLOWANCE", uint256(0));
        if (daily != 0) CreditsFacet(g.diamond).setDailyAllowance(daily);
    }

    // ── 5. Pricing + operational keys ────────────────────────────────────

    function _configurePlatform(Genesis memory g) internal {
        LocalharnessRegistryFacet(g.diamond).setRegistrationCost(
            vm.envOr("REGISTRATION_COST_WEI", uint256(1 ether))
        );
        uint256 mainCost = vm.envOr("MAIN_COST_WEI", uint256(0));
        if (mainCost != 0) MainIdentityFacet(g.diamond).setMainCost(mainCost);
        address meterAddr = vm.envOr("METER_ADDR", address(0));
        if (meterAddr != address(0)) CreditMeterFacet(g.diamond).setMeter(meterAddr);
        else console.log("NEXT: setMeter(<proxy meter key>)");
        _configureMintGate(g.diamond);
    }

    // ── 7. Post-flight smoke + re-pin log (static calls) ─────────────────

    function _postFlight(Genesis memory g) internal view {
        IDiamondLoupe.Facet[] memory facets = IDiamondLoupe(g.diamond).facets();
        require(facets.length == 27, "facet count != 27");
        uint256 selectorTotal;
        for (uint256 i; i < facets.length; i++) selectorTotal += facets[i].functionSelectors.length;
        require(selectorTotal == 215, "selector count != 215");
        require(OwnershipFacet(g.diamond).owner() == g.deployer, "owner mismatch");
        require(LocalharnessRegistryFacet(g.diamond).nextId() == 1, "init not run");
        require(CreditsFacet(g.diamond).creditsToken() == g.credits, "credits unwired");
        require(TbaFacet(g.diamond).tbaAccountImpl() == g.accountImpl, "tba unwired");
        require(X402Facet(g.diamond).x402DomainSeparator() != bytes32(0), "x402 dead");

        console.log("--- RESET GENESIS complete: 27 facets / 215 selectors ---");
        console.log("diamond:        ", g.diamond);
        console.log("owner:          ", g.deployer);
        console.log("lhToken:        ", g.credits);
        console.log("erc6551Registry:", g.tbaRegistry);
        console.log("accountImpl:    ", g.accountImpl);
        console.log("guardedCutFacet:", g.guardedCut);
        console.log("loupeFacet:     ", IDiamondLoupe(g.diamond).facetAddress(IDiamondLoupe.facets.selector));
        console.log("ownershipFacet: ", IDiamondLoupe(g.diamond).facetAddress(OwnershipFacet.owner.selector));
        console.log("Re-pin: chain.rs diamond/lh_token/guarded_cut/loupe/ownership; proxy env; wasm bundle.");
    }

    // ── Batch A: identity core (8 facets, 53 selectors) ──────────────────

    function _coreCuts() internal returns (IDiamond.FacetCut[] memory cuts) {
        cuts = new IDiamond.FacetCut[](8);
        cuts[0] = _add(address(new DiamondLoupeFacet()), _loupeSel());
        cuts[1] = _add(address(new OwnershipFacet()), _ownershipSel());
        cuts[2] = _add(address(new LocalharnessRegistryFacet()), _registrySel());
        cuts[3] = _add(address(new ERC721Facet()), _erc721Sel());
        cuts[4] = _add(address(new TbaFacet()), _tbaSel());
        cuts[5] = _add(address(new MainIdentityFacet()), _mainIdentitySel());
        cuts[6] = _add(address(new DeviceRegistryFacet()), _deviceRegistrySel());
        cuts[7] = _add(address(new ReleaseFacet()), _releaseSel());
    }

    /// 5 — introspection.
    function _loupeSel() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](5);
        s[0] = DiamondLoupeFacet.facets.selector;
        s[1] = DiamondLoupeFacet.facetFunctionSelectors.selector;
        s[2] = DiamondLoupeFacet.facetAddresses.selector;
        s[3] = DiamondLoupeFacet.facetAddress.selector;
        s[4] = DiamondLoupeFacet.supportsInterface.selector;
    }

    /// 2 — EIP-173.
    function _ownershipSel() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](2);
        s[0] = OwnershipFacet.transferOwnership.selector;
        s[1] = OwnershipFacet.owner.selector;
    }

    /// 14 — names + metadata + cost + treasury (base 10 + SwapRegistryFacetAddCost
    /// + SwapTreasuryAndMainCost; the AddRegistryNameValidation register() body).
    function _registrySel() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](14);
        s[0] = LocalharnessRegistryFacet.register.selector;
        s[1] = LocalharnessRegistryFacet.setMetadata.selector;
        s[2] = LocalharnessRegistryFacet.isTaken.selector;
        s[3] = LocalharnessRegistryFacet.ownerOfName.selector;
        s[4] = LocalharnessRegistryFacet.ownerOfId.selector;
        s[5] = LocalharnessRegistryFacet.idOfName.selector;
        s[6] = LocalharnessRegistryFacet.nameOfId.selector;
        s[7] = LocalharnessRegistryFacet.idOf.selector;
        s[8] = LocalharnessRegistryFacet.nextId.selector;
        s[9] = LocalharnessRegistryFacet.metadata.selector;
        s[10] = LocalharnessRegistryFacet.setRegistrationCost.selector;
        s[11] = LocalharnessRegistryFacet.registrationCost.selector;
        s[12] = LocalharnessRegistryFacet.treasuryBalance.selector;
        s[13] = LocalharnessRegistryFacet.withdrawTreasury.selector;
    }

    /// 12 — full ERC-721 + Metadata (overloads need explicit hashes).
    function _erc721Sel() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](12);
        s[0] = ERC721Facet.balanceOf.selector;
        s[1] = ERC721Facet.ownerOf.selector;
        s[2] = ERC721Facet.approve.selector;
        s[3] = ERC721Facet.getApproved.selector;
        s[4] = ERC721Facet.setApprovalForAll.selector;
        s[5] = ERC721Facet.isApprovedForAll.selector;
        s[6] = ERC721Facet.transferFrom.selector;
        s[7] = bytes4(keccak256("safeTransferFrom(address,address,uint256)"));
        s[8] = bytes4(keccak256("safeTransferFrom(address,address,uint256,bytes)"));
        s[9] = ERC721Facet.name.selector;
        s[10] = ERC721Facet.symbol.selector;
        s[11] = ERC721Facet.tokenURI.selector;
    }

    /// 6 — EIP-6551 helpers.
    function _tbaSel() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](6);
        s[0] = TbaFacet.setTbaConfig.selector;
        s[1] = TbaFacet.tbaRegistry.selector;
        s[2] = TbaFacet.tbaAccountImpl.selector;
        s[3] = TbaFacet.tokenBoundAccount.selector;
        s[4] = TbaFacet.tokenBoundAccountByName.selector;
        s[5] = TbaFacet.createTokenBoundAccount.selector;
    }

    /// 7 — MAIN identity (base 5 + SwapTreasuryAndMainCost's setMainCost/mainCost).
    function _mainIdentitySel() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](7);
        s[0] = MainIdentityFacet.registerMain.selector;
        s[1] = MainIdentityFacet.clearMain.selector;
        s[2] = MainIdentityFacet.mainOf.selector;
        s[3] = MainIdentityFacet.mainNameOf.selector;
        s[4] = MainIdentityFacet.isMain.selector;
        s[5] = MainIdentityFacet.setMainCost.selector;
        s[6] = MainIdentityFacet.mainCost.selector;
    }

    /// 4 — enumerable device links.
    function _deviceRegistrySel() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](4);
        s[0] = DeviceRegistryFacet.linkDevice.selector;
        s[1] = DeviceRegistryFacet.unlinkDevice.selector;
        s[2] = DeviceRegistryFacet.devicesOf.selector;
        s[3] = DeviceRegistryFacet.isDeviceLinked.selector;
    }

    /// 3 — holder burn (base) + AddAdminReset's admin pair.
    function _releaseSel() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](3);
        s[0] = ReleaseFacet.releaseName.selector;
        s[1] = ReleaseFacet.adminBurnNames.selector;
        s[2] = ReleaseFacet.adminResetAll.selector;
    }

    // ── Batch B: economy (7 facets, 49 selectors) ────────────────────────

    function _economyCuts() internal returns (IDiamond.FacetCut[] memory cuts) {
        cuts = new IDiamond.FacetCut[](7);
        cuts[0] = _add(address(new CreditsFacet()), _creditsSel());
        cuts[1] = _add(address(new RedeemFacet()), _redeemSel());
        cuts[2] = _add(address(new InviteFacet()), _inviteSel());
        cuts[3] = _add(address(new CreditMeterFacet()), _creditMeterSel());
        cuts[4] = _add(address(new MintGateFacet()), _mintGateSel());
        cuts[5] = _add(address(new X402Facet()), _x402Sel());
        cuts[6] = _add(address(new TitheFacet()), _titheSel());
    }

    /// 7 — $LH distribution (daily faucet stays 0/disabled).
    function _creditsSel() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](7);
        s[0] = CreditsFacet.setCreditsToken.selector;
        s[1] = CreditsFacet.setDailyAllowance.selector;
        s[2] = CreditsFacet.claimDaily.selector;
        s[3] = CreditsFacet.creditsToken.selector;
        s[4] = CreditsFacet.dailyAllowance.selector;
        s[5] = CreditsFacet.lastClaimDay.selector;
        s[6] = CreditsFacet.canClaim.selector;
    }

    /// 5 — redeem codes.
    function _redeemSel() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](5);
        s[0] = RedeemFacet.redeem.selector;
        s[1] = RedeemFacet.addRedeemCodes.selector;
        s[2] = RedeemFacet.disableRedeemCodes.selector;
        s[3] = RedeemFacet.redeemAmountOf.selector;
        s[4] = RedeemFacet.isRedeemed.selector;
    }

    /// 5 — escrowed bearer invites.
    function _inviteSel() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](5);
        s[0] = InviteFacet.createInvite.selector;
        s[1] = InviteFacet.acceptInvite.selector;
        s[2] = InviteFacet.reclaimInvite.selector;
        s[3] = InviteFacet.getInvite.selector;
        s[4] = InviteFacet.escrowedOf.selector;
    }

    /// 7 — per-message meter (base 5 + AddMintGateFacet's withdrawableOf;
    /// UpgradeCreditMeterFacet bodies are the source).
    function _creditMeterSel() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](7);
        s[0] = CreditMeterFacet.depositCredits.selector;
        s[1] = CreditMeterFacet.meter.selector;
        s[2] = CreditMeterFacet.setMeter.selector;
        s[3] = CreditMeterFacet.creditOf.selector;
        s[4] = CreditMeterFacet.meterAddress.selector;
        s[5] = CreditMeterFacet.withdrawCredits.selector;
        s[6] = CreditMeterFacet.withdrawableOf.selector;
    }

    /// 17 — fiat on-ramp valve (fiatLocked machinery included: the lock plumbing
    /// is internal to mintFromFiat/withdraw; separating it needs source edits).
    function _mintGateSel() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](17);
        s[0] = MintGateFacet.mintFromFiat.selector;
        s[1] = MintGateFacet.clawbackFiatMint.selector;
        s[2] = MintGateFacet.setFiatIssuerSigner.selector;
        s[3] = MintGateFacet.setClawbacker.selector;
        s[4] = MintGateFacet.setPerReceiptMaxWei.selector;
        s[5] = MintGateFacet.setFiatLockSecs.selector;
        s[6] = MintGateFacet.setFiatMintWindow.selector;
        s[7] = MintGateFacet.fiatIssuerSigner.selector;
        s[8] = MintGateFacet.clawbacker.selector;
        s[9] = MintGateFacet.perReceiptMaxWei.selector;
        s[10] = MintGateFacet.fiatLockSecs.selector;
        s[11] = MintGateFacet.fiatLockedOf.selector;
        s[12] = MintGateFacet.receiptUsed.selector;
        s[13] = MintGateFacet.receiptInfo.selector;
        s[14] = MintGateFacet.fiatMintWindow.selector;
        s[15] = MintGateFacet.circulatingSupply.selector;
        s[16] = MintGateFacet.fiatMintDomainSeparator.selector;
    }

    /// 4 — x402 exact settle (settleUpto is source-ahead of the old chain: NEW).
    function _x402Sel() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](4);
        s[0] = X402Facet.settle.selector;
        s[1] = X402Facet.settleUpto.selector;
        s[2] = X402Facet.authorizationState.selector;
        s[3] = X402Facet.x402DomainSeparator.selector;
    }

    /// 4 — voluntary revenue share.
    function _titheSel() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](4);
        s[0] = TitheFacet.setTithe.selector;
        s[1] = TitheFacet.revokeTithe.selector;
        s[2] = TitheFacet.collectTithe.selector;
        s[3] = TitheFacet.titheOf.selector;
    }

    // ── Batch C: coordination + comms (11 facets, 112 selectors) ─────────

    function _coordinationCuts() internal returns (IDiamond.FacetCut[] memory cuts) {
        cuts = new IDiamond.FacetCut[](11);
        cuts[0] = _add(address(new BountyFacet()), _bountySel());
        cuts[1] = _add(address(new PartyFacet()), _partySel());
        cuts[2] = _add(address(new GuildFacet()), _guildSel());
        cuts[3] = _add(address(new VotingFacet()), _votingSel());
        cuts[4] = _add(address(new WeightedVotingFacet()), _weightedVotingSel());
        cuts[5] = _add(address(new ReputationFacet()), _reputationSel());
        cuts[6] = _add(address(new ValidationFacet()), _validationSel());
        cuts[7] = _add(address(new SessionRoomFacet()), _sessionRoomSel());
        cuts[8] = _add(address(new SignalingFacet()), _signalingSel());
        cuts[9] = _add(address(new SubscribeFacet()), _subscribeSel());
        cuts[10] = _add(address(new MessageFacet()), _messageSel());
    }

    /// 13 — escrowed bounties (rung 1).
    function _bountySel() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](13);
        s[0] = BountyFacet.postBounty.selector;
        s[1] = BountyFacet.claimBounty.selector;
        s[2] = BountyFacet.submitResult.selector;
        s[3] = BountyFacet.acceptResult.selector;
        s[4] = BountyFacet.cancelBounty.selector;
        s[5] = BountyFacet.reclaimExpired.selector;
        s[6] = BountyFacet.getBounty.selector;
        s[7] = BountyFacet.bountyTaskOf.selector;
        s[8] = BountyFacet.resultOf.selector;
        s[9] = BountyFacet.openBounties.selector;
        s[10] = BountyFacet.bountiesOf.selector;
        s[11] = BountyFacet.bountyCount.selector;
        s[12] = BountyFacet.activeBountyCountOf.selector;
    }

    /// 15 — consent-gated bps-split squads (rung 2).
    function _partySel() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](15);
        s[0] = PartyFacet.formParty.selector;
        s[1] = PartyFacet.joinParty.selector;
        s[2] = PartyFacet.fundParty.selector;
        s[3] = PartyFacet.completeParty.selector;
        s[4] = PartyFacet.disbandParty.selector;
        s[5] = PartyFacet.getParty.selector;
        s[6] = PartyFacet.partyMembersOf.selector;
        s[7] = PartyFacet.partySharesOf.selector;
        s[8] = PartyFacet.partyConsentOf.selector;
        s[9] = PartyFacet.partyFundersOf.selector;
        s[10] = PartyFacet.partyContributionOf.selector;
        s[11] = PartyFacet.partiesOf.selector;
        s[12] = PartyFacet.partyCount.selector;
        s[13] = PartyFacet.activePartyCountOf.selector;
        s[14] = PartyFacet.liveParties.selector;
    }

    /// 16 — guilds (rung 3).
    function _guildSel() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](16);
        s[0] = GuildFacet.createGuild.selector;
        s[1] = GuildFacet.inviteToGuild.selector;
        s[2] = GuildFacet.acceptGuildInvite.selector;
        s[3] = GuildFacet.leaveGuild.selector;
        s[4] = GuildFacet.setRole.selector;
        s[5] = GuildFacet.fundGuild.selector;
        s[6] = GuildFacet.spendTreasury.selector;
        s[7] = GuildFacet.guildMembersOf.selector;
        s[8] = GuildFacet.roleOf.selector;
        s[9] = GuildFacet.isGuildMember.selector;
        s[10] = GuildFacet.treasuryBalanceOf.selector;
        s[11] = GuildFacet.guildAddress.selector;
        s[12] = GuildFacet.guildName.selector;
        s[13] = GuildFacet.guildsOf.selector;
        s[14] = GuildFacet.isGuild.selector;
        s[15] = GuildFacet.guildCount.selector;
    }

    /// 9 — VotingFacet's OWN selectors only (rung 4; it inherits GuildFacet —
    /// the guild selectors stay routed to the GuildFacet cut above).
    function _votingSel() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](9);
        s[0] = VotingFacet.propose.selector;
        s[1] = VotingFacet.vote.selector;
        s[2] = VotingFacet.execute.selector;
        s[3] = VotingFacet.getProposal.selector;
        s[4] = VotingFacet.proposalMemoOf.selector;
        s[5] = VotingFacet.proposalsOf.selector;
        s[6] = VotingFacet.hasVoted.selector;
        s[7] = VotingFacet.tallyOf.selector;
        s[8] = VotingFacet.proposalCount.selector;
    }

    /// 12 — WeightedVotingFacet's OWN selectors only (same inheritance rule).
    function _weightedVotingSel() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](12);
        s[0] = WeightedVotingFacet.setShares.selector;
        s[1] = WeightedVotingFacet.sharesOf.selector;
        s[2] = WeightedVotingFacet.totalSharesOf.selector;
        s[3] = WeightedVotingFacet.proposeWeighted.selector;
        s[4] = WeightedVotingFacet.voteWeighted.selector;
        s[5] = WeightedVotingFacet.executeWeighted.selector;
        s[6] = WeightedVotingFacet.weightedProposal.selector;
        s[7] = WeightedVotingFacet.weightedProposalCount.selector;
        s[8] = WeightedVotingFacet.weightedProposalMemoOf.selector;
        s[9] = WeightedVotingFacet.weightedProposalsOf.selector;
        s[10] = WeightedVotingFacet.hasVotedWeighted.selector;
        s[11] = WeightedVotingFacet.weightedTallyOf.selector;
    }

    /// 4 — attestation trust.
    function _reputationSel() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](4);
        s[0] = ReputationFacet.attest.selector;
        s[1] = ReputationFacet.reputationOf.selector;
        s[2] = ReputationFacet.attestationsOf.selector;
        s[3] = ReputationFacet.hasAttested.selector;
    }

    /// 13 — stake/challenge/resolve escrow.
    function _validationSel() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](13);
        s[0] = ValidationFacet.stakeValidation.selector;
        s[1] = ValidationFacet.challengeValidation.selector;
        s[2] = ValidationFacet.resolveValidation.selector;
        s[3] = ValidationFacet.reclaimStake.selector;
        s[4] = ValidationFacet.reclaimUnresolved.selector;
        s[5] = ValidationFacet.getValidation.selector;
        s[6] = ValidationFacet.validationResolverOf.selector;
        s[7] = ValidationFacet.hasValidated.selector;
        s[8] = ValidationFacet.validationsOfWork.selector;
        s[9] = ValidationFacet.validationsOf.selector;
        s[10] = ValidationFacet.validationCount.selector;
        s[11] = ValidationFacet.validationStakedOf.selector;
        s[12] = ValidationFacet.activeValidationCountOf.selector;
    }

    /// 11 — encrypted shared KV rooms (#22).
    function _sessionRoomSel() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](11);
        s[0] = SessionRoomFacet.createRoom.selector;
        s[1] = SessionRoomFacet.roomAddMember.selector;
        s[2] = SessionRoomFacet.roomRemoveMember.selector;
        s[3] = SessionRoomFacet.appendOp.selector;
        s[4] = SessionRoomFacet.clearRoom.selector;
        s[5] = SessionRoomFacet.opsOf.selector;
        s[6] = SessionRoomFacet.opCount.selector;
        s[7] = SessionRoomFacet.roomEpoch.selector;
        s[8] = SessionRoomFacet.roomCreator.selector;
        s[9] = SessionRoomFacet.roomIsMember.selector;
        s[10] = SessionRoomFacet.roomMembersOf.selector;
    }

    /// 7 — owner-signed WebRTC signaling/presence.
    function _signalingSel() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](7);
        s[0] = SignalingFacet.announce.selector;
        s[1] = SignalingFacet.leave.selector;
        s[2] = SignalingFacet.peersOf.selector;
        s[3] = SignalingFacet.postSignal.selector;
        s[4] = SignalingFacet.inboxOf.selector;
        s[5] = SignalingFacet.inboxLength.selector;
        s[6] = SignalingFacet.clearInbox.selector;
    }

    /// 5 — cartridge-feed subscriptions.
    function _subscribeSel() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](5);
        s[0] = SubscribeFacet.subscribe.selector;
        s[1] = SubscribeFacet.unsubscribe.selector;
        s[2] = SubscribeFacet.isSubscribed.selector;
        s[3] = SubscribeFacet.subscriberCount.selector;
        s[4] = SubscribeFacet.subscribersOf.selector;
    }

    /// 7 — agent-to-agent inbox.
    function _messageSel() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](7);
        s[0] = MessageFacet.sendMessage.selector;
        s[1] = MessageFacet.inboxCount.selector;
        s[2] = MessageFacet.inboxRange.selector;
        s[3] = MessageFacet.messageAt.selector;
        s[4] = MessageFacet.inboxLastRead.selector;
        s[5] = MessageFacet.markRead.selector;
        s[6] = MessageFacet.unreadCount.selector;
    }

    // ── plumbing ─────────────────────────────────────────────────────────

    function _add(address facet, bytes4[] memory sel) internal pure returns (IDiamond.FacetCut memory) {
        return IDiamond.FacetCut(facet, IDiamond.FacetCutAction.Add, sel);
    }

    function _configureMintGate(address diamondAddr) internal {
        MintGateFacet g = MintGateFacet(diamondAddr);
        address fiatSigner = vm.envOr("FIAT_ISSUER_SIGNER", address(0));
        address clawbacker = vm.envOr("CLAWBACKER", address(0));
        if (fiatSigner != address(0)) g.setFiatIssuerSigner(fiatSigner);
        else console.log("NEXT: setFiatIssuerSigner(<proxy webhook signer>)");
        if (clawbacker != address(0)) g.setClawbacker(clawbacker);
        uint256 lockSecs = vm.envOr("FIAT_LOCK_SECS", uint256(0));
        if (lockSecs != 0) g.setFiatLockSecs(lockSecs);
        uint256 perReceipt = vm.envOr("PER_RECEIPT_MAX_WEI", uint256(0));
        if (perReceipt != 0) g.setPerReceiptMaxWei(perReceipt);
        uint256 windowCap = vm.envOr("FIAT_WINDOW_CAP_WEI", uint256(0));
        if (windowCap != 0) g.setFiatMintWindow(windowCap, vm.envOr("FIAT_WINDOW_SECS", uint256(1 days)));
    }
}
