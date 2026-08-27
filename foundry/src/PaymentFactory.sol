// SPDX-License-Identifier: MIT
pragma solidity ^0.8.13;

import {CREATE3} from "foundry/lib/solady/src/utils/CREATE3.sol";
import {Payment} from "foundry/src/Payment.sol";

/// @notice Ownerless CREATE3 deployer for `Payment`. Every invoice parameter is
/// committed into the deployment salt, so the counterfactual address fixes the
/// destination of funds before anyone pays. Execution is permissionless; the
/// deployed `Payment` reports the outcome through its `Settled`/`Recovered` events.
contract PaymentFactory {
    function paymentAddress(
        address token,
        uint256 amount,
        address receiver,
        uint64 expirationTimestamp,
        address recovery,
        bytes32 salt
    ) external view returns (address payable) {
        return payable(CREATE3.predictDeterministicAddress(
                deploymentSalt(token, amount, receiver, expirationTimestamp, recovery, salt)
            ));
    }

    function execute(
        address token,
        uint256 amount,
        address receiver,
        uint64 expirationTimestamp,
        address recovery,
        bytes32 salt
    ) external {
        CREATE3.deployDeterministic({
            salt: deploymentSalt(token, amount, receiver, expirationTimestamp, recovery, salt),
            initCode: abi.encodePacked(
                type(Payment).creationCode, abi.encode(token, amount, receiver, expirationTimestamp, recovery)
            )
        });
    }

    function deploymentSalt(
        address token,
        uint256 amount,
        address receiver,
        uint64 expirationTimestamp,
        address recovery,
        bytes32 salt
    ) private pure returns (bytes32) {
        return keccak256(abi.encode(token, amount, receiver, expirationTimestamp, recovery, salt));
    }
}
