use alloy_primitives::Address;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AddressParseError {
    #[error("must be a 20-byte EVM address beginning with 0x")]
    Invalid,
    #[error("must not be the zero address")]
    Zero,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TokenAddress(pub Address);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BeneficiaryAddress(pub Address);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RecoveryAddress(pub Address);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FactoryAddress(pub Address);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PaymentAddress(pub Address);

macro_rules! parse_address {
    ($type:ident, $allow_zero:literal) => {
        impl FromStr for $type {
            type Err = AddressParseError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                let address = Address::from_str(value).map_err(|_| AddressParseError::Invalid)?;
                if !$allow_zero && address.is_zero() {
                    return Err(AddressParseError::Zero);
                }
                Ok(Self(address))
            }
        }
    };
}

parse_address!(TokenAddress, true);
parse_address!(BeneficiaryAddress, false);
parse_address!(RecoveryAddress, false);
parse_address!(FactoryAddress, true);
parse_address!(PaymentAddress, true);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payout_addresses_share_strict_validation() {
        assert!(BeneficiaryAddress::from_str("not-an-address").is_err());
        assert!(
            BeneficiaryAddress::from_str("0x0000000000000000000000000000000000000000").is_err()
        );
        assert!(BeneficiaryAddress::from_str("0x70997970C51812dc3A010C7d01b50e0d17dc79C8").is_ok());
    }
}
