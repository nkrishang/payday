// SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.13;

import {Test} from "foundry/lib/forge-std/src/Test.sol";
import {Vm} from "foundry/lib/forge-std/src/Vm.sol";
import {CREATE3} from "foundry/lib/solady/src/utils/CREATE3.sol";
import {MockUSDC} from "foundry/src/MockUSDC.sol";
import {Payment} from "foundry/src/Payment.sol";
import {PaymentFactory} from "foundry/src/PaymentFactory.sol";

contract PaymentDeployer {
    function deploy(address token, uint256 amount, address receiver, uint64 expirationTimestamp, address recovery)
        external
        returns (Payment)
    {
        return new Payment(token, amount, receiver, expirationTimestamp, recovery);
    }
}

contract PaymentTest is Test {
    event Settled(address indexed receiver, uint256 amount);
    event Recovered(address indexed recovery, uint256 amount);

    MockUSDC public token;
    PaymentFactory public factory;

    address internal constant RECEIVER = address(0xBEEF);
    address internal constant RECOVERY = address(0xCAFE);

    function setUp() public {
        token = new MockUSDC();
        factory = new PaymentFactory();
    }

    function test_exact_funding_settles_invoice_amount() public {
        (address paymentAddress, uint64 expirationTimestamp, bytes32 salt) = _invoice(10e6, 1);
        token.mint(paymentAddress, 10e6);

        vm.expectEmit(true, true, true, true, paymentAddress);
        emit Settled(RECEIVER, 10e6);
        factory.execute(address(token), 10e6, RECEIVER, expirationTimestamp, RECOVERY, salt);

        assertEq(token.balanceOf(RECEIVER), 10e6);
        assertEq(token.balanceOf(RECOVERY), 0);
        assertEq(token.balanceOf(paymentAddress), 0);
        assertGt(paymentAddress.code.length, 0);
        assertTrue(Payment(paymentAddress).settled(), "deployment must record that the receiver was paid");
    }

    /// @notice The receiver takes exactly the invoice amount; the overpayment is
    /// Payday custody and leaves for the recovery wallet in the same deployment.
    function test_overpayment_splits_receiver_and_recovery() public {
        (address paymentAddress, uint64 expirationTimestamp, bytes32 salt) = _invoice(10e6, 2);
        token.mint(paymentAddress, 12e6);

        vm.expectEmit(true, true, true, true, paymentAddress);
        emit Settled(RECEIVER, 10e6);
        vm.expectEmit(true, true, true, true, paymentAddress);
        emit Recovered(RECOVERY, 2e6);
        factory.execute(address(token), 10e6, RECEIVER, expirationTimestamp, RECOVERY, salt);

        assertEq(token.balanceOf(RECEIVER), 10e6);
        assertEq(token.balanceOf(RECOVERY), 2e6);
        assertEq(token.balanceOf(paymentAddress), 0);
        assertTrue(Payment(paymentAddress).settled(), "an overpaid deployment still records that the receiver was paid");
    }

    function testFuzz_overpayment_remainder_always_goes_to_recovery(uint96 invoiceAmount, uint96 overpayment) public {
        uint256 amount = bound(invoiceAmount, 1, type(uint96).max);
        uint256 remainder = bound(overpayment, 1, type(uint96).max);
        (address paymentAddress, uint64 expirationTimestamp, bytes32 salt) = _invoice(amount, 3);
        token.mint(paymentAddress, amount + remainder);

        vm.expectEmit(true, true, true, true, paymentAddress);
        emit Settled(RECEIVER, amount);
        vm.expectEmit(true, true, true, true, paymentAddress);
        emit Recovered(RECOVERY, remainder);
        factory.execute(address(token), amount, RECEIVER, expirationTimestamp, RECOVERY, salt);

        assertEq(token.balanceOf(RECEIVER), amount, "the receiver never takes more than the invoice amount");
        assertEq(token.balanceOf(RECOVERY), remainder, "every unit above the invoice amount is recovered");
        assertEq(token.balanceOf(paymentAddress), 0);
    }

    function test_underfunded_live_deployment_reverts() public {
        (address paymentAddress, uint64 expirationTimestamp, bytes32 salt) = _invoice(10e6, 4);
        token.mint(paymentAddress, 10e6 - 1);

        vm.expectRevert(CREATE3.DeploymentFailed.selector);
        factory.execute(address(token), 10e6, RECEIVER, expirationTimestamp, RECOVERY, salt);

        assertEq(paymentAddress.code.length, 0, "a failed deployment must leave the address usable for the invoice");
        assertEq(token.balanceOf(paymentAddress), 10e6 - 1, "a partial payment stays put until completed or expired");
        assertEq(token.balanceOf(RECEIVER), 0);
        assertEq(token.balanceOf(RECOVERY), 0);
    }

    /// @notice CREATE3 discards constructor revert data, so deploy directly to
    /// pin the custom error and the balances it reports.
    function test_underfunded_deployment_reverts_with_insufficient_token_balance() public {
        uint64 expirationTimestamp = uint64(block.timestamp + 1 hours);
        PaymentDeployer deployer = new PaymentDeployer();
        address predicted = vm.computeCreateAddress(address(deployer), vm.getNonce(address(deployer)));
        token.mint(predicted, 10e6 - 1);

        vm.expectRevert(abi.encodeWithSelector(Payment.InsufficientTokenBalance.selector, 10e6 - 1, 10e6));
        deployer.deploy(address(token), 10e6, RECEIVER, expirationTimestamp, RECOVERY);

        assertEq(predicted.code.length, 0);
        assertEq(token.balanceOf(predicted), 10e6 - 1, "a partial payment stays put until completed or expired");
    }

    function test_zero_remainder_emits_no_recovered_event() public {
        (address paymentAddress, uint64 expirationTimestamp, bytes32 salt) = _invoice(10e6, 5);
        token.mint(paymentAddress, 10e6);

        vm.recordLogs();
        factory.execute(address(token), 10e6, RECEIVER, expirationTimestamp, RECOVERY, salt);
        Vm.Log[] memory logs = vm.getRecordedLogs();

        // The token logs its own transfers, so only count what the Payment emitted.
        uint256 paymentLogs;
        for (uint256 i; i < logs.length; ++i) {
            if (logs[i].emitter != paymentAddress) continue;
            paymentLogs++;
            assertEq(logs[i].topics[0], Settled.selector, "an exact payment must only report settlement");
        }
        assertEq(paymentLogs, 1, "an exact payment must emit exactly one event");
        assertEq(token.balanceOf(RECOVERY), 0);
    }

    /// @notice Both legs of an overpaid settlement clear together: when the
    /// recovery wallet cannot take the remainder, the receiver is not paid
    /// either and the address stays usable for a later attempt.
    function test_overpayment_reverts_atomically_when_recovery_transfer_fails() public {
        (address paymentAddress, uint64 expirationTimestamp, bytes32 salt) = _invoice(10e6, 6);
        token.mint(paymentAddress, 12e6);
        token.setBlacklisted(RECOVERY, true);

        vm.expectRevert(CREATE3.DeploymentFailed.selector);
        factory.execute(address(token), 10e6, RECEIVER, expirationTimestamp, RECOVERY, salt);

        assertEq(token.balanceOf(RECEIVER), 0, "the receiver leg must roll back with the recovery leg");
        assertEq(token.balanceOf(RECOVERY), 0);
        assertEq(token.balanceOf(paymentAddress), 12e6, "the full balance stays at the address");
        assertEq(paymentAddress.code.length, 0);
    }

    /// @notice End-to-end recovery: derive, pre-fund, expire, deploy through the
    /// factory, and verify the constructor routes even a partial balance.
    function test_expired_payment_recovers_all_funds() public {
        (address paymentAddress, uint64 expirationTimestamp, bytes32 salt) = _invoice(10e6, 1);
        token.mint(paymentAddress, 4e6);

        vm.warp(expirationTimestamp + 1);
        vm.expectEmit(true, true, true, true, paymentAddress);
        emit Recovered(RECOVERY, 4e6);
        factory.execute(address(token), 10e6, RECEIVER, expirationTimestamp, RECOVERY, salt);

        assertEq(token.balanceOf(RECEIVER), 0);
        assertEq(token.balanceOf(RECOVERY), 4e6);
        assertEq(token.balanceOf(paymentAddress), 0);
        assertGt(paymentAddress.code.length, 0);
        assertFalse(Payment(paymentAddress).settled(), "deployment must record that the recovery wallet was paid");
    }

    /// @notice Expiry is decided before funding: a fully funded, even overpaid,
    /// expired deployment pays the receiver nothing and recovers everything.
    function test_expired_fully_funded_deployment_recovers_everything() public {
        (address paymentAddress, uint64 expirationTimestamp, bytes32 salt) = _invoice(10e6, 7);
        token.mint(paymentAddress, 12e6);

        vm.warp(expirationTimestamp + 1);
        vm.recordLogs();
        factory.execute(address(token), 10e6, RECEIVER, expirationTimestamp, RECOVERY, salt);
        Vm.Log[] memory logs = vm.getRecordedLogs();

        // The token logs its own transfers, so only count what the Payment emitted.
        uint256 paymentLogs;
        for (uint256 i; i < logs.length; ++i) {
            if (logs[i].emitter != paymentAddress) continue;
            paymentLogs++;
            assertEq(logs[i].topics[0], Recovered.selector, "an expired deployment must never report settlement");
            assertEq(logs[i].topics[1], bytes32(uint256(uint160(RECOVERY))));
            assertEq(abi.decode(logs[i].data, (uint256)), 12e6, "the whole balance is recovered, not a remainder");
        }
        assertEq(paymentLogs, 1, "an expired deployment must emit exactly one event");

        assertEq(token.balanceOf(RECEIVER), 0, "the receiver is never paid after expiry");
        assertEq(token.balanceOf(RECOVERY), 12e6);
        assertEq(token.balanceOf(paymentAddress), 0);
        assertFalse(Payment(paymentAddress).settled(), "deployment must record that the recovery wallet was paid");
    }

    function test_expiration_boundary_still_pays_beneficiary() public {
        (address paymentAddress, uint64 expirationTimestamp, bytes32 salt) = _invoice(10e6, 2);
        token.mint(paymentAddress, 10e6);

        vm.warp(expirationTimestamp);
        factory.execute(address(token), 10e6, RECEIVER, expirationTimestamp, RECOVERY, salt);

        assertEq(token.balanceOf(RECEIVER), 10e6);
        assertEq(token.balanceOf(RECOVERY), 0);
        assertTrue(Payment(paymentAddress).settled());
    }

    function test_recover_forwards_late_funds_to_recovery_from_any_caller(address caller) public {
        (address paymentAddress, uint64 expirationTimestamp, bytes32 salt) = _invoice(10e6, 3);
        token.mint(paymentAddress, 10e6);
        factory.execute(address(token), 10e6, RECEIVER, expirationTimestamp, RECOVERY, salt);
        assertEq(token.balanceOf(RECEIVER), 10e6);

        // A repeat payment after settlement can no longer reach the receiver;
        // anyone may forward it to the Payday recovery wallet.
        token.mint(paymentAddress, 3e6);
        vm.expectEmit(true, true, true, true, paymentAddress);
        emit Recovered(RECOVERY, 3e6);
        vm.prank(caller);
        assertEq(Payment(paymentAddress).recover(), 3e6);

        assertEq(token.balanceOf(paymentAddress), 0, "late funds must not be stranded");
        assertEq(token.balanceOf(RECOVERY), 3e6);
        assertEq(token.balanceOf(RECEIVER), 10e6, "recovery never touches the settled payment");
    }

    function test_recover_with_nothing_to_collect_is_a_noop() public {
        (address paymentAddress, uint64 expirationTimestamp, bytes32 salt) = _invoice(10e6, 4);
        token.mint(paymentAddress, 10e6);
        factory.execute(address(token), 10e6, RECEIVER, expirationTimestamp, RECOVERY, salt);

        vm.recordLogs();
        assertEq(Payment(paymentAddress).recover(), 0);
        Vm.Log[] memory logs = vm.getRecordedLogs();
        assertEq(logs.length, 0, "an empty recovery must not emit");
        assertEq(token.balanceOf(RECOVERY), 0);
    }

    /// @notice A partial payment recovered after expiry, then completed late by
    /// the payer, ends with every unit in the recovery wallet instead of stuck.
    function test_late_completion_after_expiry_recovery_is_recoverable() public {
        (address paymentAddress, uint64 expirationTimestamp, bytes32 salt) = _invoice(10e6, 5);
        token.mint(paymentAddress, 4e6);
        vm.warp(expirationTimestamp + 1);
        factory.execute(address(token), 10e6, RECEIVER, expirationTimestamp, RECOVERY, salt);
        assertEq(token.balanceOf(RECOVERY), 4e6);

        token.mint(paymentAddress, 6e6);
        vm.expectRevert(CREATE3.DeploymentFailed.selector);
        factory.execute(address(token), 10e6, RECEIVER, expirationTimestamp, RECOVERY, salt);

        assertEq(Payment(paymentAddress).recover(), 6e6);
        assertEq(token.balanceOf(RECOVERY), 10e6);
        assertEq(token.balanceOf(paymentAddress), 0);
        assertEq(token.balanceOf(RECEIVER), 0);
    }

    function test_recover_reverts_when_the_token_rejects_the_transfer() public {
        (address paymentAddress, uint64 expirationTimestamp, bytes32 salt) = _invoice(10e6, 6);
        token.mint(paymentAddress, 10e6);
        factory.execute(address(token), 10e6, RECEIVER, expirationTimestamp, RECOVERY, salt);
        token.mint(paymentAddress, 1e6);

        token.setPaused(true);
        vm.expectRevert();
        Payment(paymentAddress).recover();

        token.setPaused(false);
        assertEq(Payment(paymentAddress).recover(), 1e6);
    }

    function test_mock_usdc_has_six_decimals() public view {
        assertEq(token.decimals(), 6);
    }

    function _invoice(uint256 amount, uint256 saltSeed)
        private
        view
        returns (address paymentAddress, uint64 expirationTimestamp, bytes32 salt)
    {
        expirationTimestamp = uint64(block.timestamp + 1 hours);
        salt = bytes32(saltSeed);
        paymentAddress = factory.paymentAddress(address(token), amount, RECEIVER, expirationTimestamp, RECOVERY, salt);
    }
}
