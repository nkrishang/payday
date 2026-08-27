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
    #[error("amount must be an unsigned decimal number such as \"1\" or \"1.50\"")]
    InvalidFormat,
    #[error("Parsing number with more decimal units than target decimals.")]
    TooManyDecimalUnits,
    #[error(transparent)]
    Units(#[from] UnitsError),
}

impl Amount {
    /// Parse a human-readable unsigned decimal string into base units.
    ///
    /// Only `digits` or `digits.digits` is accepted: signs, whitespace,
    /// exponents and bare separators are rejected here rather than being
    /// coerced (`parse_units` would otherwise accept "-1.5" as 1.5).
    pub fn from_decimal_str(amount_string: &str, decimals: u8) -> Result<Amount, AmountParseError> {
        let (whole, fraction) = amount_string
            .split_once('.')
            .map_or((amount_string, ""), |(whole, fraction)| (whole, fraction));
        let is_digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
        if whole.is_empty()
            || !is_digits(whole)
            || !is_digits(fraction)
            || (amount_string.contains('.') && fraction.is_empty())
        {
            return Err(AmountParseError::InvalidFormat);
        }
        if fraction.len() > decimals as usize {
            return Err(AmountParseError::TooManyDecimalUnits);
        }

        let units: U256 = parse_units(amount_string, decimals)?.into();
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

    #[test]
    fn rejects_signs_whitespace_exponents_and_bare_separators() {
        for invalid in [
            "-1.5", "+1", " 1", "1 ", "1e6", ".5", "1.", "", "0x10", "1_000",
        ] {
            assert!(
                matches!(
                    Amount::from_decimal_str(invalid, USDC_DECIMALS),
                    Err(AmountParseError::InvalidFormat)
                ),
                "{invalid:?} must be rejected as malformed"
            );
        }
    }

    #[test]
    fn zero_parses_and_is_left_to_the_caller_to_reject() {
        assert_eq!(
            Amount::from_decimal_str("0.000000", USDC_DECIMALS)
                .unwrap()
                .0,
            U256::ZERO
        );
    }
}
