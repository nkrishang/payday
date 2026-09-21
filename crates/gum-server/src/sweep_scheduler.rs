//! Turns funded deposit requests into `SweepBatch` commands.
//!
//! The `invoices` table is the durable queue (see `gum_ledger::sweeps`);
//! this loop only decides *when* to look at it. It runs on a timer and is
//! woken early by the internal RPC whenever a finalized range funds a
//! request, so a payment normally becomes a command within a second of the
//! indexer reporting it, and never later than one tick.
//!
//! Each pass drains every chain's queue in batches until
//! [`InvoiceRepository::schedule_sweep_job`] finds nothing eligible. The
//! ledger opens the job and publishes its command in one transaction, so a
//! crash anywhere in this loop loses nothing: whatever was not committed is
//! still in the queue on the next pass, and whatever was committed is on
//! the bus. Halted chains schedule nothing (the ledger checks the open
//! fault inside the same transaction).
//!
//! The pass also watches for jobs that have been open longer than
//! [`STUCK_JOB_AGE`]: the signers own their execution and reconcile them
//! against the chain themselves, so this is an alert, not a repair.

use std::sync::Arc;
use std::time::Duration;

use gum_bus::Publisher;
use gum_core::ChainRegistry;
use gum_ledger::{InvoiceRepository, SweepPolicy};
use tokio::sync::{Notify, watch};
use tracing::{error, info, warn};

/// An open job older than this has almost certainly stalled in the
/// signers; it is logged at error level on every pass until it resolves.
pub const STUCK_JOB_AGE: Duration = Duration::from_secs(60 * 60);
/// Batches scheduled per chain per pass before yielding to the next chain.
const MAX_BATCHES_PER_PASS: usize = 16;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PassReport {
    pub jobs: Vec<uuid::Uuid>,
    pub deposit_requests: usize,
}

pub struct SweepScheduler {
    invoices: InvoiceRepository,
    publisher: Publisher,
    /// One policy per served chain, resolved once from the registry: the
    /// base policy with each chain's batch size fitted to its gas budget.
    policies: Vec<(u64, SweepPolicy)>,
    interval: Duration,
    wake: Arc<Notify>,
}

/// A chain whose gas configuration cannot produce a usable sweep batch.
#[derive(Debug, thiserror::Error)]
#[error("chain {chain_id}: {problem}")]
pub struct SweepPolicyError {
    chain_id: u64,
    problem: String,
}

/// Fit `base` to one chain's gas configuration.
///
/// `block_gas_limit` is the chain's configured safe execution capacity; a
/// sweep batch may budget half of it, capped by `transaction_gas_limit`
/// where the chain caps single transactions below that. The batch size is
/// the budget's [`gum_chain::sweep_batch_capacity`], unless the chain sets
/// `sweep_batch_size` explicitly — which is validated against the budget,
/// never silently clamped. A chain with no gas configuration at all keeps
/// the base policy's historical batch size.
fn resolve_sweep_policy(
    chain_id: u64,
    chain: &gum_core::ChainConfig,
    base: &SweepPolicy,
) -> Result<SweepPolicy, SweepPolicyError> {
    let problem = |problem: &str| SweepPolicyError {
        chain_id,
        problem: problem.to_string(),
    };
    if chain.block_gas_limit.is_none()
        && (chain.sweep_batch_size.is_some() || chain.transaction_gas_limit.is_some())
    {
        return Err(problem(
            "sweep_batch_size or transaction_gas_limit set without block_gas_limit; \
             batch sizing is validated against the block budget, so set block_gas_limit too",
        ));
    }
    let batch_size = match (chain.sweep_batch_size, chain.block_gas_limit) {
        (None, None) => base.batch_size,
        (explicit, Some(block_gas_limit)) => {
            let budget = (block_gas_limit / 2).min(chain.transaction_gas_limit.unwrap_or(u64::MAX));
            let capacity = gum_chain::sweep_batch_capacity(budget);
            if capacity == 0 {
                return Err(problem(
                    "gas budget cannot carry even one sweep batch item (one item is 400k gas plus a 100k base)",
                ));
            }
            let capacity = u32::try_from(capacity)
                .map_err(|_| problem("derived sweep batch size overflows u32"))?;
            match explicit {
                Some(explicit) if u64::from(explicit) > u64::from(capacity) => {
                    Err(problem(&format!(
                        "sweep_batch_size {explicit} exceeds the gas budget's capacity of {capacity} items",
                    )))
                }
                Some(explicit) => Ok(i64::from(explicit)),
                None => Ok(i64::from(capacity)),
            }?
        }
        // Unreachable: an explicit size without block_gas_limit is rejected
        // above, but the match needs the case.
        (Some(explicit), None) => i64::from(explicit),
    };
    Ok(SweepPolicy {
        batch_size,
        ..*base
    })
}

impl SweepScheduler {
    pub fn new(
        invoices: InvoiceRepository,
        publisher: Publisher,
        networks: Arc<ChainRegistry>,
        policy: &SweepPolicy,
        interval: Duration,
        wake: Arc<Notify>,
    ) -> Result<Self, SweepPolicyError> {
        let mut policies = Vec::with_capacity(networks.chains().len());
        for chain in networks.chains() {
            let resolved = resolve_sweep_policy(chain.chain_id, chain, policy)?;
            info!(
                chain_id = chain.chain_id,
                batch_size = resolved.batch_size,
                block_gas_limit = ?chain.block_gas_limit,
                transaction_gas_limit = ?chain.transaction_gas_limit,
                "sweep batch size resolved"
            );
            policies.push((chain.chain_id, resolved));
        }
        Ok(Self {
            invoices,
            publisher,
            policies,
            interval,
            wake,
        })
    }

