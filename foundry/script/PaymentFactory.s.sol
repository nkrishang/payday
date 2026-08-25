// SPDX-License-Identifier: MIT
pragma solidity ^0.8.13;

import {Script} from "foundry/lib/forge-std/src/Script.sol";
import {BatchSweeper} from "foundry/src/BatchSweeper.sol";
import {PaymentFactory} from "foundry/src/PaymentFactory.sol";

contract PaymentFactoryScript is Script {
    PaymentFactory public factory;
    BatchSweeper public batchSweeper;

    function setUp() public {}

    function run() public {
        uint256 expectedChainId = vm.envUint("GATEWAY_CHAIN_ID");
        require(block.chainid == expectedChainId, "unexpected deployment chain");

        vm.startBroadcast();

        factory = new PaymentFactory();
        batchSweeper = new BatchSweeper(factory);

        vm.stopBroadcast();
    }
}
