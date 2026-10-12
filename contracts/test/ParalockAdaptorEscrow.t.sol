// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {AdaptorTestBase} from "./AdaptorTestBase.sol";
import {ParalockAdaptorEscrow} from "../src/ParalockAdaptorEscrow.sol";
import {MockUSD} from "../src/MockUSD.sol";
import {MockERC20, FeeOnTransferERC20, FalseReturnERC20, NoReturnERC20} from "./mocks/MockERC20.sol";
import {ITIP20} from "tempo-std/interfaces/ITIP20.sol";
import {StdTokens} from "tempo-std/StdTokens.sol";

/// @notice Full adaptor-escrow suite on a real TIP-20 precompile (AlphaUSD) in forge's Tempo EVM
///         (`network = "tempo"`), plus the memo tagging that only TIP-20 supports.
contract ParalockAdaptorEscrowTIP20Test is AdaptorTestBase {
    function _deployToken() internal pure override returns (address) {
        return StdTokens.ALPHA_USD_ADDRESS;
    }

    /// @dev Forge's Tempo genesis pre-funds the test contract with every dev stablecoin.
    function _fund(address to, uint256 amount) internal override {
        ITIP20(token).transfer(to, amount);
    }

    function test_tip20_everyTransferIsTaggedWithId() public {
        ParalockAdaptorEscrow.Swap memory s = _defaultSwap();
        bytes32 id = _id(s);

        vm.expectEmit(token);
        emit ITIP20.TransferWithMemo(maker, address(escrow), s.amount, id);
        _lock(s);

        vm.expectEmit(token);
        emit ITIP20.TransferWithMemo(address(escrow), taker, s.amount, id);
        _claim(s);

        ParalockAdaptorEscrow.Swap memory t = _swap(keccak256("t"), AMOUNT, uint64(vm.getBlockTimestamp()) + DURATION);
        bytes32 tid = _lock(t);
        vm.warp(t.expiry);
        vm.expectEmit(token);
        emit ITIP20.TransferWithMemo(address(escrow), maker, t.amount, tid);
        escrow.refund(t);
    }

    function test_tip20_lockBubblesPrecompileError() public {
        vm.prank(maker);
        ITIP20(token).approve(address(escrow), 0);
        ParalockAdaptorEscrow.Swap memory s = _defaultSwap();
        vm.expectRevert(ITIP20.InsufficientAllowance.selector);
        _lock(s);
    }
}

/// @notice Full adaptor-escrow suite on MockUSD, the plain ERC-20 "USDC" of the local Base / Ethereum devnets.
contract ParalockAdaptorEscrowMockUSDTest is AdaptorTestBase {
    function _deployToken() internal override returns (address) {
        return address(new MockUSD());
    }

    function _fund(address to, uint256 amount) internal override {
        MockUSD(token).mint(to, amount);
    }
}

/// @notice Non-standard ERC-20 handling (the non-TIP-20 code path).
contract ParalockAdaptorEscrowERC20Test is AdaptorTestBase {
    function _deployToken() internal override returns (address) {
        return address(new MockERC20());
    }

    function _fund(address to, uint256 amount) internal override {
        MockERC20(token).mint(to, amount);
    }

    function test_lock_rejectsFeeOnTransferToken() public {
        token = address(new FeeOnTransferERC20());
        _fund(maker, AMOUNT);
        vm.prank(maker);
        MockERC20(token).approve(address(escrow), AMOUNT);

        ParalockAdaptorEscrow.Swap memory s = _defaultSwap();
        vm.expectRevert(abi.encodeWithSelector(ParalockAdaptorEscrow.UnsupportedToken.selector, token));
        _lock(s);
    }

    function test_lock_rejectsFalseReturningToken() public {
        token = address(new FalseReturnERC20());
        ParalockAdaptorEscrow.Swap memory s = _defaultSwap();
        vm.expectRevert(abi.encodeWithSelector(ParalockAdaptorEscrow.TokenTransferFailed.selector, token));
        _lock(s);
    }

    function test_lock_rejectsTokenWithoutCode() public {
        token = makeAddr("not a token");
        ParalockAdaptorEscrow.Swap memory s = _defaultSwap();
        vm.expectRevert();
        _lock(s);
    }

    function test_lockClaimRefund_supportNoReturnToken() public {
        token = address(new NoReturnERC20());
        _fund(maker, AMOUNT * 2);
        vm.prank(maker);
        MockERC20(token).approve(address(escrow), AMOUNT * 2);
        takerBaseline = 0;
        escrowBaseline = 0;

        ParalockAdaptorEscrow.Swap memory s = _defaultSwap();
        _lock(s);
        _claim(s);
        assertEq(_takerReceived(), AMOUNT);

        ParalockAdaptorEscrow.Swap memory t = _swap(keccak256("t"), AMOUNT, uint64(vm.getBlockTimestamp()) + DURATION);
        _lock(t);
        vm.warp(t.expiry);
        escrow.refund(t);
        assertEq(_balance(maker), AMOUNT);
    }
}