    pub async fn run(self, mut shutdown: watch::Receiver<bool>) {
        info!(
            interval_ms = self.interval.as_millis() as u64,
            "sweep scheduler started"
        );
        loop {
            for (chain_id, policy) in &self.policies {
                match self.pass(*chain_id, policy).await {
                    Ok(report) if !report.jobs.is_empty() => {
                        info!(
                            chain_id = *chain_id,
                            jobs = report.jobs.len(),
                            deposit_requests = report.deposit_requests,
                            "sweep jobs scheduled"
                        );
                    }
                    Ok(_) => {}
                    Err(error) => {
                        warn!(chain_id = *chain_id, error = %error, "sweep scheduling pass failed; retrying next tick");
                    }
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(self.interval) => {}
                _ = self.wake.notified() => {}
                result = shutdown.changed() => {
                    if result.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
            }
        }
    }

    /// One pass over one chain: schedule until the queue is empty or the
    /// batch budget is spent, then report stuck jobs.
    pub async fn pass(
        &self,
        chain_id: u64,
        policy: &SweepPolicy,
    ) -> Result<PassReport, gum_ledger::SweepError> {
        let mut report = PassReport::default();
        for _ in 0..MAX_BATCHES_PER_PASS {
            match self
                .invoices
                .schedule_sweep_job(chain_id, policy, &self.publisher)
                .await?
            {
                Some(job) => {
                    info!(chain_id, job_id = %job.job_id, correlation_id = %job.correlation_id, deposit_requests = job.invoice_ids.len(), "sweep requested");
                    report.deposit_requests += job.invoice_ids.len();
                    report.jobs.push(job.job_id);
                }
                None => break,
            }
        }
        let now = chrono::Utc::now();
        for job in self.invoices.open_sweep_jobs(chain_id).await? {
            let age = now.signed_duration_since(job.created_at);
            if age.to_std().unwrap_or_default() > STUCK_JOB_AGE {
                error!(chain_id, job_id = %job.id, correlation_id = %job.correlation_id, age_secs = age.num_seconds(), submitted = job.submitted_tx_hash.is_some(), "sweep job open past the stuck threshold; check gum-signers");
            }
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::Address;
    use gum_core::{ChainConfig, Currency, FinalitySource, TokenConfig};

    fn chain(
        sweep_batch_size: Option<u32>,
        block_gas_limit: Option<u64>,
        transaction_gas_limit: Option<u64>,
    ) -> ChainConfig {
        ChainConfig {
            chain_id: 1,
            tokens: vec![TokenConfig {
                currency: Currency::Usdc,
                address: Address::repeat_byte(1),
            }],
            factory: Address::repeat_byte(2),
            batch_sweeper: Address::repeat_byte(3),
            factory_code_hash: Default::default(),
            batch_sweeper_code_hash: Default::default(),
            start_block: 0,
            finality_source: FinalitySource::Finalized,
            finality_confirmations: 0,
            block_time_ms: 1000,
            log_range_size: 100,
            signer_low_balance_wei: None,
            explorer_base_url: None,
            cctp: None,
            block_gas_limit,
            transaction_gas_limit,
            sweep_batch_size,
        }
    }

    fn base() -> SweepPolicy {
        SweepPolicy::default()
    }

    #[test]
    fn a_chain_without_gas_configuration_keeps_the_default_batch() {
        let resolved = resolve_sweep_policy(1, &chain(None, None, None), &base()).unwrap();
        assert_eq!(resolved.batch_size, 20);
    }

    #[test]
    fn the_batch_derives_from_the_gas_budget() {
        // Monad's profile: 150M block, 30M transaction cap. Half the block
        // is 75M, the cap binds at 30M, and 30M carries 74 items.
        let resolved = resolve_sweep_policy(
            1,
            &chain(None, Some(150_000_000), Some(30_000_000)),
            &base(),
        )
        .unwrap();
        assert_eq!(resolved.batch_size, 74);
        // Without the transaction cap the half-block budget binds instead:
        // (75M - 100k) / 400k = 187.
        let resolved =
            resolve_sweep_policy(1, &chain(None, Some(150_000_000), None), &base()).unwrap();
        assert_eq!(resolved.batch_size, 187);
    }

    #[test]
    fn an_explicit_batch_is_validated_against_the_budget() {
        let resolved = resolve_sweep_policy(
            1,
            &chain(Some(74), Some(150_000_000), Some(30_000_000)),
            &base(),
        )
        .unwrap();
        assert_eq!(resolved.batch_size, 74);
        let error = resolve_sweep_policy(
            1,
            &chain(Some(75), Some(150_000_000), Some(30_000_000)),
            &base(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("exceeds"), "{error}");
    }

    #[test]
    fn a_budget_that_cannot_carry_one_item_is_rejected() {
        let error =
            resolve_sweep_policy(1, &chain(None, Some(300_000), None), &base()).unwrap_err();
        assert!(error.to_string().contains("one item"), "{error}");
    }

    #[test]
    fn gas_fields_without_a_block_limit_are_rejected() {
        let error = resolve_sweep_policy(1, &chain(Some(50), None, None), &base()).unwrap_err();
        assert!(error.to_string().contains("block_gas_limit"), "{error}");
        let error =
            resolve_sweep_policy(1, &chain(None, None, Some(30_000_000)), &base()).unwrap_err();
        assert!(error.to_string().contains("block_gas_limit"), "{error}");
    }
}
