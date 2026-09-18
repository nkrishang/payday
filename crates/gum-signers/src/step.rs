//! Withdrawal step preconditions.
//!
//! A step's command names a fact whose truth on chain means the step
//! already happened: the EIP-3009 authorization is consumed, or the CCTP
//! message nonce is received. The executor checks it before spending a
//! nonce and again after a revert, so a competitor's (or a lost previous
//! run's) transaction concludes the step instead of failing it.

use alloy_primitives::{Address, B256};
use alloy_sol_types::{SolCall, sol};
use gum_chain::{BlockHeader, ChainError, ChainReader};
use gum_contracts::StepPrecondition;
use tracing::warn;

sol! {
    function usedNonces(bytes32 nonce) view returns (uint256);
}

/// What a precondition check found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Precondition {
    /// The step is still ours to do.
    Holds,
    /// The step already happened; `tx_hash` is the transaction that did
    /// it, when the search located one.
    Violated { tx_hash: Option<B256> },
}

/// Evaluate `precondition` at the finality boundary `boundary`.
pub async fn check(
    chain: &dyn ChainReader,
    log_range_size: u64,
    precondition: &StepPrecondition,
    boundary: &BlockHeader,
) -> Result<Precondition, ChainError> {
    match precondition {
        StepPrecondition::AuthorizationUnused {
            token,
            authorizer,
            nonce,
            search_from_block,
        } => {
            if !chain
                .authorization_state(*token, *authorizer, *nonce, boundary.number)
                .await?
            {
                return Ok(Precondition::Holds);
            }
            let tx_hash = find_authorization_use(
                chain,
                *token,
                *authorizer,
                *nonce,
                *search_from_block,
                boundary.number,
                log_range_size,
            )
            .await?;
            Ok(Precondition::Violated { tx_hash })
        }
        StepPrecondition::MessageNonceUnused {
            message_transmitter,
            nonce,
            ..
        } => {
            let output = chain
                .finalized_view_call(
                    *message_transmitter,
                    usedNoncesCall { nonce: *nonce }.abi_encode().into(),
                )
                .await?;
            let used = usedNoncesCall::abi_decode_returns(&output).map_err(|error| {
                ChainError::Transient(format!(
                    "could not decode MessageTransmitter.usedNonces response: {error}"
                ))
            })?;
            Ok(if used.is_zero() {
                Precondition::Holds
            } else {
                Precondition::Violated { tx_hash: None }
            })
        }
    }
}

/// The transaction that consumed the authorization, searched newest-first
/// in provider-sized chunks down to `from_block`. `None` when the history
/// searched holds no `AuthorizationUsed` event for it.
async fn find_authorization_use(
    chain: &dyn ChainReader,
    token: Address,
    authorizer: Address,
    nonce: B256,
    from_block: u64,
    to_block: u64,
    log_range_size: u64,
) -> Result<Option<B256>, ChainError> {
    let chunk = log_range_size.max(1);
    let mut upper = to_block;
    loop {
        let lower = upper.saturating_sub(chunk - 1).max(from_block);
        match chain
            .authorization_used_tx(token, authorizer, nonce, lower, upper)
            .await?
        {
            Some((tx_hash, receipt)) => {
                if !receipt.succeeded {
                    return Err(ChainError::Transient(format!(
                        "AuthorizationUsed in {tx_hash} belongs to a reverted transaction"
                    )));
                }
                return Ok(Some(tx_hash));
            }
            None if lower <= from_block => {
                warn!(%authorizer, %nonce, from_block, to_block, "authorization is consumed but its transaction is outside the searched history");
                return Ok(None);
            }
            None => upper = lower - 1,
        }
    }
}
