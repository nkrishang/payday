// SPDX-License-Identifier: MIT
pragma solidity ^0.8.13;

import {ERC20} from "foundry/lib/solady/src/tokens/ERC20.sol";
import {SafeTransferLib} from "foundry/lib/solady/src/utils/SafeTransferLib.sol";

/// @notice Deployed by `PaymentFactory` at an invoice's counterfactual address.
///
/// The constructor routes the balance present at deployment. While the invoice
/// is live it pays the receiver exactly the invoice amount and forwards any
/// remainder to the recovery wallet; once the invoice has expired the whole
/// balance goes to the recovery wallet instead. Deployment is a one-way door,
/// so anything that lands at this address afterwards can be forwarded to the
/// recovery wallet by anyone through `recover`. Funds are never stranded, and
/// the receiver never takes more than the invoice amount.
///
/// The invoice is payable on exactly one chain, chosen by the payer and
/// committed into the address as `chainId`. The factory lives at the same
/// address on every supported chain, so the same counterfactual address exists
/// everywhere; a deployment on any other chain touches no token at all, records
/// `settled == false`, and leaves whatever landed there for `recover`, which
/// takes the token to forward because the committed one may not exist on that
/// chain.
contract Payment {
    error InsufficientTokenBalance(uint256 balance, uint256 required);

    /// @notice The receiver was paid the invoice amount at deployment. `amount` is always the
    /// exact invoice amount, never the balance that happened to be present.
    event Settled(address indexed receiver, uint256 amount);
    /// @notice A balance of `token` was forwarded to the recovery wallet: the overpayment
    /// remainder of a live deployment, the whole balance of an expired deployment, or a later
    /// balance forwarded through `recover`.
    event Recovered(address indexed recovery, address indexed token, uint256 amount);
    /// @notice Deployed on a chain other than the one the payer chose; nothing was routed.
    event WrongChain(uint256 expectedChainId, uint256 actualChainId);

    address internal immutable recovery;
    /// @notice True when deployment paid the receiver, false when it paid the recovery wallet
    /// or ran on the wrong chain.
    bool public immutable settled;

    constructor(
        address token,
        uint256 amount,
        address receiver,
        uint64 expirationTimestamp,
        address recovery_,
        uint256 chainId
    ) {
        recovery = recovery_;

        // No token call may happen here: the committed token is this chain's
        // USDC only on the chain the payer chose, and the guard must hold
        // even where that address has no code.
        if (block.chainid != chainId) {
            settled = false;
            emit WrongChain(chainId, block.chainid);
            return;
        }

        bool expired = block.timestamp > expirationTimestamp;
        settled = !expired;

        uint256 balance = ERC20(token).balanceOf(address(this));
        if (expired) {
            SafeTransferLib.safeTransfer({token: token, to: recovery_, amount: balance});
            emit Recovered(recovery_, token, balance);
            return;
        }

        if (balance < amount) revert InsufficientTokenBalance(balance, amount);

        SafeTransferLib.safeTransfer({token: token, to: receiver, amount: amount});
        emit Settled(receiver, amount);

        // An overpayment is the payer's to get back, not the receiver's to
        // keep, and it leaves in this same transaction so the deterministic
        // address never strands it. A failed recovery transfer reverts the
        // whole deployment: the receiver is not paid until both legs can clear.
        uint256 remainder = balance - amount;
        if (remainder != 0) {
            SafeTransferLib.safeTransfer({token: token, to: recovery_, amount: remainder});
            emit Recovered(recovery_, token, remainder);
        }
    }

    /// @notice Forward this contract's full balance of `token` to the recovery wallet.
    /// Permissionless: the destination is committed into this address, so a
    /// caller can only spend gas on the payer's behalf. The token is a
    /// parameter so a wrong-chain deployment can return that chain's USDC,
    /// and so any other token mistakenly sent here goes back the same way.
    function recover(address token) external returns (uint256 amount) {
        amount = ERC20(token).balanceOf(address(this));
        if (amount == 0) return 0;
        SafeTransferLib.safeTransfer({token: token, to: recovery, amount: amount});
        emit Recovered(recovery, token, amount);
    }
}
