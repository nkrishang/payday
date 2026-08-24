// SPDX-License-Identifier: MIT
pragma solidity ^0.8.13;

import {ERC20} from "foundry/lib/solady/src/tokens/ERC20.sol";
import {SafeTransferLib} from "foundry/lib/solady/src/utils/SafeTransferLib.sol";

contract Payment {
    error InsufficientTokenBalance(uint256 balance, uint256 required);

    constructor(address token, uint256 amount, address receiver) {
        uint256 balance = ERC20(token).balanceOf(address(this));
        if (balance < amount) revert InsufficientTokenBalance(balance, amount);

        // Sweep the complete balance so an overpayment is never stranded at
        // the deterministic payment address.
        SafeTransferLib.safeTransfer({token: token, to: receiver, amount: balance});
    }
}
