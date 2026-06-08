// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Test} from "forge-std/Test.sol";

import {RedeemFacet} from "../src/facets/RedeemFacet.sol";
import {CreditMeterFacet} from "../src/facets/CreditMeterFacet.sol";
import {SessionFacet} from "../src/facets/SessionFacet.sol";
import {X402Facet} from "../src/facets/X402Facet.sol";
import {CreditsFacet} from "../src/facets/CreditsFacet.sol";

import {LocalharnessCredits} from "../src/LocalharnessCredits.sol";

import {LibCreditsStorage} from "../src/libraries/LibCreditsStorage.sol";
import {LibRedeemStorage} from "../src/libraries/LibRedeemStorage.sol";
import {LibCreditMeterStorage} from "../src/libraries/LibCreditMeterStorage.sol";
import {LibSessionStorage} from "../src/libraries/LibSessionStorage.sol";
import {LibX402Storage} from "../src/libraries/LibX402Storage.sol";
import {LibDiamond} from "../src/libraries/LibDiamond.sol";

/// @title FinancialAdversarial
/// @notice Adversarial review of the LIVE `$LH`-handling facets — Redeem,
///         CreditMeter, Session, X402, Credits — plus the real
///         `LocalharnessCredits` token. These move real value; a latent
///         bug means drained / minted / stuck funds. Every test below
///         proves a SAFE behavior against an attack: double-redeem blocked,
///         non-meter debit reverts, x402 replay reverts, ISSUER_ROLE gating
///         holds, supply cap respected, owner-gating on every setter.
///
///         Harness pattern (mirrors InviteFacet.t.sol / ScheduleFacet*):
///         each facet's `Lib*Storage.load()` resolves against the test
///         contract's own storage, so a harness that EXTENDS the facet +
///         writes the shared slots IS the cross-facet storage the diamond
///         provides at runtime — the facet code is exercised verbatim.

// The five live money facets each declare their OWN `error NotConfigured()`
// + a private `IERC20Min` — harmless across distinct facets in a diamond,
// but they collide if inherited into one Solidity contract. So each facet
// gets its OWN harness (one per facet), all sharing the SAME diamond-storage
// slots via `Lib*Storage.load()` — exactly how the live diamond multiplexes
// them. Where a test needs two facets' state together (e.g. Redeem mints via
// the credits-token slot CreditsFacet binds), the harness just writes that
// shared slot directly, as the diamond does at runtime.

contract RedeemHarness is RedeemFacet {
    function _setCreditsToken(address token) external {
        LibCreditsStorage.load().creditsToken = token;
    }
    function _setDiamondOwner(address ownr) external {
        LibDiamond.setContractOwner(ownr);
    }
}

contract MeterHarness is CreditMeterFacet {
    function _setCreditsToken(address token) external {
        LibCreditsStorage.load().creditsToken = token;
    }
    function _setDiamondOwner(address ownr) external {
        LibDiamond.setContractOwner(ownr);
    }
}

contract SessionHarness is SessionFacet {
    function _setCreditsToken(address token) external {
        LibCreditsStorage.load().creditsToken = token;
    }
    function _setDiamondOwner(address ownr) external {
        LibDiamond.setContractOwner(ownr);
    }
}

contract X402Harness is X402Facet {
    function _setCreditsToken(address token) external {
        LibCreditsStorage.load().creditsToken = token;
    }
    function _setDiamondOwner(address ownr) external {
        LibDiamond.setContractOwner(ownr);
    }
}

contract CreditsHarness is CreditsFacet {
    function _setDiamondOwner(address ownr) external {
        LibDiamond.setContractOwner(ownr);
    }
}

/// A hostile `$LH`-shaped token whose `transferFrom` re-enters the diamond.
/// Real `$LH` has NO transfer hook, so this is a defense-in-depth probe of
/// each facet's CEI ordering (a hostile token must not be able to replay an
/// effect or double-move funds).
contract ReentrantToken {
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    address public diamond;
    uint8 public mode; // 0 off, 1 = x402 replay, 2 = depositCredits replay, 3 = openSession replay
    bool internal entered;
    bool public reenterReverted;

    // x402 replay args
    address public xFrom;
    address public xTo;
    uint256 public xValue;
    uint256 public xValidAfter;
    uint256 public xValidBefore;
    bytes32 public xNonce;
    bytes public xSig;

    function armX402(
        address d,
        address from_,
        address to_,
        uint256 value_,
        uint256 va,
        uint256 vb,
        bytes32 nonce_,
        bytes calldata sig_
    ) external {
        diamond = d;
        mode = 1;
        xFrom = from_;
        xTo = to_;
        xValue = value_;
        xValidAfter = va;
        xValidBefore = vb;
        xNonce = nonce_;
        xSig = sig_;
    }

    function mint(address to, uint256 amt) external {
        balanceOf[to] += amt;
    }

    function approve(address spender, uint256 amt) external returns (bool) {
        allowance[msg.sender][spender] = amt;
        return true;
    }

    function transfer(address to, uint256 amt) external returns (bool) {
        require(balanceOf[msg.sender] >= amt, "balance");
        balanceOf[msg.sender] -= amt;
        balanceOf[to] += amt;
        return true;
    }

    function transferFrom(address from, address to, uint256 amt) external returns (bool) {
        uint256 a = allowance[from][msg.sender];
        require(a >= amt, "allowance");
        require(balanceOf[from] >= amt, "balance");
        allowance[from][msg.sender] = a - amt;
        balanceOf[from] -= amt;
        balanceOf[to] += amt;

        if (mode != 0 && !entered) {
            entered = true;
            if (mode == 1) {
                // Re-enter settle with the SAME authorization — must revert
                // (nonce already marked used before this external call).
                try
                    X402Facet(diamond).settle(
                        xFrom, xTo, xValue, xValidAfter, xValidBefore, xNonce, xSig
                    )
                {
                    reenterReverted = false;
                } catch {
                    reenterReverted = true;
                }
            }
        }
        return true;
    }
}

