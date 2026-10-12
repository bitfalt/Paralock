// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {HTLCTestBase} from "./HTLCTestBase.sol";
import {ParalockHTLC} from "../src/ParalockHTLC.sol";
import {ITIP20} from "tempo-std/interfaces/ITIP20.sol";
import {StdTokens} from "tempo-std/StdTokens.sol";

/// @notice Full behavioral suite on a real TIP-20 precompile (AlphaUSD) in forge's Tempo EVM (`network = "tempo"`),
///         plus the memo tagging that only TIP-20 supports.
contract ParalockHTLCTIP20Test is HTLCTestBase {
    function _deployToken() internal virtual override returns (address) {
        return StdTokens.ALPHA_USD_ADDRESS;
    }

    /// @dev Forge's Tempo genesis pre-funds the test contract with every dev stablecoin.
    function _fund(address to, uint256 amount) internal virtual override {
        ITIP20(token).transfer(to, amount);
    }

    function test_tip20_everyTransferIsTaggedWithHashlock() public {
        ParalockHTLC.Swap memory s = _defaultSwap();

        vm.expectEmit(token);
        emit ITIP20.TransferWithMemo(maker, address(htlc), s.amount, s.hashlock);
        _lock(s);

        vm.expectEmit(token);
        emit ITIP20.TransferWithMemo(address(htlc), taker, s.amount, s.hashlock);
        htlc.claim(s, SECRET);

        ParalockHTLC.Swap memory t = _swap(keccak256("t"), AMOUNT, uint64(vm.getBlockTimestamp()) + DURATION);
        _lock(t);
        vm.warp(t.expiry);
        vm.expectEmit(token);
        emit ITIP20.TransferWithMemo(address(htlc), maker, t.amount, t.hashlock);
        htlc.refund(t);
    }

    function test_tip20_lockBubblesPrecompileError() public {
        vm.prank(maker);
        ITIP20(token).approve(address(htlc), 0);
        ParalockHTLC.Swap memory s = _defaultSwap();
        vm.expectRevert(ITIP20.InsufficientAllowance.selector);
        _lock(s);
    }

    function test_tip20_lockRevertsForUncreatedToken() public {
        token = address(0x20C0000000000000000000000000000000DeaD01);
        ParalockHTLC.Swap memory s = _defaultSwap();
        vm.expectRevert();
        _lock(s);
    }
}

/// @notice The same suite against a live Tempo node (anvil `--network tempo` or Moderato) with its real AlphaUSD.
///         Skipped unless `TEMPO_FORK_URL` is set, e.g. `TEMPO_FORK_URL=http://localhost:8545 forge test`.
contract ParalockHTLCTempoForkTest is ParalockHTLCTIP20Test {
    /// @dev Anvil dev account 0, which holds u64::MAX of every dev stablecoin on a local Tempo chain.
    address internal constant DEV0 = 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266;

    function setUp() public override {
        string memory forkUrl = vm.envOr("TEMPO_FORK_URL", string(""));
        if (bytes(forkUrl).length == 0) {
            vm.skip(true);
            return;
        }
        vm.createSelectFork(forkUrl);
        super.setUp();
    }

    function _fund(address to, uint256 amount) internal override {
        vm.prank(DEV0);
        ITIP20(token).transfer(to, amount);
    }
}
