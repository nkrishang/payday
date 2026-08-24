// SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.13;

import {Test} from "foundry/lib/forge-std/src/Test.sol";
import {ERC20} from "foundry/lib/solady/src/tokens/ERC20.sol";
import {PaymentFactory, PaymentExecuted} from "foundry/src/PaymentFactory.sol";

contract MockERC20 is ERC20 {
    function name() public pure override returns (string memory) {
        return "Mock Token";
    }

    function symbol() public pure override returns (string memory) {
        return "MT";
    }

    function mint(address to, uint256 amount) external {
        _mint(to, amount);
    }
}

contract PaymentTest is Test {
    address private constant NATIVE_TOKEN_ADDRESS = 0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE;

    MockERC20 public token;
    PaymentFactory public factory;

    function setUp() public {
        token = new MockERC20();
        factory = new PaymentFactory();

        vm.label(address(token), "MockToken");
        vm.label(address(factory), "PaymentFactory");
    }

    function test_payment_native(address sender, address receiver, uint256 amount, bytes32 salt) public {
        // Ignore: address(0) and pre-compile addresses (they revert on receiving native tokens in foundry's test VM).
        vm.assume(uint160(sender) > uint160(0x11));
        vm.assume(uint160(receiver) > uint160(0x11));
        // Ignore Forge-reserved addresses (vm cheatcode, console, Create2Deployer),
        // which behave specially and are not valid payers/receivers.
        assumeNotForgeAddress(sender);
        assumeNotForgeAddress(receiver);

        // Get the deterministic payment address.
        address payable paymentAddress =
            factory.paymentAddress({token: NATIVE_TOKEN_ADDRESS, amount: amount, receiver: receiver, salt: salt});
        vm.label(paymentAddress, "Payment");

        // Sender makes payment to the deterministic payment address.
        vm.deal(sender, amount);
        vm.prank(sender);
        (bool success,) = paymentAddress.call{value: amount}("");
        assertTrue(success);

        // Execute payment
        assertEq(receiver.balance, 0);
        assertEq(paymentAddress.balance, amount);

        vm.expectEmit(true, true, true, true);
        emit PaymentExecuted(paymentAddress, receiver, NATIVE_TOKEN_ADDRESS, amount);
        factory.execute({token: NATIVE_TOKEN_ADDRESS, amount: amount, receiver: receiver, salt: salt});

        assertEq(receiver.balance, amount);
        assertEq(paymentAddress.balance, 0);
    }

    function test_payment_erc20(address sender, address receiver, uint256 amount, bytes32 salt) public {
        // Ignore: address(0) and pre-compile addresses (they revert on receiving native tokens in foundry's test VM).
        vm.assume(uint160(sender) > uint160(0x11));
        vm.assume(uint160(receiver) > uint160(0x11));
        // Ignore Forge-reserved addresses (vm cheatcode, console, Create2Deployer),
        // which behave specially and are not valid payers/receivers.
        assumeNotForgeAddress(sender);
        assumeNotForgeAddress(receiver);

        // Get the deterministic payment address.
        address paymentAddress =
            factory.paymentAddress({token: address(token), amount: amount, receiver: receiver, salt: salt});
        vm.label(paymentAddress, "Payment");

        // Sender makes payment to the deterministic payment address.
        token.mint(sender, amount);
        vm.prank(sender);
        bool success = token.transfer(paymentAddress, amount);
        assertTrue(success);

        // Execute payment
        assertEq(token.balanceOf(receiver), 0);
        assertEq(token.balanceOf(paymentAddress), amount);

        vm.expectEmit(true, true, true, true);
        emit PaymentExecuted(paymentAddress, receiver, address(token), amount);
        factory.execute({token: address(token), amount: amount, receiver: receiver, salt: salt});

        assertEq(token.balanceOf(receiver), amount);
        assertEq(token.balanceOf(paymentAddress), 0);
    }
}
