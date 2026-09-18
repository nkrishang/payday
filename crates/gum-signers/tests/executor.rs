//! Executor behaviour against a mock chain and a real Postgres schema.
//!
//! Every test drives `ChainWorker::pass` by hand, so each pass boundary is
//! also a simulated crash-and-restart: nothing carries over between passes
//! except the database and the chain.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use alloy_primitives::{Address, B256, Bytes, U256};
use chrono::Utc;
use gum_bus::{BackoffPolicy, Delivery, Handler};
use gum_chain::SweepOutcome;
use gum_chain::mock::{self, MockChain};
use gum_contracts::{
    AbandonReason, BusMessage, ChainControl, CorrelationId, ExecutionCommand, ExecutionEvent,
    StepPrecondition, StepResult, SweepBatchCommand, SweepItem, SweepItemOutcome, SweepItemStatus,
    WithdrawalStepCommand, WithdrawalStepKind,
};
use gum_core::{
    Amount, BeneficiaryAddress, ChainConfig, ChainId, Currency, FinalitySource, RecoveryAddress,
    Salt, TokenAddress, TokenConfig,
};
use gum_signers::{ChainControlHandler, ChainWorker, CommandHandler, Policy, store};
use sqlx::PgPool;
use tokio::sync::Notify;
use uuid::Uuid;

const LATEST: u64 = 100;

fn config() -> ChainConfig {
    ChainConfig {
        chain_id: mock::CHAIN_ID,
        tokens: vec![TokenConfig {
            currency: Currency::Usdc,
            address: mock::usdc(),
        }],
        factory: mock::factory().0,
        batch_sweeper: mock::batch_sweeper(),
        factory_code_hash: B256::ZERO,
        batch_sweeper_code_hash: B256::ZERO,
        start_block: 1,
        finality_source: FinalitySource::Finalized,
        finality_confirmations: 0,
        block_time_ms: 1_000,
        log_range_size: 50,
        signer_low_balance_wei: None,
        explorer_base_url: None,
        cctp: None,
    }
}

fn policy() -> Policy {
    Policy {
        // Zero: every unmined transaction is "past the timeout" on the
        // next pass, so replacement paths need no sleeping.
        pending_timeout: Duration::ZERO,
        max_submissions: 2,
        max_attempts: 2,
        retry_backoff: BackoffPolicy::new(Duration::ZERO, Duration::ZERO),
        low_balance_wei: U256::ZERO,
    }
}

struct Harness {
    pool: PgPool,
    chain: Arc<MockChain>,
    worker: ChainWorker,
    commands: CommandHandler,
}

impl Harness {
    fn new(pool: PgPool, chain: MockChain) -> Self {
        Self::with_policy(pool, chain, policy())
    }

    fn with_policy(pool: PgPool, chain: MockChain, policy: Policy) -> Self {
        let chain = Arc::new(chain);
        let wake = Arc::new(Notify::new());
        let worker = ChainWorker::new(chain.clone(), config(), pool.clone(), policy, wake.clone());
        let wakers = Arc::new(HashMap::from([(mock::CHAIN_ID, wake)]));
        Self {
            pool,
            chain,
            worker,
            commands: CommandHandler::new(wakers),
        }
    }

    /// Deliver a command the way the bus consumer does, in its own
    /// transaction, and commit. Returns the delivery's message id.
    async fn deliver(&self, command: &ExecutionCommand) -> Uuid {
        let delivery = delivery(command.clone(), Uuid::now_v7());
        let mut tx = self.pool.begin().await.unwrap();
        self.commands.handle(&mut tx, &delivery).await.unwrap();
        tx.commit().await.unwrap();
        delivery.message_id
    }

