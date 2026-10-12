// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

/// @title MockUSD
/// @author Paralock
/// @notice A 6-decimal "USDC" stand-in for local Base / Ethereum devnets (anvil). Plain ERC-20; only the deployer can
///         mint. Never deploy it to a public network.
contract MockUSD {
    string public constant name = "USD Coin (Paralock local)";
    string public constant symbol = "USDC";
    uint8 public constant decimals = 6;

    /// @notice The only account allowed to mint.
    address public immutable minter;

    uint256 public totalSupply;
    mapping(address account => uint256) public balanceOf;
    mapping(address owner => mapping(address spender => uint256)) public allowance;

    event Transfer(address indexed from, address indexed to, uint256 value);
    event Approval(address indexed owner, address indexed spender, uint256 value);

    error NotMinter();
    error InsufficientBalance();
    error InsufficientAllowance();
    error ZeroAddress();

    constructor() {
        minter = msg.sender;
    }

    function mint(address to, uint256 amount) external {
        if (msg.sender != minter) revert NotMinter();
        if (to == address(0)) revert ZeroAddress();
        totalSupply += amount;
        unchecked {
            balanceOf[to] += amount; // bounded by totalSupply
        }
        emit Transfer(address(0), to, amount);
    }

    function approve(address spender, uint256 amount) external returns (bool) {
        allowance[msg.sender][spender] = amount;
        emit Approval(msg.sender, spender, amount);
        return true;
    }

    function transfer(address to, uint256 amount) external returns (bool) {
        _transfer(msg.sender, to, amount);
        return true;
    }

    function transferFrom(address from, address to, uint256 amount) external returns (bool) {
        uint256 allowed = allowance[from][msg.sender];
        if (allowed != type(uint256).max) {
            if (allowed < amount) revert InsufficientAllowance();
            unchecked {
                allowance[from][msg.sender] = allowed - amount;
            }
        }
        _transfer(from, to, amount);
        return true;
    }

    function _transfer(address from, address to, uint256 amount) private {
        if (to == address(0)) revert ZeroAddress();
        uint256 balance = balanceOf[from];
        if (balance < amount) revert InsufficientBalance();
        unchecked {
            balanceOf[from] = balance - amount;
            balanceOf[to] += amount; // bounded by totalSupply
        }
        emit Transfer(from, to, amount);
    }
}
