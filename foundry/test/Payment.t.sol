// SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.13;

import {Test} from "foundry/lib/forge-std/src/Test.sol";
import {Vm} from "foundry/lib/forge-std/src/Vm.sol";
import {CREATE3} from "foundry/lib/solady/src/utils/CREATE3.sol";
import {MockUSDC} from "foundry/src/MockUSDC.sol";
import {Payment} from "foundry/src/Payment.sol";
import {PaymentFactory} from "foundry/src/PaymentFactory.sol";

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

    function test_payment_usdc(address sender, address receiver, uint96 invoiceAmount, uint96 overpayment, bytes32 salt)
        public
    {
        vm.assume(invoiceAmount > 0);
        vm.assume(uint160(sender) > uint160(0x11) && uint160(receiver) > uint160(0x11));
        assumeNotForgeAddress(sender);
        assumeNotForgeAddress(receiver);

        uint256 paid = uint256(invoiceAmount) + overpayment;
        uint64 expirationTimestamp = uint64(block.timestamp + 1 days);
        address paymentAddress =
            factory.paymentAddress(address(token), invoiceAmount, receiver, expirationTimestamp, RECOVERY, salt);
        token.mint(sender, paid);
        vm.prank(sender);
        assertTrue(token.transfer(paymentAddress, paid));

        vm.expectEmit(true, true, true, true, paymentAddress);
        emit Settled(receiver, paid);
        factory.execute(address(token), invoiceAmount, receiver, expirationTimestamp, RECOVERY, salt);

        assertEq(token.balanceOf(receiver), paid, "beneficiary receives full balance");
        assertEq(token.balanceOf(paymentAddress), 0, "overpayment must not be stranded");
        assertGt(paymentAddress.code.length, 0);
        assertTrue(Payment(paymentAddress).settled(), "deployment must record that the receiver was paid");
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
        assertFalse(Payment(paymentAddress).settled(), "deployment must record that the recovery address was paid");
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
        // anyone may forward it to the invoice's recovery address.
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
