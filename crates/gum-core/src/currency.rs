//! The stablecoins Gum accepts. A currency is the product-level asset a
//! merchant prices in ("USDT"); on each chain it is one exact contract, named
//! by the chain registry, whose own symbol may differ ("USDT0" on Monad).
//!
//! What separates the currencies is how a merchant's balance moves between
//! chains. USDC has CCTP, which burns on one chain and mints on another at
//! exactly 1:1, so a USDC request may be paid on any supported chain and the
//! merchant withdraws to whichever chain they like. Nothing else has such a
//! path, and Gum never gives a merchant a rate worse than 1:1, so every
//! other currency's request pins the chain it settles on and withdraws on
//! that chain alone. A payer may still pay from another chain through Relay:
//! the spread there is the payer's.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Currency {
    #[serde(rename = "USDC")]
    Usdc,
    #[serde(rename = "USDT")]
    Usdt,
}

/// How a merchant's balance in a currency crosses chains at 1:1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrossChainSettlement {
    /// Circle's CCTP: burn on the source chain, mint on the destination.
    Cctp,
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("unknown currency: {0:?}")]
pub struct CurrencyParseError(pub String);

impl Currency {
    pub const ALL: [Currency; 2] = [Currency::Usdc, Currency::Usdt];

    /// The wire code: what `currency` carries on every deposit request,
    /// withdrawal, webhook, and proof.
    pub fn code(self) -> &'static str {
        match self {
            Currency::Usdc => "USDC",
            Currency::Usdt => "USDT",
        }
    }

    /// Base units per whole unit, as every supported deployment defines it.
    /// The chain reader checks each contract's `decimals()` against this at
    /// startup, so a deployment whose token disagrees never serves.
    pub fn decimals(self) -> u8 {
        match self {
            Currency::Usdc | Currency::Usdt => 6,
        }
    }

    /// Who issues it, for the operator- and merchant-facing text that names
    /// the party able to freeze an address.
    pub fn issuer_name(self) -> &'static str {
        match self {
            Currency::Usdc => "Circle",
            Currency::Usdt => "Tether",
        }
    }

    /// The symbol the contract on `chain_id` shows in a payer's wallet.
    /// Tether's USDT on Monad and Arbitrum is USDT0, the omnichain token
    /// operated under its license; the merchant still prices in USDT.
    pub fn symbol_on(self, chain_id: u64) -> &'static str {
        match (self, chain_id) {
            (Currency::Usdt, 143 | 42_161) => "USDT0",
            _ => self.code(),
        }
    }

    /// The 1:1 cross-chain path a merchant's balance may take, if any.
    pub fn cross_chain_settlement(self) -> Option<CrossChainSettlement> {
        match self {
            Currency::Usdc => Some(CrossChainSettlement::Cctp),
            Currency::Usdt => None,
        }
    }

    /// Whether a deposit request in this currency must name the one chain
    /// it settles on: true exactly when the merchant could not later move
    /// the balance elsewhere at 1:1.
    pub fn requires_pinned_chain(self) -> bool {
        self.cross_chain_settlement().is_none()
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl FromStr for Currency {
    type Err = CurrencyParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Currency::ALL
            .into_iter()
            .find(|currency| currency.code() == value)
            .ok_or_else(|| CurrencyParseError(value.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip_exactly() {
        for currency in Currency::ALL {
            assert_eq!(currency.code().parse::<Currency>().unwrap(), currency);
            assert_eq!(
                serde_json::to_value(currency).unwrap(),
                serde_json::Value::String(currency.code().into())
            );
        }
        assert_eq!(
            "usdc".parse::<Currency>(),
            Err(CurrencyParseError("usdc".into()))
        );
        assert!("USDT0".parse::<Currency>().is_err());
    }

    #[test]
    fn only_usdc_settles_across_chains() {
        assert_eq!(
            Currency::Usdc.cross_chain_settlement(),
            Some(CrossChainSettlement::Cctp)
        );
        assert!(!Currency::Usdc.requires_pinned_chain());
        assert_eq!(Currency::Usdt.cross_chain_settlement(), None);
        assert!(Currency::Usdt.requires_pinned_chain());
    }

    #[test]
    fn usdt_shows_as_usdt0_where_tether_deploys_the_omnichain_token() {
        assert_eq!(Currency::Usdt.symbol_on(143), "USDT0");
        assert_eq!(Currency::Usdt.symbol_on(42_161), "USDT0");
        assert_eq!(Currency::Usdt.symbol_on(31_337), "USDT");
        assert_eq!(Currency::Usdc.symbol_on(143), "USDC");
        assert_eq!(Currency::Usdt.issuer_name(), "Tether");
    }
}
