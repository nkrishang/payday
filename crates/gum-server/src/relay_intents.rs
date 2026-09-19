//! Follows cross-chain payments made through Relay (see `gum-relay` and
//! `gum_ledger::relay_intents`).
//!
//! The API quoted the payment and the payment page reported the origin
//! transaction; from then on the intent is `sent` and this loop asks Relay
//! what became of it. Relay fills in seconds, so a pending intent is polled
//! every few seconds until it is `success`, `failure` or `refund`.
//!
//! On success the intent fills only when the origin chain itself proves the
//! payment: a receipt this deployment reads from that chain, whose signer
//! and whose token `Transfer` debit show the attested wallet spending
//! exactly the quoted input amount. Relay's record of a "depositor" is
//! whatever the caller supplied, and a hash Relay or the page reported is
//! not a sender; neither is ever evidence. Missing, unfinalized or
//! mismatched evidence defers the poll; a day without an answer fails the
//! intent. The resolution then attributes the solver's transfer (parked by
//! the crediting path, or credited later against the recorded fill) to the
//! payer, or hands it back to the likely-unsolicited path.
//!
//! Every step is a single-row database update keyed on the intent's
//! current status, so a crash at any point costs at most one poll interval
//! and a duplicate poll finds the intent already resolved. Nothing here
//! signs anything or touches the bus.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use alloy_primitives::{Address, B256, U256};
use chrono::Utc;
use gum_chain::ChainReader;
use gum_core::ChainRegistry;
use gum_ledger::{DbRelayIntent, FillResolution, RelayIntentRepository, RelayIntentStatus};
use gum_relay::{IntentState, RelayApi, RelayError};
use tokio::sync::watch;
use tracing::{info, warn};
use uuid::Uuid;

/// How often a sent intent is asked about. Relay's status endpoint is
/// public and cheap; a fill takes a few seconds.
const RELAY_POLL: Duration = Duration::from_secs(3);
/// Backoff after a Relay error.
const RELAY_ERROR_BACKOFF: Duration = Duration::from_secs(15);
/// Backoff while the origin chain is not one this deployment serves: the
/// intent cannot be verified and no evidence will ever arrive for it.
const RELAY_UNSERVED_BACKOFF: Duration = Duration::from_secs(300);
/// A sent intent Relay never resolves is failed after this. Long, because
/// a fill that lands after the intent failed is flagged for good.
const RELAY_SENT_CUTOFF: Duration = Duration::from_secs(24 * 3600);
/// Intents looked at per chain per pass.
const RELAY_POLL_BATCH: i64 = 20;

pub struct RelayIntentPoller {
    relay_intents: RelayIntentRepository,
    /// Absent when no Relay key is configured: quotes are not offered, and
    /// only the database housekeeping runs.
    relay: Option<Arc<dyn RelayApi>>,
    /// Read-only access to every chain this deployment serves, keyed by
    /// chain id; the origin chain's receipt is the evidence.
    chains: HashMap<u64, Arc<dyn ChainReader>>,
    registry: Arc<ChainRegistry>,
}

impl RelayIntentPoller {
    pub fn new(
        relay_intents: RelayIntentRepository,
        relay: Option<Arc<dyn RelayApi>>,
        chains: HashMap<u64, Arc<dyn ChainReader>>,
        registry: Arc<ChainRegistry>,
    ) -> Self {
        Self {
            relay_intents,
            relay,
            chains,
            registry,
        }
    }

