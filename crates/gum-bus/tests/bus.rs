//! The bus's guarantees, exercised against Postgres with the real schema.
//! `ChainControl` is the smallest real message type, so it is the one used;
//! nothing here depends on its meaning.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use gum_bus::{
    BackoffPolicy, BusError, Consumer, ConsumerOptions, Delivery, Disposition, HandledDelivery,
    Handler, HandlerError, PublishOutcome, Publisher, handle_batch,
};
use gum_contracts::{ChainControl, CorrelationId};
use sqlx::PgPool;
use uuid::Uuid;

const SERVER: Publisher = Publisher::new("gum-server");

fn signers(pool: &PgPool, options: ConsumerOptions) -> Consumer {
    Consumer::new("gum-signers", pool.clone(), options)
}

fn fast() -> ConsumerOptions {
    ConsumerOptions {
        lease: Duration::from_millis(200),
        max_attempts: 3,
        backoff: BackoffPolicy::new(Duration::from_millis(10), Duration::from_millis(20)),
        poll_interval: Duration::from_millis(10),
        batch_size: 10,
    }
}

fn halted(fault: u128) -> ChainControl {
    ChainControl::Halted {
        fault_id: Uuid::from_u128(fault),
        chain_id: 8453,
        reason: "finalized block changed hash".into(),
    }
}

async fn publish(pool: &PgPool, message: &ChainControl) -> Result<PublishOutcome, BusError> {
    let mut tx = pool.begin().await.unwrap();
    let outcome = SERVER
        .publish(&mut tx, message, CorrelationId::new(), None)
        .await?;
    tx.commit().await.unwrap();
    Ok(outcome)
}

async fn state(pool: &PgPool, message_id: Uuid) -> (String, i32, Option<String>) {
    sqlx::query_as(
        "SELECT state, attempt_count, last_error FROM bus.deliveries WHERE message_id = $1 AND consumer = 'gum-signers'",
    )
    .bind(message_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Publishing inside a transaction that rolls back leaves no message: the
/// outbox property. Publishing the same key twice is one message; the
/// same key with another payload is refused.
#[sqlx::test(migrator = "gum_schema::MIGRATOR")]
async fn publish_is_transactional_and_deduplicated(pool: PgPool) {
    let mut tx = pool.begin().await.unwrap();
    SERVER
        .publish(&mut tx, &halted(1), CorrelationId::new(), None)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM bus.messages")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "a rolled-back publish never happened");

    let first = publish(&pool, &halted(1)).await.unwrap();
    let PublishOutcome::Published(id) = first else {
        panic!("{first:?}");
    };
    assert_eq!(
        publish(&pool, &halted(1)).await.unwrap(),
        PublishOutcome::Duplicate(id)
    );

    let conflicting = ChainControl::Halted {
        fault_id: Uuid::from_u128(1),
        chain_id: 8453,
        reason: "a different story".into(),
    };
    assert!(matches!(
        publish(&pool, &conflicting).await,
        Err(BusError::ConflictingPayload { message_id, .. }) if message_id == id
    ));

    // Fan-out happened in the same transaction, to the subscribers only.
    let consumers: Vec<String> = sqlx::query_scalar(
        "SELECT consumer FROM bus.deliveries WHERE message_id = $1 ORDER BY consumer",
    )
    .bind(id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(consumers, vec!["gum-signers".to_string()]);
}

/// A claimed delivery is invisible to a second claimer until its lease
/// expires; then it is redelivered with a higher attempt number, and the
/// first claimer's late acknowledgement is refused.
#[sqlx::test(migrator = "gum_schema::MIGRATOR")]
async fn leases_expire_and_stale_acknowledgements_are_refused(pool: PgPool) {
    let id = publish(&pool, &halted(2)).await.unwrap().message_id();
    let consumer = signers(&pool, fast());

    let first = consumer.claim::<ChainControl>().await.unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].message_id, id);
    assert_eq!(first[0].attempt, 1);
    assert!(
        consumer.claim::<ChainControl>().await.unwrap().is_empty(),
        "leased"
    );

    tokio::time::sleep(Duration::from_millis(250)).await;
    let second = consumer.claim::<ChainControl>().await.unwrap();
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].attempt, 2);
    assert_ne!(second[0].lease_token, first[0].lease_token);

    // The slow first handler finishes now: its ack must not commit over
    // the second claimer's work.
    let mut tx = pool.begin().await.unwrap();
    let stale = consumer.ack(&mut tx, &first[0]).await;
    assert!(matches!(stale, Err(BusError::LeaseLost { message_id, .. }) if message_id == id));
    tx.rollback().await.unwrap();

    let mut tx = pool.begin().await.unwrap();
    consumer.ack(&mut tx, &second[0]).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(state(&pool, id).await.0, "done");
    assert!(consumer.claim::<ChainControl>().await.unwrap().is_empty());
}

