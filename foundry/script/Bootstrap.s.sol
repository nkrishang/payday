// SPDX-License-Identifier: MIT
pragma solidity ^0.8.13;

import {Script} from "foundry/lib/forge-std/src/Script.sol";
import {console} from "foundry/lib/forge-std/src/console.sol";
import {BatchSweeper} from "foundry/src/BatchSweeper.sol";
import {MockUSDC} from "foundry/src/MockUSDC.sol";
import {PaymentFactory} from "foundry/src/PaymentFactory.sol";

/// @notice Deploys deterministic local payment fixtures on a fresh Anvil.
contract BootstrapScript is Script {
    // Account #0's nonce-0 and nonce-1 CREATE addresses, respectively.
    address internal constant FACTORY = 0x5FbDB2315678afecb367f032d93F642f64180aa3;
    address internal constant USDC = 0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512;
    address internal constant BATCH_SWEEPER = 0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0;
    address internal constant ANVIL_ACCOUNT_0 = 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266;
    address internal constant ANVIL_ACCOUNT_1 = 0x70997970C51812dc3A010C7d01b50e0d17dc79C8;
    uint256 internal constant FIXTURE_BALANCE = 1_000_000 * 1e6;

    function run() external {
        if (FACTORY.code.length == 0) {
            vm.broadcast();
            PaymentFactory factory = new PaymentFactory();
            require(address(factory) == FACTORY, "factory address mismatch; use fresh Anvil account #0");
            console.log("PaymentFactory deployed at", FACTORY);
        }

        if (USDC.code.length == 0) {
            vm.broadcast();
            MockUSDC token = new MockUSDC();
            require(address(token) == USDC, "USDC address mismatch; use fresh Anvil account #0");
            console.log("MockUSDC deployed at", USDC);
        }

        if (BATCH_SWEEPER.code.length == 0) {
            vm.broadcast();
            BatchSweeper batchSweeper = new BatchSweeper(PaymentFactory(FACTORY));
            require(
                address(batchSweeper) == BATCH_SWEEPER, "batch sweeper address mismatch; use fresh Anvil account #0"
            );
            console.log("BatchSweeper deployed at", BATCH_SWEEPER);
        }

        _topUp(ANVIL_ACCOUNT_0);
        _topUp(ANVIL_ACCOUNT_1);
    }

    function _topUp(address account) internal {
        MockUSDC token = MockUSDC(USDC);
        uint256 balance = token.balanceOf(account);
        if (balance < FIXTURE_BALANCE) {
            vm.broadcast();
            token.mint(account, FIXTURE_BALANCE - balance);
            console.log("MockUSDC funded", account);
        }
    }
}