    async fn control(&self, control: ChainControl) {
        let delivery = delivery(control, Uuid::now_v7());
        let mut tx = self.pool.begin().await.unwrap();
        ChainControlHandler
            .handle(&mut tx, &delivery)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    async fn pass(&self) -> gum_signers::PassReport {
        let report = self.worker.pass().await;
        assert!(
            report.errors.is_empty(),
            "pass reported errors: {:?}",
            report.errors
        );
        report
    }

    async fn events(&self) -> Vec<ExecutionEvent> {
        let rows: Vec<serde_json::Value> = sqlx::query_scalar(
            "SELECT payload FROM bus.messages WHERE topic = $1 ORDER BY published_at, id",
        )
        .bind(ExecutionEvent::TOPIC)
        .fetch_all(&self.pool)
        .await
        .unwrap();
        rows.into_iter()
            .map(|payload| serde_json::from_value(payload).unwrap())
            .collect()
    }

    async fn causation_ids(&self) -> Vec<Option<Uuid>> {
        sqlx::query_scalar(
            "SELECT causation_id FROM bus.messages WHERE topic = $1 ORDER BY published_at, id",
        )
        .bind(ExecutionEvent::TOPIC)
        .fetch_all(&self.pool)
        .await
        .unwrap()
    }

    async fn job_state(&self, job_id: Uuid) -> String {
        sqlx::query_scalar("SELECT state FROM execution.jobs WHERE id = $1")
            .bind(job_id)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn open(&self) -> Vec<store::OpenTransaction> {
        store::open_transactions(&self.pool, mock::CHAIN_ID)
            .await
            .unwrap()
    }
}

fn delivery<M: BusMessage>(message: M, message_id: Uuid) -> Delivery<M> {
    Delivery {
        message_id,
        kind: message.kind().to_owned(),
        message,
        correlation_id: CorrelationId::new(),
        causation_id: None,
        published_at: Utc::now(),
        attempt: 1,
        lease_token: Uuid::now_v7(),
    }
}

fn item(salt: u8, amount: u64) -> SweepItem {
    let receiver = Address::repeat_byte(0xBE);
    let recovery = Address::repeat_byte(0xEC);
    let salt = B256::repeat_byte(salt);
    let expiration_timestamp = 4_000_000_000;
    let payment_address = gum_core::predict_payment_address(
        mock::factory(),
        TokenAddress(mock::usdc()),
        Amount(U256::from(amount)),
        BeneficiaryAddress(receiver),
        expiration_timestamp,
        RecoveryAddress(recovery),
        Salt(salt),
        ChainId(mock::CHAIN_ID),
    )
    .0;
    SweepItem {
        invoice_id: Uuid::now_v7(),
        payment_address,
        currency: Currency::Usdc,
        token: mock::usdc(),
        amount: U256::from(amount),
        receiver,
        expiration_timestamp,
        recovery,
        salt,
        status: SweepItemStatus::Open,
        settlement_recorded: false,
        first_observed_block: Some(1),
    }
}

fn sweep(items: Vec<SweepItem>) -> ExecutionCommand {
    ExecutionCommand::SweepBatch(SweepBatchCommand {
        job_id: Uuid::now_v7(),
        chain_id: mock::CHAIN_ID,
        items,
    })
}

fn step(precondition: Option<StepPrecondition>, not_after: Option<u64>) -> ExecutionCommand {
    ExecutionCommand::WithdrawalStep(WithdrawalStepCommand {
        job_id: Uuid::now_v7(),
        leg_id: Uuid::now_v7(),
        chain_id: mock::CHAIN_ID,
        step: WithdrawalStepKind::Transfer,
        to: mock::usdc(),
        calldata: Bytes::from_static(&[0xAB, 0xCD]),
        gas_limit: 120_000,
        precondition,
        not_after,
    })
}

fn authorization(nonce: B256) -> StepPrecondition {
    StepPrecondition::AuthorizationUnused {
        token: mock::usdc(),
        authorizer: Address::repeat_byte(0xA0),
        nonce,
        search_from_block: 1,
    }
}

// ---------------------------------------------------------------------------
// Accepting commands
// ---------------------------------------------------------------------------

#[sqlx::test(migrator = "gum_schema::MIGRATOR")]
async fn a_redelivered_command_creates_one_job_and_no_event(pool: PgPool) {
    let h = Harness::new(pool, MockChain::new(LATEST).with(|s| s.mine_at = None));
    let command = sweep(vec![item(1, 10)]);

    h.deliver(&command).await;
    h.deliver(&command).await;

    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM execution.jobs")
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(jobs, 1);
    assert_eq!(h.job_state(command.job_id()).await, "queued");
    assert!(h.events().await.is_empty());
}

#[sqlx::test(migrator = "gum_schema::MIGRATOR")]
async fn a_command_for_an_unserved_chain_is_rejected_at_acceptance(pool: PgPool) {
    let h = Harness::new(pool, MockChain::new(LATEST));
    let mut command = sweep(vec![item(1, 10)]);
    let ExecutionCommand::SweepBatch(batch) = &mut command else {
        unreachable!()
    };
    batch.chain_id = 999;

    let message_id = h.deliver(&command).await;
    h.pass().await;

    assert_eq!(h.job_state(command.job_id()).await, "rejected");
    assert!(h.chain.submissions().is_empty());
    match h.events().await.as_slice() {
        [
            ExecutionEvent::ExecutionRejected {
                job_id, chain_id, ..
            },
        ] => {
            assert_eq!(*job_id, command.job_id());
            assert_eq!(*chain_id, 999);
        }
        other => panic!("unexpected events: {other:?}"),
    }
    assert_eq!(h.causation_ids().await, vec![Some(message_id)]);
}

// ---------------------------------------------------------------------------
// Sweeps
// ---------------------------------------------------------------------------

#[sqlx::test(migrator = "gum_schema::MIGRATOR")]
async fn a_sweep_batch_is_submitted_finalized_and_classified_per_item(pool: PgPool) {
    let settled = item(1, 10);
    let returned = item(2, 20);
    let chain = MockChain::new(LATEST).with(|s| {
        s.next_outcomes.insert(
            settled.payment_address,
            SweepOutcome::Settled {
                amount: U256::from(10),
                recovered_amount: U256::from(3),
            },
        );
        s.next_outcomes.insert(
            returned.payment_address,
            SweepOutcome::Recovered {
                amount: U256::from(20),
            },
        );
    });
    let h = Harness::new(pool, chain);
    let command = sweep(vec![settled.clone(), returned.clone()]);
    let message_id = h.deliver(&command).await;

    let first = h.pass().await;
    assert_eq!((first.started, first.resolved), (1, 0));
    assert_eq!(h.job_state(command.job_id()).await, "executing");
    let submissions = h.chain.submissions();
    assert_eq!(submissions.len(), 1);
    assert_eq!(submissions[0].nonce, 0);
    assert_eq!(submissions[0].sweeps.len(), 2);

    // "Restart": the next pass finds the receipt, the block is final.
    let second = h.pass().await;
    assert_eq!((second.reconciled, second.resolved), (1, 1));
    assert_eq!(h.job_state(command.job_id()).await, "finalized");
    assert!(h.open().await.is_empty());
    assert!(
        store::busy_signers(&h.pool, mock::CHAIN_ID)
            .await
            .unwrap()
            .is_empty()
    );

    let events = h.events().await;
    assert_eq!(events.len(), 2, "{events:?}");
    match &events[0] {
        ExecutionEvent::SweepSubmitted {
            job_id,
            signer,
            nonce,
            tx_hash,
            replacement,
            ..
        } => {
            assert_eq!(*job_id, command.job_id());
            assert_eq!(*signer, mock::mock_signer(0));
            assert_eq!((*nonce, *replacement), (0, 0));
            assert_eq!(*tx_hash, submissions[0].tx_hash);
        }
        other => panic!("unexpected first event: {other:?}"),
    }
    match &events[1] {
        ExecutionEvent::SweepFinalized {
            job_id,
            tx_hash,
            block,
            block_hash,
            items,
            ..
        } => {
            assert_eq!(*job_id, command.job_id());
            assert_eq!(*tx_hash, submissions[0].tx_hash);
            assert_eq!(*block, LATEST);
            assert_eq!(*block_hash, mock::block_hash(LATEST));
            assert_eq!(items.len(), 2);
            assert_eq!(items[0].invoice_id, settled.invoice_id);
            assert_eq!(
                items[0].outcome,
                SweepItemOutcome::Settled {
                    overpayment_recovered: U256::from(3)
                }
            );
            assert_eq!(items[1].invoice_id, returned.invoice_id);
            assert_eq!(
                items[1].outcome,
                SweepItemOutcome::Returned {
                    amount: U256::from(20)
                }
            );
        }
        other => panic!("unexpected second event: {other:?}"),
    }
    // Both events are caused by the command's bus message.
    assert_eq!(
        h.causation_ids().await,
        vec![Some(message_id), Some(message_id)]
    );

    // A third pass has nothing to do and publishes nothing more.
    let third = h.pass().await;
    assert_eq!(third, gum_signers::PassReport::default());
    assert_eq!(h.events().await.len(), 2);
}

#[sqlx::test(migrator = "gum_schema::MIGRATOR")]
async fn signed_bytes_that_never_left_are_resent_unchanged_after_restart(pool: PgPool) {
    let chain = MockChain::new(LATEST).with(|s| {
        s.submit_error = Some(|| gum_chain::ChainError::Transient("node down".into()));
    });
    let h = Harness::new(pool, chain);
    let command = sweep(vec![item(1, 10)]);
    h.deliver(&command).await;

    // Signing succeeds, broadcasting fails: the attempt is durable without
    // a broadcast timestamp.
    let first = h.pass().await;
    assert_eq!(first.started, 1);
    assert!(h.chain.submissions().is_empty());
    let open = h.open().await;
    assert_eq!(open.len(), 1);
    let attempt = open[0].newest_attempt().clone();
    assert!(attempt.broadcast_at.is_none());
    assert!(
        h.events().await.is_empty(),
        "nothing was submitted, so nothing is announced"
    );

    // Restart: the exact same bytes go out, nothing new is signed.
    h.pass().await;
    let submissions = h.chain.submissions();
    assert_eq!(submissions.len(), 1);
    assert_eq!(submissions[0].tx_hash, attempt.tx_hash);
    let open = h.open().await;
    assert_eq!(open[0].attempts.len(), 1);
    assert!(open[0].newest_attempt().broadcast_at.is_some());
    assert!(matches!(
        h.events().await.as_slice(),
        [ExecutionEvent::SweepSubmitted { tx_hash, .. }] if *tx_hash == attempt.tx_hash
    ));

    // And the receipt the broadcast produced finalizes it.
    h.pass().await;
    assert_eq!(h.job_state(command.job_id()).await, "finalized");
}

#[sqlx::test(migrator = "gum_schema::MIGRATOR")]
async fn an_unmined_transaction_is_replaced_with_higher_fees_then_stalls_once(pool: PgPool) {
    let h = Harness::new(pool, MockChain::new(LATEST).with(|s| s.mine_at = None));
    let command = sweep(vec![item(1, 10)]);
    h.deliver(&command).await;

    h.pass().await; // sign + broadcast attempt 0
    h.pass().await; // past the (zero) timeout: replace with attempt 1
    let submissions = h.chain.submissions();
    assert_eq!(submissions.len(), 2);
    assert_eq!(
        submissions[0].nonce, submissions[1].nonce,
        "a replacement keeps the nonce"
    );
    assert_ne!(submissions[0].tx_hash, submissions[1].tx_hash);
    assert!(submissions[1].fees.max_fee_per_gas > submissions[0].fees.max_fee_per_gas);
    assert!(
        submissions[1].fees.max_priority_fee_per_gas > submissions[0].fees.max_priority_fee_per_gas
    );
    let open = h.open().await;
    assert_eq!(open[0].attempts.len(), 2);

    // At the replacement limit: one stall alert, no more fee raises, but
    // the newest attempt is kept alive.
    h.pass().await;
    h.pass().await;
    let submissions = h.chain.submissions();
    assert_eq!(submissions.len(), 2, "no third transaction is signed");
    let events = h.events().await;
    let stalls = events
        .iter()
        .filter(|event| matches!(event, ExecutionEvent::ExecutionStalled { .. }))
        .count();
    assert_eq!(stalls, 1, "{events:?}");
    let submitted: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            ExecutionEvent::SweepSubmitted { replacement, .. } => Some(*replacement),
            _ => None,
        })
        .collect();
    assert_eq!(submitted, vec![0, 1]);
    assert_eq!(h.job_state(command.job_id()).await, "executing");
    assert!(h.open().await[0].stall_reported_at.is_some());
}

