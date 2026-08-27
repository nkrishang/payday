// SPDX-License-Identifier: MIT
pragma solidity ^0.8.13;

import {ERC20} from "foundry/lib/solady/src/tokens/ERC20.sol";
import {SafeTransferLib} from "foundry/lib/solady/src/utils/SafeTransferLib.sol";

/// @notice Deployed by `PaymentFactory` at an invoice's counterfactual address.
///
/// The constructor routes the balance present at deployment: to the receiver
/// when the invoice is still live and fully funded, to the recovery address
/// once it has expired. Deployment is a one-way door, so anything that lands
/// at this address afterwards can be forwarded to the recovery address by
/// anyone through `recover`; funds are never stranded.
contract Payment {
    error InsufficientTokenBalance(uint256 balance, uint256 required);

    /// @notice The balance present at deployment was paid to the invoice receiver.
    event Settled(address indexed receiver, uint256 amount);
    /// @notice A balance was forwarded to the recovery address, at deployment or via `recover`.
    event Recovered(address indexed recovery, uint256 amount);

    address internal immutable token;
    address internal immutable recovery;
    /// @notice True when deployment paid the receiver, false when it paid the recovery address.
    bool public immutable settled;

    constructor(address token_, uint256 amount, address receiver, uint64 expirationTimestamp, address recovery_) {
        token = token_;
        recovery = recovery_;
        bool expired = block.timestamp > expirationTimestamp;
        settled = !expired;

        uint256 balance = ERC20(token_).balanceOf(address(this));
        if (expired) {
            SafeTransferLib.safeTransfer({token: token_, to: recovery_, amount: balance});
            emit Recovered(recovery_, balance);
            return;
        }

        if (balance < amount) revert InsufficientTokenBalance(balance, amount);

        // Sweep the complete balance so an overpayment is never stranded at
        // the deterministic payment address.
        SafeTransferLib.safeTransfer({token: token_, to: receiver, amount: balance});
        emit Settled(receiver, balance);
    }

    /// @notice Forward the full token balance to the recovery address.
    /// Permissionless: the destination is committed into this address, so a
    /// caller can only spend gas on the invoice owner's behalf.
    function recover() external returns (uint256 amount) {
        amount = ERC20(token).balanceOf(address(this));
        if (amount == 0) return 0;
        SafeTransferLib.safeTransfer({token: token, to: recovery, amount: amount});
        emit Recovered(recovery, amount);
    }
}
