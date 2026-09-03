// SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.13;

import {Test} from "foundry/lib/forge-std/src/Test.sol";
import {Vm} from "foundry/lib/forge-std/src/Vm.sol";
import {CREATE3} from "foundry/lib/solady/src/utils/CREATE3.sol";
import {SafeTransferLib} from "foundry/lib/solady/src/utils/SafeTransferLib.sol";
import {BatchSweeper} from "foundry/src/BatchSweeper.sol";
import {MockUSDC} from "foundry/src/MockUSDC.sol";
import {Payment} from "foundry/src/Payment.sol";
import {PaymentFactory} from "foundry/src/PaymentFactory.sol";

contract BatchSweeperTest is Test {
    event SweepFailed(address indexed paymentAddress, address indexed token, bytes revertData);
    event SweepRecovered(address indexed paymentAddress, address indexed token, uint256 amount);

    /// @dev Must match `SWEEP_BATCH_BASE_GAS` / `SWEEP_GAS_PER_ITEM` in the indexer's chain client.
    uint256 internal constant SWEEP_BATCH_BASE_GAS = 100_000;
    uint256 internal constant SWEEP_GAS_PER_ITEM = 400_000;
    uint256 internal constant SWEEP_BATCH_LIMIT = 20;

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
        assertEq(token.balanceOf(address(0xA1)), 10e6, "the receiver takes exactly the invoice amount");
        assertEq(token.balanceOf(address(0xCAFE)), 1e6, "the overpayment remainder goes to recovery");
        assertEq(token.balanceOf(first), 0, "an overpayment must not be stranded");
        assertEq(token.balanceOf(address(0xA2)), 0);
        assertEq(token.balanceOf(address(0xA3)), 30e6);
    }

    /// @notice A failed recovery leg reverts only its own deployment, and the
    /// receiver leg rolls back with it. Siblings with a zero remainder never
    /// touch the recovery wallet, so they settle as usual in the same batch.
    function test_overpayment_recovery_failure_does_not_rollback_sibling_sweeps() public {
        BatchSweeper.Sweep[] memory sweeps = new BatchSweeper.Sweep[](3);
        sweeps[0] = _sweep(10e6, address(0xA4), bytes32(uint256(1)));
        sweeps[1] = _sweep(20e6, address(0xA5), bytes32(uint256(2)));
        sweeps[2] = _sweep(30e6, address(0xA6), bytes32(uint256(3)));
        address overpaid = _paymentAddress(sweeps[0]);
        token.mint(overpaid, 11e6);
        token.mint(_paymentAddress(sweeps[1]), 20e6);
        token.mint(_paymentAddress(sweeps[2]), 30e6);
        token.setBlacklisted(address(0xCAFE), true);

        vm.expectEmit(true, true, false, true, address(batchSweeper));
        emit SweepFailed(overpaid, address(token), abi.encodeWithSelector(CREATE3.DeploymentFailed.selector));
        batchSweeper.executeBatch(sweeps);

        assertEq(overpaid.code.length, 0);
        assertEq(token.balanceOf(overpaid), 11e6, "the overpaid item keeps its full balance for a later attempt");
        assertEq(token.balanceOf(address(0xA4)), 0, "the receiver leg rolls back with the failed recovery leg");
        assertGt(_paymentAddress(sweeps[1]).code.length, 0);
        assertGt(_paymentAddress(sweeps[2]).code.length, 0);
        assertEq(token.balanceOf(address(0xA5)), 20e6);
        assertEq(token.balanceOf(address(0xA6)), 30e6);
        assertEq(token.balanceOf(address(0xCAFE)), 0);
    }

    function test_already_deployed_item_is_recovered_instead_of_re_executed() public {
        BatchSweeper.Sweep[] memory sweeps = new BatchSweeper.Sweep[](2);
        sweeps[0] = _sweep(10e6, address(0xB1), bytes32(uint256(1)));
        sweeps[1] = _sweep(20e6, address(0xB2), bytes32(uint256(2)));
        address first = _paymentAddress(sweeps[0]);
        address second = _paymentAddress(sweeps[1]);
        token.mint(first, 10e6);
        token.mint(second, 20e6);
        _execute(sweeps[0]);

        vm.expectEmit(true, true, false, true, address(batchSweeper));
        emit SweepRecovered(first, address(token), 0);
        batchSweeper.executeBatch(sweeps);

        assertGt(second.code.length, 0);
        assertEq(token.balanceOf(address(0xB1)), 10e6);
        assertEq(token.balanceOf(address(0xB2)), 20e6);
    }

    function test_already_deployed_item_with_late_funds_recovers_them() public {
        BatchSweeper.Sweep[] memory sweeps = new BatchSweeper.Sweep[](1);
        sweeps[0] = _sweep(10e6, address(0xC1), bytes32(uint256(1)));
        address paymentAddress = _paymentAddress(sweeps[0]);
        token.mint(paymentAddress, 10e6);
        _execute(sweeps[0]);
        token.mint(paymentAddress, 7e6);

        vm.expectEmit(true, true, false, true, address(batchSweeper));
        emit SweepRecovered(paymentAddress, address(token), 7e6);
        batchSweeper.executeBatch(sweeps);

        assertEq(token.balanceOf(paymentAddress), 0);
        assertEq(token.balanceOf(address(0xCAFE)), 7e6, "late funds go to the recovery wallet");
        assertEq(token.balanceOf(address(0xC1)), 10e6, "the settled payment is untouched");
    }

    /// @notice The indexer submits `SWEEP_BATCH_BASE_GAS + SWEEP_GAS_PER_ITEM * n`
    /// without simulation. The worst case is a full batch in which every fresh
    /// item is overpaid (two transfers each) alongside an already-deployed item
    /// with late funds; every sibling must resolve within that budget.
    function test_production_gas_budget_covers_a_full_batch_with_a_pre_executed_item() public {
        BatchSweeper.Sweep[] memory sweeps = new BatchSweeper.Sweep[](SWEEP_BATCH_LIMIT);
        uint256 expectedRecovered;
        for (uint256 i; i < sweeps.length; ++i) {
            sweeps[i] = _sweep((i + 1) * 1e6, address(uint160(0x1000 + i)), bytes32(uint256(100 + i)));
            uint256 remainder = (i + 1) * 1e5;
            token.mint(_paymentAddress(sweeps[i]), sweeps[i].amount + remainder);
            expectedRecovered += remainder;
        }
        _execute(sweeps[0]);
        token.mint(_paymentAddress(sweeps[0]), 5e6);
        expectedRecovered += 5e6;

        uint256 budget = SWEEP_BATCH_BASE_GAS + SWEEP_GAS_PER_ITEM * sweeps.length;
        vm.recordLogs();
        uint256 before = gasleft();
        (bool ok,) = address(batchSweeper).call{gas: budget}(abi.encodeCall(BatchSweeper.executeBatch, (sweeps)));
        uint256 used = before - gasleft();
        Vm.Log[] memory logs = vm.getRecordedLogs();
        emit log_named_uint("full overpaid batch gas used", used);
        assertTrue(ok, "batch must fit the production gas budget");

        uint256 recovered;
        uint256 failed;
        for (uint256 i; i < logs.length; ++i) {
            if (logs[i].topics[0] == SweepRecovered.selector) recovered++;
            if (logs[i].topics[0] == SweepFailed.selector) failed++;
        }
        assertEq(recovered, 1);
        assertEq(failed, 0);
        for (uint256 i; i < sweeps.length; ++i) {
            assertGt(_paymentAddress(sweeps[i]).code.length, 0);
            assertEq(token.balanceOf(_paymentAddress(sweeps[i])), 0);
            assertEq(token.balanceOf(sweeps[i].receiver), sweeps[i].amount, "each receiver takes its invoice amount");
        }
        assertEq(token.balanceOf(address(0xCAFE)), expectedRecovered, "recovery holds every remainder and late fund");
    }

    function test_each_item_fits_its_share_of_the_gas_budget_with_headroom() public {
        BatchSweeper.Sweep[] memory sweeps = new BatchSweeper.Sweep[](1);
        sweeps[0] = _sweep(10e6, address(0xD1), bytes32(uint256(1)));
        token.mint(_paymentAddress(sweeps[0]), 10e6);

        uint256 before = gasleft();
        batchSweeper.executeBatch(sweeps);
        uint256 used = before - gasleft();
        emit log_named_uint("fresh exact item gas used", used);
        // Circle's proxied FiatToken costs more per transfer than the mock;
        // keep at least 40% of the per-item budget as headroom for it.
        assertLt(used, SWEEP_BATCH_BASE_GAS + (SWEEP_GAS_PER_ITEM * 6) / 10, "per-item gas budget too tight");
    }

    /// @notice An overpaid item is the most expensive fresh deployment, two
    /// token transfers instead of one, and must keep the same headroom.
    function test_each_overpaid_item_fits_its_share_of_the_gas_budget_with_headroom() public {
        BatchSweeper.Sweep[] memory sweeps = new BatchSweeper.Sweep[](1);
        sweeps[0] = _sweep(10e6, address(0xD2), bytes32(uint256(1)));
        token.mint(_paymentAddress(sweeps[0]), 11e6);

        uint256 before = gasleft();
        batchSweeper.executeBatch(sweeps);
        uint256 used = before - gasleft();
        emit log_named_uint("fresh overpaid item gas used", used);
        assertLt(used, SWEEP_BATCH_BASE_GAS + (SWEEP_GAS_PER_ITEM * 6) / 10, "per-item gas budget too tight");
        assertEq(token.balanceOf(address(0xD2)), 10e6);
        assertEq(token.balanceOf(address(0xCAFE)), 1e6);
    }

    function test_paused_token_fails_every_item_until_unpaused() public {
        BatchSweeper.Sweep[] memory sweeps = new BatchSweeper.Sweep[](2);
        sweeps[0] = _sweep(10e6, address(0xE1), bytes32(uint256(1)));
        sweeps[1] = _sweep(20e6, address(0xE2), bytes32(uint256(2)));
        token.mint(_paymentAddress(sweeps[0]), 10e6);
        token.mint(_paymentAddress(sweeps[1]), 20e6);

        token.setPaused(true);
        vm.expectEmit(true, true, false, true, address(batchSweeper));
        emit SweepFailed(
            _paymentAddress(sweeps[0]), address(token), abi.encodeWithSelector(CREATE3.DeploymentFailed.selector)
        );
        vm.expectEmit(true, true, false, true, address(batchSweeper));
        emit SweepFailed(
            _paymentAddress(sweeps[1]), address(token), abi.encodeWithSelector(CREATE3.DeploymentFailed.selector)
        );
        batchSweeper.executeBatch(sweeps);
        assertEq(_paymentAddress(sweeps[0]).code.length, 0, "a paused token must leave no deployment behind");

        token.setPaused(false);
        batchSweeper.executeBatch(sweeps);
        assertEq(token.balanceOf(address(0xE1)), 10e6);
        assertEq(token.balanceOf(address(0xE2)), 20e6);
    }

    /// @notice Expiry is decided per item: an expired item holding more than
    /// its invoice amount recovers the whole balance and pays its receiver
    /// nothing, while the live items in the same batch settle as usual.
    function test_expired_overfunded_item_recovers_everything_while_siblings_settle() public {
        BatchSweeper.Sweep[] memory sweeps = new BatchSweeper.Sweep[](3);
        sweeps[0] = _sweep(10e6, address(0xE3), bytes32(uint256(1)));
        sweeps[1] = _sweep(20e6, address(0xE4), bytes32(uint256(2)));
        sweeps[2] = _sweep(30e6, address(0xE5), bytes32(uint256(3)));
        // Only the first item expires; its siblings keep the helper's one-day expiry.
        sweeps[0].expirationTimestamp = uint64(block.timestamp + 1 hours);
        address expired = _paymentAddress(sweeps[0]);
        token.mint(expired, 12e6);
        token.mint(_paymentAddress(sweeps[1]), 20e6);
        token.mint(_paymentAddress(sweeps[2]), 30e6);

        vm.warp(sweeps[0].expirationTimestamp + 1);
        vm.recordLogs();
        batchSweeper.executeBatch(sweeps);
        Vm.Log[] memory logs = vm.getRecordedLogs();
        for (uint256 i; i < logs.length; ++i) {
            assertTrue(logs[i].topics[0] != SweepFailed.selector, "no item in the batch may fail");
        }

        assertGt(expired.code.length, 0);
        assertFalse(Payment(expired).settled(), "the expired item must record a recovery, not a settlement");
        assertEq(token.balanceOf(address(0xE3)), 0, "an expired item never pays its receiver");
        assertEq(token.balanceOf(expired), 0, "an expired item must not strand its balance");
        assertEq(token.balanceOf(address(0xCAFE)), 12e6, "the expired item's whole balance goes to recovery");
        assertGt(_paymentAddress(sweeps[1]).code.length, 0);
        assertGt(_paymentAddress(sweeps[2]).code.length, 0);
        assertEq(token.balanceOf(address(0xE4)), 20e6);
        assertEq(token.balanceOf(address(0xE5)), 30e6);
    }

    function test_blacklisted_receiver_fails_only_its_item() public {
        BatchSweeper.Sweep[] memory sweeps = new BatchSweeper.Sweep[](2);
        sweeps[0] = _sweep(10e6, address(0xF1), bytes32(uint256(1)));
        sweeps[1] = _sweep(20e6, address(0xF2), bytes32(uint256(2)));
        token.mint(_paymentAddress(sweeps[0]), 10e6);
        token.mint(_paymentAddress(sweeps[1]), 20e6);
        token.setBlacklisted(address(0xF2), true);

        vm.expectEmit(true, true, false, true, address(batchSweeper));
        emit SweepFailed(
            _paymentAddress(sweeps[1]), address(token), abi.encodeWithSelector(CREATE3.DeploymentFailed.selector)
        );
        batchSweeper.executeBatch(sweeps);

        assertEq(token.balanceOf(address(0xF1)), 10e6);
        assertEq(_paymentAddress(sweeps[1]).code.length, 0);
        assertEq(token.balanceOf(_paymentAddress(sweeps[1])), 20e6, "funds stay at the address for later recovery");
    }

    function test_failed_recovery_is_reported_with_the_token_revert() public {
        BatchSweeper.Sweep[] memory sweeps = new BatchSweeper.Sweep[](1);
        sweeps[0] = _sweep(10e6, address(0xF3), bytes32(uint256(1)));
        address paymentAddress = _paymentAddress(sweeps[0]);
        token.mint(paymentAddress, 10e6);
        _execute(sweeps[0]);
        token.mint(paymentAddress, 1e6);
        token.setBlacklisted(address(0xCAFE), true);

        vm.expectEmit(true, true, false, true, address(batchSweeper));
        emit SweepFailed(
            paymentAddress, address(token), abi.encodeWithSelector(SafeTransferLib.TransferFailed.selector)
        );
        batchSweeper.executeBatch(sweeps);
        assertEq(token.balanceOf(paymentAddress), 1e6);
    }

    function _execute(BatchSweeper.Sweep memory sweep) private {
        factory.execute(
            sweep.token, sweep.amount, sweep.receiver, sweep.expirationTimestamp, sweep.recovery, sweep.salt
        );
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
