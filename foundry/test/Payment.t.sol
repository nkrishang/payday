// SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.13;

import {Test} from "foundry/lib/forge-std/src/Test.sol";
import {MockUSDC} from "foundry/src/MockUSDC.sol";
import {PaymentFactory, PaymentExecuted} from "foundry/src/PaymentFactory.sol";

contract PaymentTest is Test {
    MockUSDC public token;
    PaymentFactory public factory;

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
        address recovery = address(0xCAFE);
        address paymentAddress =
            factory.paymentAddress(address(token), invoiceAmount, receiver, expirationTimestamp, recovery, salt);
        token.mint(sender, paid);
        vm.prank(sender);
        assertTrue(token.transfer(paymentAddress, paid));

        vm.expectEmit(true, true, true, true);
        emit PaymentExecuted(paymentAddress, receiver, address(token), invoiceAmount);
        factory.execute(address(token), invoiceAmount, receiver, expirationTimestamp, recovery, salt);

        assertEq(token.balanceOf(receiver), paid, "beneficiary receives full balance");
        assertEq(token.balanceOf(paymentAddress), 0, "overpayment must not be stranded");
        assertGt(paymentAddress.code.length, 0);
    }

    /// @notice End-to-end recovery: derive, pre-fund, expire, deploy through the
    /// factory, and verify the constructor routes even a partial balance.
    function test_expired_payment_recovers_all_funds() public {
        uint256 invoiceAmount = 10e6;
        uint256 partialPayment = 4e6;
        address receiver = address(0xBEEF);
        address recovery = address(0xCAFE);
        uint64 expirationTimestamp = uint64(block.timestamp + 1 hours);
        bytes32 salt = bytes32(uint256(1));
        address paymentAddress =
            factory.paymentAddress(address(token), invoiceAmount, receiver, expirationTimestamp, recovery, salt);
        token.mint(paymentAddress, partialPayment);

        vm.warp(expirationTimestamp + 1);
        factory.execute(address(token), invoiceAmount, receiver, expirationTimestamp, recovery, salt);

        assertEq(token.balanceOf(receiver), 0);
        assertEq(token.balanceOf(recovery), partialPayment);
        assertEq(token.balanceOf(paymentAddress), 0);
        assertGt(paymentAddress.code.length, 0);
    }

    function test_expiration_boundary_still_pays_beneficiary() public {
        uint256 amount = 10e6;
        address receiver = address(0xBEEF);
        address recovery = address(0xCAFE);
        uint64 expirationTimestamp = uint64(block.timestamp + 1 hours);
        bytes32 salt = bytes32(uint256(2));
        address paymentAddress =
            factory.paymentAddress(address(token), amount, receiver, expirationTimestamp, recovery, salt);
        token.mint(paymentAddress, amount);

        vm.warp(expirationTimestamp);
        factory.execute(address(token), amount, receiver, expirationTimestamp, recovery, salt);

        assertEq(token.balanceOf(receiver), amount);
        assertEq(token.balanceOf(recovery), 0);
    }

    function test_mock_usdc_has_six_decimals() public view {
        assertEq(token.decimals(), 6);
    }
}