    pub async fn run(self, mut shutdown: watch::Receiver<bool>) {
        info!(
            relay_configured = self.relay.is_some(),
            "relay intent poller started"
        );
        loop {
            for chain in self.registry.chains() {
                self.pass(chain.chain_id).await;
            }
            tokio::select! {
                _ = tokio::time::sleep(RELAY_POLL) => {}
                result = shutdown.changed() => {
                    if result.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
            }
        }
    }

    /// One pass over the intents heading for `destination_chain_id`.
    /// Never fails: every problem is logged and the intent it concerns is
    /// deferred, so one bad intent or a Relay outage cannot stall the rest.
    pub async fn pass(&self, destination_chain_id: u64) {
        // Database-only housekeeping runs with or without a Relay client,
        // so parked transfers are never stranded once the key is gone.
        match self.relay_intents.expire_quoted(destination_chain_id).await {
            Ok(0) => {}
            Ok(expired) => info!(
                chain_id = destination_chain_id,
                expired, "relay quotes expired unsent"
            ),
            Err(error) => {
                warn!(chain_id = destination_chain_id, error = %error, "failed to expire relay quotes")
            }
        }
        match self
            .relay_intents
            .stale_sent(destination_chain_id, RELAY_SENT_CUTOFF)
            .await
        {
            Ok(stale) => {
                for intent in stale {
                    match self
                        .relay_intents
                        .resolve_failed(intent.id, RelayIntentStatus::Failed, "unresolved")
                        .await
                    {
                        Ok(resolution) => {
                            warn!(intent_id = %intent.id, deposit_request_id = %intent.invoice_id, ?resolution, "relay intent unresolved for a day; failed")
                        }
                        Err(error) => {
                            warn!(intent_id = %intent.id, error = %error, "failed to fail a stale relay intent")
                        }
                    }
                }
            }
            Err(error) => {
                warn!(chain_id = destination_chain_id, error = %error, "failed to read stale relay intents")
            }
        }
        let Some(relay) = &self.relay else {
            return;
        };
        let due = match self
            .relay_intents
            .due_for_poll(destination_chain_id, RELAY_POLL_BATCH)
            .await
        {
            Ok(due) => due,
            Err(error) => {
                warn!(chain_id = destination_chain_id, error = %error, "failed to read relay intents due for a poll");
                return;
            }
        };
        for intent in due {
            if let Err(error) = self.poll(relay.as_ref(), &intent).await {
                warn!(intent_id = %intent.id, deposit_request_id = %intent.invoice_id, error = %error, "relay poll failed; retrying later");
                self.defer(intent.id, RELAY_ERROR_BACKOFF, None).await;
            }
        }
    }

    async fn defer(&self, id: Uuid, backoff: Duration, relay_status: Option<&str>) {
        let next = Utc::now() + chrono::Duration::from_std(backoff).unwrap_or_default();
        if let Err(error) = self.relay_intents.defer(id, next, relay_status).await {
            warn!(intent_id = %id, error = %error, "failed to defer a relay intent");
        }
    }

    async fn poll(&self, relay: &dyn RelayApi, intent: &DbRelayIntent) -> Result<(), PollError> {
        let request_id = intent
            .request_id()
            .ok_or(PollError::Malformed("request id"))?;
        let payer_wallet = intent
            .payer_wallet()
            .ok_or(PollError::Malformed("payer wallet"))?;
        let origin_currency = Address::try_from(intent.origin_currency.as_slice())
            .map_err(|_| PollError::Malformed("origin currency"))?;
        let quoted_in = U256::from_str_radix(&intent.quoted_in_amount, 10)
            .map_err(|_| PollError::Malformed("quoted input amount"))?;
        let origin_chain = intent.origin_chain_id as u64;

        // Attribution needs the origin chain's own receipt. Without it there
        // is no evidence to be had, from Relay or from the page; the intent
        // waits out its timeout instead of guessing.
        let Some(origin) = self.chains.get(&origin_chain) else {
            self.defer(intent.id, RELAY_UNSERVED_BACKOFF, None).await;
            return Ok(());
        };

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
                warn!(intent_id = %intent.id, deposit_request_id = %intent.invoice_id, relay_status = %status.raw, ?resolution, "relay intent not filled");
                Ok(())
            }
            IntentState::Success => {
                // The record names the transactions; its id must be this
                // request's (the parser checked). Status hashes are a
                // progress signal, never attribution.
                let Some(record) = relay.request(request_id).await? else {
                    // Success before the record exists: it lags the status.
                    self.relay_intents
                        .defer(intent.id, later, Some(&status.raw))
                        .await?;
                    return Ok(());
                };
                let fills: Vec<B256> = record
                    .out_txs
                    .iter()
                    .filter(|(chain, _)| *chain == intent.destination_chain_id as u64)
                    .map(|(_, hash)| *hash)
                    .collect();
                let mut candidates: Vec<B256> = record
                    .in_txs
                    .iter()
                    .filter(|(chain, _)| *chain == origin_chain)
                    .map(|(_, hash)| *hash)
                    .collect();
                candidates.sort();
                candidates.dedup();
                if record.status != "success" || fills.is_empty() || candidates.is_empty() {
                    self.relay_intents
                        .defer(intent.id, later, Some(&status.raw))
                        .await?;
                    return Ok(());
                }

                // The evidence: one successful origin transaction signed by
                // the attested wallet whose receipt debits that wallet on
                // the quoted token for exactly the quoted amount. Anything
                // else (unmined, reverted, another signer, another amount)
                // is either no evidence yet or someone else's payment.
                let mut verified: Option<B256> = None;
                let mut conclusive_foreign = 0;
                let mut successful = 0;
                for candidate in &candidates {
                    let Some(receipt) = origin
                        .token_payment_receipt(*candidate, origin_currency)
                        .await?
                    else {
                        // Unmined: not evidence yet.
                        self.relay_intents
                            .defer(intent.id, later, Some(&status.raw))
                            .await?;
                        return Ok(());
                    };
                    if !receipt.outcome.succeeded {
                        continue;
                    }
                    successful += 1;
                    if receipt.sender != payer_wallet {
                        conclusive_foreign += 1;
                        continue;
                    }
                    if receipt.transfers.iter().any(|transfer| {
                        transfer.sender == payer_wallet && transfer.amount == quoted_in
                    }) {
                        verified = Some(*candidate);
                        break;
                    }
                }
                match verified {
                    Some(origin_tx) => {
                        match self
                            .relay_intents
                            .resolve_filled(intent.id, &fills, origin_tx, &status.raw)
                            .await?
                        {
                            FillResolution::Resolved(resolution) => {
                                info!(intent_id = %intent.id, deposit_request_id = %intent.invoice_id, chain_id = origin_chain, tx_hash = %origin_tx, fills = fills.len(), ?resolution, "relay intent filled on the origin chain's receipt");
                            }
                            FillResolution::OriginTransactionClaimed => {
                                warn!(intent_id = %intent.id, tx_hash = %origin_tx, "verified origin transaction already attributed to another intent");
                                self.relay_intents
                                    .resolve_failed(
                                        intent.id,
                                        RelayIntentStatus::Failed,
                                        "origin_transaction_claimed",
                                    )
                                    .await?;
                            }
                            FillResolution::NotPending => {}
                        }
                    }
                    None if successful > 0 && conclusive_foreign == successful => {
                        warn!(intent_id = %intent.id, deposit_request_id = %intent.invoice_id, expected = %payer_wallet, "relay fill was not paid by the attested wallet");
                        self.relay_intents
                            .resolve_failed(
                                intent.id,
                                RelayIntentStatus::Failed,
                                "depositor_mismatch",
                            )
                            .await?;
                    }
                    None => {
                        // No candidate proved the wallet paid, and none
                        // proved it did not: keep waiting for evidence.
                        self.relay_intents
                            .defer(intent.id, later, Some(&status.raw))
                            .await?;
                    }
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum PollError {
    #[error("{0}")]
    Relay(#[from] RelayError),
    #[error("chain error: {0}")]
    Chain(#[from] gum_chain::ChainError),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("relay intent row is malformed: {0}")]
    Malformed(&'static str),
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use gum_chain::mock::MockChain;
    use gum_chain::{TokenPaymentReceipt, TokenTransfer, TransactionOutcome};
    use gum_ledger::test_helpers::{account, insert_bound, issuance_input, test_payer_wallet};
    use gum_ledger::{DbInvoice, NewRelayIntent};
    use gum_relay::{IntentStatus, Quote, QuoteRequest, RelayChain, RelayRequest};
    use sqlx::PgPool;

    use super::*;

    /// The test invoice's chain (`gum_ledger::test_helpers::issuance_input`).
    const DESTINATION: u64 = 1;
    const ORIGIN: u64 = 8453;
    const QUOTED_IN: u64 = 1_020_000;

    /// A successful origin payment: the wallet signed the transaction and
    /// its receipt debits the wallet for exactly `amount`.
    fn wallet_receipt(sender: Address, tx: B256, amount: u64) -> TokenPaymentReceipt {
        TokenPaymentReceipt {
            transaction_hash: tx,
            sender,
            outcome: TransactionOutcome {
                succeeded: true,
                block: 5,
                block_hash: B256::repeat_byte(0x05),
            },
            transfers: vec![TokenTransfer {
                log_index: 0,
                sender,
                recipient: Address::repeat_byte(0x11),
                amount: U256::from(amount),
            }],
        }
    }

    /// Relay as the poller sees it: one answer per request id, and the
    /// request record naming the transactions on their chains.
    #[derive(Default)]
    struct FakeRelay {
        statuses: Mutex<HashMap<B256, IntentStatus>>,
        records: Mutex<HashMap<B256, RelayRequest>>,
    }

    impl FakeRelay {
        fn answer(&self, request: u8, origin_tx: B256, fill: B256) {
            let id = B256::repeat_byte(request);
            self.statuses.lock().unwrap().insert(
                id,
                IntentStatus {
                    state: IntentState::Success,
                    raw: "success".into(),
                    in_tx_hashes: vec![origin_tx],
                    tx_hashes: vec![fill],
                },
            );
            self.records.lock().unwrap().insert(
                id,
                RelayRequest {
                    status: "success".into(),
                    in_txs: vec![(ORIGIN, origin_tx)],
                    out_txs: vec![(DESTINATION, fill)],
                },
            );
        }
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

    async fn invoice(pool: &PgPool, key: &str) -> DbInvoice {
        let owner = account(pool, 1).await;
        insert_bound(pool, &issuance_input(owner, key, None), None).await
    }

    async fn sent_intent(
        pool: &PgPool,
        invoice_id: Uuid,
        request: u8,
        origin_tx: B256,
    ) -> DbRelayIntent {
        let intents = RelayIntentRepository::new(pool.clone());
        let intent = intents
            .insert(&NewRelayIntent {
                id: Uuid::now_v7(),
                invoice_id,
                request_id: B256::repeat_byte(request),
                origin_chain_id: ORIGIN,
                destination_chain_id: DESTINATION,
                origin_currency: Address::repeat_byte(0x0c),
                payer_wallet: test_payer_wallet(),
                quoted_in_amount: U256::from(QUOTED_IN),
                quoted_out_amount: U256::from(1_000_000u64),
                expires_at: Utc::now() + chrono::Duration::minutes(15),
            })
            .await
            .unwrap();
        intents
            .mark_sent(intent.id, invoice_id, test_payer_wallet(), origin_tx)
            .await
            .unwrap();
        intent
    }

    fn poller(
        pool: &PgPool,
        relay: Option<Arc<FakeRelay>>,
        origin: Option<Arc<MockChain>>,
    ) -> RelayIntentPoller {
        let mut chains: HashMap<u64, Arc<dyn ChainReader>> = HashMap::new();
        if let Some(origin) = origin {
            chains.insert(ORIGIN, origin);
        }
        RelayIntentPoller::new(
            RelayIntentRepository::new(pool.clone()),
            relay.map(|relay| relay as Arc<dyn RelayApi>),
            chains,
            Arc::new(
                ChainRegistry::new(vec![gum_core::ChainConfig {
                    chain_id: DESTINATION,
                    tokens: vec![gum_core::TokenConfig {
                        currency: gum_core::Currency::Usdc,
                        address: Address::repeat_byte(0xaa),
                    }],
                    factory: Address::repeat_byte(0xfa),
                    batch_sweeper: Address::repeat_byte(0x55),
                    factory_code_hash: B256::ZERO,
                    batch_sweeper_code_hash: B256::ZERO,
                    start_block: 0,
                    finality_source: gum_core::FinalitySource::Finalized,
                    finality_confirmations: 0,
                    block_time_ms: 1000,
                    log_range_size: 100,
                    signer_low_balance_wei: None,
                    explorer_base_url: None,
                    cctp: None,
                    block_gas_limit: None,
                    transaction_gas_limit: None,
                    sweep_batch_size: None,
                }])
                .unwrap(),
            ),
        )
    }

    async fn make_due(pool: &PgPool) {
        sqlx::query("UPDATE relay_intents SET next_check_at = now()")
            .execute(pool)
            .await
            .unwrap();
    }

    /// Relay naming a "depositor" is never evidence: only the origin
    /// chain's own receipt, the attested wallet's signature and its exact
    /// quoted debit, fills the intent.
    #[sqlx::test(migrator = "gum_schema::MIGRATOR")]
    async fn a_fill_fills_only_on_the_origin_chain_s_receipt(pool: PgPool) {
        let relay = Arc::new(FakeRelay::default());
        let intents = RelayIntentRepository::new(pool.clone());
        let origin_chain = Arc::new(MockChain::new(50));
        let poller = poller(&pool, Some(relay.clone()), Some(origin_chain.clone()));

        // The origin chain is served but has not produced the receipt yet:
        // the intent stays pending, unattributed.
        let (origin, fill) = (B256::repeat_byte(0xA1), B256::repeat_byte(0xF1));
        let intent = sent_intent(
            &pool,
            invoice(&pool, "relay-evidence").await.id,
            0x01,
            origin,
        )
        .await;
        relay.answer(0x01, origin, fill);
        poller.pass(DESTINATION).await;
        let row = intents.find(intent.id).await.unwrap().unwrap();
        assert_eq!(row.status, "sent");
        assert_eq!(row.attribution_source, None);
        assert_eq!(row.verified_origin_tx_hash(), None);

        // A successful receipt signed by the attested wallet, debiting it
        // for exactly the quoted input: this fills the intent.
        origin_chain.set(|state| {
            state.token_receipts.insert(
                origin,
                wallet_receipt(test_payer_wallet(), origin, QUOTED_IN),
            );
        });
        make_due(&pool).await;
        poller.pass(DESTINATION).await;
        let row = intents.find(intent.id).await.unwrap().unwrap();
        assert_eq!(row.status, "filled");
        assert_eq!(row.attribution_source.as_deref(), Some("receipt"));
        assert_eq!(row.verified_origin_tx_hash(), Some(origin));
        assert_eq!(row.fill_tx_hashes(), vec![fill]);

        // A wallet that signed but spent a different amount is not the
        // quoted payment: the intent waits instead of filling.
        let (origin_b, fill_b) = (B256::repeat_byte(0xA2), B256::repeat_byte(0xF2));
        let wrong_amount = sent_intent(
            &pool,
            invoice(&pool, "relay-wrong-amount").await.id,
            0x02,
            origin_b,
        )
        .await;
        relay.answer(0x02, origin_b, fill_b);
        origin_chain.set(|state| {
            state.token_receipts.insert(
                origin_b,
                wallet_receipt(test_payer_wallet(), origin_b, QUOTED_IN - 1),
            );
        });
        make_due(&pool).await;
        poller.pass(DESTINATION).await;
        assert_eq!(
            intents.find(wrong_amount.id).await.unwrap().unwrap().status,
            "sent"
        );

        // A receipt signed by somebody else is conclusively not the
        // wallet's payment: the intent fails.
        let (origin_c, fill_c) = (B256::repeat_byte(0xA3), B256::repeat_byte(0xF3));
        let foreign = sent_intent(
            &pool,
            invoice(&pool, "relay-foreign").await.id,
            0x03,
            origin_c,
        )
        .await;
        relay.answer(0x03, origin_c, fill_c);
        origin_chain.set(|state| {
            state.token_receipts.insert(
                origin_c,
                wallet_receipt(Address::repeat_byte(0x99), origin_c, QUOTED_IN),
            );
        });
        make_due(&pool).await;
        poller.pass(DESTINATION).await;
        let row = intents.find(foreign.id).await.unwrap().unwrap();
        assert_eq!(row.status, "failed");
        assert_eq!(row.relay_status.as_deref(), Some("depositor_mismatch"));

        // A reverted origin transaction is not evidence of a payment.
        let (origin_d, fill_d) = (B256::repeat_byte(0xA4), B256::repeat_byte(0xF4));
        let reverted = sent_intent(
            &pool,
            invoice(&pool, "relay-reverted").await.id,
            0x04,
            origin_d,
        )
        .await;
        relay.answer(0x04, origin_d, fill_d);
        origin_chain.set(|state| {
            let mut receipt = wallet_receipt(test_payer_wallet(), origin_d, QUOTED_IN);
            receipt.outcome.succeeded = false;
            state.token_receipts.insert(origin_d, receipt);
        });
        make_due(&pool).await;
        poller.pass(DESTINATION).await;
        assert_eq!(
            intents.find(reverted.id).await.unwrap().unwrap().status,
            "sent"
        );
    }

    /// Without the origin chain this deployment cannot verify anything:
    /// Relay's record, whatever it names, defers and never attributes.
    #[sqlx::test(migrator = "gum_schema::MIGRATOR")]
    async fn an_unserved_origin_chain_never_attributes(pool: PgPool) {
        let relay = Arc::new(FakeRelay::default());
        let intents = RelayIntentRepository::new(pool.clone());
        let (origin, fill) = (B256::repeat_byte(0xA5), B256::repeat_byte(0xF5));
        let intent = sent_intent(
            &pool,
            invoice(&pool, "relay-unserved").await.id,
            0x05,
            origin,
        )
        .await;
        relay.answer(0x05, origin, fill);

        poller(&pool, Some(relay), None).pass(DESTINATION).await;
        let row = intents.find(intent.id).await.unwrap().unwrap();
        assert_eq!(row.status, "sent");
        assert_eq!(row.attribution_source, None);
        assert_eq!(row.relay_status, None, "it was never even asked");
        assert!(row.next_check_at.unwrap() > Utc::now());
    }

    /// The housekeeping that is only database work (expiring unsent quotes
    /// and failing stale sent intents, unparking what they held) runs
    /// without a Relay client too.
    #[sqlx::test(migrator = "gum_schema::MIGRATOR")]
    async fn housekeeping_without_a_relay_client_expires_and_unparks(pool: PgPool) {
        let intents = RelayIntentRepository::new(pool.clone());
        let invoice = invoice(&pool, "relay-cleanup").await;
        let (origin, fill) = (B256::repeat_byte(0xA6), B256::repeat_byte(0xF6));
        let stale = sent_intent(&pool, invoice.id, 0x06, origin).await;
        sqlx::query("UPDATE relay_intents SET sent_at = now() - interval '2 days'")
            .execute(&pool)
            .await
            .unwrap();
        // What the stale intent parked: a foreign transfer, parked.
        sqlx::query(
            r#"INSERT INTO payment_observations
                 (chain_id, token_address, block_number, block_hash, block_timestamp,
                  transaction_hash, transaction_index, log_index, sender_address,
                  recipient_address, invoice_id, amount, disposition, relay_parked_at)
               VALUES ($4, '\x0000000000000000000000000000000000000002', 7, $3,
                       700, $1, 0, 0, '\x0000000000000000000000000000000000000050',
                       '\x0000000000000000000000000000000000000011', $2, '1000000',
                       'credited', now())"#,
        )
        .bind(fill.as_slice())
        .bind(invoice.id)
        .bind(B256::repeat_byte(0x07).as_slice())
        .bind(DESTINATION as i64)
        .execute(&pool)
        .await
        .unwrap();

        poller(&pool, None, None).pass(DESTINATION).await;
        let row = intents.find(stale.id).await.unwrap().unwrap();
        assert_eq!(row.status, "failed");
        assert_eq!(row.relay_status.as_deref(), Some("unresolved"));
        let unparked: (bool, Option<Uuid>) = sqlx::query_as(
            "SELECT relay_parked_at IS NOT NULL, relay_intent_id FROM payment_observations WHERE transaction_hash = $1",
        )
        .bind(fill.as_slice())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(unparked, (false, None));
    }

    /// Pending answers defer the poll; a request Relay does not know is
    /// still waiting; success without a fill yet is retried; a refund
    /// resolves as such.
    #[sqlx::test(migrator = "gum_schema::MIGRATOR")]
    async fn pending_and_incomplete_answers_defer(pool: PgPool) {
        let relay = Arc::new(FakeRelay::default());
        let intents = RelayIntentRepository::new(pool.clone());
        let intent = sent_intent(
            &pool,
            invoice(&pool, "relay-pending").await.id,
            0x05,
            B256::repeat_byte(0xA5),
        )
        .await;
        let poller = poller(
            &pool,
            Some(relay.clone()),
            Some(Arc::new(MockChain::new(50))),
        );

        poller.pass(DESTINATION).await;
        let row = intents.find(intent.id).await.unwrap().unwrap();
        assert_eq!(row.status, "sent");
        assert_eq!(row.relay_status.as_deref(), Some("unknown"));
        assert!(row.next_check_at.unwrap() > Utc::now());

        // Success without a fill yet is retried.
        make_due(&pool).await;
        relay.statuses.lock().unwrap().insert(
            B256::repeat_byte(0x05),
            IntentStatus {
                state: IntentState::Success,
                raw: "success".into(),
                in_tx_hashes: vec![],
                tx_hashes: vec![],
            },
        );
        poller.pass(DESTINATION).await;
        assert_eq!(
            intents.find(intent.id).await.unwrap().unwrap().status,
            "sent"
        );

        // A refund resolves it as such.
        make_due(&pool).await;
        relay.statuses.lock().unwrap().insert(
            B256::repeat_byte(0x05),
            IntentStatus {
                state: IntentState::Refund,
                raw: "refund".into(),
                in_tx_hashes: vec![],
                tx_hashes: vec![],
            },
        );
        poller.pass(DESTINATION).await;
        assert_eq!(
            intents.find(intent.id).await.unwrap().unwrap().status,
            "refunded"
        );
    }
}
