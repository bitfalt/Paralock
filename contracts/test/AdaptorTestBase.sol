// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {ParalockAdaptorEscrow} from "../src/ParalockAdaptorEscrow.sol";

interface IAdaptorToken {
    function approve(address spender, uint256 amount) external returns (bool);
    function balanceOf(address account) external view returns (uint256);
}

/// @notice Behavioral suite of ParalockAdaptorEscrow that every token backend must pass. Subclasses pick the token
///         and how to fund accounts.
abstract contract AdaptorTestBase is Test {
    ParalockAdaptorEscrow internal escrow;
    address internal token;
    /// @dev Pre-existing balances (forge's Tempo genesis pre-funds some addresses with dev stablecoins).
    uint256 internal escrowBaseline;
    uint256 internal takerBaseline;

    address internal maker = makeAddr("maker");
    address internal taker = makeAddr("taker");
    address internal relayer = makeAddr("relayer");

    /// @dev The one-time key whose signature releases the funds (held by the maker in the adaptor protocol).
    uint256 internal signerKey = uint256(keccak256("paralock adaptor signer"));
    address internal signer;

    uint256 internal constant SECP256K1_N = 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141;
    bytes32 internal constant SALT = keccak256("paralock salt");
    uint256 internal constant AMOUNT = 250e6; // 250 USD at 6 decimals
    uint64 internal constant DURATION = 1 hours;

    /// @dev Token under test.
    function _deployToken() internal virtual returns (address);

    /// @dev Give `to` `amount` tokens.
    function _fund(address to, uint256 amount) internal virtual;

    function setUp() public virtual {
        signer = vm.addr(signerKey);
        token = _deployToken();
        escrow = new ParalockAdaptorEscrow();
        escrowBaseline = _balance(address(escrow));
        takerBaseline = _balance(taker);
        _fund(maker, AMOUNT * 4);
        vm.prank(maker);
        IAdaptorToken(token).approve(address(escrow), type(uint256).max);
    }

    // ---------------------------------------------------------------- helpers

    function _swap(bytes32 salt, uint256 amount, uint64 expiry)
        internal
        view
        returns (ParalockAdaptorEscrow.Swap memory)
    {
        return ParalockAdaptorEscrow.Swap({
            token: token, sender: maker, recipient: taker, signer: signer, amount: amount, expiry: expiry, salt: salt
        });
    }

    function _defaultSwap() internal view returns (ParalockAdaptorEscrow.Swap memory) {
        return _swap(SALT, AMOUNT, uint64(vm.getBlockTimestamp()) + DURATION);
    }

    function _id(ParalockAdaptorEscrow.Swap memory s) internal pure returns (bytes32) {
        return keccak256(abi.encode(s));
    }

    function _lock(ParalockAdaptorEscrow.Swap memory s) internal returns (bytes32 id) {
        vm.prank(s.sender);
        id = escrow.lock(s);
    }

    /// @dev The digest, recomputed independently of the contract.
    function _digest(bytes32 id) internal view returns (bytes32) {
        return keccak256(abi.encode(keccak256("ParalockAdaptorClaim"), block.chainid, address(escrow), id));
    }

    function _sign(uint256 key, bytes32 id) internal view returns (uint8 v, bytes32 r, bytes32 s) {
        (v, r, s) = vm.sign(key, _digest(id));
        assertLe(uint256(s), SECP256K1_N / 2, "vm.sign returns low-s signatures");
    }

    function _claim(ParalockAdaptorEscrow.Swap memory s) internal {
        (uint8 v, bytes32 r, bytes32 sigS) = _sign(signerKey, _id(s));
        escrow.claim(s, v, r, sigS);
    }

    function _balance(address who) internal view returns (uint256) {
        return IAdaptorToken(token).balanceOf(who);
    }

    function _escrowed() internal view returns (uint256) {
        return _balance(address(escrow)) - escrowBaseline;
    }

    function _takerReceived() internal view returns (uint256) {
        return _balance(taker) - takerBaseline;
    }

    function _status(bytes32 id) internal view returns (uint8) {
        return uint8(escrow.statusOf(id));
    }

    // ---------------------------------------------------------------- lock

    function test_lock_escrowsFundsAndStoresOneSlot() public {
        ParalockAdaptorEscrow.Swap memory s = _defaultSwap();
        bytes32 expectedId = _id(s);
        uint256 makerBefore = _balance(maker);

        vm.expectEmit(address(escrow));
        emit ParalockAdaptorEscrow.Locked(
            expectedId, s.sender, s.recipient, s.token, s.signer, s.amount, s.expiry, s.salt
        );
        vm.record();
        bytes32 id = _lock(s);
        (, bytes32[] memory writes) = vm.accesses(address(escrow));

        assertEq(writes.length, 1, "exactly one storage slot per swap");
        assertEq(id, expectedId, "id = keccak256(abi.encode(swap))");
        assertEq(id, escrow.idOf(s));
        assertEq(_balance(maker), makerBefore - s.amount);
        assertEq(_escrowed(), s.amount);
        assertEq(_status(id), 1);
        assertTrue(escrow.isActive(id));
    }

    function test_lock_revertsWhenCallerIsNotSender() public {
        ParalockAdaptorEscrow.Swap memory s = _defaultSwap();
        vm.prank(taker);
        vm.expectRevert(ParalockAdaptorEscrow.NotSender.selector);
        escrow.lock(s);
    }

    function test_lock_revertsOnInvalidParams() public {
        ParalockAdaptorEscrow.Swap memory s = _defaultSwap();
        s.amount = 0;
        vm.expectRevert(ParalockAdaptorEscrow.InvalidAmount.selector);
        _lock(s);

        s = _defaultSwap();
        s.recipient = address(0);
        vm.expectRevert(ParalockAdaptorEscrow.InvalidRecipient.selector);
        _lock(s);

        s = _defaultSwap();
        s.signer = address(0);
        vm.expectRevert(ParalockAdaptorEscrow.InvalidSigner.selector);
        _lock(s);

        s = _defaultSwap();
        s.expiry = uint64(vm.getBlockTimestamp());
        vm.expectRevert(ParalockAdaptorEscrow.InvalidExpiry.selector);
        _lock(s);

        s.expiry = uint64(vm.getBlockTimestamp()) - 1;
        vm.expectRevert(ParalockAdaptorEscrow.InvalidExpiry.selector);
        _lock(s);
    }

    function test_lock_revertsWithoutAllowance() public {
        ParalockAdaptorEscrow.Swap memory s = _defaultSwap();
        vm.prank(maker);
        IAdaptorToken(token).approve(address(escrow), s.amount - 1);
        vm.expectRevert();
        _lock(s);
        assertEq(_status(_id(s)), 0, "failed lock leaves no state");
    }

    function test_lock_revertsOnActiveIdReuse() public {
        ParalockAdaptorEscrow.Swap memory s = _defaultSwap();
        bytes32 id = _lock(s);
        vm.expectRevert(abi.encodeWithSelector(ParalockAdaptorEscrow.SwapExists.selector, id));
        _lock(s);

        // A fresh salt makes an otherwise identical swap a different, independent one.
        s.salt = keccak256("another salt");
        bytes32 other = _lock(s);
        assertTrue(other != id);
        assertEq(_escrowed(), 2 * s.amount);
    }

    function test_lock_revertsOnSettledIdReuse() public {
        ParalockAdaptorEscrow.Swap memory s = _defaultSwap();
        bytes32 id = _lock(s);
        _claim(s);

        // The claim signature is public now: re-locking under the same id would let anyone claim instantly.
        vm.expectRevert(abi.encodeWithSelector(ParalockAdaptorEscrow.SwapExists.selector, id));
        _lock(s);

        ParalockAdaptorEscrow.Swap memory t = _swap(keccak256("t"), AMOUNT, uint64(vm.getBlockTimestamp()) + DURATION);
        bytes32 tid = _lock(t);
        vm.warp(t.expiry);
        escrow.refund(t);
        vm.warp(t.expiry - 1);
        vm.expectRevert(abi.encodeWithSelector(ParalockAdaptorEscrow.SwapExists.selector, tid));
        _lock(t);
    }

    // ---------------------------------------------------------------- claim digest

    function test_claimDigest_bindsTypehashChainContractAndId() public {
        bytes32 id = _id(_defaultSwap());
        assertEq(escrow.CLAIM_TYPEHASH(), keccak256("ParalockAdaptorClaim"));
        assertEq(escrow.claimDigest(id), _digest(id));
        assertTrue(escrow.claimDigest(id) != escrow.claimDigest(bytes32(uint256(id) ^ 1)));

        ParalockAdaptorEscrow other = new ParalockAdaptorEscrow();
        assertTrue(other.claimDigest(id) != escrow.claimDigest(id), "bound to the contract");

        bytes32 before = escrow.claimDigest(id);
        vm.chainId(block.chainid + 1);
        assertTrue(escrow.claimDigest(id) != before, "bound to the chain");
    }

    // ---------------------------------------------------------------- claim

    function test_claim_paysRecipientAndPublishesSignature() public {
        ParalockAdaptorEscrow.Swap memory s = _defaultSwap();
        bytes32 id = _lock(s);
        (uint8 v, bytes32 r, bytes32 sigS) = _sign(signerKey, id);

        vm.expectEmit(address(escrow));
        emit ParalockAdaptorEscrow.Claimed(id, v, r, sigS);
        vm.prank(taker);
        escrow.claim(s, v, r, sigS);

        assertEq(_takerReceived(), s.amount);
        assertEq(_escrowed(), 0);
        assertEq(_status(id), 2);
        assertFalse(escrow.isActive(id));
    }

    function test_claim_byRelayerPaysRecipient() public {
        ParalockAdaptorEscrow.Swap memory s = _defaultSwap();
        _lock(s);
        vm.prank(relayer);
        _claim(s);
        assertEq(_takerReceived(), s.amount);
        assertEq(_balance(relayer), 0);
    }

    function test_claim_succeedsOneSecondBeforeExpiry() public {
        ParalockAdaptorEscrow.Swap memory s = _defaultSwap();
        _lock(s);
        vm.warp(s.expiry - 1);
        _claim(s);
        assertEq(_takerReceived(), s.amount);
    }

    function test_claim_revertsOnWrongSigner() public {
        ParalockAdaptorEscrow.Swap memory s = _defaultSwap();
        bytes32 id = _lock(s);

        (uint8 v, bytes32 r, bytes32 sigS) = _sign(uint256(keccak256("someone else")), id);
        vm.expectRevert(ParalockAdaptorEscrow.InvalidSignature.selector);
        escrow.claim(s, v, r, sigS);

        // The maker's own (sender) key is not the signer either.
        (v, r, sigS) = _sign(uint256(keccak256("maker key")), id);
        vm.expectRevert(ParalockAdaptorEscrow.InvalidSignature.selector);
        escrow.claim(s, v, r, sigS);
        assertEq(_status(id), 1);
    }

    function test_claim_revertsOnSignatureForAnotherSwapOrPrefixedDigest() public {
        ParalockAdaptorEscrow.Swap memory s = _defaultSwap();
        _lock(s);
        ParalockAdaptorEscrow.Swap memory t = _swap(keccak256("t"), AMOUNT, s.expiry);
        _lock(t);

        (uint8 v, bytes32 r, bytes32 sigS) = _sign(signerKey, _id(t));
        vm.expectRevert(ParalockAdaptorEscrow.InvalidSignature.selector);
        escrow.claim(s, v, r, sigS);

        // An EIP-191 personal_sign over the digest is not accepted: the digest is signed raw.
        bytes32 prefixed = keccak256(abi.encodePacked("\x19Ethereum Signed Message:\n32", _digest(_id(s))));
        (v, r, sigS) = vm.sign(signerKey, prefixed);
        if (uint256(sigS) > SECP256K1_N / 2) (v, sigS) = (v ^ 1, bytes32(SECP256K1_N - uint256(sigS)));
        vm.expectRevert(ParalockAdaptorEscrow.InvalidSignature.selector);
        escrow.claim(s, v, r, sigS);

        // A garbage signature (r = 0) recovers the zero address, which is never a signer.
        vm.expectRevert(ParalockAdaptorEscrow.InvalidSignature.selector);
        escrow.claim(s, 27, bytes32(0), bytes32(uint256(1)));
    }

    function test_claim_rejectsHighSAndBadV() public {
        ParalockAdaptorEscrow.Swap memory s = _defaultSwap();
        bytes32 id = _lock(s);
        (uint8 v, bytes32 r, bytes32 sigS) = _sign(signerKey, id);

        // The malleated twin (r, n - s, v ^ 1) is a valid ECDSA signature for the same key and digest...
        uint8 highV = v == 27 ? 28 : 27;
        bytes32 highS = bytes32(SECP256K1_N - uint256(sigS));
        assertEq(ecrecover(_digest(id), highV, r, highS), signer);
        // ...but only the canonical low-s form is accepted.
        vm.expectRevert(ParalockAdaptorEscrow.MalleableSignature.selector);
        escrow.claim(s, highV, r, highS);

        vm.expectRevert(ParalockAdaptorEscrow.MalleableSignature.selector);
        escrow.claim(s, v - 27, r, sigS); // raw recovery id 0/1 instead of 27/28

        vm.expectRevert(ParalockAdaptorEscrow.MalleableSignature.selector);
        escrow.claim(s, 29, r, sigS);

        assertEq(_status(id), 1);
        escrow.claim(s, v, r, sigS);
        assertEq(_takerReceived(), s.amount);
    }

    function test_claim_revertsAtAndAfterExpiry() public {
        ParalockAdaptorEscrow.Swap memory s = _defaultSwap();
        bytes32 id = _lock(s);
        (uint8 v, bytes32 r, bytes32 sigS) = _sign(signerKey, id);

        vm.warp(s.expiry);
        vm.expectRevert(abi.encodeWithSelector(ParalockAdaptorEscrow.SwapExpired.selector, s.expiry));
        escrow.claim(s, v, r, sigS);

        vm.warp(s.expiry + 1 days);
        vm.expectRevert(abi.encodeWithSelector(ParalockAdaptorEscrow.SwapExpired.selector, s.expiry));
        escrow.claim(s, v, r, sigS);
    }

    function test_claim_revertsOnDoubleClaim() public {
        ParalockAdaptorEscrow.Swap memory s = _defaultSwap();
        bytes32 id = _lock(s);
        _claim(s);

        (uint8 v, bytes32 r, bytes32 sigS) = _sign(signerKey, id);
        vm.expectRevert(abi.encodeWithSelector(ParalockAdaptorEscrow.SwapSettled.selector, id));
        escrow.claim(s, v, r, sigS);
        assertEq(_takerReceived(), s.amount);
    }

    function test_claim_revertsForUnknownSwap() public {
        ParalockAdaptorEscrow.Swap memory s = _defaultSwap();
        bytes32 id = _id(s);
        (uint8 v, bytes32 r, bytes32 sigS) = _sign(signerKey, id);
        vm.expectRevert(abi.encodeWithSelector(ParalockAdaptorEscrow.SwapNotFound.selector, id));
        escrow.claim(s, v, r, sigS);
    }

    function test_claim_revertsAfterRefund() public {
        ParalockAdaptorEscrow.Swap memory s = _defaultSwap();
        bytes32 id = _lock(s);
        vm.warp(s.expiry);
        escrow.refund(s);

        vm.warp(s.expiry - 1); // even if the clock went backwards, a settled swap stays settled
        (uint8 v, bytes32 r, bytes32 sigS) = _sign(signerKey, id);
        vm.expectRevert(abi.encodeWithSelector(ParalockAdaptorEscrow.SwapSettled.selector, id));
        escrow.claim(s, v, r, sigS);
    }

    /// @dev Swapping in your own signer key and signing for it addresses a different, empty swap.
    function test_claim_revertsWhenSignerIsReplaced() public {
        ParalockAdaptorEscrow.Swap memory s = _defaultSwap();
        _lock(s);

        uint256 attackerKey = uint256(keccak256("attacker"));
        ParalockAdaptorEscrow.Swap memory forged = _defaultSwap();
        forged.signer = vm.addr(attackerKey);
        forged.recipient = relayer;
        bytes32 forgedId = _id(forged);
        (uint8 v, bytes32 r, bytes32 sigS) = _sign(attackerKey, forgedId);

        vm.expectRevert(abi.encodeWithSelector(ParalockAdaptorEscrow.SwapNotFound.selector, forgedId));
        escrow.claim(forged, v, r, sigS);
        assertEq(_escrowed(), s.amount);
    }

    /// @dev Tampering with any field (e.g. redirecting payment, inflating the amount) addresses an empty swap.
    function testFuzz_claimAndRefund_revertOnTamperedParams(uint8 field, uint256 delta) public {
        ParalockAdaptorEscrow.Swap memory s = _defaultSwap();
        bytes32 id = _lock(s);
        (uint8 v, bytes32 r, bytes32 sigS) = _sign(signerKey, id);

        ParalockAdaptorEscrow.Swap memory forged = _defaultSwap();
        delta = bound(delta, 1, type(uint32).max);
        field = uint8(bound(field, 0, 6));
        if (field == 0) forged.token = address(uint160(forged.token) ^ uint160(delta));
        else if (field == 1) forged.sender = relayer;
        else if (field == 2) forged.recipient = relayer;
        else if (field == 3) forged.signer = address(uint160(forged.signer) ^ uint160(delta));
        else if (field == 4) forged.amount += delta;
        else if (field == 5) forged.expiry += uint64(delta);
        else forged.salt = bytes32(uint256(forged.salt) ^ delta);
        bytes32 forgedId = _id(forged);

        vm.expectRevert(abi.encodeWithSelector(ParalockAdaptorEscrow.SwapNotFound.selector, forgedId));
        escrow.claim(forged, v, r, sigS);

        vm.warp(forged.expiry > s.expiry ? forged.expiry : s.expiry);
        vm.expectRevert(abi.encodeWithSelector(ParalockAdaptorEscrow.SwapNotFound.selector, forgedId));
        escrow.refund(forged);

        assertTrue(escrow.isActive(id));
        assertEq(_escrowed(), s.amount);
    }

    // ---------------------------------------------------------------- refund

    function test_refund_returnsFundsToSenderAtExpiry() public {
        ParalockAdaptorEscrow.Swap memory s = _defaultSwap();
        uint256 makerBefore = _balance(maker);
        bytes32 id = _lock(s);

        vm.warp(s.expiry);
        vm.expectEmit(address(escrow));
        emit ParalockAdaptorEscrow.Refunded(id);
        vm.prank(relayer); // anyone may trigger the refund; funds always go to the sender
        escrow.refund(s);

        assertEq(_balance(maker), makerBefore);
        assertEq(_balance(relayer), 0);
        assertEq(_escrowed(), 0);
        assertEq(_status(id), 2);
    }

    function test_refund_revertsBeforeExpiry() public {
        ParalockAdaptorEscrow.Swap memory s = _defaultSwap();
        _lock(s);
        vm.warp(s.expiry - 1);
        vm.expectRevert(abi.encodeWithSelector(ParalockAdaptorEscrow.SwapNotExpired.selector, s.expiry));
        escrow.refund(s);
    }

    function test_refund_revertsAfterClaimAndOnDoubleRefund() public {
        ParalockAdaptorEscrow.Swap memory s = _defaultSwap();
        bytes32 id = _lock(s);
        _claim(s);
        vm.warp(s.expiry);
        vm.expectRevert(abi.encodeWithSelector(ParalockAdaptorEscrow.SwapSettled.selector, id));
        escrow.refund(s);

        ParalockAdaptorEscrow.Swap memory t = _swap(keccak256("t"), AMOUNT, uint64(vm.getBlockTimestamp()) + DURATION);
        bytes32 tid = _lock(t);
        vm.warp(t.expiry);
        escrow.refund(t);
        vm.expectRevert(abi.encodeWithSelector(ParalockAdaptorEscrow.SwapSettled.selector, tid));
        escrow.refund(t);
    }

    function test_refund_revertsForUnknownSwap() public {
        ParalockAdaptorEscrow.Swap memory s = _defaultSwap();
        vm.warp(s.expiry);
        vm.expectRevert(abi.encodeWithSelector(ParalockAdaptorEscrow.SwapNotFound.selector, _id(s)));
        escrow.refund(s);
    }

    // ---------------------------------------------------------------- isolation & fuzz

    function test_concurrentSwapsAreIsolated() public {
        ParalockAdaptorEscrow.Swap memory a = _swap(keccak256("a"), AMOUNT, uint64(vm.getBlockTimestamp()) + DURATION);
        ParalockAdaptorEscrow.Swap memory b =
            _swap(keccak256("b"), AMOUNT * 2, uint64(vm.getBlockTimestamp()) + 2 * DURATION);
        _lock(a);
        bytes32 bid = _lock(b);
        assertEq(_escrowed(), a.amount + b.amount);

        _claim(a);
        assertTrue(escrow.isActive(bid));

        vm.warp(b.expiry);
        uint256 makerBefore = _balance(maker);
        escrow.refund(b);
        assertEq(_balance(maker), makerBefore + b.amount);
        assertEq(_takerReceived(), a.amount);
        assertEq(_escrowed(), 0);
    }

    function testFuzz_lockThenClaim(uint256 key, bytes32 salt, uint256 amount, uint64 duration, uint64 claimAfter)
        public
    {
        signerKey = bound(key, 1, SECP256K1_N - 1);
        signer = vm.addr(signerKey);
        amount = bound(amount, 1, AMOUNT * 4);
        duration = uint64(bound(duration, 1, 365 days));
        claimAfter = uint64(bound(claimAfter, 0, duration - 1));
        ParalockAdaptorEscrow.Swap memory s = _swap(salt, amount, uint64(vm.getBlockTimestamp()) + duration);

        bytes32 id = _lock(s);
        vm.warp(vm.getBlockTimestamp() + claimAfter);
        (uint8 v, bytes32 r, bytes32 sigS) = _sign(signerKey, id);
        vm.prank(relayer);
        escrow.claim(s, v, r, sigS);

        assertEq(_takerReceived(), amount);
        assertEq(_escrowed(), 0);
        assertEq(_status(id), 2);
    }

    function testFuzz_claimRejectsAnyOtherKey(uint256 otherKey) public {
        otherKey = bound(otherKey, 1, SECP256K1_N - 1);
        vm.assume(otherKey != signerKey);
        ParalockAdaptorEscrow.Swap memory s = _defaultSwap();
        bytes32 id = _lock(s);
        (uint8 v, bytes32 r, bytes32 sigS) = _sign(otherKey, id);
        vm.expectRevert(ParalockAdaptorEscrow.InvalidSignature.selector);
        escrow.claim(s, v, r, sigS);
    }

    function testFuzz_lockThenRefund(bytes32 salt, uint256 amount, uint64 duration, uint64 refundAfter) public {
        amount = bound(amount, 1, AMOUNT * 4);
        duration = uint64(bound(duration, 1, 365 days));
        refundAfter = uint64(bound(refundAfter, duration, uint256(duration) + 365 days));
        ParalockAdaptorEscrow.Swap memory s = _swap(salt, amount, uint64(vm.getBlockTimestamp()) + duration);
        uint256 makerBefore = _balance(maker);

        bytes32 id = _lock(s);
        vm.warp(vm.getBlockTimestamp() + refundAfter);
        (uint8 v, bytes32 r, bytes32 sigS) = _sign(signerKey, id);
        vm.expectRevert(abi.encodeWithSelector(ParalockAdaptorEscrow.SwapExpired.selector, s.expiry));
        escrow.claim(s, v, r, sigS);
        escrow.refund(s);

        assertEq(_balance(maker), makerBefore);
        assertEq(_escrowed(), 0);
    }

    function testFuzz_lockRejectsNonFutureExpiry(uint64 expiry) public {
        expiry = uint64(bound(expiry, 0, vm.getBlockTimestamp()));
        ParalockAdaptorEscrow.Swap memory s = _swap(SALT, AMOUNT, expiry);
        vm.expectRevert(ParalockAdaptorEscrow.InvalidExpiry.selector);
        _lock(s);
    }
}
