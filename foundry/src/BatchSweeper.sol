// SPDX-License-Identifier: MIT
pragma solidity ^0.8.13;

import {Payment} from "foundry/src/Payment.sol";
import {PaymentFactory} from "foundry/src/PaymentFactory.sol";

/// @notice Executes independent invoice sweeps in one transaction.
///
/// An item whose `Payment` does not exist yet is deployed through the factory,
/// which settles or recovers the balance present at deployment. An item whose
/// `Payment` already exists (a previous batch, a third party, or an expiry
/// recovery) has any later balance of its token forwarded through
/// `Payment.recover`; the
/// CREATE2 collision that a second `execute` would hit burns every unit of gas
/// forwarded to it, so it is never attempted. A failed item cannot roll back
/// its siblings; the caller reconciles from the emitted events.
contract BatchSweeper {
    struct Sweep {
        address token;
        uint256 amount;
        address receiver;
        uint64 expirationTimestamp;
        address recovery;
        bytes32 salt;
        uint256 chainId;
    }

    /// @notice `factory.execute` (fresh deployment) or `Payment.recover` reverted with `revertData`.
    event SweepFailed(address indexed paymentAddress, address indexed token, bytes revertData);
    /// @notice The `Payment` already existed; `amount` was forwarded to its recovery address.
    /// Zero means nothing had arrived since it was deployed.
    event SweepRecovered(address indexed paymentAddress, address indexed token, uint256 amount);

    PaymentFactory public immutable factory;

    constructor(PaymentFactory factory_) {
        factory = factory_;
    }

    function executeBatch(Sweep[] calldata sweeps) external {
        for (uint256 i; i < sweeps.length; ++i) {
            Sweep calldata sweep = sweeps[i];
            address paymentAddress = factory.paymentAddress(
                sweep.token,
                sweep.amount,
                sweep.receiver,
                sweep.expirationTimestamp,
                sweep.recovery,
                sweep.salt,
                sweep.chainId
            );

            if (paymentAddress.code.length != 0) {
                // Only the factory can deploy at this address, so the code is a Payment.
                try Payment(paymentAddress).recover(sweep.token) returns (uint256 amount) {
                    emit SweepRecovered(paymentAddress, sweep.token, amount);
                } catch (bytes memory revertData) {
                    emit SweepFailed(paymentAddress, sweep.token, revertData);
                }
                continue;
            }

            try factory.execute(
                sweep.token,
                sweep.amount,
                sweep.receiver,
                sweep.expirationTimestamp,
                sweep.recovery,
                sweep.salt,
                sweep.chainId
            ) {}
            catch (bytes memory revertData) {
                emit SweepFailed(paymentAddress, sweep.token, revertData);
            }
        }
    }
}
