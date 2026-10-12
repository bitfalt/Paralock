// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {MockUSD} from "../src/MockUSD.sol";

contract MockUSDTest is Test {
    MockUSD internal usd;
    address internal alice = makeAddr("alice");
    address internal bob = makeAddr("bob");

    function setUp() public {
        usd = new MockUSD();
    }

    function test_metadata() public view {
        assertEq(usd.symbol(), "USDC");
        assertEq(usd.decimals(), 6);
        assertEq(usd.minter(), address(this));
    }

    function test_mint_onlyMinter() public {
        usd.mint(alice, 100e6);
        assertEq(usd.balanceOf(alice), 100e6);
        assertEq(usd.totalSupply(), 100e6);

        vm.prank(alice);
        vm.expectRevert(MockUSD.NotMinter.selector);
        usd.mint(alice, 1);
    }

    function test_transferAndTransferFrom() public {
        usd.mint(alice, 100e6);
        vm.prank(alice);
        assertTrue(usd.transfer(bob, 40e6));
        assertEq(usd.balanceOf(alice), 60e6);
        assertEq(usd.balanceOf(bob), 40e6);

        vm.prank(alice);
        usd.approve(address(this), 10e6);
        assertTrue(usd.transferFrom(alice, bob, 10e6));
        assertEq(usd.allowance(alice, address(this)), 0);
        assertEq(usd.balanceOf(bob), 50e6);

        vm.expectRevert(MockUSD.InsufficientAllowance.selector);
        usd.transferFrom(alice, bob, 1);

        vm.prank(bob);
        vm.expectRevert(MockUSD.InsufficientBalance.selector);
        usd.transfer(alice, 50e6 + 1);
    }

    function test_infiniteAllowanceIsNotDecremented() public {
        usd.mint(alice, 100e6);
        vm.prank(alice);
        usd.approve(address(this), type(uint256).max);
        usd.transferFrom(alice, bob, 30e6);
        assertEq(usd.allowance(alice, address(this)), type(uint256).max);
    }
}
