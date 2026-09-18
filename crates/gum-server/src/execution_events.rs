//! The server's consumer of `execution.events`: the signers' evidence
//! becomes deposit-request and withdrawal lifecycle transitions.
//!
//! The bus delivers at least once, so every branch here is idempotent by
//! construction: the ledger's `apply_*` functions lock the job or step row
//! and report [`SweepError::NotOpen`] / [`StepError::NotOpen`] when it was
//! already resolved, which a duplicate delivery acknowledges as "nothing
//! to do". Every write goes through the transaction the bus commits the
//! acknowledgement in, so a crash between the two leaves the delivery
//! unacknowledged and the transition unapplied, and the redelivery does
//! both.
//!
//! Evidence the ledger cannot make sense of (an item for a request that is
//! not in the job, a result for a step the leg is not running) is a bug in
//! one of the two services, never something a retry fixes; those deliveries
//! are dead-lettered for an operator (`gum-server bus dead`).

use async_trait::async_trait;
use gum_bus::{Delivery, Handler, HandlerError};
use gum_contracts::ExecutionEvent;
use gum_ledger::{
    FinalizedSweep, InvoiceRepository, OnboardingDemoPaymentRepository, StepApplied, StepError,
    StepPolicy, SweepError, SweepPolicy, WithdrawalRepository,
};
use sqlx::{Postgres, Transaction};
use tracing::{error, info, warn};

pub struct ExecutionEventHandler {
    invoices: InvoiceRepository,
    withdrawals: WithdrawalRepository,
    onboarding: OnboardingDemoPaymentRepository,
    sweep_policy: SweepPolicy,
    step_policy: StepPolicy,
}

impl ExecutionEventHandler {
    pub fn new(
        invoices: InvoiceRepository,
        withdrawals: WithdrawalRepository,
        sweep_policy: SweepPolicy,
        step_policy: StepPolicy,
    ) -> Self {
        Self {
            onboarding: OnboardingDemoPaymentRepository::new(invoices.pool().clone()),
            invoices,
            withdrawals,
            sweep_policy,
            step_policy,
        }
    }
}

/// How the ledger's verdict on sweep evidence maps onto the bus.
fn sweep_outcome(result: Result<(), SweepError>) -> Result<(), HandlerError> {
    match result {
        Ok(()) => Ok(()),
        Err(SweepError::NotOpen(job_id)) => {
            info!(%job_id, "sweep evidence for a resolved job; duplicate delivery acknowledged");
            Ok(())
        }
        Err(SweepError::Database(error)) => Err(error.into()),
        Err(SweepError::Bus(error)) => Err(error.into()),
        Err(error @ (SweepError::ForeignItem { .. } | SweepError::MissingOutcomes { .. })) => {
            error!(error = %error, "sweep evidence does not match the job; dead-lettering");
            Err(HandlerError::permanent(error))
        }
        Err(error @ (SweepError::Unbound(_) | SweepError::Decode(_, _))) => {
            error!(error = %error, "ledger rows behind the job are corrupt; dead-lettering");
            Err(HandlerError::permanent(error))
        }
    }
}

