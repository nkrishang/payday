//! The deposit-request lifecycle as the server drives it, end to end
//! across the process boundary the bus represents:
//!
//! ```text
//! indexer reports range -> funded -> scheduler: SweepBatch command
//!   -> (gum-signers) SweepFinalized event -> handler -> fulfilled
//! ```
//!
//! `gum-signers` is played by publishing its events under its producer
//! name. The tests are about the seams the design relies on: the command
//! is on the bus in the same transaction that opened the job; a redelivered
//! event is acknowledged without a second effect; evidence that does not
//! fit the job is dead-lettered instead of retried; an abandoned or
//! rejected job puts its requests back in the queue; a halted chain
//! schedules nothing.

use std::sync::Arc;
use std::time::Duration;

use alloy_primitives::{Address, B256, U256};
use gum_bus::{
    BusError, Consumer, ConsumerOptions, Disposition, HandledDelivery, Publisher, handle_batch,
};
use gum_contracts::rpc::PaymentObservation;
use gum_contracts::{
    AbandonReason, CorrelationId, ExecutionCommand, ExecutionEvent, SweepItemOutcome,
    SweepItemResult,
};
use gum_core::{ChainConfig, ChainRegistry, Currency, FinalitySource, TokenConfig};
use gum_ledger::test_helpers::{
    TEST_TOKEN, account, insert_bound, issuance_input, test_payer_wallet,
};
use gum_ledger::{
    ChainFaultRepository, CursorRepository, DbInvoice, InvoiceRepository, StepPolicy, SweepPolicy,
    WithdrawalRepository,
};
use sqlx::PgPool;
use tokio::sync::Notify;
use uuid::Uuid;

use crate::execution_events::ExecutionEventHandler;
use crate::sweep_scheduler::SweepScheduler;

/// `gum_ledger::test_helpers::issuance_input` issues on chain 1.
const CHAIN: u64 = 1;
const AMOUNT: u64 = 1_000_000;
const SERVER: Publisher = Publisher::new("gum-server");
const SIGNERS: Publisher = Publisher::new("gum-signers");

fn registry() -> Arc<ChainRegistry> {
    Arc::new(
        ChainRegistry::new(vec![ChainConfig {
            chain_id: CHAIN,
            tokens: vec![TokenConfig {
                currency: Currency::Usdc,
                address: TEST_TOKEN,
            }],
            factory: Address::repeat_byte(0xfa),
            batch_sweeper: Address::repeat_byte(0x55),
            factory_code_hash: B256::ZERO,
            batch_sweeper_code_hash: B256::ZERO,
            start_block: 0,
            finality_source: FinalitySource::Finalized,
            finality_confirmations: 0,
            block_time_ms: 1000,
            log_range_size: 100,
            signer_low_balance_wei: None,
            explorer_base_url: None,
            cctp: None,
        }])
        .unwrap(),
    )
}

struct Harness {
    pool: PgPool,
    invoices: InvoiceRepository,
    scheduler: SweepScheduler,
    handler: ExecutionEventHandler,
    /// The server's own consumer of `execution.events`.
    server: Consumer,
    /// The signers' consumer of `execution.commands`, to see what the
    /// scheduler put on the bus.
    signers: Consumer,
    next_block: std::cell::Cell<u64>,
}

impl Harness {
    fn new(pool: &PgPool) -> Self {
        let invoices = InvoiceRepository::new(pool.clone());
        Self {
            pool: pool.clone(),
            scheduler: SweepScheduler::new(
                invoices.clone(),
                SERVER,
                registry(),
                SweepPolicy::default(),
                Duration::from_secs(5),
                Arc::new(Notify::new()),
            ),
            handler: ExecutionEventHandler::new(
                invoices.clone(),
                WithdrawalRepository::new(pool.clone()),
                SweepPolicy::default(),
                StepPolicy::default(),
            ),
            invoices,
            server: Consumer::new("gum-server", pool.clone(), ConsumerOptions::default()),
            signers: Consumer::new("gum-signers", pool.clone(), ConsumerOptions::default()),
            next_block: std::cell::Cell::new(100),
        }
    }

    async fn issue(&self, key: &str) -> DbInvoice {
        let owner = account(&self.pool, 1).await;
        insert_bound(&self.pool, &issuance_input(owner, key, None), None).await
    }

