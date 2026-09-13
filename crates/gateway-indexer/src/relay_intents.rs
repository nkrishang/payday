//! Following cross-chain payments made through Relay (see `gateway-relay`
//! and `gateway_db::relay_intents`).
//!
//! `gatewayd` quoted the payment and the page reported the origin
//! transaction; from then on the intent is `sent` and this chain's worker,
//! the destination's, asks Relay what became of it. Relay fills in seconds,
//! so a `sent` intent is polled every few seconds until it is `success`,
//! `failure`, or `refund`; `delayed` is still pending. On success the
//! request record names the origin depositor, which must be the attested
//! wallet, and when the origin chain is one this deployment serves the
//! origin transaction's signer is read from that chain as well. The
//! resolution then attributes the solver's transfer (parked by the
//! crediting path, or credited later against the recorded fill) to the
//! payer, or hands it back to the likely-unsolicited path.
//!
//! Nothing here touches the signer or a nonce, and nothing here may fail
//! the sweep tick: a Relay outage defers the intent and moves on.

use std::time::Duration;

use alloy_primitives::B256;
use chrono::Utc;
use gateway_db::{AttributionSource, DbRelayIntent, RelayIntentStatus, Resolution};
use gateway_relay::{IntentState, RelayError};
use tracing::{info, warn};

use crate::indexer::Indexer;

/// How often a sent intent is asked about. Relay's status endpoint is
/// public and cheap; a fill takes a few seconds.
const RELAY_POLL: Duration = Duration::from_secs(3);
/// Backoff after a Relay error.
const RELAY_ERROR_BACKOFF: Duration = Duration::from_secs(15);
/// A sent intent Relay never resolves is failed after this. Long, because
/// a fill that lands after the intent failed is flagged for good.
const RELAY_SENT_CUTOFF: Duration = Duration::from_secs(24 * 3600);
/// Intents looked at per tick.
const RELAY_POLL_BATCH: i64 = 20;

impl Indexer {
    pub(crate) async fn relay_intent_housekeeping(&self) {
        let Some(relay) = &self.relay else {
            return;
        };
        let chain_id = self.cfg.chain_id.0;
        // Every worker forgets unsent quotes; the table is small.
        match self.relay_intents.expire_quoted().await {
            Ok(expired) if expired > 0 => info!(expired, "relay quotes expired unsent"),
            Ok(_) => {}
            Err(error) => warn!(%error, "failed to expire relay quotes"),
        }
        match self.relay_intents.stale_sent(RELAY_SENT_CUTOFF).await {
            Ok(stale) => {
                for intent in stale
                    .into_iter()
                    .filter(|intent| intent.destination_chain_id as u64 == chain_id)
                {
                    match self
                        .relay_intents
                        .resolve_failed(intent.id, RelayIntentStatus::Failed, "unresolved")
                        .await
                    {
                        Ok(resolution) => {
                            warn!(intent_id = %intent.id, ?resolution, "relay intent unresolved for a day; failed")
                        }
                        Err(error) => {
                            warn!(intent_id = %intent.id, %error, "failed to fail a stale relay intent")
                        }
                    }
                }
            }
            Err(error) => warn!(%error, "failed to read stale relay intents"),
        }
        let due = match self
            .relay_intents
            .due_for_poll(chain_id, RELAY_POLL_BATCH)
            .await
        {
            Ok(due) => due,
            Err(error) => {
                warn!(%error, "failed to read relay intents due for a poll");
                return;
            }
        };
        for intent in due {
            if let Err(error) = self.poll_relay_intent(relay.as_ref(), &intent).await {
                warn!(intent_id = %intent.id, %error, "relay poll failed; retrying later");
                let next = Utc::now()
                    + chrono::Duration::from_std(RELAY_ERROR_BACKOFF).unwrap_or_default();
                if let Err(error) = self.relay_intents.defer(intent.id, next, None).await {
                    warn!(intent_id = %intent.id, %error, "failed to defer a relay intent");
                }
            }
        }
    }

