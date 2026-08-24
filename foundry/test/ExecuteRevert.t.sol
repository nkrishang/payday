// SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.13;

import {Test} from "foundry/lib/forge-std/src/Test.sol";
import {CREATE3} from "foundry/lib/solady/src/utils/CREATE3.sol";
import {SafeTransferLib} from "foundry/lib/solady/src/utils/SafeTransferLib.sol";
import {PaymentFactory} from "foundry/src/PaymentFactory.sol";

/// A receiver that rejects any native transfer by reverting in `receive()`.
contract RejectingReceiver {
    receive() external payable {
        revert("no thanks");
    }
}

/// Verifies HOW `execute` reverts, so the Rust backend knows what it can observe.
///
/// Two distinct on-chain situations both make `execute` fail:
///   (1) the payment was already executed (address already has code), and
///   (2) the receiver rejects the native transfer in the `Payment` constructor.
/// This test checks whether case (2)'s inner `ETHTransferFailed` is surfaced or
/// swallowed by CREATE3, and whether the two cases are distinguishable by error
/// selector alone.
///
/// Conclusion (asserted below): both revert with the SAME `DeploymentFailed`
/// selector, so the selector cannot tell them apart. The distinguishing signal
/// is whether the payment address has code afterwards — present only in case (1)
/// — which the backend reads over RPC (`eth_getCode`) to classify a failure.
contract ExecuteRevertTest is Test {
    address private constant NATIVE_TOKEN_ADDRESS = 0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE;

    PaymentFactory public factory;

    function setUp() public {
        factory = new PaymentFactory();
    }

    /// Fund the payment address for (receiver, amount, salt) with `amount` wei.
    function _fund(address receiver, uint256 amount, bytes32 salt) internal returns (address payable paymentAddress) {
        paymentAddress =
            factory.paymentAddress({token: NATIVE_TOKEN_ADDRESS, amount: amount, receiver: receiver, salt: salt});
        vm.deal(paymentAddress, amount);
    }

    /// Case (2): receiver rejects ETH. Does `execute` bubble up
    /// `ETHTransferFailed`, or swallow it into `DeploymentFailed`?
    function test_receiver_rejects_reverts_with_DeploymentFailed() public {
        RejectingReceiver receiver = new RejectingReceiver();
        uint256 amount = 1 ether;
        bytes32 salt = bytes32(uint256(1));

        address payable paymentAddress = _fund(address(receiver), amount, salt);

        // If the inner error surfaced, this would need to be
        // SafeTransferLib.ETHTransferFailed.selector instead.
        vm.expectRevert(CREATE3.DeploymentFailed.selector);
        factory.execute({token: NATIVE_TOKEN_ADDRESS, amount: amount, receiver: address(receiver), salt: salt});

        // The blocked-case signal: deployment failed, so NO code exists at the
        // payment address. This is how the backend tells case (2) from case (1).
        assertEq(paymentAddress.code.length, 0, "blocked payment must leave no code at the address");
    }

    /// Prove the inner error is NOT what surfaces: expecting `ETHTransferFailed`
    /// here would fail if CREATE3 swallowed it. Kept as an explicit counter-check.
    function test_receiver_rejects_does_not_surface_ETHTransferFailed() public {
        RejectingReceiver receiver = new RejectingReceiver();
        uint256 amount = 1 ether;
        bytes32 salt = bytes32(uint256(2));

        _fund(address(receiver), amount, salt);

        bool sawEthTransferFailed;
        try factory.execute({
            token: NATIVE_TOKEN_ADDRESS,
            amount: amount,
            receiver: address(receiver),
            salt: salt
        }) {
            revert("execute unexpectedly succeeded");
        } catch (bytes memory err) {
            bytes4 selector = bytes4(err);
            sawEthTransferFailed = (selector == SafeTransferLib.ETHTransferFailed.selector);
            assertEq(selector, CREATE3.DeploymentFailed.selector, "expected DeploymentFailed selector");
        }
        assertFalse(sawEthTransferFailed, "ETHTransferFailed must be swallowed, not surfaced");
    }

    /// Case (1): a second `execute` for an already-executed payment ALSO reverts
    /// with `DeploymentFailed` (address already has code) — same selector as the
    /// blocked case, so the selector alone cannot tell success-then-repeat from a
    /// genuinely blocked payment.
    function test_already_executed_reverts_with_DeploymentFailed() public {
        address receiver = address(0xBEEF);
        uint256 amount = 1 ether;
        bytes32 salt = bytes32(uint256(3));

        address payable paymentAddress = _fund(receiver, amount, salt);

        // First call succeeds and sweeps the funds.
        factory.execute({token: NATIVE_TOKEN_ADDRESS, amount: amount, receiver: receiver, salt: salt});
        assertEq(receiver.balance, amount);

        // The fulfilled-case signal: a successful sweep leaves the `Payment`
        // contract deployed at the payment address.
        assertGt(paymentAddress.code.length, 0, "executed payment must leave code at the address");

        // Second call reverts with the SAME selector as the blocked case, but the
        // code presence above is what lets the backend classify it as fulfilled.
        vm.expectRevert(CREATE3.DeploymentFailed.selector);
        factory.execute({token: NATIVE_TOKEN_ADDRESS, amount: amount, receiver: receiver, salt: salt});
    }
}