#[sqlx::test(migrator = "gum_schema::MIGRATOR")]
async fn a_nonce_consumed_by_a_foreign_transaction_abandons_and_frees_the_lane(pool: PgPool) {
    let h = Harness::new(pool, MockChain::new(LATEST).with(|s| s.mine_at = None));
    let first = sweep(vec![item(1, 10)]);
    h.deliver(&first).await;
    h.pass().await;
    assert_eq!(h.open().await.len(), 1);

    // Someone spent nonce 0 from this key outside the pool; none of our
    // attempts has a receipt.
    h.chain.set(|s| {
        s.mined_nonces.insert(mock::mock_signer(0), 1);
    });
    let report = h.pass().await;
    assert_eq!(report.resolved, 1);
    assert_eq!(h.job_state(first.job_id()).await, "abandoned");
    assert!(h.open().await.is_empty());
    let events = h.events().await;
    assert!(
        matches!(
            events.last(),
            Some(ExecutionEvent::SweepAbandoned {
                reason: AbandonReason::NonceConsumed { signer, nonce: 0 },
                ..
            }) if *signer == mock::mock_signer(0)
        ),
        "{events:?}"
    );

    // The lane is free: the next job starts on the next nonce.
    let second = sweep(vec![item(2, 10)]);
    h.deliver(&second).await;
    h.pass().await;
    let submissions = h.chain.submissions();
    assert_eq!(submissions.last().unwrap().nonce, 1);
    assert_eq!(h.job_state(second.job_id()).await, "executing");
}

