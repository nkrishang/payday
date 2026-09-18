//! Bus handlers: they only accept work into the `execution` schema and wake
//! the chain worker. Everything that talks to a node or a key happens in
//! the worker, never inside a delivery lease.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use gum_bus::{Delivery, Handler, HandlerError, Publisher};
use gum_contracts::{ChainControl, ExecutionCommand, ExecutionEvent};
use sqlx::{Postgres, Transaction};
use tokio::sync::Notify;
use tracing::{info, warn};

use crate::store::{self, JobState};

/// Accepts `ExecutionCommand`s as jobs. A command for a chain this process
/// does not serve is rejected at once, in the same transaction that
/// acknowledges it, so the server learns immediately instead of waiting
/// for a job that would never start.
#[derive(Clone)]
pub struct CommandHandler {
    chains: Arc<HashSet<u64>>,
    wakers: Arc<HashMap<u64, Arc<Notify>>>,
    publisher: Publisher,
}

impl CommandHandler {
    pub fn new(wakers: Arc<HashMap<u64, Arc<Notify>>>) -> Self {
        Self {
            chains: Arc::new(wakers.keys().copied().collect()),
            wakers,
            publisher: Publisher::new("gum-signers"),
        }
    }
}

#[async_trait]
impl Handler<ExecutionCommand> for CommandHandler {
    async fn handle(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        delivery: &Delivery<ExecutionCommand>,
    ) -> Result<(), HandlerError> {
        let command = &delivery.message;
        let chain_id = command.chain_id();
        let rejection =
            (!self.chains.contains(&chain_id)).then(|| ExecutionEvent::ExecutionRejected {
                job_id: command.job_id(),
                chain_id,
                reason: format!("chain {chain_id} is not served by this executor"),
            });
        let state = if rejection.is_some() {
            JobState::Rejected
        } else {
            JobState::Queued
        };
        let inserted = store::insert_job(
            tx,
            command,
            delivery.correlation_id.0,
            delivery.message_id,
            state,
            rejection.as_ref(),
        )
        .await?;
        if !inserted {
            info!(job_id = %command.job_id(), "command redelivered for a known job; ignored");
            return Ok(());
        }
        match rejection {
            Some(event) => {
                warn!(job_id = %command.job_id(), chain_id, "command rejected: unknown chain");
                self.publisher
                    .publish(
                        tx,
                        &event,
                        delivery.correlation_id,
                        Some(delivery.message_id),
                    )
                    .await?;
            }
            None => {
                info!(job_id = %command.job_id(), chain_id, message_type = delivery.kind, "job accepted");
                if let Some(waker) = self.wakers.get(&chain_id) {
                    waker.notify_one();
                }
            }
        }
        Ok(())
    }
}

/// Mirrors `ChainControl` into `execution.chain_halts`. Halted chains start
/// no new transactions; open ones are still reconciled to a conclusion.
#[derive(Clone, Default)]
pub struct ChainControlHandler;

#[async_trait]
impl Handler<ChainControl> for ChainControlHandler {
    async fn handle(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        delivery: &Delivery<ChainControl>,
    ) -> Result<(), HandlerError> {
        store::apply_chain_control(tx, &delivery.message).await?;
        match &delivery.message {
            ChainControl::Halted {
                chain_id, reason, ..
            } => {
                warn!(
                    chain_id,
                    reason, "chain halted; no new transactions will start"
                )
            }
            ChainControl::Resumed { chain_id, .. } => info!(chain_id, "chain resumed"),
        }
        Ok(())
    }
}
