use alloy_primitives::{
    U256,
    utils::{UnitsError, parse_units},
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Circle-issued USDC uses six decimal places on supported EVM chains.
pub const USDC_DECIMALS: u8 = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Amount(pub U256);

#[derive(Debug, Error)]
pub enum AmountParseError {
    #[error("Parsing number with more decimal units than target decimals.")]
    TooManyDecimalUnits,
    #[error(transparent)]
    Units(#[from] UnitsError),
}

impl Amount {
    pub fn from_decimal_str(amount_string: &str, decimals: u8) -> Result<Amount, AmountParseError> {
        let units: U256 = parse_units(amount_string, decimals)?.into();

        // Custom error check: `alloy_primitives::parse_units` will truncate values if there are
        // more decimal units than the target `decimals` e.g. "0.01" for "1 decimals" will be
        // truncated to just "0" rather than erroring.
        //
        // We want to error here, instead, to avoid any unintended / incorrect truncation at the
        // types level itself.
        if let Some(dot_pos) = amount_string.find('.') {
            let frac_len = amount_string[dot_pos + 1..].len();
            if frac_len > decimals as usize {
                return Err(AmountParseError::TooManyDecimalUnits);
            }
        }

        Ok(Amount(units))
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn parse_whole_amount() {
        let amount = Amount::from_decimal_str("123", USDC_DECIMALS).unwrap();
        assert_eq!(amount.0.to_string(), "123000000");
    }

    #[test]
    fn parse_decimal_amount() {
        let amount = Amount::from_decimal_str("123.456", USDC_DECIMALS).unwrap();
        assert_eq!(amount.0.to_string(), "123456000");
    }

    #[test]
    fn parse_insufficient_decimals() {
        let result = Amount::from_decimal_str("0.01", 1);
        assert!(matches!(result, Err(AmountParseError::TooManyDecimalUnits)));
    }

    #[test]
    fn parse_too_many_decimal_points() {
        let result = Amount::from_decimal_str("100.1.33", USDC_DECIMALS);
        assert!(result.is_err());
    }
}