#[sqlx::test(migrator = "gum_schema::MIGRATOR")]
async fn a_receipt_whose_block_left_the_canonical_chain_is_forgotten(pool: PgPool) {
    let h = Harness::new(pool, MockChain::new(LATEST));
    let command = sweep(vec![item(1, 10)]);
    h.deliver(&command).await;
    h.pass().await;

    // The receipt says block 100 has hash H; the chain now says otherwise.
    h.chain.set(|s| {
        s.hashes.insert(LATEST, B256::repeat_byte(0x99));
    });
    let report = h.pass().await;
    assert_eq!(report.resolved, 0);
    let open = h.open().await;
    assert_eq!(open.len(), 1);
    assert!(
        open[0].mined.is_none(),
        "the receipt was recorded and then cleared"
    );
    assert!(
        !h.events()
            .await
            .iter()
            .any(|e| matches!(e, ExecutionEvent::SweepFinalized { .. })),
        "nothing is finalized on a non-canonical receipt"
    );

    // The canonical chain comes back with the receipt's block.
    h.chain.set(|s| {
        s.hashes.remove(&LATEST);
    });
    h.pass().await;
    assert_eq!(h.job_state(command.job_id()).await, "finalized");
}

#[sqlx::test(migrator = "gum_schema::MIGRATOR")]
async fn a_whole_batch_revert_is_abandoned_with_the_transaction_hash(pool: PgPool) {
    let h = Harness::new(
        pool,
        MockChain::new(LATEST).with(|s| s.next_receipt_succeeds = false),
    );
    let command = sweep(vec![item(1, 10)]);
    h.deliver(&command).await;
    h.pass().await;
    h.pass().await;

    let tx_hash = h.chain.submissions()[0].tx_hash;
    assert_eq!(h.job_state(command.job_id()).await, "abandoned");
    assert!(matches!(
        h.events().await.last(),
        Some(ExecutionEvent::SweepAbandoned {
            reason: AbandonReason::Reverted { tx_hash: reverted },
            ..
        }) if *reverted == tx_hash
    ));
}

