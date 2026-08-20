use alloy_primitives::{Address, address};
use serde::{Deserialize, Serialize};

pub const NATIVE_TOKEN_ADDRESS: TokenAddress =
    TokenAddress(address!("0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE"));

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TokenAddress(pub Address);

impl TokenAddress {
    pub fn is_native(&self) -> bool {
        *self == NATIVE_TOKEN_ADDRESS
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Beneficiary(pub Address);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FactoryAddress(pub Address);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PaymentAddress(pub Address);