    /// The indexer reports one finalized range in which `sender` paid
    /// `amount` to the request's address.
    async fn pay(&self, invoice: &DbInvoice, sender: Address, amount: u64) {
        let block = self.next_block.get();
        self.next_block.set(block + 1);
        let cursor = CursorRepository::new(self.pool.clone())
            .get(CHAIN)
            .await
            .unwrap();
        let recipient = Address::from_slice(invoice.payment_address.as_deref().unwrap());
        self.invoices
            .apply_finalized_range(
                CHAIN,
                cursor,
                block,
                B256::with_last_byte(block as u8),
                1_700_000_000 + block,
                &[PaymentObservation {
                    token: TEST_TOKEN,
                    block_number: block,
                    block_hash: B256::with_last_byte(block as u8),
                    block_timestamp: 1_700_000_000 + block,
                    transaction_hash: B256::left_padding_from(&[0xAB, block as u8]),
                    transaction_index: 0,
                    log_index: 0,
                    sender,
                    recipient,
                    amount: U256::from(amount),
                }],
            )
            .await
            .unwrap();
    }

    async fn row(&self, id: Uuid) -> DbInvoice {
        self.invoices.find_by_id(id).await.unwrap().unwrap()
    }

    /// Commands the scheduler put on the bus, acknowledged as the signers
    /// would.
    async fn commands(&self) -> Vec<ExecutionCommand> {
        let deliveries = self.signers.claim::<ExecutionCommand>().await.unwrap();
        let mut commands = Vec::new();
        for delivery in deliveries {
            let mut tx = self.pool.begin().await.unwrap();
            self.signers.ack(&mut tx, &delivery).await.unwrap();
            tx.commit().await.unwrap();
            commands.push(delivery.message);
        }
        commands
    }

    /// `gum-signers` publishes `event`.
    async fn signers_publish(&self, event: &ExecutionEvent) -> Uuid {
        self.try_signers_publish(event).await.unwrap()
    }

    async fn try_signers_publish(&self, event: &ExecutionEvent) -> Result<Uuid, BusError> {
        let mut tx = self.pool.begin().await.unwrap();
        let outcome = SIGNERS
            .publish(&mut tx, event, CorrelationId::new(), None)
            .await?;
        tx.commit().await.unwrap();
        Ok(outcome.message_id())
    }

    /// The server's consumer takes one batch.
    async fn consume(&self) -> Vec<HandledDelivery> {
        handle_batch(&self.server, &self.handler).await.unwrap()
    }

    /// Model a second delivery of an already-handled message: the bus's
    /// at-least-once guarantee means a consumer must expect this (a lease
    /// that expired under a slow handler, an operator's `bus retry`).
    async fn redeliver(&self, message_id: Uuid) {
        sqlx::query(
            "UPDATE bus.deliveries SET state = 'pending', lease_token = NULL, leased_until = NULL, done_at = NULL, available_at = now() WHERE message_id = $1 AND consumer = 'gum-server'",
        )
        .bind(message_id)
        .execute(&self.pool)
        .await
        .unwrap();
    }

    fn finalized(job_id: Uuid, items: Vec<SweepItemResult>) -> ExecutionEvent {
        ExecutionEvent::SweepFinalized {
            job_id,
            chain_id: CHAIN,
            tx_hash: B256::repeat_byte(0xee),
            block: 500,
            block_hash: B256::repeat_byte(0x05),
            block_timestamp: 1_700_000_500,
            transaction_index: 3,
            items,
        }
    }
}

fn settled(invoice_id: Uuid) -> SweepItemResult {
    SweepItemResult {
        invoice_id,
        outcome: SweepItemOutcome::Settled {
            overpayment_recovered: U256::ZERO,
        },
    }
}

fn done(outcome: &HandledDelivery) -> bool {
    matches!(outcome, HandledDelivery::Done { .. })
}

