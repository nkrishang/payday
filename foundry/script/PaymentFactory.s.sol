// SPDX-License-Identifier: MIT
pragma solidity ^0.8.13;

import {Script} from "foundry/lib/forge-std/src/Script.sol";
import {console} from "foundry/lib/forge-std/src/console.sol";
import {BatchSweeper} from "foundry/src/BatchSweeper.sol";
import {PaymentFactory} from "foundry/src/PaymentFactory.sol";

/// @notice Deploys one contract generation: a `PaymentFactory` and the
/// `BatchSweeper` bound to it. `Payment.creationCode` is embedded in the
/// factory, so any `Payment` change is a new generation and both contracts
/// move together; the services pin the generation by runtime code hash.
contract PaymentFactoryScript is Script {
    PaymentFactory public factory;
    BatchSweeper public batchSweeper;

    function setUp() public {}

    function run() public {
        uint256 expectedChainId = vm.envUint("PAYDAY_CHAIN_ID");
        require(block.chainid == expectedChainId, "unexpected deployment chain");

        vm.startBroadcast();

        factory = new PaymentFactory();
        batchSweeper = new BatchSweeper(factory);

        vm.stopBroadcast();

        // gatewayd and gateway-indexer refuse to start unless the live code
        // hashes match these values, so print them ready to paste into the
        // environment alongside the addresses.
        console.log("PAYDAY_FACTORY_ADDRESS=%s", vm.toString(address(factory)));
        console.log("PAYDAY_FACTORY_CODE_HASH=%s", vm.toString(address(factory).codehash));
        console.log("PAYDAY_BATCH_SWEEPER_ADDRESS=%s", vm.toString(address(batchSweeper)));
        console.log("PAYDAY_BATCH_SWEEPER_CODE_HASH=%s", vm.toString(address(batchSweeper).codehash));
    }
}
