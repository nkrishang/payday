// SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.13;

import {Test} from "foundry/lib/forge-std/src/Test.sol";
import {CREATE3} from "foundry/lib/solady/src/utils/CREATE3.sol";
import {BatchSweeper} from "foundry/src/BatchSweeper.sol";
import {MockUSDC} from "foundry/src/MockUSDC.sol";
import {PaymentFactory} from "foundry/src/PaymentFactory.sol";

contract BatchSweeperTest is Test {
    event SweepFailed(address indexed paymentAddress, address indexed token, bytes revertData);

    MockUSDC private token;
    PaymentFactory private factory;
    BatchSweeper private batchSweeper;

    function setUp() public {
        token = new MockUSDC();
        factory = new PaymentFactory();
        batchSweeper = new BatchSweeper(factory);
    }

    function test_mixed_batch_sweeps_successes_and_reports_failure() public {
        BatchSweeper.Sweep[] memory sweeps = new BatchSweeper.Sweep[](3);
        sweeps[0] = _sweep(10e6, address(0xA1), bytes32(uint256(1)));
        sweeps[1] = _sweep(20e6, address(0xA2), bytes32(uint256(2)));
        sweeps[2] = _sweep(30e6, address(0xA3), bytes32(uint256(3)));

        address first = _paymentAddress(sweeps[0]);
        address failed = _paymentAddress(sweeps[1]);
        address third = _paymentAddress(sweeps[2]);
        token.mint(first, 11e6);
        token.mint(failed, 20e6 - 1);
        token.mint(third, 30e6);

        vm.expectEmit(true, true, false, true, address(batchSweeper));
        emit SweepFailed(failed, address(token), abi.encodeWithSelector(CREATE3.DeploymentFailed.selector));
        batchSweeper.executeBatch(sweeps);

        assertGt(first.code.length, 0);
        assertEq(failed.code.length, 0);
        assertGt(third.code.length, 0);
        assertEq(token.balanceOf(address(0xA1)), 11e6, "overpayment must be swept completely");
        assertEq(token.balanceOf(address(0xA2)), 0);
        assertEq(token.balanceOf(address(0xA3)), 30e6);
    }

    function test_already_deployed_failure_does_not_revert_siblings() public {
        BatchSweeper.Sweep[] memory sweeps = new BatchSweeper.Sweep[](2);
        sweeps[0] = _sweep(10e6, address(0xB1), bytes32(uint256(1)));
        sweeps[1] = _sweep(20e6, address(0xB2), bytes32(uint256(2)));
        address first = _paymentAddress(sweeps[0]);
        address second = _paymentAddress(sweeps[1]);
        token.mint(first, 10e6);
        token.mint(second, 20e6);
        factory.execute(
            sweeps[0].token,
            sweeps[0].amount,
            sweeps[0].receiver,
            sweeps[0].expirationTimestamp,
            sweeps[0].recovery,
            sweeps[0].salt
        );

        vm.expectEmit(true, true, false, true, address(batchSweeper));
        emit SweepFailed(first, address(token), abi.encodeWithSelector(CREATE3.DeploymentFailed.selector));
        batchSweeper.executeBatch(sweeps);

        assertGt(first.code.length, 0);
        assertGt(second.code.length, 0);
        assertEq(token.balanceOf(address(0xB2)), 20e6);
    }

    function _sweep(uint256 amount, address receiver, bytes32 salt) private view returns (BatchSweeper.Sweep memory) {
        return BatchSweeper.Sweep({
            token: address(token),
            amount: amount,
            receiver: receiver,
            expirationTimestamp: uint64(block.timestamp + 1 days),
            recovery: address(0xCAFE),
            salt: salt
        });
    }

    function _paymentAddress(BatchSweeper.Sweep memory sweep) private view returns (address) {
        return factory.paymentAddress(
            sweep.token, sweep.amount, sweep.receiver, sweep.expirationTimestamp, sweep.recovery, sweep.salt
        );
    }
}