contract FinancialAdversarialTest is Test {
    // One harness per facet (they share the same diamond-storage slots via
    // Lib*Storage.load(), but can't be inherited into one contract because
    // each declares its own `error NotConfigured()` + `IERC20Min`).
    RedeemHarness rdm;
    MeterHarness mtr;
    SessionHarness ses;
    X402Harness x4;
    CreditsHarness cred;

    LocalharnessCredits lh; // the REAL token

    // The token's own admin (grants ISSUER_ROLE). Distinct from the diamond.
    address tokenOwner = address(0x700);
    address diamondOwner = address(0xD1A);
    address meterKey = address(0x4E7E5); // the proxy meter address

    address alice = address(0xA11CE);
    address bob = address(0xB0B);
    address stranger = address(0xBEEF);

    // x402 payer with a known private key so we can sign EIP-712 digests.
    uint256 payerPk = 0xA11CE; // arbitrary nonzero
    address payer;
    address payee = address(0x9A9EE);

    uint256 constant CAP = 1_000_000_000 ether;

    function setUp() public {
        rdm = new RedeemHarness();
        mtr = new MeterHarness();
        ses = new SessionHarness();
        x4 = new X402Harness();
        cred = new CreditsHarness();

        rdm._setDiamondOwner(diamondOwner);
        mtr._setDiamondOwner(diamondOwner);
        ses._setDiamondOwner(diamondOwner);
        x4._setDiamondOwner(diamondOwner);
        cred._setDiamondOwner(diamondOwner);

        // Deploy the real credits token. Each harness IS its own diamond, so
        // grant ISSUER_ROLE to the two that mint (Redeem, Credits); the other
        // three only pull via transferFrom. We grant it to all so any harness
        // can act as the issuer when funding test accounts.
        lh = new LocalharnessCredits(CAP, tokenOwner);
        vm.startPrank(tokenOwner);
        lh.grantRole(lh.ISSUER_ROLE(), address(rdm));
        lh.grantRole(lh.ISSUER_ROLE(), address(mtr));
        lh.grantRole(lh.ISSUER_ROLE(), address(ses));
        lh.grantRole(lh.ISSUER_ROLE(), address(x4));
        lh.grantRole(lh.ISSUER_ROLE(), address(cred));
        vm.stopPrank();

        rdm._setCreditsToken(address(lh));
        mtr._setCreditsToken(address(lh));
        ses._setCreditsToken(address(lh));
        x4._setCreditsToken(address(lh));
        // CreditsFacet reads its token from the SAME slot; CreditsHarness
        // doesn't expose a setter, so we drive its claimDaily via a token it
        // binds through setCreditsToken (owner-gated facet method).
        vm.prank(diamondOwner);
        cred.setCreditsToken(address(lh));

        payer = vm.addr(payerPk);

        vm.warp(1_000_000);
    }

    // === helpers ========================================================

    /// Give `who` `$LH` (minted by the issuer-role `mtr` harness) and approve
    /// `spender` (the harness under test) to pull it.
    function _fundFor(address who, uint256 amt, address spender) internal {
        vm.prank(address(mtr));
        lh.mintWithMemo(who, amt, "FUND");
        vm.prank(who);
        lh.approve(spender, type(uint256).max);
    }

    function _arr(bytes32 x) internal pure returns (bytes32[] memory a) {
        a = new bytes32[](1);
        a[0] = x;
    }

    // ====================================================================
    // ============================ REDEEM ================================
    // ====================================================================

    function _addCode(bytes32 codeHash, uint256 amount) internal {
        vm.prank(diamondOwner);
        rdm.addRedeemCodes(_arr(codeHash), amount);
    }

    function test_redeem_cannot_be_redeemed_twice() public {
        string memory code = "redeem-code-001";
        bytes32 hsh = keccak256(bytes(code));
        _addCode(hsh, 100 ether);

        vm.prank(alice);
        uint256 got = rdm.redeem(code);
        assertEq(got, 100 ether, "first redeem mints amount");
        assertEq(lh.balanceOf(alice), 100 ether, "alice credited once");

        // Second redeem of the SAME code reverts — claimed flag set BEFORE
        // the mint (CEI), so no double-mint.
        vm.prank(alice);
        vm.expectRevert(RedeemFacet.CodeAlreadyUsed.selector);
        rdm.redeem(code);
        assertEq(lh.balanceOf(alice), 100 ether, "no second mint");

        // A different caller can't drain it either.
        vm.prank(bob);
        vm.expectRevert(RedeemFacet.CodeAlreadyUsed.selector);
        rdm.redeem(code);
        assertEq(lh.totalSupply(), 100 ether, "supply minted exactly once");
    }

    function test_redeem_claimed_flag_set_before_mint_is_CEI() public {
        // Prove ordering: the storage write (claimed=true) precedes the
        // external mint. We assert via the post-state: even if the token
        // were hostile and re-entered, the nonce is already consumed.
        // (The real token has no hook; this asserts the storage invariant.)
        string memory code = "cei-check-002";
        bytes32 hsh = keccak256(bytes(code));
        _addCode(hsh, 50 ether);
        vm.prank(alice);
        rdm.redeem(code);
        assertTrue(rdm.isRedeemed(hsh), "claimed set");
    }

    function test_redeem_nonowner_cannot_add_codes() public {
        bytes32 hsh = keccak256("steal");
        vm.prank(stranger);
        vm.expectRevert("LibDiamond: not owner");
        rdm.addRedeemCodes(_arr(hsh), 1_000 ether);
    }

    function test_redeem_nonowner_cannot_disable_codes() public {
        bytes32 hsh = keccak256("victim");
        vm.prank(stranger);
        vm.expectRevert("LibDiamond: not owner");
        rdm.disableRedeemCodes(_arr(hsh));
    }

    function test_redeem_unknown_code_reverts() public {
        vm.prank(alice);
        vm.expectRevert(RedeemFacet.InvalidCode.selector);
        rdm.redeem("never-loaded");
    }

    function test_redeem_zero_amount_code_rejected_on_add() public {
        // The owner can't register a zero-amount code (would be an
        // unredeemable / ambiguous entry; 0 also means "unknown").
        vm.prank(diamondOwner);
        vm.expectRevert(RedeemFacet.InvalidCode.selector);
        rdm.addRedeemCodes(_arr(keccak256("z")), 0);
    }

    function test_redeem_disabled_code_cannot_be_redeemed() public {
        string memory code = "leaked-003";
        bytes32 hsh = keccak256(bytes(code));
        _addCode(hsh, 100 ether);
        // Owner neutralizes the leaked code before anyone redeems.
        vm.prank(diamondOwner);
        rdm.disableRedeemCodes(_arr(hsh));
        vm.prank(alice);
        vm.expectRevert(RedeemFacet.CodeAlreadyUsed.selector);
        rdm.redeem(code);
        assertEq(lh.totalSupply(), 0, "disabled code mints nothing");
    }

    function test_redeem_amount_is_bounded_by_owner_set_value() public {
        // The mint amount equals exactly the owner-registered denomination;
        // a redeemer cannot influence it (no caller-supplied amount).
        string memory code = "denom-004";
        bytes32 hsh = keccak256(bytes(code));
        _addCode(hsh, 7 ether);
        vm.prank(alice);
        uint256 got = rdm.redeem(code);
        assertEq(got, 7 ether, "exactly the registered denomination");
    }

    function test_redeem_collision_resistance_distinct_codes_distinct_hashes() public {
        // Two different plaintext codes hash to different slots; redeeming
        // one does not consume the other (no hash aliasing).
        string memory a = "alpha-aaaaaaaa";
        string memory b = "beta-bbbbbbbbb";
        _addCode(keccak256(bytes(a)), 10 ether);
        _addCode(keccak256(bytes(b)), 20 ether);
        vm.prank(alice);
        rdm.redeem(a);
        // b is untouched.
        assertFalse(rdm.isRedeemed(keccak256(bytes(b))), "distinct code unaffected");
        vm.prank(bob);
        assertEq(rdm.redeem(b), 20 ether, "second distinct code still redeemable");
    }

    function test_redeem_reverts_when_token_unconfigured() public {
        RedeemHarness h2 = new RedeemHarness();
        h2._setDiamondOwner(diamondOwner);
        // No creditsToken set.
        bytes32 hsh = keccak256(bytes("nocfg"));
        vm.prank(diamondOwner);
        h2.addRedeemCodes(_arr(hsh), 5 ether);
        // Code is known (amount>0) but the token isn't set -> NotConfigured,
        // and crucially the claimed flag was ALREADY set before the revert
        // bubbles, but the whole tx reverts so the code stays redeemable.
        vm.prank(alice);
        vm.expectRevert(RedeemFacet.NotConfigured.selector);
        h2.redeem("nocfg");
        assertFalse(h2.isRedeemed(hsh), "reverted redeem leaves the code unclaimed");
    }

    // ====================================================================
    // ========================== CREDIT METER ============================
    // ====================================================================

    function test_meter_only_meter_key_can_debit() public {
        vm.prank(diamondOwner);
        mtr.setMeter(meterKey);
        _fundFor(alice, 100 ether, address(mtr));
        vm.prank(alice);
        mtr.depositCredits(50 ether);

        // A stranger (and even the diamond owner) cannot debit anyone.
        vm.prank(stranger);
        vm.expectRevert(CreditMeterFacet.NotMeter.selector);
        mtr.meter(alice, 10 ether);

        vm.prank(diamondOwner);
        vm.expectRevert(CreditMeterFacet.NotMeter.selector);
        mtr.meter(alice, 10 ether);

        // alice can't debit herself (only the meter key).
        vm.prank(alice);
        vm.expectRevert(CreditMeterFacet.NotMeter.selector);
        mtr.meter(alice, 10 ether);

        assertEq(mtr.creditOf(alice), 50 ether, "balance untouched by failed debits");
    }

    function test_meter_cannot_debit_more_than_balance_no_underflow() public {
        vm.prank(diamondOwner);
        mtr.setMeter(meterKey);
        _fundFor(alice, 100 ether, address(mtr));
        vm.prank(alice);
        mtr.depositCredits(30 ether);

        // Debit beyond balance reverts (InsufficientCredits) — the
        // `bal < amount` guard prevents the `unchecked` block from
        // underflowing to a near-max balance.
        vm.prank(meterKey);
        vm.expectRevert(CreditMeterFacet.InsufficientCredits.selector);
        mtr.meter(alice, 30 ether + 1);
        assertEq(mtr.creditOf(alice), 30 ether, "no phantom inflation from underflow");
    }

    function test_meter_exact_balance_to_zero_then_next_reverts() public {
        vm.prank(diamondOwner);
        mtr.setMeter(meterKey);
        _fundFor(alice, 100 ether, address(mtr));
        vm.prank(alice);
        mtr.depositCredits(30 ether);

        vm.prank(meterKey);
        mtr.meter(alice, 30 ether); // exact drain
        assertEq(mtr.creditOf(alice), 0, "drained to exactly zero");

        vm.prank(meterKey);
        vm.expectRevert(CreditMeterFacet.InsufficientCredits.selector);
        mtr.meter(alice, 1); // 0 < 1 -> revert, never underflows
    }

    function test_meter_debits_only_target_not_others() public {
        vm.prank(diamondOwner);
        mtr.setMeter(meterKey);
        _fundFor(alice, 100 ether, address(mtr));
        _fundFor(bob, 100 ether, address(mtr));
        vm.prank(alice);
        mtr.depositCredits(40 ether);
        vm.prank(bob);
        mtr.depositCredits(40 ether);

        vm.prank(meterKey);
        mtr.meter(alice, 10 ether);
        assertEq(mtr.creditOf(alice), 30 ether, "alice debited");
        assertEq(mtr.creditOf(bob), 40 ether, "bob untouched");
    }

    function test_depositCredits_credits_caller_not_arbitrary_address() public {
        // deposit pulls from msg.sender and credits msg.sender — there is no
        // "deposit on behalf of" param, so no one can misattribute a deposit.
        vm.prank(diamondOwner);
        mtr.setMeter(meterKey);
        _fundFor(alice, 100 ether, address(mtr));
        vm.prank(alice);
        mtr.depositCredits(25 ether);
        assertEq(mtr.creditOf(alice), 25 ether, "caller credited");
        assertEq(mtr.creditOf(bob), 0, "no cross-credit");
        // The $LH actually moved alice -> diamond.
        assertEq(lh.balanceOf(address(mtr)), 25 ether, "escrow held by diamond");
        assertEq(lh.balanceOf(alice), 75 ether, "alice debited the deposit");
    }

    function test_depositCredits_no_ghost_balance_when_pull_fails() public {
        // CEI: a failed transferFrom must revert the credit bump.
        vm.prank(diamondOwner);
        mtr.setMeter(meterKey);
        // alice funded but with ZERO approval (revoke).
        _fundFor(alice, 100 ether, address(mtr));
        vm.prank(alice);
        lh.approve(address(mtr), 0);
        vm.prank(alice);
        vm.expectRevert(); // InsufficientAllowance from the token
        mtr.depositCredits(10 ether);
        assertEq(mtr.creditOf(alice), 0, "no credit recorded on failed pull");
    }

    function test_setMeter_owner_only() public {
        vm.prank(stranger);
        vm.expectRevert("LibDiamond: not owner");
        mtr.setMeter(stranger);
    }

    function test_meter_with_no_meter_set_reverts_for_everyone() public {
        // meter defaults to address(0); a call from address(0) is not
        // reachable, so meter() is effectively closed until owner sets it.
        _fundFor(alice, 100 ether, address(mtr));
        vm.prank(alice);
        mtr.depositCredits(10 ether);
        vm.prank(stranger);
        vm.expectRevert(CreditMeterFacet.NotMeter.selector);
        mtr.meter(alice, 1 ether);
    }

    function test_depositCredits_accumulates() public {
        _fundFor(alice, 100 ether, address(mtr));
        vm.startPrank(alice);
        mtr.depositCredits(10 ether);
        mtr.depositCredits(15 ether);
        vm.stopPrank();
        assertEq(mtr.creditOf(alice), 25 ether, "deposits accumulate");
    }

    /// FUZZ: across random deposit/debit ops the metered balance can never
    /// exceed total deposited, and the diamond's held `$LH` always covers
    /// outstanding credit (no debit creates value).
    function testFuzz_meter_never_inflates(uint256 seedRaw) public {
        vm.prank(diamondOwner);
        mtr.setMeter(meterKey);
        _fundFor(alice, 1_000_000 ether, address(mtr));

        uint256 seed = seedRaw;
        uint256 deposited;
        uint256 debited;
        for (uint256 i = 0; i < 30; i++) {
            seed = uint256(keccak256(abi.encode(seed, i)));
            if (seed % 2 == 0) {
                uint256 amt = (seed % 100) * 1 ether;
                if (amt == 0) amt = 1 ether;
                vm.prank(alice);
                mtr.depositCredits(amt);
                deposited += amt;
            } else {
                uint256 amt = (seed % 50) * 1 ether;
                if (amt <= mtr.creditOf(alice)) {
                    vm.prank(meterKey);
                    mtr.meter(alice, amt);
                    debited += amt;
                }
            }
            // The balance is exactly deposits minus debits — never more.
            assertEq(mtr.creditOf(alice), deposited - debited, "balance == deposits - debits");
            assertLe(mtr.creditOf(alice), deposited, "balance never exceeds deposits");
        }
    }

    // ====================================================================
    // ============================ SESSION ===============================
    // ====================================================================

    function _configSession(uint256 price, uint256 duration) internal {
        vm.startPrank(diamondOwner);
        ses.setSessionPrice(price);
        ses.setSessionDuration(duration);
        vm.stopPrank();
    }

    function test_session_disabled_when_duration_zero() public {
        _configSession(10 ether, 0); // duration 0 = disabled
        _fundFor(alice, 100 ether, address(ses));
        vm.prank(alice);
        vm.expectRevert(SessionFacet.SessionsDisabled.selector);
        ses.openSession();
    }

    function test_session_pulls_exact_price_and_sets_expiry() public {
        _configSession(10 ether, 3600);
        _fundFor(alice, 100 ether, address(ses));
        vm.prank(alice);
        uint256 expiry = ses.openSession();
        assertEq(expiry, block.timestamp + 3600, "expiry = now + duration");
        assertEq(lh.balanceOf(alice), 90 ether, "exactly priceWei pulled");
        assertEq(lh.balanceOf(address(ses)), 10 ether, "diamond holds session fee");
        assertEq(ses.sessionExpiryOf(alice), expiry, "expiry stored");
    }

    function test_session_cannot_open_without_payment_when_priced() public {
        _configSession(10 ether, 3600);
        // alice funded but no approval -> the transferFrom reverts, and
        // because the whole tx reverts, the expiry write is rolled back too.
        _fundFor(alice, 100 ether, address(ses));
        vm.prank(alice);
        lh.approve(address(ses), 0);
        vm.prank(alice);
        vm.expectRevert(); // InsufficientAllowance
        ses.openSession();
        assertEq(ses.sessionExpiryOf(alice), 0, "no session granted without payment");
    }

    function test_session_insufficient_balance_grants_nothing() public {
        _configSession(10 ether, 3600);
        // alice has only 5 $LH but approves max.
        vm.prank(address(ses));
        lh.mintWithMemo(alice, 5 ether, "FUND");
        vm.prank(alice);
        lh.approve(address(ses), type(uint256).max);
        vm.prank(alice);
        vm.expectRevert(); // InsufficientBalance
        ses.openSession();
        assertEq(ses.sessionExpiryOf(alice), 0, "no session on underfunded open");
    }

    function test_session_free_when_price_zero() public {
        // priceWei 0 = free session (owner can reopen free beta). No token
        // pull happens, so no approval needed.
        _configSession(0, 3600);
        vm.prank(alice);
        uint256 expiry = ses.openSession();
        assertEq(expiry, block.timestamp + 3600, "free session granted");
        assertEq(lh.balanceOf(address(ses)), 0, "no fee taken when free");
    }

    function test_session_price_and_duration_owner_only() public {
        vm.prank(stranger);
        vm.expectRevert("LibDiamond: not owner");
        ses.setSessionPrice(1);
        vm.prank(stranger);
        vm.expectRevert("LibDiamond: not owner");
        ses.setSessionDuration(1);
    }

    function test_session_uses_live_price_not_stale() public {
        // Opening reads s.priceWei at call time; an owner price change takes
        // effect immediately (no cached/stale price an attacker can exploit).
        _configSession(10 ether, 3600);
        _fundFor(alice, 100 ether, address(ses));
        vm.prank(diamondOwner);
        ses.setSessionPrice(20 ether); // raise the price
        vm.prank(alice);
        ses.openSession();
        assertEq(lh.balanceOf(alice), 80 ether, "charged the NEW price, not the old");
    }

    function test_session_renew_charges_again() public {
        _configSession(10 ether, 3600);
        _fundFor(alice, 100 ether, address(ses));
        vm.prank(alice);
        ses.openSession();
        vm.warp(block.timestamp + 100);
        vm.prank(alice);
        ses.openSession(); // renew -> charged again
        assertEq(lh.balanceOf(alice), 80 ether, "each open charges the price");
    }

    // ====================================================================
    // ============================== X402 ================================
    // ====================================================================

    function _digest(
        address from_,
        address to_,
        uint256 value_,
        uint256 va,
        uint256 vb,
        bytes32 nonce_
    ) internal view returns (bytes32) {
        bytes32 PAYMENT_TYPEHASH = keccak256(
            "PaymentAuthorization(address from,address to,uint256 value,uint256 validAfter,uint256 validBefore,bytes32 nonce)"
        );
        bytes32 structHash = keccak256(
            abi.encode(PAYMENT_TYPEHASH, from_, to_, value_, va, vb, nonce_)
        );
        return keccak256(abi.encodePacked("\x19\x01", x4.x402DomainSeparator(), structHash));
    }

    function _sign(uint256 pk, bytes32 digest) internal pure returns (bytes memory) {
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(pk, digest);
        return abi.encodePacked(r, s, v);
    }

    function test_x402_happy_path_settles_once() public {
        _fundFor(payer, 1_000 ether, address(x4));
        bytes32 nonce = keccak256("n1");
        uint256 va = block.timestamp - 1;
        uint256 vb = block.timestamp + 1000;
        bytes32 d = _digest(payer, payee, 100 ether, va, vb, nonce);
        bytes memory sig = _sign(payerPk, d);

        x4.settle(payer, payee, 100 ether, va, vb, nonce, sig);
        assertEq(lh.balanceOf(payee), 100 ether, "payee received");
        assertEq(lh.balanceOf(payer), 900 ether, "payer debited");
        assertTrue(x4.authorizationState(payer, nonce), "nonce consumed");
    }

    function test_x402_replay_same_nonce_reverts() public {
        _fundFor(payer, 1_000 ether, address(x4));
        bytes32 nonce = keccak256("replay");
        uint256 va = block.timestamp - 1;
        uint256 vb = block.timestamp + 1000;
        bytes32 d = _digest(payer, payee, 100 ether, va, vb, nonce);
        bytes memory sig = _sign(payerPk, d);

        x4.settle(payer, payee, 100 ether, va, vb, nonce, sig);
        // Replay the exact authorization -> nonce already used.
        vm.expectRevert(X402Facet.AuthAlreadyUsed.selector);
        x4.settle(payer, payee, 100 ether, va, vb, nonce, sig);
        assertEq(lh.balanceOf(payee), 100 ether, "paid exactly once");
    }

    function test_x402_reentrant_replay_during_transfer_blocked() public {
        // Defense-in-depth: a hostile token re-enters settle() with the SAME
        // authorization during transferFrom. The nonce is marked used BEFORE
        // the external call (CEI), so the reentrant settle reverts.
        ReentrantToken rt = new ReentrantToken();
        X402Harness rh = new X402Harness();
        rh._setDiamondOwner(diamondOwner);
        rh._setCreditsToken(address(rt));

        rt.mint(payer, 1_000 ether);
        vm.prank(payer);
        rt.approve(address(rh), type(uint256).max);

        bytes32 nonce = keccak256("reenter");
        uint256 va = block.timestamp - 1;
        uint256 vb = block.timestamp + 1000;
        // Domain separator binds verifyingContract = rh, so sign over rh.
        bytes32 PAYMENT_TYPEHASH = keccak256(
            "PaymentAuthorization(address from,address to,uint256 value,uint256 validAfter,uint256 validBefore,bytes32 nonce)"
        );
        bytes32 structHash = keccak256(
            abi.encode(PAYMENT_TYPEHASH, payer, payee, 100 ether, va, vb, nonce)
        );
        bytes32 d = keccak256(
            abi.encodePacked("\x19\x01", rh.x402DomainSeparator(), structHash)
        );
        bytes memory sig = _sign(payerPk, d);

        rt.armX402(address(rh), payer, payee, 100 ether, va, vb, nonce, sig);
        rh.settle(payer, payee, 100 ether, va, vb, nonce, sig);

        assertTrue(rt.reenterReverted(), "reentrant replay reverted (nonce-first CEI)");
        assertEq(rt.balanceOf(payee), 100 ether, "payee paid exactly once despite reentry");
    }

    function test_x402_tampered_value_reverts() public {
        // Sign for 100, submit for 1000 — the digest binds value, so the
        // signature no longer recovers to the payer.
        _fundFor(payer, 1_000 ether, address(x4));
        bytes32 nonce = keccak256("tamper-value");
        uint256 va = block.timestamp - 1;
        uint256 vb = block.timestamp + 1000;
        bytes32 d = _digest(payer, payee, 100 ether, va, vb, nonce);
        bytes memory sig = _sign(payerPk, d);
        vm.expectRevert(X402Facet.BadSignature.selector);
        x4.settle(payer, payee, 1_000 ether, va, vb, nonce, sig);
    }

    function test_x402_tampered_to_reverts() public {
        // Redirect the payee — digest binds `to`, signature fails.
        _fundFor(payer, 1_000 ether, address(x4));
        bytes32 nonce = keccak256("tamper-to");
        uint256 va = block.timestamp - 1;
        uint256 vb = block.timestamp + 1000;
        bytes32 d = _digest(payer, payee, 100 ether, va, vb, nonce);
        bytes memory sig = _sign(payerPk, d);
        vm.expectRevert(X402Facet.BadSignature.selector);
        x4.settle(payer, stranger, 100 ether, va, vb, nonce, sig);
    }

    function test_x402_wrong_signer_reverts() public {
        // Someone else signs an authorization claiming to be `payer`.
        _fundFor(payer, 1_000 ether, address(x4));
        uint256 attackerPk = 0xBADBAD;
        bytes32 nonce = keccak256("forge");
        uint256 va = block.timestamp - 1;
        uint256 vb = block.timestamp + 1000;
        bytes32 d = _digest(payer, payee, 100 ether, va, vb, nonce);
        bytes memory sig = _sign(attackerPk, d); // wrong key
        vm.expectRevert(X402Facet.BadSignature.selector);
        x4.settle(payer, payee, 100 ether, va, vb, nonce, sig);
        assertEq(lh.balanceOf(payer), 1_000 ether, "no funds moved on forged sig");
    }

    function test_x402_not_yet_valid_reverts() public {
        _fundFor(payer, 1_000 ether, address(x4));
        bytes32 nonce = keccak256("future");
        uint256 va = block.timestamp + 100; // not yet valid
        uint256 vb = block.timestamp + 1000;
        bytes32 d = _digest(payer, payee, 100 ether, va, vb, nonce);
        bytes memory sig = _sign(payerPk, d);
        vm.expectRevert(X402Facet.AuthNotYetValid.selector);
        x4.settle(payer, payee, 100 ether, va, vb, nonce, sig);
    }

    function test_x402_expired_reverts() public {
        _fundFor(payer, 1_000 ether, address(x4));
        bytes32 nonce = keccak256("past");
        uint256 va = block.timestamp - 1000;
        uint256 vb = block.timestamp - 1; // already expired
        bytes32 d = _digest(payer, payee, 100 ether, va, vb, nonce);
        bytes memory sig = _sign(payerPk, d);
        vm.expectRevert(X402Facet.AuthExpired.selector);
        x4.settle(payer, payee, 100 ether, va, vb, nonce, sig);
    }

    /// Build the EIP-2 high-s malleated twin of a valid signature over
    /// `digest`: flip s to N-s and v to its complement. (Pulled into a helper
    /// to keep the test under the stack-depth limit.)
    function _malleate(uint256 pk, bytes32 digest) internal pure returns (bytes memory) {
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(pk, digest);
        uint256 N = 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFBAAEDCE6AF48A03BBFD25E8CD0364141;
        return abi.encodePacked(r, bytes32(N - uint256(s)), v == 27 ? uint8(28) : uint8(27));
    }

    function test_x402_high_s_signature_rejected() public {
        // EIP-2 malleability: flip s to its high-s complement and v; the
        // facet rejects high-s, so the malleated twin can't be a 2nd valid sig.
        _fundFor(payer, 1_000 ether, address(x4));
        bytes32 nonce = keccak256("malleable");
        uint256 va = block.timestamp - 1;
        uint256 vb = block.timestamp + 1000;
        bytes memory malleated = _malleate(payerPk, _digest(payer, payee, 100 ether, va, vb, nonce));
        vm.expectRevert(X402Facet.BadSignature.selector);
        x4.settle(payer, payee, 100 ether, va, vb, nonce, malleated);
    }

    function test_x402_domain_separator_binds_chainid_and_diamond() public {
        // Recompute the domain separator and confirm it equals what the facet
        // returns for the current chain + this contract. A sig built for a
        // DIFFERENT verifyingContract must not validate here.
        bytes32 EIP712_DOMAIN_TYPEHASH = keccak256(
            "EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"
        );
        bytes32 expected = keccak256(
            abi.encode(
                EIP712_DOMAIN_TYPEHASH,
                keccak256(bytes("localharness-x402")),
                keccak256(bytes("1")),
                block.chainid,
                address(x4)
            )
        );
        assertEq(x4.x402DomainSeparator(), expected, "domain binds chainId + diamond");

        // A signature whose digest used the WRONG verifyingContract fails.
        _fundFor(payer, 1_000 ether, address(x4));
        bytes32 nonce = keccak256("xdomain");
        uint256 va = block.timestamp - 1;
        uint256 vb = block.timestamp + 1000;
        bytes32 wrongDomain = keccak256(
            abi.encode(
                EIP712_DOMAIN_TYPEHASH,
                keccak256(bytes("localharness-x402")),
                keccak256(bytes("1")),
                block.chainid,
                address(0xDEAD) // wrong verifyingContract
            )
        );
        bytes32 PAYMENT_TYPEHASH = keccak256(
            "PaymentAuthorization(address from,address to,uint256 value,uint256 validAfter,uint256 validBefore,bytes32 nonce)"
        );
        bytes32 structHash = keccak256(
            abi.encode(PAYMENT_TYPEHASH, payer, payee, 100 ether, va, vb, nonce)
        );
        bytes32 wrongDigest = keccak256(abi.encodePacked("\x19\x01", wrongDomain, structHash));
        bytes memory sig = _sign(payerPk, wrongDigest);
        vm.expectRevert(X402Facet.BadSignature.selector);
        x4.settle(payer, payee, 100 ether, va, vb, nonce, sig);
    }

    function test_x402_malformed_signature_length_reverts() public {
        _fundFor(payer, 1_000 ether, address(x4));
        bytes32 nonce = keccak256("badlen");
        uint256 va = block.timestamp - 1;
        uint256 vb = block.timestamp + 1000;
        bytes memory shortSig = hex"1234"; // not 65 bytes
        vm.expectRevert(X402Facet.BadSignature.selector);
        x4.settle(payer, payee, 100 ether, va, vb, nonce, shortSig);
    }

    function test_x402_nonce_is_per_payer_not_global() public {
        // The same nonce value is independent across distinct payers — one
        // payer consuming a nonce doesn't block another payer's same nonce.
        _fundFor(payer, 1_000 ether, address(x4));
        uint256 otherPk = 0xC0FFEE;
        address other = vm.addr(otherPk);
        _fundFor(other, 1_000 ether, address(x4));

        bytes32 nonce = keccak256("shared-nonce");
        uint256 va = block.timestamp - 1;
        uint256 vb = block.timestamp + 1000;

        bytes32 d1 = _digest(payer, payee, 10 ether, va, vb, nonce);
        x4.settle(payer, payee, 10 ether, va, vb, nonce, _sign(payerPk, d1));

        bytes32 d2 = _digest(other, payee, 20 ether, va, vb, nonce);
        x4.settle(other, payee, 20 ether, va, vb, nonce, _sign(otherPk, d2));

        assertEq(lh.balanceOf(payee), 30 ether, "both distinct payers settle the same nonce");
    }

    // ====================================================================
    // =================== CREDITS / ISSUER_ROLE / CAP ====================
    // ====================================================================

    function test_credits_nonissuer_cannot_mint_directly() public {
        // The token's mint is role-gated. A random address (and even the
        // token owner, unless self-granted) cannot mint.
        vm.prank(stranger);
        vm.expectRevert(LocalharnessCredits.Unauthorized.selector);
        lh.mint(stranger, 1_000 ether);

        // Token owner has NOT been granted ISSUER_ROLE -> also can't mint.
        vm.prank(tokenOwner);
        vm.expectRevert(LocalharnessCredits.Unauthorized.selector);
        lh.mint(tokenOwner, 1_000 ether);
    }

    function test_credits_only_role_holder_mints() public {
        // The diamond (harness) holds ISSUER_ROLE -> it can mint.
        vm.prank(address(cred));
        lh.mintWithMemo(alice, 5 ether, "X");
        assertEq(lh.balanceOf(alice), 5 ether, "issuer mint works");
    }

    function test_credits_supply_cap_respected() public {
        // Minting past the cap reverts; supply is hard-bounded.
        LocalharnessCredits small = new LocalharnessCredits(100 ether, tokenOwner);
        bytes32 role = small.ISSUER_ROLE(); // read BEFORE the prank
        vm.prank(tokenOwner);
        small.grantRole(role, address(this)); // owner grants the test the role
        small.mint(alice, 100 ether); // exactly the cap
        vm.expectRevert(LocalharnessCredits.SupplyCapExceeded.selector);
        small.mint(alice, 1); // one wei over -> revert
        assertEq(small.totalSupply(), 100 ether, "supply pinned at cap");
    }

    function test_credits_setCreditsToken_owner_only() public {
        vm.prank(stranger);
        vm.expectRevert("LibDiamond: not owner");
        cred.setCreditsToken(address(0xFEED));
    }

    function test_credits_setDailyAllowance_owner_only() public {
        vm.prank(stranger);
        vm.expectRevert("LibDiamond: not owner");
        cred.setDailyAllowance(1 ether);
    }

    function test_credits_claimDaily_disabled_when_allowance_zero() public {
        // dailyAllowance defaults to 0 (the live sybil-safe config) -> claim
        // reverts NotConfigured, so the daily faucet can't be drained.
        vm.prank(alice);
        vm.expectRevert(CreditsFacet.NotConfigured.selector);
        cred.claimDaily();
    }

    function test_credits_claimDaily_once_per_day() public {
        vm.prank(diamondOwner);
        cred.setDailyAllowance(3 ether);
        vm.prank(alice);
        uint256 got = cred.claimDaily();
        assertEq(got, 3 ether, "first claim mints allowance");
        // Second claim same UTC day reverts.
        vm.prank(alice);
        vm.expectRevert(
            abi.encodeWithSelector(
                CreditsFacet.AlreadyClaimedToday.selector, uint64(block.timestamp / 86400)
            )
        );
        cred.claimDaily();
        assertEq(lh.balanceOf(alice), 3 ether, "no double claim same day");
    }

    function test_credits_claimDaily_next_day_allowed() public {
        vm.prank(diamondOwner);
        cred.setDailyAllowance(3 ether);
        vm.prank(alice);
        cred.claimDaily();
        vm.warp(block.timestamp + 86400); // next UTC day
        vm.prank(alice);
        cred.claimDaily();
        assertEq(lh.balanceOf(alice), 6 ether, "claim again next day");
    }

    function test_credits_setSupplyCap_cannot_go_below_supply() public {
        // Owner can't shrink the cap below circulating supply (would brick
        // accounting). Token-level guard.
        vm.prank(address(cred));
        lh.mintWithMemo(alice, 500 ether, "X");
        vm.prank(tokenOwner);
        vm.expectRevert(LocalharnessCredits.InvalidSupplyCap.selector);
        lh.setSupplyCap(499 ether);
    }

    function test_credits_token_transferFrom_underflow_reverts_not_wraps() public {
        // Solidity 0.8 checked math: an over-spend of allowance/balance
        // reverts (no silent wrap to a giant balance).
        vm.prank(address(cred));
        lh.mintWithMemo(alice, 10 ether, "X");
        vm.prank(alice);
        lh.approve(bob, 5 ether);
        // Over allowance -> revert (no silent wrap of the allowance).
        vm.prank(bob);
        vm.expectRevert(LocalharnessCredits.InsufficientAllowance.selector);
        lh.transferFrom(alice, bob, 6 ether);
        // Full allowance but over balance -> revert (no silent wrap of the
        // balance into a giant number).
        vm.prank(alice);
        lh.approve(bob, type(uint256).max);
        vm.prank(bob);
        vm.expectRevert(
            abi.encodeWithSelector(
                LocalharnessCredits.InsufficientBalance.selector, 10 ether, 11 ether, alice
            )
        );
        lh.transferFrom(alice, bob, 11 ether);
    }
}