/// A handler that fails is retried after a backoff, and parked as dead
/// after the attempt budget; a permanent failure is parked at once. The
/// acknowledgement is transactional with the handler's writes: a handler
/// that wrote and then failed leaves nothing behind.
struct Flaky {
    fail_first: usize,
    permanent: bool,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl Handler<ChainControl> for Flaky {
    async fn handle(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        delivery: &Delivery<ChainControl>,
    ) -> Result<(), HandlerError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        // A side effect in the handler's transaction, to prove it rolls
        // back with the failure.
        sqlx::query("INSERT INTO chain_faults (id, chain_id, reason, cleared_at) VALUES ($1, 8453, 'handler wrote this', now())")
            .bind(Uuid::now_v7())
            .execute(&mut **tx)
            .await?;
        if self.permanent {
            return Err(HandlerError::permanent("this can never work"));
        }
        if call < self.fail_first {
            return Err(HandlerError::retryable(format!(
                "attempt {} failed",
                delivery.attempt
            )));
        }
        Ok(())
    }
}

async fn faults(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM chain_faults")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrator = "gum_schema::MIGRATOR")]
async fn failures_back_off_then_dead_letter_and_writes_roll_back(pool: PgPool) {
    let id = publish(&pool, &halted(3)).await.unwrap().message_id();
    let consumer = signers(&pool, fast());
    let calls = Arc::new(AtomicUsize::new(0));
    let handler = Flaky {
        fail_first: 1,
        permanent: false,
        calls: calls.clone(),
    };

    let outcomes = handle_batch(&consumer, &handler).await.unwrap();
    let HandledDelivery::Failed {
        disposition: Disposition::Retry { available_at },
        ..
    } = &outcomes[0]
    else {
        panic!("{outcomes:?}");
    };
    assert!(*available_at > chrono::Utc::now() - chrono::Duration::milliseconds(50));
    let (state_, attempts, error) = state(&pool, id).await;
    assert_eq!((state_.as_str(), attempts), ("pending", 1));
    assert_eq!(error.as_deref(), Some("attempt 1 failed"));
    assert_eq!(
        faults(&pool).await,
        0,
        "the failed handler's write rolled back"
    );

    // Not yet available; then it is, and succeeds.
    assert!(handle_batch(&consumer, &handler).await.unwrap().is_empty());
    tokio::time::sleep(Duration::from_millis(30)).await;
    let outcomes = handle_batch(&consumer, &handler).await.unwrap();
    assert!(
        matches!(outcomes.as_slice(), [HandledDelivery::Done { .. }]),
        "{outcomes:?}"
    );
    let (state_, attempts, error) = state(&pool, id).await;
    assert_eq!((state_.as_str(), attempts, error), ("done", 2, None));
    assert_eq!(
        faults(&pool).await,
        1,
        "the successful handler's write committed"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    // Exhausting the budget parks the delivery; a retry by an operator
    // gives it a fresh budget.
    let stubborn = publish(&pool, &halted(4)).await.unwrap().message_id();
    let handler = Flaky {
        fail_first: usize::MAX,
        permanent: false,
        calls: Arc::new(AtomicUsize::new(0)),
    };
    for expected_attempt in 1..=3 {
        tokio::time::sleep(Duration::from_millis(30)).await;
        let outcomes = handle_batch(&consumer, &handler).await.unwrap();
        assert_eq!(
            outcomes.len(),
            1,
            "attempt {expected_attempt}: {outcomes:?}"
        );
    }
    let (state_, attempts, _) = state(&pool, stubborn).await;
    assert_eq!((state_.as_str(), attempts), ("dead", 3));
    let dead = consumer.dead(10).await.unwrap();
    assert_eq!(dead.len(), 1);
    assert_eq!(dead[0].message_id, stubborn);
    assert_eq!(dead[0].kind, "halted");
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(
        handle_batch(&consumer, &handler).await.unwrap().is_empty(),
        "dead is not claimable"
    );
    assert!(consumer.retry_dead(stubborn).await.unwrap());
    assert!(
        !consumer.retry_dead(stubborn).await.unwrap(),
        "only dead deliveries are retried"
    );
    let outcomes = handle_batch(&consumer, &handler).await.unwrap();
    assert_eq!(outcomes.len(), 1);
    assert_eq!(state(&pool, stubborn).await.1, 1, "fresh attempt budget");

    // A permanent failure skips the budget.
    let hopeless = publish(&pool, &halted(5)).await.unwrap().message_id();
    let handler = Flaky {
        fail_first: 0,
        permanent: true,
        calls: Arc::new(AtomicUsize::new(0)),
    };
    tokio::time::sleep(Duration::from_millis(30)).await;
    let outcomes = handle_batch(&consumer, &handler).await.unwrap();
    let dead_now = outcomes
        .iter()
        .find(|outcome| matches!(outcome, HandledDelivery::Failed { message_id, .. } if *message_id == hopeless));
    assert!(
        matches!(
            dead_now,
            Some(HandledDelivery::Failed {
                disposition: Disposition::Dead,
                ..
            })
        ),
        "{outcomes:?}"
    );
    assert_eq!(state(&pool, hopeless).await.0, "dead");
}

/// A payload this binary cannot decode, or a schema version above the one
/// it compiled against, is dead-lettered at claim time rather than crashing
/// the consumer or being retried forever.
#[sqlx::test(migrator = "gum_schema::MIGRATOR")]
async fn undecodable_and_future_messages_are_dead_lettered_at_claim(pool: PgPool) {
    let future = Uuid::now_v7();
    let garbage = Uuid::now_v7();
    for (id, version, payload) in [
        (
            future,
            99_i16,
            serde_json::json!({"kind": "resumed", "fault_id": Uuid::nil(), "chain_id": 1}),
        ),
        (garbage, 1_i16, serde_json::json!({"kind": "teleported"})),
    ] {
        sqlx::query(
            r#"INSERT INTO bus.messages (id, topic, kind, schema_version, payload, payload_hash, producer, deduplication_key, correlation_id)
               VALUES ($1, 'chain.control', 'x', $2, $3, $4, 'test', $1::text, $1)"#,
        )
        .bind(id)
        .bind(version)
        .bind(payload)
        .bind(vec![0u8; 32])
        .execute(&pool)
        .await
        .unwrap();
    }
    let fine = publish(&pool, &halted(6)).await.unwrap().message_id();

    let consumer = signers(&pool, fast());
    let claimed = consumer.claim::<ChainControl>().await.unwrap();
    assert_eq!(
        claimed.iter().map(|d| d.message_id).collect::<Vec<_>>(),
        vec![fine]
    );
    let (future_state, _, future_error) = state(&pool, future).await;
    assert_eq!(future_state, "dead");
    assert!(future_error.unwrap().contains("schema version 99"));
    let (garbage_state, _, garbage_error) = state(&pool, garbage).await;
    assert_eq!(garbage_state, "dead");
    assert!(garbage_error.unwrap().contains("does not decode"));
}

/// Two consumers of one subscription split the work: a delivery goes to
/// exactly one of them.
#[sqlx::test(migrator = "gum_schema::MIGRATOR")]
async fn competing_consumers_never_share_a_delivery(pool: PgPool) {
    for fault in 10..30 {
        publish(&pool, &halted(fault)).await.unwrap();
    }
    let a = signers(
        &pool,
        ConsumerOptions {
            batch_size: 7,
            ..fast()
        },
    );
    let b = a.clone();
    let (from_a, from_b) = tokio::join!(a.claim::<ChainControl>(), b.claim::<ChainControl>());
    let mut ids: Vec<Uuid> = from_a
        .unwrap()
        .into_iter()
        .chain(from_b.unwrap())
        .map(|delivery| delivery.message_id)
        .collect();
    assert_eq!(ids.len(), 14);
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 14, "no delivery was claimed twice");
    assert_eq!(
        a.backlog(Duration::ZERO).await.unwrap(),
        20,
        "all still pending until acked"
    );
}