// ---------------------------------------------------------------------------
// Lanes, halts, transient failures
// ---------------------------------------------------------------------------

#[sqlx::test(migrator = "gum_schema::MIGRATOR")]
async fn one_signer_runs_one_transaction_at_a_time(pool: PgPool) {
    let h = Harness::new(pool, MockChain::new(LATEST).with(|s| s.mine_at = None));
    let first = sweep(vec![item(1, 10)]);
    let second = sweep(vec![item(2, 20)]);
    h.deliver(&first).await;
    h.deliver(&second).await;

    let report = h.pass().await;
    assert_eq!(report.started, 1);
    assert_eq!(h.job_state(first.job_id()).await, "executing");
    assert_eq!(h.job_state(second.job_id()).await, "queued");
    assert_eq!(h.open().await.len(), 1);

    // A second pass with the lane still busy starts nothing new and signs
    // only a replacement of the first.
    h.pass().await;
    assert_eq!(h.job_state(second.job_id()).await, "queued");
    assert!(h.chain.submissions().iter().all(|s| s.nonce == 0));
}

#[sqlx::test(migrator = "gum_schema::MIGRATOR")]
async fn two_signers_run_two_transactions_on_distinct_keys(pool: PgPool) {
    let h = Harness::new(
        pool,
        MockChain::new(LATEST)
            .with(|s| s.mine_at = None)
            .with_signers(2),
    );
    let jobs = [
        sweep(vec![item(1, 10)]),
        sweep(vec![item(2, 20)]),
        sweep(vec![item(3, 30)]),
    ];
    for job in &jobs {
        h.deliver(job).await;
    }

    let report = h.pass().await;
    assert_eq!(report.started, 2, "two lanes, three jobs");
    assert_eq!(h.job_state(jobs[0].job_id()).await, "executing");
    assert_eq!(h.job_state(jobs[1].job_id()).await, "executing");
    assert_eq!(h.job_state(jobs[2].job_id()).await, "queued");
    let signers: std::collections::HashSet<Address> =
        h.open().await.iter().map(|t| t.signer).collect();
    assert_eq!(signers.len(), 2);
    assert!(
        h.chain.submissions().iter().all(|s| s.nonce == 0),
        "each key starts at its own nonce 0"
    );
}

