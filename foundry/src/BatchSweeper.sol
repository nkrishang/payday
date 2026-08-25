// SPDX-License-Identifier: MIT
pragma solidity ^0.8.13;

import {PaymentFactory} from "foundry/src/PaymentFactory.sol";

/// @notice Executes independent PaymentFactory sweeps in one transaction.
/// A failed item cannot roll back successful siblings; callers reconcile a
/// failure from its deterministic payment address and the emitted revert data.
contract BatchSweeper {
    struct Sweep {
        address token;
        uint256 amount;
        address receiver;
        uint64 expirationTimestamp;
        address recovery;
        bytes32 salt;
    }

    event SweepFailed(address indexed paymentAddress, address indexed token, bytes revertData);

    PaymentFactory public immutable factory;

    constructor(PaymentFactory factory_) {
        factory = factory_;
    }

    function executeBatch(Sweep[] calldata sweeps) external {
        for (uint256 i; i < sweeps.length; ++i) {
            Sweep calldata sweep = sweeps[i];
            address paymentAddress = factory.paymentAddress(
                sweep.token, sweep.amount, sweep.receiver, sweep.expirationTimestamp, sweep.recovery, sweep.salt
            );

            try factory.execute(
                sweep.token, sweep.amount, sweep.receiver, sweep.expirationTimestamp, sweep.recovery, sweep.salt
            ) {}
            catch (bytes memory revertData) {
                emit SweepFailed(paymentAddress, sweep.token, revertData);
            }
        }
    }
}
