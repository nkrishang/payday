// SPDX-License-Identifier: MIT
pragma solidity ^0.8.13;

import {Script} from "foundry/lib/forge-std/src/Script.sol";
import {console} from "foundry/lib/forge-std/src/console.sol";
import {ITokenMessengerV2, WithdrawalForwarder} from "foundry/src/WithdrawalForwarder.sol";

/// @notice Deploys the `WithdrawalForwarder` bound to CCTP V2's
/// `TokenMessengerV2`, which Circle deploys at one address on every EVM chain.
///
/// The forwarder is stateless, so nothing requires it to share an address
/// across chains; it is deployed from a fresh key (nonce 0) anyway so that
/// the address a merchant is asked to authorize is the same everywhere and
/// the runtime code hash, which pins the immutable, is one value in
/// GUM_CHAINS.
contract WithdrawalForwarderScript is Script {
    address internal constant TOKEN_MESSENGER_V2 = 0x28b5a0e9C621a5BadaA536219b3a228C8168cf5d;

    function run() public {
        uint256 expectedChainId = vm.envUint("GUM_CHAIN_ID");
        require(block.chainid == expectedChainId, "unexpected deployment chain");
        require(TOKEN_MESSENGER_V2.code.length > 0, "CCTP V2 TokenMessengerV2 is not deployed on this chain");
        (, address deployer,) = vm.readCallers();
        require(
            vm.getNonce(deployer) == 0, "deploy the forwarder from a fresh key so every chain gets the same address"
        );

        vm.broadcast();
        WithdrawalForwarder forwarder = new WithdrawalForwarder(ITokenMessengerV2(TOKEN_MESSENGER_V2));

        console.log("GUM_WITHDRAWAL_FORWARDER_ADDRESS=%s", vm.toString(address(forwarder)));
        console.log("GUM_WITHDRAWAL_FORWARDER_CODE_HASH=%s", vm.toString(address(forwarder).codehash));
    }
}
