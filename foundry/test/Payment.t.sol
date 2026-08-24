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
        address paymentAddress = factory.paymentAddress(address(token), invoiceAmount, receiver, salt);
        token.mint(sender, paid);
        vm.prank(sender);
        assertTrue(token.transfer(paymentAddress, paid));

        vm.expectEmit(true, true, true, true);
        emit PaymentExecuted(paymentAddress, receiver, address(token), invoiceAmount);
        factory.execute(address(token), invoiceAmount, receiver, salt);

        assertEq(token.balanceOf(receiver), paid, "beneficiary receives full balance");
        assertEq(token.balanceOf(paymentAddress), 0, "overpayment must not be stranded");
        assertGt(paymentAddress.code.length, 0);
    }

    function test_mock_usdc_has_six_decimals() public view {
        assertEq(token.decimals(), 6);
    }
}