#[sqlx::test(migrator = "gum_schema::MIGRATOR")]
async fn a_halted_chain_starts_nothing_but_still_reconciles(pool: PgPool) {
    let h = Harness::new(pool, MockChain::new(LATEST).with_signers(2));
    let in_flight = sweep(vec![item(1, 10)]);
    h.deliver(&in_flight).await;
    h.pass().await;
    assert_eq!(h.job_state(in_flight.job_id()).await, "executing");

    let fault_id = Uuid::now_v7();
    h.control(ChainControl::Halted {
        fault_id,
        chain_id: mock::CHAIN_ID,
        reason: "cursor discontinuity".into(),
    })
    .await;
    let queued = sweep(vec![item(2, 20)]);
    h.deliver(&queued).await;

    let report = h.pass().await;
    assert_eq!(report.started, 0);
    assert_eq!(
        report.resolved, 1,
        "the in-flight transaction still concludes"
    );
    assert_eq!(h.job_state(in_flight.job_id()).await, "finalized");
    assert_eq!(h.job_state(queued.job_id()).await, "queued");

    // A stale resume (another fault id) changes nothing.
    h.control(ChainControl::Resumed {
        fault_id: Uuid::now_v7(),
        chain_id: mock::CHAIN_ID,
    })
    .await;
    h.pass().await;
    assert_eq!(h.job_state(queued.job_id()).await, "queued");

    h.control(ChainControl::Resumed {
        fault_id,
        chain_id: mock::CHAIN_ID,
    })
    .await;
    h.pass().await;
    assert_eq!(h.job_state(queued.job_id()).await, "executing");
}