    async fn poll_relay_intent(
        &self,
        relay: &dyn gateway_relay::RelayApi,
        intent: &DbRelayIntent,
    ) -> Result<(), RelayPollError> {
        let request_id = intent
            .request_id()
            .ok_or(RelayPollError::Malformed("request id"))?;
        let payer_wallet = intent
            .payer_wallet()
            .ok_or(RelayPollError::Malformed("payer wallet"))?;
        let status = relay.status(request_id).await?;
        let later = Utc::now() + chrono::Duration::from_std(RELAY_POLL).unwrap_or_default();
        match status.state {
            IntentState::Waiting | IntentState::Pending => {
                self.relay_intents
                    .defer(intent.id, later, Some(&status.raw))
                    .await?;
                Ok(())
            }
            IntentState::Failure | IntentState::Refund => {
                let outcome = if status.state == IntentState::Refund {
                    RelayIntentStatus::Refunded
                } else {
                    RelayIntentStatus::Failed
                };
                let resolution = self
                    .relay_intents
                    .resolve_failed(intent.id, outcome, &status.raw)
                    .await?;
                warn!(intent_id = %intent.id, relay_status = %status.raw, ?resolution, "relay intent not filled");
                Ok(())
            }
            IntentState::Success => {
                // The fill's transactions and, from the request record, who
                // deposited on the origin chain.
                let record = relay.request(request_id).await?;
                let fills: Vec<B256> = record
                    .as_ref()
                    .map(|record| {
                        record
                            .out_txs
                            .iter()
                            .filter(|(chain, _)| *chain == intent.destination_chain_id as u64)
                            .map(|(_, hash)| *hash)
                            .collect()
                    })
                    .filter(|fills: &Vec<B256>| !fills.is_empty())
                    .unwrap_or_else(|| status.tx_hashes.clone());
                if fills.is_empty() {
                    // Success without a fill yet: the record lags the status.
                    self.relay_intents
                        .defer(intent.id, later, Some(&status.raw))
                        .await?;
                    return Ok(());
                }
                let origin_tx = record
                    .as_ref()
                    .and_then(|record| record.in_txs.first().map(|(_, hash)| *hash))
                    .or_else(|| status.in_tx_hashes.first().copied())
                    .or_else(|| intent.origin_tx_hash());
                let depositor = record.as_ref().and_then(|record| record.depositor);
                // The origin sender: from the origin chain itself when we
                // serve it, else Relay's record of the depositor.
                let origin_chain = intent.origin_chain_id as u64;
                let (sender, source) = match (self.peers.get(&origin_chain), origin_tx) {
                    (Some(origin), Some(tx)) => match origin.transaction_sender(tx).await {
                        Ok(Some(sender)) => (Some(sender), AttributionSource::Receipt),
                        Ok(None) => (depositor, AttributionSource::RelayApi),
                        Err(error) => {
                            warn!(intent_id = %intent.id, %error, "origin transaction read failed; taking Relay's depositor");
                            (depositor, AttributionSource::RelayApi)
                        }
                    },
                    _ => (depositor, AttributionSource::RelayApi),
                };
                let resolution: Resolution = match sender {
                    Some(sender) if sender == payer_wallet => {
                        self.relay_intents
                            .resolve_filled(intent.id, &fills, origin_tx, &status.raw, source)
                            .await?
                    }
                    other => {
                        warn!(intent_id = %intent.id, depositor = ?other, expected = %payer_wallet, "relay fill was not deposited by the attested wallet");
                        self.relay_intents
                            .resolve_failed(
                                intent.id,
                                RelayIntentStatus::Failed,
                                "depositor_mismatch",
                            )
                            .await?
                    }
                };
                info!(intent_id = %intent.id, fills = fills.len(), source = source.as_str(), ?resolution, "relay intent filled");
                Ok(())
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum RelayPollError {
    #[error("{0}")]
    Relay(#[from] RelayError),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("relay intent row is malformed: {0}")]
    Malformed(&'static str),
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use alloy_primitives::{Address, B256, U256};
    use async_trait::async_trait;
    use gateway_core::ChainId;
    use gateway_db::{InvoiceRepository, NewRelayIntent, RelayIntentRepository};
    use gateway_relay::{
        IntentState, IntentStatus, Quote, QuoteRequest, RelayApi, RelayChain, RelayError,
        RelayRequest,
    };
    use sqlx::PgPool;
    use uuid::Uuid;

    use crate::chain::ChainClient;
    use crate::indexer::Indexer;
    use crate::indexer::tests::{
        CHAIN_ID, FakeIris, MockChain, config, insert, issue, payer_wallet, test_registry,
    };

    const ORIGIN: u64 = 8453;

    /// Relay as the poller sees it: one answer per request id, and the
    /// request record with the depositor it names.
    #[derive(Default)]
    struct FakeRelay {
        statuses: Mutex<HashMap<B256, IntentStatus>>,
        records: Mutex<HashMap<B256, RelayRequest>>,
    }

    #[async_trait]
    impl RelayApi for FakeRelay {
        async fn chains(&self) -> Result<Vec<RelayChain>, RelayError> {
            Ok(Vec::new())
        }

        async fn quote(&self, _: &QuoteRequest) -> Result<Quote, RelayError> {
            Err(RelayError::Transport("not quoting here".into()))
        }

        async fn status(&self, request_id: B256) -> Result<IntentStatus, RelayError> {
            Ok(self
                .statuses
                .lock()
                .unwrap()
                .get(&request_id)
                .cloned()
                .unwrap_or(IntentStatus {
                    state: IntentState::Waiting,
                    raw: "unknown".into(),
                    in_tx_hashes: vec![],
                    tx_hashes: vec![],
                }))
        }

        async fn request(&self, request_id: B256) -> Result<Option<RelayRequest>, RelayError> {
            Ok(self.records.lock().unwrap().get(&request_id).cloned())
        }
    }

    fn success(origin_tx: B256, fill: B256) -> IntentStatus {
        IntentStatus {
            state: IntentState::Success,
            raw: "success".into(),
            in_tx_hashes: vec![origin_tx],
            tx_hashes: vec![fill],
        }
    }

    fn record(depositor: Address, origin_tx: B256, fill: B256) -> RelayRequest {
        RelayRequest {
            status: "success".into(),
            depositor: Some(depositor),
            in_txs: vec![(ORIGIN, origin_tx)],
            out_txs: vec![(CHAIN_ID, fill)],
        }
    }

    async fn sent_intent(
        pool: &PgPool,
        invoice_id: Uuid,
        request: u8,
        origin_tx: B256,
    ) -> gateway_db::DbRelayIntent {
        let intents = RelayIntentRepository::new(pool.clone());
        let intent = intents
            .insert(&NewRelayIntent {
                id: Uuid::now_v7(),
                invoice_id,
                request_id: B256::repeat_byte(request),
                origin_chain_id: ORIGIN,
                destination_chain_id: CHAIN_ID,
                origin_currency: Address::repeat_byte(0x0c),
                payer_wallet: payer_wallet(),
                quoted_in_amount: U256::from(1_020_000u64),
                quoted_out_amount: U256::from(1_000_000u64),
                expires_at: chrono::Utc::now() + chrono::Duration::minutes(15),
            })
            .await
            .unwrap();
        intents
            .mark_sent(intent.id, invoice_id, origin_tx)
            .await
            .unwrap();
        intent
    }

    fn worker(pool: &PgPool, relay: Arc<FakeRelay>, origin: Option<Arc<MockChain>>) -> Indexer {
        let mut peers: HashMap<u64, Arc<dyn ChainClient>> = HashMap::new();
        if let Some(origin) = origin {
            peers.insert(ORIGIN, origin);
        }
        Indexer::new(
            InvoiceRepository::new(pool.clone()),
            gateway_db::CursorRepository::new(pool.clone()),
            Arc::new(MockChain::new(100)),
            config(),
            Arc::new(FakeIris::default()),
            test_registry(None),
            Some(relay),
            Arc::new(peers),
        )
    }

    /// Relay's success with the wallet as depositor fills the intent; with
    /// another depositor it fails it. The origin chain's own record of the
    /// sender wins over Relay's when the chain is one this deployment
    /// serves.
    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn a_filled_request_resolves_by_its_depositor(pool: PgPool) {
        let relay = Arc::new(FakeRelay::default());
        let intents = RelayIntentRepository::new(pool.clone());
        let genuine = issue(ChainId(CHAIN_ID), 1_000_000, 4_000_000_000);
        insert(&pool, &genuine, "relay-genuine").await;
        let impostor = issue(ChainId(CHAIN_ID), 1_000_000, 4_000_000_000);
        insert(&pool, &impostor, "relay-impostor").await;
        let (origin_a, fill_a) = (B256::repeat_byte(0xA1), B256::repeat_byte(0xF1));
        let (origin_b, fill_b) = (B256::repeat_byte(0xA2), B256::repeat_byte(0xF2));
        let filled = sent_intent(&pool, genuine.id.0, 0x01, origin_a).await;
        let failed = sent_intent(&pool, impostor.id.0, 0x02, origin_b).await;
        relay
            .statuses
            .lock()
            .unwrap()
            .insert(B256::repeat_byte(0x01), success(origin_a, fill_a));
        relay.records.lock().unwrap().insert(
            B256::repeat_byte(0x01),
            record(payer_wallet(), origin_a, fill_a),
        );
        relay
            .statuses
            .lock()
            .unwrap()
            .insert(B256::repeat_byte(0x02), success(origin_b, fill_b));
        relay.records.lock().unwrap().insert(
            B256::repeat_byte(0x02),
            record(Address::repeat_byte(0x99), origin_b, fill_b),
        );

        // Without the origin chain, Relay's record is the attribution.
        worker(&pool, relay.clone(), None)
            .relay_intent_housekeeping()
            .await;
        let filled = intents.find(filled.id).await.unwrap().unwrap();
        assert_eq!(filled.status, "filled");
        assert_eq!(filled.attribution_source.as_deref(), Some("relay_api"));
        assert_eq!(filled.fill_tx_hashes(), vec![fill_a]);
        assert_eq!(filled.origin_tx_hash(), Some(origin_a));
        let failed = intents.find(failed.id).await.unwrap().unwrap();
        assert_eq!(failed.status, "failed");
        assert_eq!(failed.relay_status.as_deref(), Some("depositor_mismatch"));

        // With the origin chain served, its own transaction record decides:
        // Relay may name the wallet, but the chain says someone else sent it.
        let third = issue(ChainId(CHAIN_ID), 1_000_000, 4_000_000_000);
        insert(&pool, &third, "relay-receipt").await;
        let (origin_c, fill_c) = (B256::repeat_byte(0xA3), B256::repeat_byte(0xF3));
        let checked = sent_intent(&pool, third.id.0, 0x03, origin_c).await;
        relay
            .statuses
            .lock()
            .unwrap()
            .insert(B256::repeat_byte(0x03), success(origin_c, fill_c));
        relay.records.lock().unwrap().insert(
            B256::repeat_byte(0x03),
            record(payer_wallet(), origin_c, fill_c),
        );
        let origin = Arc::new(MockChain::new(50).with(|state| {
            state.senders.insert(origin_c, Address::repeat_byte(0x98));
        }));
        worker(&pool, relay.clone(), Some(origin.clone()))
            .relay_intent_housekeeping()
            .await;
        let checked_row = intents.find(checked.id).await.unwrap().unwrap();
        assert_eq!(checked_row.status, "failed");
        // And when the chain agrees, the attribution says it came from there.
        let fourth = issue(ChainId(CHAIN_ID), 1_000_000, 4_000_000_000);
        insert(&pool, &fourth, "relay-receipt-ok").await;
        let (origin_d, fill_d) = (B256::repeat_byte(0xA4), B256::repeat_byte(0xF4));
        let attributed = sent_intent(&pool, fourth.id.0, 0x04, origin_d).await;
        relay
            .statuses
            .lock()
            .unwrap()
            .insert(B256::repeat_byte(0x04), success(origin_d, fill_d));
        origin.set(|state| {
            state.senders.insert(origin_d, payer_wallet());
        });
        worker(&pool, relay, Some(origin))
            .relay_intent_housekeeping()
            .await;
        let attributed = intents.find(attributed.id).await.unwrap().unwrap();
        assert_eq!(attributed.status, "filled");
        assert_eq!(attributed.attribution_source.as_deref(), Some("receipt"));
    }

    /// Pending answers defer the poll; a request Relay does not know is
    /// still waiting; success without a fill yet is retried.
    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn pending_and_incomplete_answers_defer(pool: PgPool) {
        let relay = Arc::new(FakeRelay::default());
        let intents = RelayIntentRepository::new(pool.clone());
        let invoice = issue(ChainId(CHAIN_ID), 1_000_000, 4_000_000_000);
        insert(&pool, &invoice, "relay-pending").await;
        let intent = sent_intent(&pool, invoice.id.0, 0x05, B256::repeat_byte(0xA5)).await;
        let worker = worker(&pool, relay.clone(), None);
        worker.relay_intent_housekeeping().await;
        let row = intents.find(intent.id).await.unwrap().unwrap();
        assert_eq!(row.status, "sent");
        assert_eq!(row.relay_status.as_deref(), Some("unknown"));
        assert!(row.next_check_at.unwrap() > chrono::Utc::now());
        // Not due again yet, so nothing is asked; make it due and answer
        // success without a fill.
        sqlx::query("UPDATE relay_intents SET next_check_at = now()")
            .execute(&pool)
            .await
            .unwrap();
        relay.statuses.lock().unwrap().insert(
            B256::repeat_byte(0x05),
            IntentStatus {
                state: IntentState::Success,
                raw: "success".into(),
                in_tx_hashes: vec![],
                tx_hashes: vec![],
            },
        );
        worker.relay_intent_housekeeping().await;
        let row = intents.find(intent.id).await.unwrap().unwrap();
        assert_eq!(row.status, "sent");
        // A refund resolves it as such.
        sqlx::query("UPDATE relay_intents SET next_check_at = now()")
            .execute(&pool)
            .await
            .unwrap();
        relay.statuses.lock().unwrap().insert(
            B256::repeat_byte(0x05),
            IntentStatus {
                state: IntentState::Refund,
                raw: "refund".into(),
                in_tx_hashes: vec![],
                tx_hashes: vec![],
            },
        );
        worker.relay_intent_housekeeping().await;
        let row = intents.find(intent.id).await.unwrap().unwrap();
        assert_eq!(row.status, "refunded");
    }
}