#[sqlx::test(migrator = "gum_schema::MIGRATOR")]
async fn a_payment_becomes_a_command_and_the_signers_evidence_settles_it(pool: PgPool) {
    let h = Harness::new(&pool);
    let invoice = h.issue("lifecycle").await;
    assert_eq!(invoice.status, "created");

    // Payment detected by the indexer, reported through the ledger.
    h.pay(&invoice, test_payer_wallet(), AMOUNT).await;
    let funded = h.row(invoice.id).await;
    assert_eq!(funded.status, "funded");
    assert_eq!(funded.sweep_job_id, None);

    // The scheduler opens exactly one job and its command is on the bus.
    let report = h.scheduler.pass(CHAIN).await.unwrap();
    assert_eq!(report.jobs.len(), 1);
    assert_eq!(report.deposit_requests, 1);
    let job_id = report.jobs[0];
    assert_eq!(h.row(invoice.id).await.sweep_job_id, Some(job_id));
    let commands = h.commands().await;
    let ExecutionCommand::SweepBatch(command) = &commands[0] else {
        panic!("expected a sweep batch, got {commands:?}");
    };
    assert_eq!(commands.len(), 1);
    assert_eq!(command.job_id, job_id);
    assert_eq!(command.chain_id, CHAIN);
    assert_eq!(command.items.len(), 1);
    assert_eq!(command.items[0].invoice_id, invoice.id);
    assert_eq!(command.items[0].amount, U256::from(AMOUNT));
    assert_eq!(
        command.items[0].payment_address.as_slice(),
        invoice.payment_address.as_deref().unwrap()
    );

    // While the job is open the queue is empty: no second command.
    assert!(h.scheduler.pass(CHAIN).await.unwrap().jobs.is_empty());
    assert!(h.commands().await.is_empty());

    // The signers report a submission, then the finalized receipt.
    h.signers_publish(&ExecutionEvent::SweepSubmitted {
        job_id,
        chain_id: CHAIN,
        signer: Address::repeat_byte(0x51),
        nonce: 7,
        tx_hash: B256::repeat_byte(0xee),
        replacement: 0,
    })
    .await;
    let finalized = h
        .signers_publish(&Harness::finalized(job_id, vec![settled(invoice.id)]))
        .await;
    let outcomes = h.consume().await;
    assert_eq!(outcomes.len(), 2);
    assert!(outcomes.iter().all(done), "{outcomes:?}");

    let row = h.row(invoice.id).await;
    assert_eq!(row.status, "fulfilled");
    assert_eq!(row.sweep_job_id, None);
    assert_eq!(
        row.execute_tx_hash.as_deref(),
        Some(B256::repeat_byte(0xee).as_slice())
    );
    let job = h.invoices.sweep_job(job_id).await.unwrap().unwrap();
    assert!(job.resolved_at.is_some());

    // At-least-once: the same finalized event delivered again is
    // acknowledged and changes nothing.
    let settled_at = row.settled_at;
    h.redeliver(finalized).await;
    let outcomes = h.consume().await;
    assert_eq!(outcomes.len(), 1);
    assert!(done(&outcomes[0]), "{outcomes:?}");
    let again = h.row(invoice.id).await;
    assert_eq!(again.status, "fulfilled");
    assert_eq!(again.settled_at, settled_at);
    let recoveries: i64 =
        sqlx::query_scalar("SELECT count(*) FROM recovered_funds WHERE invoice_id = $1")
            .bind(invoice.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(recoveries, 0);

    // Nothing left for anyone.
    assert!(h.consume().await.is_empty());
    assert!(h.scheduler.pass(CHAIN).await.unwrap().jobs.is_empty());
}

#[sqlx::test(migrator = "gum_schema::MIGRATOR")]
async fn evidence_that_does_not_fit_the_job_is_dead_lettered_not_retried(pool: PgPool) {
    let h = Harness::new(&pool);
    let invoice = h.issue("mismatch").await;
    let stranger = h.issue("stranger").await;
    h.pay(&invoice, test_payer_wallet(), AMOUNT).await;
    let job_id = h.scheduler.pass(CHAIN).await.unwrap().jobs[0];
    h.commands().await;

    // A receipt for the wrong request: a bug in one of the two services,
    // which no retry fixes.
    let foreign = h
        .signers_publish(&Harness::finalized(job_id, vec![settled(stranger.id)]))
        .await;
    let outcomes = h.consume().await;
    assert_eq!(outcomes.len(), 1);
    match &outcomes[0] {
        HandledDelivery::Failed {
            message_id,
            disposition,
            error,
        } => {
            assert_eq!(*message_id, foreign);
            assert_eq!(*disposition, Disposition::Dead);
            assert!(error.contains(&stranger.id.to_string()), "{error}");
        }
        other => panic!("expected a dead letter, got {other:?}"),
    }
    let dead = h.server.dead(10).await.unwrap();
    assert_eq!(dead.len(), 1);
    assert_eq!(dead[0].message_id, foreign);
    assert_eq!(dead[0].kind, "sweep_finalized");

    // The job and both requests are untouched.
    assert_eq!(h.row(invoice.id).await.status, "funded");
    assert_eq!(h.row(invoice.id).await.sweep_job_id, Some(job_id));
    assert_eq!(h.row(stranger.id).await.status, "created");
    assert!(
        h.invoices
            .sweep_job(job_id)
            .await
            .unwrap()
            .unwrap()
            .resolved_at
            .is_none()
    );

    // An operator's retry of the dead letter reaches the same verdict.
    assert!(h.server.retry_dead(foreign).await.unwrap());
    let outcomes = h.consume().await;
    assert!(matches!(
        outcomes.as_slice(),
        [HandledDelivery::Failed {
            disposition: Disposition::Dead,
            ..
        }]
    ));

    // One job has one finalized receipt, ever: the signers cannot publish
    // a second, different one under the same key. Resolving this job is an
    // operator's job (`gum-server bus dead` names it), not a retry's.
    let refused = h
        .try_signers_publish(&Harness::finalized(job_id, vec![settled(invoice.id)]))
        .await;
    assert!(
        matches!(refused, Err(BusError::ConflictingPayload { .. })),
        "{refused:?}"
    );
}

#[sqlx::test(migrator = "gum_schema::MIGRATOR")]
async fn abandoned_and_rejected_jobs_return_their_requests_to_the_queue(pool: PgPool) {
    let h = Harness::new(&pool);
    let invoice = h.issue("abandon").await;
    h.pay(&invoice, test_payer_wallet(), AMOUNT).await;
    let first = h.scheduler.pass(CHAIN).await.unwrap().jobs[0];
    h.commands().await;

    // The signers gave up on the transaction (a stranger consumed the
    // nonce). The request is funded and queued again; the job is closed.
    let abandoned = h
        .signers_publish(&ExecutionEvent::SweepAbandoned {
            job_id: first,
            chain_id: CHAIN,
            reason: AbandonReason::NonceConsumed {
                signer: Address::repeat_byte(0x51),
                nonce: 9,
            },
        })
        .await;
    assert!(h.consume().await.iter().all(done));
    let row = h.row(invoice.id).await;
    assert_eq!(row.status, "funded");
    assert_eq!(row.sweep_job_id, None);
    assert!(
        h.invoices
            .sweep_job(first)
            .await
            .unwrap()
            .unwrap()
            .resolved_at
            .is_some()
    );

    // A retry is a normal path: the backoff the ledger applies is what
    // stands between the request and its next job, nothing else. Clear it
    // to see the next job scheduled under a fresh id and a fresh command.
    sqlx::query("UPDATE invoices SET last_attempt_at = NULL WHERE id = $1")
        .bind(invoice.id)
        .execute(&pool)
        .await
        .unwrap();
    let second = h.scheduler.pass(CHAIN).await.unwrap().jobs[0];
    assert_ne!(second, first);
    let commands = h.commands().await;
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].job_id(), second);

    // The signers refuse the second command outright (say, a chain the
    // binary was not configured for): same treatment.
    h.signers_publish(&ExecutionEvent::ExecutionRejected {
        job_id: second,
        chain_id: CHAIN,
        reason: "chain 1 is not configured".into(),
    })
    .await;
    assert!(h.consume().await.iter().all(done));
    let row = h.row(invoice.id).await;
    assert_eq!(row.status, "funded");
    assert_eq!(row.sweep_job_id, None);

    // A late duplicate of the abandonment for the already-closed first job
    // (the signers replaying their own outbox after a restart) is a no-op
    // at the publisher: same key, same payload, no new delivery.
    let replay = h
        .try_signers_publish(&ExecutionEvent::SweepAbandoned {
            job_id: first,
            chain_id: CHAIN,
            reason: AbandonReason::NonceConsumed {
                signer: Address::repeat_byte(0x51),
                nonce: 9,
            },
        })
        .await
        .unwrap();
    assert_eq!(replay, abandoned);
    assert!(h.consume().await.is_empty());
    // A *different* conclusion for a job that already has one is a bug
    // upstream and is refused outright.
    let refused = h
        .try_signers_publish(&ExecutionEvent::SweepAbandoned {
            job_id: first,
            chain_id: CHAIN,
            reason: AbandonReason::Reverted {
                tx_hash: B256::repeat_byte(0xdd),
            },
        })
        .await;
    assert!(
        matches!(refused, Err(BusError::ConflictingPayload { .. })),
        "{refused:?}"
    );
    assert_eq!(h.row(invoice.id).await.sweep_job_id, None);
}

#[sqlx::test(migrator = "gum_schema::MIGRATOR")]
async fn a_halted_chain_schedules_nothing_until_it_is_resumed(pool: PgPool) {
    let h = Harness::new(&pool);
    let faults = ChainFaultRepository::new(pool.clone(), SERVER);
    let invoice = h.issue("halted").await;
    h.pay(&invoice, test_payer_wallet(), AMOUNT).await;

    faults
        .raise(
            CHAIN,
            "finalized block 90 changed hash",
            Some(90),
            CorrelationId::new(),
        )
        .await
        .unwrap();
    assert!(h.scheduler.pass(CHAIN).await.unwrap().jobs.is_empty());
    assert!(h.commands().await.is_empty());
    assert_eq!(h.row(invoice.id).await.status, "funded");

    faults.clear(CHAIN, CorrelationId::new()).await.unwrap();
    assert_eq!(h.scheduler.pass(CHAIN).await.unwrap().jobs.len(), 1);
    assert_eq!(h.commands().await.len(), 1);
}