#[sqlx::test(migrator = "gum_schema::MIGRATOR")]
async fn a_transient_signing_failure_defers_then_abandons_at_the_attempt_limit(pool: PgPool) {
    let chain = MockChain::new(LATEST).with(|s| {
        s.prepare_errors.insert(mock::mock_signer(0));
    });
    let h = Harness::new(pool, chain);
    let command = sweep(vec![item(1, 10)]);
    h.deliver(&command).await;

    let first = h.pass().await;
    assert_eq!((first.started, first.resolved), (0, 0));
    assert_eq!(h.job_state(command.job_id()).await, "queued");
    let attempts: i32 = sqlx::query_scalar("SELECT attempts FROM execution.jobs WHERE id = $1")
        .bind(command.job_id())
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(attempts, 1);
    assert!(
        h.open().await.is_empty(),
        "no transaction exists for a job that never signed"
    );

    // Second failure reaches `max_attempts` (2): the job is abandoned with
    // the error as the reason and the server is told.
    let second = h.pass().await;
    assert_eq!(second.resolved, 1);
    assert_eq!(h.job_state(command.job_id()).await, "abandoned");
    assert!(matches!(
        h.events().await.as_slice(),
        [ExecutionEvent::SweepAbandoned {
            reason: AbandonReason::Rejected { .. },
            ..
        }]
    ));
}

#[sqlx::test(migrator = "gum_schema::MIGRATOR")]
async fn a_transient_signing_failure_recovers_when_the_key_is_back(pool: PgPool) {
    let chain = MockChain::new(LATEST).with(|s| {
        s.prepare_errors.insert(mock::mock_signer(0));
    });
    let h = Harness::new(pool, chain);
    let command = sweep(vec![item(1, 10)]);
    h.deliver(&command).await;
    h.pass().await;
    h.chain.set(|s| s.prepare_errors.clear());

    let report = h.pass().await;
    assert_eq!(report.started, 1);
    h.pass().await;
    assert_eq!(h.job_state(command.job_id()).await, "finalized");
}

// ---------------------------------------------------------------------------
// Withdrawal steps
// ---------------------------------------------------------------------------

#[sqlx::test(migrator = "gum_schema::MIGRATOR")]
async fn a_step_whose_precondition_already_failed_concludes_without_signing(pool: PgPool) {
    let nonce = B256::repeat_byte(0x11);
    let chain = MockChain::new(LATEST).with(|s| {
        s.consumed_authorizations
            .insert((Address::repeat_byte(0xA0), nonce));
        s.authorization_events.insert(nonce, 42);
    });
    let h = Harness::new(pool, chain);
    let command = step(Some(authorization(nonce)), None);
    h.deliver(&command).await;

    let report = h.pass().await;
    assert_eq!((report.started, report.resolved), (0, 1));
    assert!(h.chain.submissions().is_empty());
    assert_eq!(h.job_state(command.job_id()).await, "finalized");
    let ExecutionCommand::WithdrawalStep(step) = &command else {
        unreachable!()
    };
    assert!(matches!(
        h.events().await.as_slice(),
        [ExecutionEvent::WithdrawalStepFinalized {
            leg_id,
            step: WithdrawalStepKind::Transfer,
            result: StepResult::ExecutedElsewhere { tx_hash: Some(found) },
            ..
        }] if *leg_id == step.leg_id && *found == mock::mock_authorization_tx_hash(nonce)
    ));
}