#[async_trait]
impl Handler<ExecutionEvent> for ExecutionEventHandler {
    async fn handle(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        delivery: &Delivery<ExecutionEvent>,
    ) -> Result<(), HandlerError> {
        let job_id = delivery.message.job_id();
        let chain_id = delivery.message.chain_id();
        match &delivery.message {
            ExecutionEvent::OnboardingPaymentSubmitted { tx_hash, .. } => {
                self.onboarding
                    .record_submission(tx, job_id, *tx_hash)
                    .await?;
                info!(%job_id, %tx_hash, "onboarding payment submitted");
                Ok(())
            }
            ExecutionEvent::SweepSubmitted {
                tx_hash,
                signer,
                nonce,
                replacement,
                ..
            } => {
                info!(%job_id, chain_id, %tx_hash, %signer, nonce, replacement, "sweep submitted");
                self.invoices
                    .record_sweep_submission(tx, job_id, *tx_hash)
                    .await?;
                Ok(())
            }
            ExecutionEvent::SweepFinalized {
                tx_hash,
                block,
                block_hash,
                block_timestamp,
                transaction_index,
                items,
                ..
            } => {
                let receipt = FinalizedSweep {
                    job_id,
                    tx_hash: *tx_hash,
                    block: *block,
                    block_hash: *block_hash,
                    block_timestamp: *block_timestamp,
                    transaction_index: *transaction_index,
                    items,
                };
                let result = self
                    .invoices
                    .apply_sweep_finalized(tx, &receipt, &self.sweep_policy)
                    .await;
                match &result {
                    Ok(applied) => {
                        info!(
                            %job_id, chain_id, %tx_hash, block,
                            settled = applied.settled.len(),
                            collected = applied.collected.len(),
                            returned = applied.returned.len(),
                            requeued = applied.requeued.len(),
                            attention = applied.attention.len(),
                            "sweep finalized; deposit requests settled"
                        );
                        for (deposit_request_id, reason) in &applied.attention {
                            warn!(%job_id, %deposit_request_id, reason, "deposit request needs operator attention");
                        }
                    }
                    Err(error) => {
                        warn!(%job_id, chain_id, error = %error, "sweep evidence not applied")
                    }
                }
                sweep_outcome(result.map(drop))
            }
            ExecutionEvent::SweepAbandoned { reason, .. } => {
                let result = self
                    .invoices
                    .apply_sweep_abandoned(tx, job_id, reason)
                    .await;
                if let Ok(requeued) = &result {
                    warn!(%job_id, chain_id, ?reason, requeued, "sweep abandoned; deposit requests returned to the queue");
                }
                sweep_outcome(result.map(drop))
            }
            ExecutionEvent::WithdrawalStepFinalized {
                leg_id,
                step,
                result,
                ..
            } => {
                match self
                    .withdrawals
                    .apply_step_result(tx, job_id, *step, result, &self.step_policy)
                    .await
                {
                    Ok(applied) => {
                        match applied {
                            StepApplied::Advanced { state, .. } => {
                                info!(%job_id, %leg_id, chain_id, ?step, state = state.as_str(), "withdrawal step finalized")
                            }
                            StepApplied::Retried { reverts, .. } => {
                                warn!(%job_id, %leg_id, chain_id, ?step, reverts, "withdrawal step reverted; retrying after backoff")
                            }
                            StepApplied::Failed { .. } => {
                                error!(%job_id, %leg_id, chain_id, ?step, ?result, "withdrawal leg failed")
                            }
                            StepApplied::Released { .. } => {
                                warn!(%job_id, %leg_id, chain_id, ?step, "withdrawal step released unexecuted; leg back in its queue")
                            }
                        }
                        Ok(())
                    }
                    Err(StepError::NotOpen(_)) => {
                        info!(%job_id, %leg_id, "step result for a step no leg runs; duplicate delivery acknowledged");
                        Ok(())
                    }
                    Err(StepError::Database(error)) => Err(error.into()),
                    Err(error @ StepError::Mismatch { .. }) => {
                        error!(%job_id, %leg_id, error = %error, "step result names the wrong step; dead-lettering");
                        Err(HandlerError::permanent(error))
                    }
                }
            }
            ExecutionEvent::ExecutionStalled {
                signer,
                submissions,
                ..
            } => {
                // An alert, not a transition: the signers keep re-broadcasting
                // the newest attempt and reconciling it, so no ledger state
                // changes until they report a conclusion.
                error!(%job_id, chain_id, %signer, submissions, "execution stalled: transaction replaced past the limit without mining; operator attention");
                Ok(())
            }
            ExecutionEvent::ExecutionRejected { reason, .. } => {
                // The job id is either a sweep job or a withdrawal step;
                // whichever is open takes the rejection.
                match self.invoices.apply_sweep_rejected(tx, job_id, reason).await {
                    Ok(requeued) => {
                        error!(%job_id, chain_id, reason, requeued, "sweep command rejected by the signers; deposit requests returned to the queue");
                        return Ok(());
                    }
                    Err(SweepError::NotOpen(_)) => {}
                    Err(error) => return sweep_outcome(Err(error)),
                }
                if self.withdrawals.reject_step(tx, job_id, reason).await? {
                    error!(%job_id, chain_id, reason, "withdrawal step rejected by the signers; leg failed");
                } else {
                    info!(%job_id, chain_id, reason, "rejection for a job nothing runs; duplicate delivery acknowledged");
                }
                Ok(())
            }
        }
    }
}
