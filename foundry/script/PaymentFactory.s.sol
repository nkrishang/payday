// SPDX-License-Identifier: MIT
pragma solidity ^0.8.13;

import {Script} from "foundry/lib/forge-std/src/Script.sol";
import {PaymentFactory} from "foundry/src/PaymentFactory.sol";

contract PaymentFactoryScript is Script {
    PaymentFactory public factory;

    function setUp() public {}

    function run() public {
        vm.startBroadcast();

        factory = new PaymentFactory();

        vm.stopBroadcast();
    }
}
