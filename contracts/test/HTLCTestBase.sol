// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {ParalockHTLC} from "../src/ParalockHTLC.sol";

interface IToken {
    function approve(address spender, uint256 amount) external returns (bool);
    function balanceOf(address account) external view returns (uint256);
}

/// @notice Behavioral suite every token backend must pass. Subclasses pick the token and how to fund accounts.
abstract contract HTLCTestBase is Test {
    ParalockHTLC internal htlc;
    address internal token;
    /// @dev Pre-existing balances: on a live Tempo fork, forge's default deploy addresses are pre-funded.
    uint256 internal escrowBaseline;
    uint256 internal takerBaseline;

    address internal maker = makeAddr("maker");
    address internal taker = makeAddr("taker");
    address internal relayer = makeAddr("relayer");

    bytes32 internal constant SECRET = keccak256("paralock secret");
    uint256 internal constant AMOUNT = 250e6; // 250 USD at 6 decimals
    uint64 internal constant DURATION = 1 hours;

    /// @dev Token under test.
    function _deployToken() internal virtual returns (address);

    /// @dev Give `to` `amount` tokens.
    function _fund(address to, uint256 amount) internal virtual;

    function setUp() public virtual {
        token = _deployToken();
        htlc = new ParalockHTLC();
        escrowBaseline = _balance(address(htlc));
        takerBaseline = _balance(taker);
        _fund(maker, AMOUNT * 4);
        vm.prank(maker);
        IToken(token).approve(address(htlc), type(uint256).max);
    }

    // ---------------------------------------------------------------- helpers

    function _swap(bytes32 secret, uint256 amount, uint64 expiry) internal view returns (ParalockHTLC.Swap memory) {
        return ParalockHTLC.Swap({
            hashlock: sha256(abi.encodePacked(secret)),
            token: token,
            sender: maker,
            recipient: taker,
            amount: amount,
            expiry: expiry
        });
    }

    function _defaultSwap() internal view returns (ParalockHTLC.Swap memory) {
        return _swap(SECRET, AMOUNT, uint64(vm.getBlockTimestamp()) + DURATION);
    }

    function _lock(ParalockHTLC.Swap memory s) internal {
        vm.prank(s.sender);
        htlc.lock(s);
    }

    function _balance(address who) internal view returns (uint256) {
        return IToken(token).balanceOf(who);
    }

    /// @dev Tokens currently held in escrow by the HTLC.
    function _escrowed() internal view returns (uint256) {
        return _balance(address(htlc)) - escrowBaseline;
    }

    /// @dev Tokens the taker received during the test.
    function _takerReceived() internal view returns (uint256) {
        return _balance(taker) - takerBaseline;
    }

    // ---------------------------------------------------------------- lock

    function test_lock_escrowsFundsAndStoresOneCommitment() public {
        ParalockHTLC.Swap memory s = _defaultSwap();
        uint256 makerBefore = _balance(maker);

        vm.expectEmit(address(htlc));
        emit ParalockHTLC.Locked(s.hashlock, s.sender, s.recipient, s.token, s.amount, s.expiry);
        vm.record();
        _lock(s);
        (, bytes32[] memory writes) = vm.accesses(address(htlc));

        assertEq(writes.length, 1, "exactly one storage slot per swap");
        assertEq(_balance(maker), makerBefore - s.amount);
        assertEq(_escrowed(), s.amount);
        assertEq(htlc.stateOf(s.hashlock), keccak256(abi.encode(s)));
        assertEq(htlc.stateOf(s.hashlock), htlc.commitment(s));
        assertTrue(htlc.isActive(s.hashlock));
        assertEq(uint8(htlc.statusOf(s.hashlock)), uint8(ParalockHTLC.Status.Active));
    }

    function test_lock_revertsWhenCallerIsNotSender() public {
        ParalockHTLC.Swap memory s = _defaultSwap();
        vm.prank(taker);
        vm.expectRevert(ParalockHTLC.NotSender.selector);
        htlc.lock(s);
    }

    function test_lock_revertsOnInvalidParams() public {
        ParalockHTLC.Swap memory s = _defaultSwap();

        s.hashlock = bytes32(0);
        vm.expectRevert(ParalockHTLC.InvalidHashlock.selector);
        _lock(s);

        s = _defaultSwap();
        s.amount = 0;
        vm.expectRevert(ParalockHTLC.InvalidAmount.selector);
        _lock(s);

        s = _defaultSwap();
        s.recipient = address(0);
        vm.expectRevert(ParalockHTLC.InvalidRecipient.selector);
        _lock(s);

        s = _defaultSwap();
        s.expiry = uint64(vm.getBlockTimestamp());
        vm.expectRevert(ParalockHTLC.InvalidExpiry.selector);
        _lock(s);

        s.expiry = uint64(vm.getBlockTimestamp()) - 1;
        vm.expectRevert(ParalockHTLC.InvalidExpiry.selector);
        _lock(s);
    }

    function test_lock_revertsWithoutAllowance() public {
        ParalockHTLC.Swap memory s = _defaultSwap();
        vm.prank(maker);
        IToken(token).approve(address(htlc), s.amount - 1);
        vm.expectRevert();
        _lock(s);
        assertEq(htlc.stateOf(s.hashlock), bytes32(0), "failed lock leaves no state");
    }

    function test_lock_revertsOnActiveHashlockReuse() public {
        ParalockHTLC.Swap memory s = _defaultSwap();
        _lock(s);

        s.amount = 1; // any params: the hashlock alone is taken
        vm.expectRevert(abi.encodeWithSelector(ParalockHTLC.HashlockUsed.selector, s.hashlock));
        _lock(s);
    }

    function test_lock_revertsOnSettledHashlockReuse() public {
        ParalockHTLC.Swap memory s = _defaultSwap();
        _lock(s);
        htlc.claim(s, SECRET);

        // The preimage is public now: re-locking under it would let anyone claim instantly.
        s.expiry = uint64(vm.getBlockTimestamp()) + 2 * DURATION;
        vm.expectRevert(abi.encodeWithSelector(ParalockHTLC.HashlockUsed.selector, s.hashlock));
        _lock(s);
    }

    // ---------------------------------------------------------------- claim

    function test_claim_paysRecipientAndRevealsPreimage() public {
        ParalockHTLC.Swap memory s = _defaultSwap();
        _lock(s);

        vm.expectEmit(address(htlc));
        emit ParalockHTLC.Claimed(s.hashlock, SECRET);
        vm.prank(taker);
        htlc.claim(s, SECRET);

        assertEq(_takerReceived(), s.amount);
        assertEq(_escrowed(), 0);
        assertEq(htlc.stateOf(s.hashlock), htlc.SETTLED());
        assertFalse(htlc.isActive(s.hashlock));
        assertEq(uint8(htlc.statusOf(s.hashlock)), uint8(ParalockHTLC.Status.Settled));
    }

    function test_claim_byAnyonePaysRecipient() public {
        ParalockHTLC.Swap memory s = _defaultSwap();
        _lock(s);

        vm.prank(relayer);
        htlc.claim(s, SECRET);

        assertEq(_takerReceived(), s.amount);
        assertEq(_balance(relayer), 0);
    }

    function test_claim_succeedsOneSecondBeforeExpiry() public {
        ParalockHTLC.Swap memory s = _defaultSwap();
        _lock(s);
        vm.warp(s.expiry - 1);
        htlc.claim(s, SECRET);
        assertEq(_takerReceived(), s.amount);
    }

    function test_claim_revertsOnWrongPreimage() public {
        ParalockHTLC.Swap memory s = _defaultSwap();
        _lock(s);

        vm.expectRevert(ParalockHTLC.InvalidPreimage.selector);
        htlc.claim(s, keccak256("wrong"));

        // keccak256 is not the hash function: the hashlock's keccak preimage must not work either.
        bytes32 keccakLock = keccak256(abi.encodePacked(SECRET));
        vm.expectRevert(ParalockHTLC.InvalidPreimage.selector);
        htlc.claim(s, keccakLock);
    }

    function test_claim_revertsAtAndAfterExpiry() public {
        ParalockHTLC.Swap memory s = _defaultSwap();
        _lock(s);

        vm.warp(s.expiry);
        vm.expectRevert(abi.encodeWithSelector(ParalockHTLC.SwapExpired.selector, s.expiry));
        htlc.claim(s, SECRET);

        vm.warp(s.expiry + 1 days);
        vm.expectRevert(abi.encodeWithSelector(ParalockHTLC.SwapExpired.selector, s.expiry));
        htlc.claim(s, SECRET);
    }

    function test_claim_revertsOnDoubleClaim() public {
        ParalockHTLC.Swap memory s = _defaultSwap();
        _lock(s);
        htlc.claim(s, SECRET);

        vm.expectRevert(abi.encodeWithSelector(ParalockHTLC.SwapSettled.selector, s.hashlock));
        htlc.claim(s, SECRET);
        assertEq(_takerReceived(), s.amount);
    }

    function test_claim_revertsForUnknownSwap() public {
        ParalockHTLC.Swap memory s = _defaultSwap();
        vm.expectRevert(abi.encodeWithSelector(ParalockHTLC.SwapNotFound.selector, s.hashlock));
        htlc.claim(s, SECRET);
    }

    function test_claim_revertsAfterRefund() public {
        ParalockHTLC.Swap memory s = _defaultSwap();
        _lock(s);
        vm.warp(s.expiry);
        htlc.refund(s);

        vm.warp(s.expiry - 1); // even if the clock went backwards, a settled swap stays settled
        vm.expectRevert(abi.encodeWithSelector(ParalockHTLC.SwapSettled.selector, s.hashlock));
        htlc.claim(s, SECRET);
    }

    /// @dev Tampering with any field (e.g. redirecting payment, inflating the amount) must not match the commitment.
    function testFuzz_claimAndRefund_revertOnMismatchedParams(uint8 field, uint256 delta) public {
        ParalockHTLC.Swap memory s = _defaultSwap();
        _lock(s);

        ParalockHTLC.Swap memory forged = _defaultSwap();
        delta = bound(delta, 1, type(uint32).max);
        field = uint8(bound(field, 0, 4));
        if (field == 0) forged.token = address(uint160(forged.token) ^ uint160(delta));
        else if (field == 1) forged.sender = relayer;
        else if (field == 2) forged.recipient = relayer;
        else if (field == 3) forged.amount += delta;
        else forged.expiry += uint64(delta);

        vm.expectRevert(abi.encodeWithSelector(ParalockHTLC.SwapMismatch.selector, s.hashlock));
        htlc.claim(forged, SECRET);

        vm.warp(forged.expiry > s.expiry ? forged.expiry : s.expiry);
        vm.expectRevert(abi.encodeWithSelector(ParalockHTLC.SwapMismatch.selector, s.hashlock));
        htlc.refund(forged);

        assertTrue(htlc.isActive(s.hashlock));
        assertEq(_escrowed(), s.amount);
    }

    // ---------------------------------------------------------------- refund

    function test_refund_returnsFundsToSenderAtExpiry() public {
        ParalockHTLC.Swap memory s = _defaultSwap();
        uint256 makerBefore = _balance(maker);
        _lock(s);

        vm.warp(s.expiry);
        vm.expectEmit(address(htlc));
        emit ParalockHTLC.Refunded(s.hashlock);
        vm.prank(relayer); // anyone may trigger the refund; funds always go to the sender
        htlc.refund(s);

        assertEq(_balance(maker), makerBefore);
        assertEq(_balance(relayer), 0);
        assertEq(_escrowed(), 0);
        assertEq(htlc.stateOf(s.hashlock), htlc.SETTLED());
    }

    function test_refund_revertsBeforeExpiry() public {
        ParalockHTLC.Swap memory s = _defaultSwap();
        _lock(s);

        vm.warp(s.expiry - 1);
        vm.expectRevert(abi.encodeWithSelector(ParalockHTLC.SwapNotExpired.selector, s.expiry));
        htlc.refund(s);
    }

    function test_refund_revertsAfterClaimAndOnDoubleRefund() public {
        ParalockHTLC.Swap memory s = _defaultSwap();
        _lock(s);
        htlc.claim(s, SECRET);
        vm.warp(s.expiry);
        vm.expectRevert(abi.encodeWithSelector(ParalockHTLC.SwapSettled.selector, s.hashlock));
        htlc.refund(s);

        ParalockHTLC.Swap memory t = _swap(keccak256("second"), AMOUNT, uint64(vm.getBlockTimestamp()) + DURATION);
        _lock(t);
        vm.warp(t.expiry);
        htlc.refund(t);
        vm.expectRevert(abi.encodeWithSelector(ParalockHTLC.SwapSettled.selector, t.hashlock));
        htlc.refund(t);
    }

    // ---------------------------------------------------------------- isolation & fuzz

    function test_concurrentSwapsAreIsolated() public {
        ParalockHTLC.Swap memory a = _swap(keccak256("a"), AMOUNT, uint64(vm.getBlockTimestamp()) + DURATION);
        ParalockHTLC.Swap memory b = _swap(keccak256("b"), AMOUNT * 2, uint64(vm.getBlockTimestamp()) + 2 * DURATION);
        _lock(a);
        _lock(b);
        assertEq(_escrowed(), a.amount + b.amount);

        htlc.claim(a, keccak256("a"));
        assertTrue(htlc.isActive(b.hashlock));

        vm.warp(b.expiry);
        uint256 makerBefore = _balance(maker);
        htlc.refund(b);
        assertEq(_balance(maker), makerBefore + b.amount);
        assertEq(_takerReceived(), a.amount);
        assertEq(_escrowed(), 0);
    }

    function testFuzz_lockThenClaim(bytes32 secret, uint256 amount, uint64 duration, uint64 claimAfter) public {
        amount = bound(amount, 1, AMOUNT * 4);
        duration = uint64(bound(duration, 1, 365 days));
        claimAfter = uint64(bound(claimAfter, 0, duration - 1));
        ParalockHTLC.Swap memory s = _swap(secret, amount, uint64(vm.getBlockTimestamp()) + duration);

        _lock(s);
        vm.warp(vm.getBlockTimestamp() + claimAfter);
        htlc.claim(s, secret);

        assertEq(_takerReceived(), amount);
        assertEq(_escrowed(), 0);
        assertEq(htlc.stateOf(s.hashlock), htlc.SETTLED());
    }

    function testFuzz_lockThenRefund(bytes32 secret, uint256 amount, uint64 duration, uint64 refundAfter) public {
        amount = bound(amount, 1, AMOUNT * 4);
        duration = uint64(bound(duration, 1, 365 days));
        refundAfter = uint64(bound(refundAfter, duration, uint256(duration) + 365 days));
        ParalockHTLC.Swap memory s = _swap(secret, amount, uint64(vm.getBlockTimestamp()) + duration);
        uint256 makerBefore = _balance(maker);

        _lock(s);
        vm.warp(vm.getBlockTimestamp() + refundAfter);
        vm.expectRevert(abi.encodeWithSelector(ParalockHTLC.SwapExpired.selector, s.expiry));
        htlc.claim(s, secret);
        htlc.refund(s);

        assertEq(_balance(maker), makerBefore);
        assertEq(_escrowed(), 0);
    }

    function testFuzz_lockRejectsNonFutureExpiry(uint64 expiry) public {
        expiry = uint64(bound(expiry, 0, vm.getBlockTimestamp()));
        ParalockHTLC.Swap memory s = _swap(SECRET, AMOUNT, expiry); // sha256 is a precompile call: keep it
        vm.expectRevert(ParalockHTLC.InvalidExpiry.selector); // out of expectRevert's "next call"
        _lock(s);
    }
}
