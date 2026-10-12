// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

/// @notice Minimal ERC-20 for unit tests. Subclasses override `_move` to model non-standard tokens.
contract MockERC20 {
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    event Transfer(address indexed from, address indexed to, uint256 amount);
    event Approval(address indexed owner, address indexed spender, uint256 amount);

    function mint(address to, uint256 amount) external {
        balanceOf[to] += amount;
        emit Transfer(address(0), to, amount);
    }

    function approve(address spender, uint256 amount) external returns (bool) {
        allowance[msg.sender][spender] = amount;
        emit Approval(msg.sender, spender, amount);
        return true;
    }

    function transfer(address to, uint256 amount) external virtual returns (bool) {
        _move(msg.sender, to, amount);
        return true;
    }

    function transferFrom(address from, address to, uint256 amount) external virtual returns (bool) {
        uint256 allowed = allowance[from][msg.sender];
        if (allowed != type(uint256).max) {
            require(allowed >= amount, "MockERC20: allowance");
            allowance[from][msg.sender] = allowed - amount;
        }
        _move(from, to, amount);
        return true;
    }

    function _move(address from, address to, uint256 amount) internal virtual {
        require(balanceOf[from] >= amount, "MockERC20: balance");
        balanceOf[from] -= amount;
        balanceOf[to] += amount;
        emit Transfer(from, to, amount);
    }
}

/// @notice Burns 1% of every transfer, so the recipient receives less than `amount`.
contract FeeOnTransferERC20 is MockERC20 {
    function _move(address from, address to, uint256 amount) internal override {
        uint256 fee = amount / 100;
        super._move(from, to, amount - fee);
        if (fee > 0) super._move(from, address(0xdead), fee);
    }
}

/// @notice Signals failure by returning false instead of reverting.
contract FalseReturnERC20 is MockERC20 {
    function transfer(address, uint256) external pure override returns (bool) {
        return false;
    }

    function transferFrom(address, address, uint256) external pure override returns (bool) {
        return false;
    }
}

/// @notice USDT-style token whose transfer functions return nothing.
contract NoReturnERC20 is MockERC20 {
    function transfer(address to, uint256 amount) external override returns (bool) {
        _move(msg.sender, to, amount);
        assembly {
            return(0, 0)
        }
    }

    function transferFrom(address from, address to, uint256 amount) external override returns (bool) {
        uint256 allowed = allowance[from][msg.sender];
        require(allowed >= amount, "NoReturnERC20: allowance");
        allowance[from][msg.sender] = allowed - amount;
        _move(from, to, amount);
        assembly {
            return(0, 0)
        }
    }
}
