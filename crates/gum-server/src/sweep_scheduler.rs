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
    networks: Arc<ChainRegistry>,
    policy: SweepPolicy,
    interval: Duration,
    wake: Arc<Notify>,
}

impl SweepScheduler {
    pub fn new(
        invoices: InvoiceRepository,
        publisher: Publisher,
        networks: Arc<ChainRegistry>,
        policy: SweepPolicy,
        interval: Duration,
        wake: Arc<Notify>,
    ) -> Self {
        Self {
            invoices,
            publisher,
            networks,
            policy,
            interval,
            wake,
        }
    }

    pub async fn run(self, mut shutdown: watch::Receiver<bool>) {
        info!(
            interval_ms = self.interval.as_millis() as u64,
            "sweep scheduler started"
        );
        loop {
            for chain in self.networks.chains() {
                match self.pass(chain.chain_id).await {
                    Ok(report) if !report.jobs.is_empty() => {
                        info!(
                            chain_id = chain.chain_id,
                            jobs = report.jobs.len(),
                            deposit_requests = report.deposit_requests,
                            "sweep jobs scheduled"
                        );
                    }
                    Ok(_) => {}
                    Err(error) => {
                        warn!(chain_id = chain.chain_id, error = %error, "sweep scheduling pass failed; retrying next tick");
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
    pub async fn pass(&self, chain_id: u64) -> Result<PassReport, gum_ledger::SweepError> {
        let mut report = PassReport::default();
        for _ in 0..MAX_BATCHES_PER_PASS {
            match self
                .invoices
                .schedule_sweep_job(chain_id, &self.policy, &self.publisher)
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
