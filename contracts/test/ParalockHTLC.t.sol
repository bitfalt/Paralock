// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {HTLCTestBase} from "./HTLCTestBase.sol";
import {ParalockHTLC} from "../src/ParalockHTLC.sol";
import {MockERC20, FeeOnTransferERC20, FalseReturnERC20, NoReturnERC20} from "./mocks/MockERC20.sol";

/// @notice Full behavioral suite on a plain ERC-20 (the non-TIP-20 code path), plus non-standard token handling.
contract ParalockHTLCERC20Test is HTLCTestBase {
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
        MockERC20(token).approve(address(htlc), AMOUNT);

        ParalockHTLC.Swap memory s = _defaultSwap();
        vm.expectRevert(abi.encodeWithSelector(ParalockHTLC.UnsupportedToken.selector, token));
        _lock(s);
    }

    function test_lock_rejectsFalseReturningToken() public {
        token = address(new FalseReturnERC20());
        ParalockHTLC.Swap memory s = _defaultSwap();
        vm.expectRevert(abi.encodeWithSelector(ParalockHTLC.TokenTransferFailed.selector, token));
        _lock(s);
    }

    function test_lock_rejectsTokenWithoutCode() public {
        token = makeAddr("not a token");
        ParalockHTLC.Swap memory s = _defaultSwap();
        vm.expectRevert();
        _lock(s);
    }

    function test_lockClaimRefund_supportNoReturnToken() public {
        token = address(new NoReturnERC20());
        _fund(maker, AMOUNT * 2);
        vm.prank(maker);
        MockERC20(token).approve(address(htlc), AMOUNT * 2);

        ParalockHTLC.Swap memory s = _defaultSwap();
        _lock(s);
        htlc.claim(s, SECRET);
        assertEq(_takerReceived(), AMOUNT);

        ParalockHTLC.Swap memory t = _swap(keccak256("t"), AMOUNT, uint64(vm.getBlockTimestamp()) + DURATION);
        _lock(t);
        vm.warp(t.expiry);
        htlc.refund(t);
        assertEq(_balance(maker), AMOUNT);
    }

    function test_lock_bubblesTokenRevertReason() public {
        vm.prank(maker);
        MockERC20(token).approve(address(htlc), 0);
        ParalockHTLC.Swap memory s = _defaultSwap();
        vm.expectRevert(bytes("MockERC20: allowance"));
        _lock(s);
    }
}
