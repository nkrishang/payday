//! CREATE3 deterministic address derivation.
//!
//! Computes the counterfactual payment address that `PaymentFactory.paymentAddress(...)`
//! returns in Solidity. Both sides MUST produce the same address for the same inputs.

use alloy_primitives::{Address, B256, b256, keccak256};
use alloy_sol_types::{SolType, sol};

use crate::{
    Amount, BeneficiaryAddress, FactoryAddress, PaymentAddress, RecoveryAddress, Salt, TokenAddress,
};

/// Hash of Solady's CREATE3 proxy init code.
/// `keccak256(abi.encodePacked(hex"67363d3d37363d34f03d5260086018f3"))`
pub const PROXY_INITCODE_HASH: B256 =
    b256!("0x21c35dbe1b344a2488cf3321d6ce542f8e9f305544ff09e4993a62319a497c1f");

/// Solidity tuple type matching `PaymentFactory.deploymentSalt`'s abi.encode parameters.
type DeploymentSaltInput = sol! { tuple(address, uint256, address, uint64, address, bytes32) };

/// Compute the deterministic CREATE3 payment address.
///
/// This must match `PaymentFactory.paymentAddress(token, amount, receiver,
/// expirationTimestamp, recovery, salt)`
/// in Solidity for the same inputs.
pub fn predict_payment_address(
    factory: FactoryAddress,
    token: TokenAddress,
    amount: Amount,
    receiver: BeneficiaryAddress,
    expiration_timestamp: u64,
    recovery: RecoveryAddress,
    salt: Salt,
) -> PaymentAddress {
    // Step 1: hash the factory's ABI-encoded payment parameters.
    let encoded = DeploymentSaltInput::abi_encode_sequence(&(
        token.0,
        amount.0,
        receiver.0,
        expiration_timestamp,
        recovery.0,
        salt.0,
    ));
    let deployment_salt = keccak256(&encoded);

    // Step 2: CREATE2 to compute the proxy address.
    // keccak256(0xff ++ factory ++ deployment_salt ++ PROXY_INITCODE_HASH)
    let mut create2_preimage = [0u8; 85]; // 1 + 20 + 32 + 32
    create2_preimage[0] = 0xff;
    create2_preimage[1..21].copy_from_slice(factory.0.as_slice());
    create2_preimage[21..53].copy_from_slice(deployment_salt.as_slice());
    create2_preimage[53..85].copy_from_slice(PROXY_INITCODE_HASH.as_slice());

    let proxy_hash = keccak256(create2_preimage);
    let proxy_address = Address::from_slice(&proxy_hash[12..]);

    // Step 3: CREATE with nonce = 1 to compute the final payment address.
    // RLP: 0xd6 (list prefix, 22 payload bytes) ++ 0x94 (string prefix, 20 bytes)
    //      ++ proxy_address ++ 0x01 (nonce)
    let mut create_preimage = [0u8; 23]; // 1 + 1 + 20 + 1
    create_preimage[0] = 0xd6;
    create_preimage[1] = 0x94;
    create_preimage[2..22].copy_from_slice(proxy_address.as_slice());
    create_preimage[22] = 0x01;

    let final_hash = keccak256(create_preimage);
    PaymentAddress(Address::from_slice(&final_hash[12..]))
}
