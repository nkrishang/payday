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

    /// @notice Fuzz test: for any (token, amount, receiver, salt), the Rust
    ///         derivation must match Solidity's `paymentAddress`.
    function testFuzz_address_parity(address token, uint256 amount, address receiver, bytes32 salt) public {
        // --- Solidity side ---
        address expected = factory.paymentAddress(token, amount, receiver, salt);

        // --- Rust side via vm.ffi ---
        // Uses the pre-built binary. Run `cargo build -p gateway-core --bin derive-address` first.
        string[] memory inputs = new string[](6);
        inputs[0] = "target/debug/derive-address";
        inputs[1] = vm.toString(address(factory));
        inputs[2] = vm.toString(token);
        inputs[3] = vm.toString(amount);
        inputs[4] = vm.toString(receiver);
        inputs[5] = vm.toString(salt);

        bytes memory result = vm.ffi(inputs);
        // vm.ffi hex-decodes "0x..." stdout output, so result is 20 raw address bytes.
        address actual = address(bytes20(result));

        assertEq(expected, actual);
    }
}
