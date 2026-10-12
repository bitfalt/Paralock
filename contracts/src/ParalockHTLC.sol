// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {ITIP20} from "tempo-std/interfaces/ITIP20.sol";

/// @title ParalockHTLC
/// @author Paralock
/// @notice Hash-time-locked escrow for TIP-20 stablecoins on Tempo: the EVM leg of a Zcash <-> stablecoin atomic swap.
/// @dev Protocol: the maker (`sender`) locks `amount` of `token` for the taker (`recipient`) under
///      `hashlock = sha256(secret)`, the same hashlock as the taker's Zcash P2SH HTLC. Anyone may `claim` with the
///      32-byte secret before `expiry`, paying the recipient and publishing the secret in `Claimed` so the maker can
///      redeem the ZEC. From `expiry` on, anyone may `refund` the sender.
///
///      Storage is one slot per swap (state creation on Tempo costs ~250k gas per slot, TIP-1000):
///      `hashlock => keccak256(abi.encode(swap))` while active, then `SETTLED` forever. Callers pass the full
///      `Swap` back on claim/refund; `Locked` carries every field so watchers can rebuild it. A hashlock can never be
///      reused, because its preimage becomes public on claim.
///
///      Token transfers carry `memo = hashlock` on TIP-20 tokens (`transferFromWithMemo` / `transferWithMemo`), so
///      explorers and reconcilers can link every movement of funds to its swap. Other ERC-20s use plain transfers
///      with return-value and balance-delta checks; fee-on-transfer and rebasing tokens are rejected or unsupported.
contract ParalockHTLC {
    /// @notice Every parameter of a swap. Its keccak256 commitment is all the contract stores.
    /// @param hashlock sha256 of the 32-byte secret; unique key of the swap.
    /// @param token TIP-20 (or ERC-20) token held in escrow.
    /// @param sender Party that locks the funds and is refunded after expiry.
    /// @param recipient Party paid on claim.
    /// @param amount Token amount in base units.
    /// @param expiry Unix timestamp (seconds) from which the swap can no longer be claimed, only refunded.
    struct Swap {
        bytes32 hashlock;
        address token;
        address sender;
        address recipient;
        uint256 amount;
        uint64 expiry;
    }

    /// @notice Lifecycle of a hashlock as seen by `statusOf`.
    enum Status {
        Empty,
        Active,
        Settled
    }

    /// @notice State value of a hashlock whose swap was claimed or refunded. Never a keccak256 output in practice.
    bytes32 public constant SETTLED = bytes32(uint256(1));

    /// @dev Every TIP-20 token lives at `0x20C0 0000 0000 0000 0000 0000 ‖ 8-byte id`; the prefix is reserved for
    ///      protocol precompiles, so it identifies TIP-20 tokens without an external call.
    bytes12 private constant TIP20_PREFIX = 0x20c000000000000000000000;

    /// @dev hashlock => swap commitment (active) | SETTLED (claimed or refunded) | 0 (never used).
    mapping(bytes32 hashlock => bytes32 state) private _states;

    /// @notice Funds were locked. Carries every `Swap` field so the struct can be rebuilt from the log alone.
    event Locked(
        bytes32 indexed hashlock,
        address indexed sender,
        address indexed recipient,
        address token,
        uint256 amount,
        uint64 expiry
    );

    /// @notice The swap was claimed and the recipient paid. `preimage` is the secret that unlocks the Zcash leg.
    event Claimed(bytes32 indexed hashlock, bytes32 preimage);

    /// @notice The swap expired and the sender was refunded.
    event Refunded(bytes32 indexed hashlock);

    /// @notice `msg.sender` is not the swap's sender.
    error NotSender();
    /// @notice The hashlock is zero.
    error InvalidHashlock();
    /// @notice The amount is zero.
    error InvalidAmount();
    /// @notice The recipient is the zero address.
    error InvalidRecipient();
    /// @notice The expiry is not in the future.
    error InvalidExpiry();
    /// @notice The hashlock was already used by an active or settled swap.
    error HashlockUsed(bytes32 hashlock);
    /// @notice No swap was ever locked under this hashlock.
    error SwapNotFound(bytes32 hashlock);
    /// @notice The swap under this hashlock was already claimed or refunded.
    error SwapSettled(bytes32 hashlock);
    /// @notice The provided swap parameters do not match the locked swap.
    error SwapMismatch(bytes32 hashlock);
    /// @notice sha256(preimage) is not the swap's hashlock.
    error InvalidPreimage();
    /// @notice Claim attempted at or after expiry.
    error SwapExpired(uint64 expiry);
    /// @notice Refund attempted before expiry.
    error SwapNotExpired(uint64 expiry);
    /// @notice The token call failed without a reason, or returned false.
    error TokenTransferFailed(address token);
    /// @notice The escrow received a different amount than requested (fee-on-transfer or rebasing token).
    error UnsupportedToken(address token);

    /// @notice Lock `s.amount` of `s.token` under `s.hashlock`. The caller must be `s.sender` and must have approved
    ///         this contract for at least `s.amount`.
    /// @param s The swap to open. `s.expiry` must be in the future.
    function lock(Swap calldata s) external {
        if (msg.sender != s.sender) revert NotSender();
        if (s.hashlock == bytes32(0)) revert InvalidHashlock();
        if (s.amount == 0) revert InvalidAmount();
        if (s.recipient == address(0)) revert InvalidRecipient();
        if (s.expiry <= block.timestamp) revert InvalidExpiry();
        if (_states[s.hashlock] != bytes32(0)) revert HashlockUsed(s.hashlock);

        _states[s.hashlock] = commitment(s);
        emit Locked(s.hashlock, s.sender, s.recipient, s.token, s.amount, s.expiry);

        _pull(s);
    }

    /// @notice Pay `s.recipient` by revealing the secret. Callable by anyone before `s.expiry`.
    /// @param s The locked swap, exactly as emitted in `Locked`.
    /// @param preimage The 32-byte secret with `sha256(preimage) == s.hashlock`.
    function claim(Swap calldata s, bytes32 preimage) external {
        if (sha256(abi.encodePacked(preimage)) != s.hashlock) revert InvalidPreimage();
        _requireActive(s);
        if (block.timestamp >= s.expiry) revert SwapExpired(s.expiry);

        _states[s.hashlock] = SETTLED;
        emit Claimed(s.hashlock, preimage);

        _push(s, s.recipient);
    }

    /// @notice Return the funds to `s.sender`. Callable by anyone from `s.expiry` on.
    /// @param s The locked swap, exactly as emitted in `Locked`.
    function refund(Swap calldata s) external {
        _requireActive(s);
        if (block.timestamp < s.expiry) revert SwapNotExpired(s.expiry);

        _states[s.hashlock] = SETTLED;
        emit Refunded(s.hashlock);

        _push(s, s.sender);
    }

    /// @notice Raw state of a hashlock: 0 if unused, the swap commitment if active, `SETTLED` once claimed/refunded.
    function stateOf(bytes32 hashlock) external view returns (bytes32) {
        return _states[hashlock];
    }

    /// @notice Lifecycle status of a hashlock.
    function statusOf(bytes32 hashlock) public view returns (Status) {
        bytes32 state = _states[hashlock];
        if (state == bytes32(0)) return Status.Empty;
        if (state == SETTLED) return Status.Settled;
        return Status.Active;
    }

    /// @notice True if a swap is locked under `hashlock` and not yet claimed or refunded.
    function isActive(bytes32 hashlock) external view returns (bool) {
        return statusOf(hashlock) == Status.Active;
    }

    /// @notice The value stored for an active swap: keccak256 of its ABI encoding.
    function commitment(Swap calldata s) public pure returns (bytes32) {
        return keccak256(abi.encode(s));
    }

    /// @dev Reverts unless `s` matches the active swap under its hashlock, with the most specific error.
    function _requireActive(Swap calldata s) private view {
        bytes32 state = _states[s.hashlock];
        if (state == commitment(s)) return;
        if (state == bytes32(0)) revert SwapNotFound(s.hashlock);
        if (state == SETTLED) revert SwapSettled(s.hashlock);
        revert SwapMismatch(s.hashlock);
    }

    /// @dev Moves `s.amount` from the sender into escrow. TIP-20 transfers are exact and tagged with the hashlock;
    ///      for other tokens the received amount is checked, so a swap can never be paid out of other swaps' escrow.
    function _pull(Swap calldata s) private {
        if (_isTIP20(s.token)) {
            _call(s.token, abi.encodeCall(ITIP20.transferFromWithMemo, (s.sender, address(this), s.amount, s.hashlock)));
            return;
        }
        uint256 balanceBefore = ITIP20(s.token).balanceOf(address(this));
        _call(s.token, abi.encodeCall(ITIP20.transferFrom, (s.sender, address(this), s.amount)));
        // forge-lint: disable-next-line(incorrect-strict-equality)
        if (ITIP20(s.token).balanceOf(address(this)) - balanceBefore != s.amount) revert UnsupportedToken(s.token);
    }

    /// @dev Pays `s.amount` out of escrow to `to`, tagged with the hashlock on TIP-20.
    function _push(Swap calldata s, address to) private {
        _call(
            s.token,
            _isTIP20(s.token)
                ? abi.encodeCall(ITIP20.transferWithMemo, (to, s.amount, s.hashlock))
                : abi.encodeCall(ITIP20.transfer, (to, s.amount))
        );
    }

    /// @dev SafeERC20-style call: bubbles token reverts, accepts no return data only from contracts (TIP-20
    ///      `transferWithMemo` returns nothing; TIP-20 precompiles carry `0xef` code), and rejects a `false` return.
    function _call(address token, bytes memory data) private {
        (bool ok, bytes memory ret) = token.call(data);
        if (!ok) {
            if (ret.length == 0) revert TokenTransferFailed(token);
            assembly ("memory-safe") {
                revert(add(ret, 0x20), mload(ret))
            }
        }
        if (ret.length == 0 ? token.code.length == 0 : (ret.length < 32 || abi.decode(ret, (uint256)) != 1)) {
            revert TokenTransferFailed(token);
        }
    }

    /// @dev True if `token` is in Tempo's reserved TIP-20 address range.
    function _isTIP20(address token) private pure returns (bool) {
        return bytes12(bytes20(token)) == TIP20_PREFIX;
    }
}