#[sqlx::test(migrator = "gum_schema::MIGRATOR")]
async fn a_step_past_its_deadline_expires_without_signing(pool: PgPool) {
    let h = Harness::new(pool, MockChain::new(LATEST));
    let command = step(None, Some(1));
    h.deliver(&command).await;

    h.pass().await;
    assert!(h.chain.submissions().is_empty());
    assert!(matches!(
        h.events().await.as_slice(),
        [ExecutionEvent::WithdrawalStepFinalized {
            result: StepResult::Expired,
            ..
        }]
    ));
}

#[sqlx::test(migrator = "gum_schema::MIGRATOR")]
async fn a_successful_step_reports_its_transaction(pool: PgPool) {
    let h = Harness::new(pool, MockChain::new(LATEST));
    let command = step(None, None);
    h.deliver(&command).await;
    h.pass().await;
    let submissions = h.chain.submissions();
    assert_eq!(submissions.len(), 1);
    assert_eq!(submissions[0].to, Some(mock::usdc()));
    assert_eq!(submissions[0].calldata, Bytes::from_static(&[0xAB, 0xCD]));
    assert_eq!(submissions[0].gas_limit, 120_000);
    assert!(
        h.events().await.is_empty(),
        "steps announce only their conclusion; submissions are sweep-only events"
    );

    h.pass().await;
    assert_eq!(h.job_state(command.job_id()).await, "finalized");
    assert!(matches!(
        h.events().await.as_slice(),
        [ExecutionEvent::WithdrawalStepFinalized {
            result: StepResult::Succeeded { tx_hash, block: LATEST, .. },
            ..
        }] if *tx_hash == submissions[0].tx_hash
    ));
}

#[sqlx::test(migrator = "gum_schema::MIGRATOR")]
async fn a_reverted_step_is_reported_as_executed_elsewhere_when_the_precondition_failed(
    pool: PgPool,
) {
    let nonce = B256::repeat_byte(0x22);
    let h = Harness::new(
        pool,
        MockChain::new(LATEST).with(|s| s.next_receipt_succeeds = false),
    );
    let command = step(Some(authorization(nonce)), None);
    h.deliver(&command).await;
    h.pass().await; // precondition holds: signed and broadcast
    assert_eq!(h.chain.submissions().len(), 1);

    // Between our broadcast and its (reverting) receipt, someone else used
    // the authorization.
    h.chain.set(|s| {
        s.consumed_authorizations
            .insert((Address::repeat_byte(0xA0), nonce));
    });
    h.pass().await;
    assert_eq!(h.job_state(command.job_id()).await, "finalized");
    assert!(matches!(
        h.events().await.as_slice(),
        [ExecutionEvent::WithdrawalStepFinalized {
            result: StepResult::ExecutedElsewhere { tx_hash: None },
            ..
        }]
    ));
}

#[sqlx::test(migrator = "gum_schema::MIGRATOR")]
async fn a_reverted_step_whose_precondition_still_holds_is_reported_reverted(pool: PgPool) {
    let nonce = B256::repeat_byte(0x33);
    let h = Harness::new(
        pool,
        MockChain::new(LATEST).with(|s| s.next_receipt_succeeds = false),
    );
    let command = step(Some(authorization(nonce)), None);
    h.deliver(&command).await;
    h.pass().await;
    h.pass().await;

    let tx_hash = h.chain.submissions()[0].tx_hash;
    assert!(matches!(
        h.events().await.as_slice(),
        [ExecutionEvent::WithdrawalStepFinalized {
            result: StepResult::Reverted { tx_hash: reverted },
            ..
        }] if *reverted == tx_hash
    ));
    // The lane is free again even though the step failed.
    assert!(h.open().await.is_empty());
}
