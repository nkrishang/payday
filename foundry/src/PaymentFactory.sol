// SPDX-License-Identifier: MIT
pragma solidity ^0.8.13;

import {CREATE3} from "foundry/lib/solady/src/utils/CREATE3.sol";
import {Payment} from "foundry/src/Payment.sol";

event PaymentExecuted(address indexed source, address indexed receiver, address indexed token, uint256 amount);

contract PaymentFactory {
    function paymentAddress(address token, uint256 amount, address receiver, bytes32 salt)
        external
        view
        returns (address payable)
    {
        return payable(CREATE3.predictDeterministicAddress(deploymentSalt(token, amount, receiver, salt)));
    }

    function execute(address token, uint256 amount, address receiver, bytes32 salt) external {
        address source = CREATE3.deployDeterministic({
            salt: deploymentSalt(token, amount, receiver, salt),
            initCode: abi.encodePacked(type(Payment).creationCode, abi.encode(token, amount, receiver))
        });

        emit PaymentExecuted(source, receiver, token, amount);
    }

    function deploymentSalt(address token, uint256 amount, address receiver, bytes32 salt)
        private
        pure
        returns (bytes32)
    {
        return keccak256(abi.encode(token, amount, receiver, salt));
    }
}
