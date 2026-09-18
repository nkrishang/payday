//! Turning a finalized `BatchSweeper` receipt into per-item evidence.
//!
//! This is evidence, not policy: the outcome says what the receipt and the
//! pinned reads at its block prove about each payment address. The server
//! decides what each outcome does to the deposit request.

use alloy_primitives::Address;
use gum_chain::{
    BlockHeader, ChainError, ChainReader, SettlementEvent, SweepOutcome, SweepReceipt,
};
use gum_contracts::{
    SettlementEvidence, SweepFailureCause, SweepItem, SweepItemOutcome, SweepItemResult,
    SweepItemStatus,
};
use gum_core::ChainConfig;
use tracing::warn;

/// Classify every item of a finalized, successful batch receipt.
///
/// `header` is the receipt block's header, already verified against the
/// receipt's block hash by the caller. A `Transient` error means the chain
/// did not answer well enough to conclude anything; the caller leaves the
/// transaction open and tries again on the next pass.
pub async fn classify_items(
    chain: &dyn ChainReader,
    config: &ChainConfig,
    items: &[SweepItem],
    receipt: &SweepReceipt,
    header: &BlockHeader,
) -> Result<Vec<SweepItemResult>, ChainError> {
    let mut results = Vec::with_capacity(items.len());
    for item in items {
        let outcome = match receipt.outcomes.get(&item.payment_address) {
            Some(outcome) => classify_item(chain, config, item, outcome, header).await?,
            None => {
                return Err(ChainError::Transient(format!(
                    "receipt carries no outcome for payment address {}",
                    item.payment_address
                )));
            }
        };
        results.push(SweepItemResult {
            invoice_id: item.invoice_id,
            outcome,
        });
    }
    Ok(results)
}

async fn classify_item(
    chain: &dyn ChainReader,
    config: &ChainConfig,
    item: &SweepItem,
    outcome: &SweepOutcome,
    header: &BlockHeader,
) -> Result<SweepItemOutcome, ChainError> {
    match outcome {
        SweepOutcome::Settled {
            recovered_amount, ..
        } => Ok(SweepItemOutcome::Settled {
            overpayment_recovered: *recovered_amount,
        }),
        SweepOutcome::Recovered { amount } => Ok(SweepItemOutcome::Returned { amount: *amount }),
        SweepOutcome::Collected { amount } => {
            // The contract already existed. When the ledger holds no
            // settlement for the request, somebody else deployed it and the
            // deployment's own logs are the only record of how it routed
            // the balance it found.
            let settlement = if item.settlement_recorded {
                None
            } else {
                let from_block = item.first_observed_block.unwrap_or(config.start_block);
                let event = find_settlement(
                    chain,
                    item.payment_address,
                    item.token,
                    from_block,
                    header.number,
                    config.log_range_size,
                )
                .await?;
                let event_header = chain.block_header(event.block_number).await?;
                if event_header.hash != event.block_hash {
                    return Err(ChainError::Transient(format!(
                        "settlement event block {} changed during classification",
                        event.block_number
                    )));
                }
                Some(SettlementEvidence {
                    tx_hash: event.transaction_hash,
                    block: event.block_number,
                    transaction_index: event.transaction_index,
                    block_timestamp: event_header.timestamp,
                    settled: event.settled,
                    recovered: event.recovered,
                })
            };
            if item.status == SweepItemStatus::Closed && settlement.is_some() {
                warn!(invoice_id = %item.invoice_id, payment_address = %item.payment_address, "closed request without a recorded settlement; reporting the discovered one");
            }
            Ok(SweepItemOutcome::LateCollected {
                amount: *amount,
                settlement,
            })
        }
        SweepOutcome::Failed { .. } => {
            let expired = header.timestamp > item.expiration_timestamp;
            let probe = chain
                .probe_failure(
                    item.currency,
                    item.token,
                    item.payment_address,
                    item.receiver,
                    item.recovery,
                    header.number,
                )
                .await?;
            // A deployed contract's failing call was `recover`, and an
            // expired request deploys straight to the recovery wallet.
            let to_recovery = probe.code_present || expired;
            // A live deployment also forwards any balance above the amount
            // to the recovery wallet in the same transaction.
            let touches_recovery =
                to_recovery || probe.balance.is_some_and(|balance| balance > item.amount);
            let cause = if probe.paused == Some(true) {
                SweepFailureCause::TokenPaused
            } else if probe.payment_blacklisted == Some(true) {
                SweepFailureCause::PaymentAddressBlacklisted
            } else if touches_recovery && probe.recovery_blacklisted == Some(true) {
                SweepFailureCause::RecoveryBlacklisted
            } else if !to_recovery && probe.receiver_blacklisted == Some(true) {
                SweepFailureCause::BeneficiaryBlacklisted
            } else if !to_recovery && probe.balance.is_some_and(|balance| balance < item.amount) {
                SweepFailureCause::BalanceBelowAmount
            } else {
                SweepFailureCause::Unknown
            };
            Ok(SweepItemOutcome::Failed { cause })
        }
    }
}

/// The transaction that created or drained `payment`, searched forward
/// from `from_block` to `to_block` in provider-sized chunks, never one
/// unbounded `eth_getLogs`. A range the provider rejects is halved.
async fn find_settlement(
    chain: &dyn ChainReader,
    payment: Address,
    token: Address,
    from_block: u64,
    to_block: u64,
    log_range_size: u64,
) -> Result<SettlementEvent, ChainError> {
    let mut chunk = log_range_size.max(1);
    let mut start = from_block;
    while start <= to_block {
        let end = start.saturating_add(chunk - 1).min(to_block);
        match chain
            .payment_settlement_tx(payment, token, start, end)
            .await
        {
            Ok(Some(event)) => return Ok(event),
            Ok(None) => start = end.saturating_add(1),
            Err(ChainError::LogRangeTooLarge(message)) if chunk > 1 => {
                chunk = (chunk / 2).max(1);
                warn!(from_block = start, to_block = end, chunk, error = %message, "provider rejected settlement log range; splitting it");
            }
            Err(error) => return Err(error),
        }
    }
    Err(ChainError::Transient(format!(
        "payment {payment} is deployed but has no finalized settlement event in blocks {from_block}..={to_block}"
    )))
}
