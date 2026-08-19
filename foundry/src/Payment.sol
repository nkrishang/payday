// SPDX-License-Identifier: MIT
pragma solidity ^0.8.13;

import {ERC20} from "foundry/lib/solady/src/tokens/ERC20.sol";
import {SafeTransferLib} from "foundry/lib/solady/src/utils/SafeTransferLib.sol";

contract Payment {
    address private constant NATIVE_TOKEN_ADDRESS = 0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE;

    constructor(address token, uint256 amount, address receiver) {
        if (token == NATIVE_TOKEN_ADDRESS) {
            SafeTransferLib.safeTransferETH({to: receiver, amount: amount});
        } else {
            ERC20(token).approve({spender: address(this), amount: amount});
            SafeTransferLib.safeTransferFrom({token: token, from: address(this), to: receiver, amount: amount});
        }
    }
}
