// SPDX-License-Identifier: MIT
pragma solidity ^0.8.13;

import {ERC20} from "foundry/lib/solady/src/tokens/ERC20.sol";
import {SafeTransferLib} from "foundry/lib/solady/src/utils/SafeTransferLib.sol";

/// @notice Deployed by `PaymentFactory` at an invoice's counterfactual address.
///
/// The constructor routes the balance present at deployment. While the invoice
/// is live it pays the receiver exactly the invoice amount and forwards any
/// remainder to the Payday recovery wallet; once the invoice has expired the
/// whole balance goes to the recovery wallet instead. Deployment is a one-way
/// door, so anything that lands at this address afterwards can be forwarded to
/// the recovery wallet by anyone through `recover`. Funds are never stranded,
/// and the receiver never takes more than the invoice amount: everything else
/// is held by Payday for manual review and return.
contract Payment {
    error InsufficientTokenBalance(uint256 balance, uint256 required);

    /// @notice The receiver was paid the invoice amount at deployment. `amount` is always the
    /// exact invoice amount, never the balance that happened to be present.
    event Settled(address indexed receiver, uint256 amount);
    /// @notice A balance was forwarded to the Payday recovery wallet: the overpayment remainder
    /// of a live deployment, the whole balance of an expired deployment, or a later balance
    /// forwarded through `recover`.
    event Recovered(address indexed recovery, uint256 amount);

    address internal immutable token;
    address internal immutable recovery;
    /// @notice True when deployment paid the receiver, false when it paid the recovery wallet.
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

        SafeTransferLib.safeTransfer({token: token_, to: receiver, amount: amount});
        emit Settled(receiver, amount);

        // An overpayment is Payday's to review and return, not the receiver's
        // to keep, and it leaves in this same transaction so the deterministic
        // address never strands it. A failed recovery transfer reverts the
        // whole deployment: the receiver is not paid until both legs can clear.
        uint256 remainder = balance - amount;
        if (remainder != 0) {
            SafeTransferLib.safeTransfer({token: token_, to: recovery_, amount: remainder});
            emit Recovered(recovery_, remainder);
        }
    }

    /// @notice Forward the full token balance to the Payday recovery wallet.
    /// Permissionless: the destination is committed into this address, so a
    /// caller can only spend gas on Payday's behalf.
    function recover() external returns (uint256 amount) {
        amount = ERC20(token).balanceOf(address(this));
        if (amount == 0) return 0;
        SafeTransferLib.safeTransfer({token: token, to: recovery, amount: amount});
        emit Recovered(recovery, amount);
    }
}
