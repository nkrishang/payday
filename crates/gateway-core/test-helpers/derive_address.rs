//! CLI binary for cross-language address parity testing.
//!
//! Called by Foundry's `vm.ffi` in `PaymentAddress.t.sol`.
//!
//! Usage: `derive-address <factory> <token> <amount> <receiver> <salt>`
//!
//! All arguments are hex strings (0x-prefixed), except `<amount>` which is a
//! decimal string (matching Solidity's `vm.toString(uint256)` output).
//! Prints the derived payment address as a 0x-prefixed hex string to stdout.

use std::io::Write;

use alloy_primitives::{Address, B256, U256};

use gateway_core::{Amount, Beneficiary, FactoryAddress, PaymentAddress, Salt, TokenAddress};
use gateway_core::predict_payment_address;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 6 {
        eprintln!("Usage: derive-address <factory> <token> <amount> <receiver> <salt>");
        std::process::exit(1);
    }

    let factory: Address = args[1]
        .parse()
        .unwrap_or_else(|e| panic!("invalid factory address '{}': {e}", args[1]));
    let token: Address = args[2]
        .parse()
        .unwrap_or_else(|e| panic!("invalid token address '{}': {e}", args[2]));
    let amount: U256 = args[3]
        .parse()
        .unwrap_or_else(|e| panic!("invalid amount '{}': {e}", args[3]));
    let receiver: Address = args[4]
        .parse()
        .unwrap_or_else(|e| panic!("invalid receiver address '{}': {e}", args[4]));
    let salt: B256 = args[5]
        .parse()
        .unwrap_or_else(|e| panic!("invalid salt '{}': {e}", args[5]));

    let payment_address: PaymentAddress = predict_payment_address(
        FactoryAddress(factory),
        TokenAddress(token),
        Amount(amount),
        Beneficiary(receiver),
        Salt(salt),
    );

    // Use write! (no trailing newline) so vm.ffi output is clean for vm.parseAddress.
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    write!(handle, "{}", payment_address.0).expect("failed to write address");
}
