// SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.13;

import {Test} from "foundry/lib/forge-std/src/Test.sol";
import {CREATE3} from "foundry/lib/solady/src/utils/CREATE3.sol";
import {MockUSDC} from "foundry/src/MockUSDC.sol";
import {PaymentFactory} from "foundry/src/PaymentFactory.sol";

/// @notice Pins the observable CREATE3 failures used by the backend.
contract ExecuteRevertTest is Test {
    MockUSDC private token;
    PaymentFactory private factory;

    function setUp() public {
        token = new MockUSDC();
        factory = new PaymentFactory();
    }

    function test_underpayment_reverts_with_DeploymentFailed_and_leaves_no_code() public {
        uint256 amount = 10e6;
        bytes32 salt = bytes32(uint256(1));
        uint64 expirationTimestamp = uint64(block.timestamp + 1 days);
        address recovery = address(0xCAFE);
        address paymentAddress =
            factory.paymentAddress(address(token), amount, address(0xBEEF), expirationTimestamp, recovery, salt);
        token.mint(paymentAddress, amount - 1);

        vm.expectRevert(CREATE3.DeploymentFailed.selector);
        factory.execute(address(token), amount, address(0xBEEF), expirationTimestamp, recovery, salt);

        assertEq(paymentAddress.code.length, 0);
        assertEq(token.balanceOf(paymentAddress), amount - 1);
    }

    function test_already_executed_reverts_with_DeploymentFailed() public {
        uint256 amount = 10e6;
        bytes32 salt = bytes32(uint256(2));
        address receiver = address(0xBEEF);
        uint64 expirationTimestamp = uint64(block.timestamp + 1 days);
        address recovery = address(0xCAFE);
        address paymentAddress =
            factory.paymentAddress(address(token), amount, receiver, expirationTimestamp, recovery, salt);
        token.mint(paymentAddress, amount);

        factory.execute(address(token), amount, receiver, expirationTimestamp, recovery, salt);
        assertGt(paymentAddress.code.length, 0);

        vm.expectRevert(CREATE3.DeploymentFailed.selector);
        factory.execute(address(token), amount, receiver, expirationTimestamp, recovery, salt);
    }
}
