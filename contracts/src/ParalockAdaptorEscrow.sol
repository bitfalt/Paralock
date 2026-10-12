// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {ITIP20} from "tempo-std/interfaces/ITIP20.sol";

/// @title ParalockAdaptorEscrow
/// @author Paralock
/// @notice Signature-locked escrow for TIP-20 / ERC-20 stablecoins: the EVM leg of a scriptless (adaptor-signature)
///         Zcash <-> stablecoin swap. Unlike ParalockHTLC, nothing on-chain links the two legs: there is no hashlock,
///         only a one-time `signer` address and a random `salt`.
/// @dev Protocol: the maker (`sender`) locks `amount` of `token` for the taker (`recipient`). The funds are released
///      to the recipient by a plain ECDSA signature from `signer` over `claimDigest(id)`, submitted by anyone before
///      `expiry`. Off-chain, the taker only holds an adaptor pre-signature for that digest; completing it requires the
///      adaptor secret, and the completed `(r, s)` published in `Claimed` lets the maker extract that secret and
///      finish the Zcash leg. From `expiry` on, anyone may `refund` the sender.
///
///      Storage is one slot per swap (state creation on Tempo costs ~250k gas per slot, TIP-1000):
///      `id = keccak256(abi.encode(swap)) => Status`. Callers pass the full `Swap` back on claim/refund and the
///      contract recomputes the id, so tampering with any field addresses a different (empty) swap. `Locked` carries
///      every field so watchers can rebuild the struct. Ids are never reusable: a settled swap stays settled forever.
///
///      The claim signature must be canonical (low `s`, `v` in {27, 28}), so the published `(r, s)` is exactly the
///      completed adaptor signature and cannot be swapped for its malleated twin.
///
///      Token transfers carry `memo = id` on TIP-20 tokens (`transferFromWithMemo` / `transferWithMemo`), exactly as
///      ParalockHTLC tags them with the hashlock. Other ERC-20s use plain transfers with return-value and
///      balance-delta checks; fee-on-transfer and rebasing tokens are rejected or unsupported.
contract ParalockAdaptorEscrow {
    /// @notice Every parameter of a swap. Its keccak256 hash is the swap id.
    /// @param token TIP-20 (or ERC-20) token held in escrow.
    /// @param sender Party that locks the funds and is refunded after expiry.
    /// @param recipient Party paid on claim.
    /// @param signer Address whose signature over `claimDigest(id)` releases the funds.
    /// @param amount Token amount in base units.
    /// @param expiry Unix timestamp (seconds) from which the swap can no longer be claimed, only refunded.
    /// @param salt Random value that makes the id unique and unlinkable across swaps.
    struct Swap {
        address token;
        address sender;
        address recipient;
        address signer;
        uint256 amount;
        uint64 expiry;
        bytes32 salt;
    }

    /// @notice Lifecycle of a swap id. Stored as 0, 1, 2.
    enum Status {
        Empty,
        Active,
        Settled
    }

    /// @notice Domain tag of the claim digest.
    bytes32 public constant CLAIM_TYPEHASH = keccak256("ParalockAdaptorClaim");

    /// @dev secp256k1 group order / 2: the largest canonical (low) `s`.
    uint256 private constant SECP256K1_N_HALF = 0x7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF5D576E7357A4501DDFE92F46681B20A0;

    /// @dev Every TIP-20 token lives at `0x20C0 0000 0000 0000 0000 0000 ‖ 8-byte id`; the prefix is reserved for
    ///      protocol precompiles, so it identifies TIP-20 tokens without an external call.
    bytes12 private constant TIP20_PREFIX = 0x20c000000000000000000000;

    /// @dev id => status. The only storage the contract writes.
    mapping(bytes32 id => Status) private _states;

    /// @notice Funds were locked. Carries every `Swap` field so the struct can be rebuilt from the log alone.
    event Locked(
        bytes32 indexed id,
        address indexed sender,
        address indexed recipient,
        address token,
        address signer,
        uint256 amount,
        uint64 expiry,
        bytes32 salt
    );

    /// @notice The swap was claimed and the recipient paid. `(r, s)` is the completed adaptor signature, from which
    ///         the maker extracts the adaptor secret.
    event Claimed(bytes32 indexed id, uint8 v, bytes32 r, bytes32 s);

    /// @notice The swap expired and the sender was refunded.
    event Refunded(bytes32 indexed id);

    /// @notice `msg.sender` is not the swap's sender.
    error NotSender();
    /// @notice The amount is zero.
    error InvalidAmount();
    /// @notice The recipient is the zero address.
    error InvalidRecipient();
    /// @notice The signer is the zero address.
    error InvalidSigner();
    /// @notice The expiry is not in the future.
    error InvalidExpiry();
    /// @notice A swap with this id was already locked (it is active or settled).
    error SwapExists(bytes32 id);
    /// @notice No swap was ever locked under this id (unknown or tampered parameters).
    error SwapNotFound(bytes32 id);
    /// @notice The swap was already claimed or refunded.
    error SwapSettled(bytes32 id);
    /// @notice Claim attempted at or after expiry.
    error SwapExpired(uint64 expiry);
    /// @notice Refund attempted before expiry.
    error SwapNotExpired(uint64 expiry);
    /// @notice The signature does not recover to the swap's signer.
    error InvalidSignature();
    /// @notice The signature is not canonical: `s` is in the upper half of the curve order, or `v` is not 27/28.
    error MalleableSignature();
    /// @notice The token call failed without a reason, or returned false.
    error TokenTransferFailed(address token);
    /// @notice The escrow received a different amount than requested (fee-on-transfer or rebasing token).
    error UnsupportedToken(address token);

    /// @notice Lock `s.amount` of `s.token` for `s.recipient`. The caller must be `s.sender` and must have approved
    ///         this contract for at least `s.amount`.
    /// @param s The swap to open. `s.expiry` must be in the future; `s.salt` should be random.
    /// @return id The swap id, `keccak256(abi.encode(s))`.
    function lock(Swap calldata s) external returns (bytes32 id) {
        if (msg.sender != s.sender) revert NotSender();
        if (s.amount == 0) revert InvalidAmount();
        if (s.recipient == address(0)) revert InvalidRecipient();
        if (s.signer == address(0)) revert InvalidSigner();
        if (s.expiry <= block.timestamp) revert InvalidExpiry();
        id = idOf(s);
        if (_states[id] != Status.Empty) revert SwapExists(id);

        _states[id] = Status.Active;
        emit Locked(id, s.sender, s.recipient, s.token, s.signer, s.amount, s.expiry, s.salt);

        _pull(s.token, s.sender, s.amount, id);
    }

    /// @notice Pay `s.recipient` with `s.signer`'s signature over `claimDigest(id)`. Callable by anyone (e.g. a
    ///         relayer) before `s.expiry`.
    /// @param s The locked swap, exactly as emitted in `Locked`.
    /// @param v Recovery id, 27 or 28.
    /// @param r Signature `r`.
    /// @param sigS Signature `s`, in the lower half of the curve order.
    function claim(Swap calldata s, uint8 v, bytes32 r, bytes32 sigS) external {
        bytes32 id = _requireActive(s);
        if (block.timestamp >= s.expiry) revert SwapExpired(s.expiry);
        if (uint256(sigS) > SECP256K1_N_HALF || (v != 27 && v != 28)) revert MalleableSignature();
        // `s.signer` is never zero (checked on lock), so ecrecover's zero-address failure value cannot match it.
        if (ecrecover(claimDigest(id), v, r, sigS) != s.signer) revert InvalidSignature();

        _states[id] = Status.Settled;
        emit Claimed(id, v, r, sigS);

        _push(s.token, s.recipient, s.amount, id);
    }

    /// @notice Return the funds to `s.sender`. Callable by anyone from `s.expiry` on.
    /// @param s The locked swap, exactly as emitted in `Locked`.
    function refund(Swap calldata s) external {
        bytes32 id = _requireActive(s);
        if (block.timestamp < s.expiry) revert SwapNotExpired(s.expiry);

        _states[id] = Status.Settled;
        emit Refunded(id);

        _push(s.token, s.sender, s.amount, id);
    }

    /// @notice The 32-byte digest `s.signer` signs (raw ECDSA, no EIP-191 prefix) to release swap `id`. Bound to this
    ///         chain and this contract, so a signature can never be replayed elsewhere.
    function claimDigest(bytes32 id) public view returns (bytes32) {
        return keccak256(abi.encode(CLAIM_TYPEHASH, block.chainid, address(this), id));
    }

    /// @notice The id of a swap: keccak256 of its ABI encoding.
    function idOf(Swap calldata s) public pure returns (bytes32) {
        return keccak256(abi.encode(s));
    }

    /// @notice Lifecycle status of a swap id.
    function statusOf(bytes32 id) external view returns (Status) {
        return _states[id];
    }

    /// @notice True if `id` is locked and not yet claimed or refunded.
    function isActive(bytes32 id) external view returns (bool) {
        return _states[id] == Status.Active;
    }

    /// @dev Returns the id of `s`, reverting unless it is active.
    function _requireActive(Swap calldata s) private view returns (bytes32 id) {
        id = idOf(s);
        Status status = _states[id];
        if (status == Status.Active) return id;
        if (status == Status.Empty) revert SwapNotFound(id);
        revert SwapSettled(id);
    }

    /// @dev Moves `amount` from `from` into escrow. TIP-20 transfers are exact and tagged with the id; for other
    ///      tokens the received amount is checked, so a swap can never be paid out of other swaps' escrow.
    function _pull(address token, address from, uint256 amount, bytes32 id) private {
        if (_isTIP20(token)) {
            _call(token, abi.encodeCall(ITIP20.transferFromWithMemo, (from, address(this), amount, id)));
            return;
        }
        uint256 balanceBefore = ITIP20(token).balanceOf(address(this));
        _call(token, abi.encodeCall(ITIP20.transferFrom, (from, address(this), amount)));
        // forge-lint: disable-next-line(incorrect-strict-equality)
        if (ITIP20(token).balanceOf(address(this)) - balanceBefore != amount) revert UnsupportedToken(token);
    }

    /// @dev Pays `amount` out of escrow to `to`, tagged with the id on TIP-20.
    function _push(address token, address to, uint256 amount, bytes32 id) private {
        _call(
            token,
            _isTIP20(token)
                ? abi.encodeCall(ITIP20.transferWithMemo, (to, amount, id))
                : abi.encodeCall(ITIP20.transfer, (to, amount))
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
