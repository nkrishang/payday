// SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.13;

import {Test} from "foundry/lib/forge-std/src/Test.sol";
import {PaymentFactory} from "foundry/src/PaymentFactory.sol";

/// @title PaymentAddress.t.sol
/// @notice Cross-language parity test: Rust CREATE3 derivation vs Solidity.
///         Uses vm.ffi to call the `derive-address` binary in gateway-core.
contract PaymentAddressTest is Test {
    PaymentFactory public factory;

    function setUp() public {
        factory = new PaymentFactory();
    }

    /// @notice Fuzz test: for any payment parameters, the Rust
    ///         derivation must match Solidity's `paymentAddress`.
    function testFuzz_address_parity(
        address token,
        uint256 amount,
        address receiver,
        uint64 expirationTimestamp,
        address recovery,
        bytes32 salt,
        uint64 chainId
    ) public {
        // --- Solidity side ---
        address expected = factory.paymentAddress(token, amount, receiver, expirationTimestamp, recovery, salt, chainId);

        // --- Rust side via vm.ffi ---
        // Uses the pre-built binary. Run `cargo build -p gateway-core --bin derive-address` first.
        string[] memory inputs = new string[](9);
        inputs[0] = "target/debug/derive-address";
        inputs[1] = vm.toString(address(factory));
        inputs[2] = vm.toString(token);
        inputs[3] = vm.toString(amount);
        inputs[4] = vm.toString(receiver);
        inputs[5] = vm.toString(expirationTimestamp);
        inputs[6] = vm.toString(recovery);
        inputs[7] = vm.toString(salt);
        inputs[8] = vm.toString(chainId);

        bytes memory result = vm.ffi(inputs);
        // vm.ffi hex-decodes "0x..." stdout output, so result is 20 raw address bytes.
        address actual = address(bytes20(result));

        assertEq(expected, actual);
    }

    function test_expiration_recovery_and_chain_change_address() public view {
        address token = address(1);
        address receiver = address(2);
        address recovery = address(3);
        bytes32 salt = bytes32(uint256(4));
        address original = factory.paymentAddress(token, 5, receiver, 100, recovery, salt, 143);

        assertNotEq(original, factory.paymentAddress(token, 5, receiver, 101, recovery, salt, 143));
        assertNotEq(original, factory.paymentAddress(token, 5, receiver, 100, address(6), salt, 143));
        assertNotEq(original, factory.paymentAddress(token, 5, receiver, 100, recovery, salt, 8453));
    }
}
