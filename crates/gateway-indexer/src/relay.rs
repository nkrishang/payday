//! The withdrawal relayer: the signer's other job besides sweeping.
//!
//! A merchant's signed legs (`gateway_db::WithdrawalRepository`) are relayed
//! by the chain worker that owns the chain they act on, with the very same
//! transaction discipline as a sweep batch: one signer nonce per step,
//! signed-then-persisted-then-broadcast, same-nonce fee bumps, and a
//! finalized receipt before anything is concluded. `Indexer::sweep_tick`
//! gives a relay step priority over a new sweep batch and never runs either
//! while the other is in flight, so the signer's nonce stream has one owner.
//!
//! Per chain, each tick:
//! - housekeeping: expire authorizations that ran out unsigned or unrelayed;
//!   poll Circle for the attestation of every burn made from this chain;
//! - one step: reconcile the in-flight step if there is one, otherwise sign
//!   and broadcast the next: a mint for an attested leg whose destination is
//!   this chain, else a transfer or burn for an authorized leg whose source
//!   is this chain.
//!
//! The relayer decides nothing about where funds go. A transfer leg's
//! authorization names the destination; a bridge leg's names the forwarder
//! and commits to the CCTP recipient through its nonce; the mint's recipient
//! is inside Circle's attested message.

use std::time::Duration;

use alloy_primitives::{Address, B256, Bytes, U256};
use alloy_sol_types::{SolCall, sol};
use chrono::Utc;
use gateway_db::{DbWithdrawal, DbWithdrawalLeg, LegKind, LegState, MinedStep, StepOutcome};
use tracing::{info, warn};

use crate::chain::{FeeEstimate, PreparedSweepTransaction};
use crate::indexer::{Indexer, IndexerError, RANGE_RETRY_BACKOFF};
use crate::iris::AttestationStatus;

sol! {
    function transferWithAuthorization(address from, address to, uint256 value, uint256 validAfter, uint256 validBefore, bytes32 nonce, bytes signature);
    function bridge(address token, address from, uint256 value, uint32 destinationDomain, bytes32 mintRecipient, bytes32 salt, uint256 validBefore, bytes signature);
    function receiveMessage(bytes message, bytes attestation) returns (bool);
    function usedNonces(bytes32 nonce) view returns (uint256);
}

/// An authorization with less than this left is not relayed: the token
/// would refuse it by the time a transaction mined under a slow fee market,
/// and the leg is expired instead so the merchant can start again.
pub const MIN_AUTHORIZATION_VALIDITY: Duration = Duration::from_secs(10 * 60);

/// Gas limits per step. Not simulated, like sweeps: the calls are fixed
/// shapes, and Monad bills the limit, so these are also the price of a step.
const TRANSFER_GAS: u64 = 120_000;
const BRIDGE_GAS: u64 = 350_000;
const MINT_GAS: u64 = 350_000;

/// Offset of the message nonce in a CCTP V2 message header
/// (version 4, sourceDomain 4, destinationDomain 4, then the nonce).
const MESSAGE_NONCE_OFFSET: usize = 12;

/// What a step's transaction is: where it goes, what it says, what it may cost.
struct StepCall {
    to: Address,
    calldata: Bytes,
    gas_limit: u64,
}

impl Indexer {
    /// The relay work that needs no nonce: expiry, and Circle's attestations
    /// for burns made from this chain.
    pub(crate) async fn relay_housekeeping(&self) -> Result<(), IndexerError> {
        let expired = self
            .withdrawals
            .expire_stale(MIN_AUTHORIZATION_VALIDITY)
            .await?;
        if expired > 0 {
            info!(expired, "withdrawal legs expired unrelayed");
        }
        let Some(cctp) = &self.cfg.cctp else {
            return Ok(());
        };
        for leg in self
            .withdrawals
            .legs_awaiting_attestation(self.cfg.chain_id.0)
            .await?
        {
            let Some(burn_tx_hash) = leg.burn_tx_hash else {
                warn!(leg_id = %leg.id, "burned leg without a burn transaction; failing it");
                self.withdrawals
                    .fail_leg(leg.id, "burned leg has no burn transaction hash")
                    .await?;
                continue;
            };
            let next_check = Utc::now()
                + chrono::Duration::from_std(self.cfg.attestation_poll).unwrap_or_default();
            match self.iris.attestation(cctp.domain, burn_tx_hash).await {
                Ok(AttestationStatus::Complete {
                    message,
                    attestation,
                }) => {
                    self.withdrawals
                        .record_attestation(leg.id, &message, &attestation)
                        .await?;
                    info!(leg_id = %leg.id, %burn_tx_hash, "withdrawal burn attested");
                }
                Ok(status) => {
                    self.withdrawals
                        .defer_attestation(leg.id, next_check)
                        .await?;
                    tracing::debug!(leg_id = %leg.id, ?status, "withdrawal attestation not ready");
                }
                Err(error) => {
                    self.withdrawals
                        .defer_attestation(leg.id, next_check)
                        .await?;
                    warn!(leg_id = %leg.id, %error, "attestation poll failed; retrying");
                }
            }
        }
        Ok(())
    }

    /// Advance this chain's relay by one step. Returns true when a step is
    /// in flight or was just submitted, in which case the signer's nonce is
    /// spoken for and no sweep batch may be submitted this tick.
    pub(crate) async fn relay_step(&self) -> Result<bool, IndexerError> {
        if let Some(leg) = self.withdrawals.open_step(self.cfg.chain_id.0).await? {
            let step = leg.step.clone().ok_or_else(|| {
                IndexerError::Configuration(format!(
                    "open relay step on leg {} has no step",
                    leg.id
                ))
            })?;
            if step.broadcast_at.is_none() {
                let transaction = PreparedSweepTransaction {
                    hash: *step.tx_hashes.last().ok_or_else(|| {
                        IndexerError::Configuration(format!(
                            "relay step on leg {} has no transaction",
                            leg.id
                        ))
                    })?,
                    raw: step.raw_transactions.last().cloned().ok_or_else(|| {
                        IndexerError::Configuration(format!(
                            "relay step on leg {} has no raw transaction",
                            leg.id
                        ))
                    })?,
                };
                self.broadcast_step(&leg, &transaction).await?;
                return Ok(true);
            }
            self.reconcile_step(&leg, &step).await?;
            return Ok(true);
        }
        let Some(leg) = self
            .withdrawals
            .next_relayable(self.cfg.chain_id.0, MIN_AUTHORIZATION_VALIDITY)
            .await?
        else {
            return Ok(false);
        };
        self.submit_step(&leg).await?;
        Ok(true)
    }

    async fn submit_step(&self, leg: &DbWithdrawalLeg) -> Result<(), IndexerError> {
        let withdrawal = self.withdrawal_of(leg).await?;
        if leg.state == LegState::Attested {
            // `receiveMessage` is permissionless: somebody may have minted
            // already, in which case the leg is done without a step.
            if self.message_nonce_used(leg).await? {
                self.withdrawals.mark_minted_elsewhere(leg.id).await?;
                info!(leg_id = %leg.id, "withdrawal mint already executed by another party");
                return Ok(());
            }
        }
        let call = self.step_call(leg, &withdrawal)?;
        let nonce = self.chain.signer_nonce(true).await?;
        let fees = self.chain.estimate_fees().await?;
        let transaction = self
            .chain
            .prepare_call(call.to, call.calldata, nonce, call.gas_limit, fees)
            .await?;
        let recorded = self
            .withdrawals
            .record_step_submission(
                leg.id,
                self.cfg.chain_id.0,
                nonce,
                call.gas_limit,
                fees.max_fee_per_gas,
                fees.max_priority_fee_per_gas,
                transaction.hash,
                &transaction.raw,
            )
            .await?;
        if !recorded {
            // Cancelled or expired between the read and the write: the signed
            // bytes are dropped, never broadcast.
            info!(leg_id = %leg.id, "withdrawal leg left the queue before its step was recorded");
            return Ok(());
        }
        self.broadcast_step(leg, &transaction).await?;
        info!(leg_id = %leg.id, state = leg.state.as_str(), tx_hash = %transaction.hash, nonce, "withdrawal step broadcast");
        Ok(())
    }

    async fn broadcast_step(
        &self,
        leg: &DbWithdrawalLeg,
        transaction: &PreparedSweepTransaction,
    ) -> Result<(), IndexerError> {
        self.chain.broadcast_sweep_transaction(transaction).await?;
        if !self
            .withdrawals
            .record_step_broadcast(leg.id, transaction.hash)
            .await?
        {
            return Err(IndexerError::Configuration(format!(
                "durable transaction {} is not the pending broadcast for withdrawal leg {}",
                transaction.hash, leg.id
            )));
        }
        Ok(())
    }

    async fn reconcile_step(
        &self,
        leg: &DbWithdrawalLeg,
        step: &gateway_db::RelayStep,
    ) -> Result<(), IndexerError> {
        for &tx_hash in step.tx_hashes.iter().rev() {
            if let Some(receipt) = self.chain.transaction_receipt(tx_hash).await? {
                return self.apply_step_receipt(leg, step, tx_hash, receipt).await;
            }
        }
        self.handle_unmined_step(leg, step).await
    }

    async fn apply_step_receipt(
        &self,
        leg: &DbWithdrawalLeg,
        step: &gateway_db::RelayStep,
        tx_hash: B256,
        receipt: crate::chain::TransactionOutcome,
    ) -> Result<(), IndexerError> {
        let mined = MinedStep {
            tx_hash,
            block: receipt.block,
            block_hash: receipt.block_hash,
        };
        if step.mined != Some(mined) {
            self.withdrawals.record_step_mined(leg.id, mined).await?;
        }
        let mut attempt = 0u32;
        let mut backoff = RANGE_RETRY_BACKOFF;
        if receipt.block > self.finality_boundary(&mut attempt, &mut backoff).await? {
            return Ok(());
        }
        let header = self
            .retried_header(receipt.block, &mut attempt, &mut backoff)
            .await?;
        if header.hash != receipt.block_hash {
            self.withdrawals.clear_step_mined(leg.id).await?;
            warn!(leg_id = %leg.id, %tx_hash, block = receipt.block, "withdrawal step receipt is no longer canonical; waiting for a new receipt");
            return Ok(());
        }
        if !receipt.succeeded {
            if leg.state == LegState::Minting && self.message_nonce_used(leg).await? {
                self.withdrawals
                    .complete_step(leg.id, StepOutcome::Minted { tx_hash: None })
                    .await?;
                info!(leg_id = %leg.id, %tx_hash, "withdrawal mint reverted because another party minted first; leg complete");
                return Ok(());
            }
            let reason = match leg.state {
                LegState::Minting => "the mint transaction reverted",
                _ if leg.kind == LegKind::Transfer => "the transfer transaction reverted",
                _ => "the burn transaction reverted",
            };
            self.withdrawals.fail_leg(leg.id, reason).await?;
            warn!(leg_id = %leg.id, %tx_hash, reason, "withdrawal step reverted; leg failed");
            return Ok(());
        }
        let outcome = match (leg.state, leg.kind) {
            (LegState::Minting, _) => StepOutcome::Minted {
                tx_hash: Some(tx_hash),
            },
            (_, LegKind::Transfer) => StepOutcome::Transferred { tx_hash },
            (_, LegKind::Bridge) => StepOutcome::Burned {
                tx_hash,
                next_check_at: Utc::now()
                    + chrono::Duration::from_std(self.cfg.attestation_poll).unwrap_or_default(),
            },
        };
        self.withdrawals.complete_step(leg.id, outcome).await?;
        info!(leg_id = %leg.id, %tx_hash, block = receipt.block, ?outcome, "withdrawal step finalized");
        Ok(())
    }

    async fn handle_unmined_step(
        &self,
        leg: &DbWithdrawalLeg,
        step: &gateway_db::RelayStep,
    ) -> Result<(), IndexerError> {
        let age = Utc::now()
            .signed_duration_since(step.submitted_at)
            .to_std()
            .unwrap_or_default();
        if age < self.cfg.sweep_pending_timeout {
            return Ok(());
        }
        if self.chain.signer_nonce(false).await? > step.nonce {
            self.withdrawals.abandon_step(leg.id).await?;
            warn!(leg_id = %leg.id, nonce = step.nonce, "withdrawal step nonce was consumed without a visible receipt; leg returned to the queue");
            return Ok(());
        }
        if step.tx_hashes.len() as u32 >= self.cfg.sweep_max_submissions {
            return Err(IndexerError::SweepStalled(format!(
                "withdrawal leg {} step (nonce {}) is unconfirmed after {} submissions; check the signer balance and fee market, then replace or cancel nonce {} manually",
                leg.id,
                step.nonce,
                step.tx_hashes.len(),
                step.nonce
            )));
        }
        let previous = FeeEstimate {
            max_fee_per_gas: step.max_fee_per_gas,
            max_priority_fee_per_gas: step.max_priority_fee_per_gas,
        };
        let fees = self.chain.estimate_fees().await?.max(previous.bumped());
        let withdrawal = self.withdrawal_of(leg).await?;
        let call = self.step_call(leg, &withdrawal)?;
        let transaction = self
            .chain
            .prepare_call(call.to, call.calldata, step.nonce, step.gas_limit, fees)
            .await?;
        if !self
            .withdrawals
            .record_step_replacement(
                leg.id,
                transaction.hash,
                fees.max_fee_per_gas,
                fees.max_priority_fee_per_gas,
                &transaction.raw,
            )
            .await?
        {
            return Err(IndexerError::Configuration(format!(
                "could not persist replacement for withdrawal leg {} step",
                leg.id
            )));
        }
        self.broadcast_step(leg, &transaction).await?;
        warn!(leg_id = %leg.id, nonce = step.nonce, tx_hash = %transaction.hash, submission = step.tx_hashes.len() + 1, max_fee_per_gas = fees.max_fee_per_gas, "replaced unconfirmed withdrawal step");
        Ok(())
    }

    async fn withdrawal_of(&self, leg: &DbWithdrawalLeg) -> Result<DbWithdrawal, IndexerError> {
        self.withdrawals
            .by_id(leg.withdrawal_id)
            .await?
            .ok_or_else(|| {
                IndexerError::Configuration(format!(
                    "withdrawal {} of leg {} is missing",
                    leg.withdrawal_id, leg.id
                ))
            })
    }

    /// The transaction a leg's current state calls for, from the stored
    /// authorization alone.
    fn step_call(
        &self,
        leg: &DbWithdrawalLeg,
        withdrawal: &DbWithdrawal,
    ) -> Result<StepCall, IndexerError> {
        let malformed = |what: &str| {
            IndexerError::Configuration(format!("withdrawal leg {} has a malformed {what}", leg.id))
        };
        let from: Address = withdrawal
            .wallet_address
            .parse()
            .map_err(|_| malformed("wallet"))?;
        let signature = Bytes::from(
            leg.signature
                .clone()
                .ok_or_else(|| malformed("signature (leg is unsigned)"))?,
        );
        match (leg.state, leg.kind) {
            (LegState::Attested | LegState::Minting, _) => {
                let cctp = self.cfg.cctp.as_ref().ok_or_else(|| {
                    IndexerError::Configuration(format!(
                        "leg {} is attested for chain {} which has no CCTP configured",
                        leg.id, self.cfg.chain_id
                    ))
                })?;
                let message = leg
                    .attestation_message
                    .clone()
                    .ok_or_else(|| malformed("attestation message"))?;
                let attestation = leg
                    .attestation
                    .clone()
                    .ok_or_else(|| malformed("attestation"))?;
                Ok(StepCall {
                    to: cctp.message_transmitter,
                    calldata: receiveMessageCall {
                        message: message.into(),
                        attestation: attestation.into(),
                    }
                    .abi_encode()
                    .into(),
                    gas_limit: MINT_GAS,
                })
            }
            (LegState::Authorized | LegState::Relaying, LegKind::Transfer) => {
                let to: Address = leg
                    .authorization_to
                    .parse()
                    .map_err(|_| malformed("payee"))?;
                let token: Address = leg.token_address.parse().map_err(|_| malformed("token"))?;
                Ok(StepCall {
                    to: token,
                    calldata: transferWithAuthorizationCall {
                        from,
                        to,
                        value: leg.amount,
                        validAfter: U256::ZERO,
                        validBefore: U256::from(leg.valid_before),
                        nonce: leg.nonce,
                        signature,
                    }
                    .abi_encode()
                    .into(),
                    gas_limit: TRANSFER_GAS,
                })
            }
            (LegState::Authorized | LegState::Relaying, LegKind::Bridge) => {
                let forwarder: Address = leg
                    .authorization_to
                    .parse()
                    .map_err(|_| malformed("forwarder"))?;
                let token: Address = leg.token_address.parse().map_err(|_| malformed("token"))?;
                let destination: Address = withdrawal
                    .destination_address
                    .parse()
                    .map_err(|_| malformed("destination"))?;
                let destination_domain = self
                    .registry
                    .cctp(leg.destination_chain_id)
                    .map(|cctp| cctp.domain)
                    .ok_or_else(|| {
                        IndexerError::Configuration(format!(
                            "leg {} bridges to chain {} which has no CCTP configured",
                            leg.id, leg.destination_chain_id
                        ))
                    })?;
                Ok(StepCall {
                    to: forwarder,
                    calldata: bridgeCall {
                        token,
                        from,
                        value: leg.amount,
                        destinationDomain: destination_domain,
                        mintRecipient: destination.into_word(),
                        salt: leg.salt.ok_or_else(|| malformed("salt"))?,
                        validBefore: U256::from(leg.valid_before),
                        signature,
                    }
                    .abi_encode()
                    .into(),
                    gas_limit: BRIDGE_GAS,
                })
            }
            (state, kind) => Err(IndexerError::Configuration(format!(
                "leg {} in state {} ({}) has no relay step",
                leg.id,
                state.as_str(),
                kind.as_str()
            ))),
        }
    }

    /// Whether the destination transmitter has already consumed the leg's
    /// attested message.
    async fn message_nonce_used(&self, leg: &DbWithdrawalLeg) -> Result<bool, IndexerError> {
        let Some(cctp) = &self.cfg.cctp else {
            return Ok(false);
        };
        let Some(message) = &leg.attestation_message else {
            return Ok(false);
        };
        let nonce = message
            .get(MESSAGE_NONCE_OFFSET..MESSAGE_NONCE_OFFSET + 32)
            .ok_or_else(|| {
                IndexerError::Configuration(format!(
                    "withdrawal leg {} attestation message is shorter than a CCTP header",
                    leg.id
                ))
            })?;
        let output = self
            .chain
            .view_call(
                cctp.message_transmitter,
                usedNoncesCall {
                    nonce: B256::from_slice(nonce),
                }
                .abi_encode()
                .into(),
            )
            .await?;
        let used = usedNoncesCall::abi_decode_returns(&output).map_err(|error| {
            IndexerError::Configuration(format!(
                "could not decode MessageTransmitterV2.usedNonces response: {error}"
            ))
        })?;
        Ok(!used.is_zero())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use alloy_primitives::{Address, B256, U256, address};
    use alloy_sol_types::SolCall;
    use gateway_core::CctpConfig;
    use gateway_db::{
        LegKind, LegState, NewWithdrawal, NewWithdrawalLeg, StepOutcome, WithdrawalRepository,
    };
    use sqlx::PgPool;
    use uuid::Uuid;

    use super::*;
    use crate::indexer::tests::{
        CHAIN_ID, FakeIris, MockChain, OTHER_CHAIN_ID, config, indexer_with_relay, usdc,
    };
    use crate::indexer::{Indexer, IndexerConfig};
    use crate::iris::AttestationStatus;

    const WALLET: Address = address!("0x1111111111111111111111111111111111111111");
    const DESTINATION: Address = address!("0x2222222222222222222222222222222222222222");
    const FORWARDER: Address = address!("0x3333333333333333333333333333333333333333");
    const TRANSMITTER: Address = address!("0x4444444444444444444444444444444444444444");
    const MESSENGER: Address = address!("0x5555555555555555555555555555555555555555");

    fn cctp() -> CctpConfig {
        CctpConfig {
            domain: 15,
            token_messenger: MESSENGER,
            message_transmitter: TRANSMITTER,
            forwarder: FORWARDER,
            forwarder_code_hash: B256::repeat_byte(0xFF),
        }
    }

    fn relay_config(chain_id: u64) -> IndexerConfig {
        IndexerConfig {
            chain_id: gateway_core::ChainId(chain_id),
            cctp: Some(cctp()),
            ..config()
        }
    }

    async fn account(pool: &PgPool) -> gateway_db::AccountId {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO accounts (id, api_key_hash, api_key_hint) VALUES ($1, $2, 'hint')",
        )
        .bind(id)
        .bind(id.as_bytes().repeat(2))
        .execute(pool)
        .await
        .unwrap();
        gateway_db::AccountId(id)
    }

    fn leg(kind: LegKind, source: u64, valid_before: u64) -> NewWithdrawalLeg {
        NewWithdrawalLeg {
            id: Uuid::now_v7(),
            position: 0,
            kind,
            source_chain_id: source,
            amount: U256::from(2_500_000u64),
            token_address: usdc().to_checksum(None),
            domain_name: "USDC".into(),
            domain_version: "2".into(),
            authorization_to: match kind {
                LegKind::Transfer => DESTINATION.to_checksum(None),
                LegKind::Bridge => FORWARDER.to_checksum(None),
            },
            nonce: B256::repeat_byte(0x42),
            salt: matches!(kind, LegKind::Bridge).then(|| B256::repeat_byte(0x55)),
            valid_before,
        }
    }

    /// A signed, authorized withdrawal with one leg.
    async fn authorized_leg(
        pool: &PgPool,
        kind: LegKind,
        source: u64,
        destination: u64,
        valid_before: u64,
    ) -> (WithdrawalRepository, Uuid, Uuid) {
        let repo = WithdrawalRepository::new(pool.clone());
        let account = account(pool).await;
        let leg = leg(kind, source, valid_before);
        let leg_id = leg.id;
        let input = NewWithdrawal {
            id: Uuid::now_v7(),
            account_id: account,
            idempotency_key: "k".into(),
            wallet_address: WALLET.to_checksum(None),
            destination_chain_id: destination,
            destination_address: DESTINATION.to_checksum(None),
            legs: vec![leg],
        };
        repo.create(&input).await.unwrap();
        repo.authorize(account, input.id, leg_id, &[0x11u8; 65])
            .await
            .unwrap();
        (repo, input.id, leg_id)
    }

    fn far_future() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 86_400
    }

    /// A CCTP V2 message with the given nonce in its header slot.
    fn message_with_nonce(nonce: B256) -> Vec<u8> {
        let mut message = vec![0u8; 376];
        message[MESSAGE_NONCE_OFFSET..MESSAGE_NONCE_OFFSET + 32].copy_from_slice(nonce.as_slice());
        message
    }

    async fn leg_state(repo: &WithdrawalRepository, withdrawal: Uuid) -> DbWithdrawalLeg {
        repo.legs(withdrawal).await.unwrap().remove(0)
    }

    fn indexer(
        pool: &PgPool,
        chain: Arc<MockChain>,
        chain_id: u64,
        iris: Arc<FakeIris>,
    ) -> Indexer {
        indexer_with_relay(pool, chain, relay_config(chain_id), iris)
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn a_transfer_leg_is_relayed_as_transfer_with_authorization(pool: PgPool) {
        let (repo, withdrawal, _) =
            authorized_leg(&pool, LegKind::Transfer, CHAIN_ID, CHAIN_ID, far_future()).await;
        let chain = Arc::new(MockChain::new(10));
        let worker = indexer(
            &pool,
            chain.clone(),
            CHAIN_ID,
            Arc::new(FakeIris::default()),
        );

        worker.sweep_tick().await.unwrap();
        let submissions = chain.submissions();
        assert_eq!(submissions.len(), 1);
        assert_eq!(submissions[0].to, Some(usdc()));
        assert_eq!(submissions[0].gas_limit, TRANSFER_GAS);
        let call = transferWithAuthorizationCall::abi_decode(&submissions[0].calldata).unwrap();
        assert_eq!(call.from, WALLET);
        assert_eq!(call.to, DESTINATION);
        assert_eq!(call.value, U256::from(2_500_000u64));
        assert_eq!(call.validAfter, U256::ZERO);
        assert_eq!(call.nonce, B256::repeat_byte(0x42));
        assert_eq!(call.signature.as_ref(), &[0x11u8; 65]);
        let leg = leg_state(&repo, withdrawal).await;
        assert_eq!(leg.state, LegState::Relaying);
        assert!(leg.step.as_ref().unwrap().broadcast_at.is_some());

        // The mock mined it in the finalized block; the next tick concludes.
        worker.sweep_tick().await.unwrap();
        let leg = leg_state(&repo, withdrawal).await;
        assert_eq!(leg.state, LegState::Completed);
        assert_eq!(leg.transfer_tx_hash, Some(submissions[0].tx_hash));
        assert!(leg.step.is_none());
        assert!(
            repo.by_id(withdrawal)
                .await
                .unwrap()
                .unwrap()
                .completed_at
                .is_some()
        );
        worker.sweep_tick().await.unwrap();
        assert_eq!(chain.submissions().len(), 1, "nothing left to relay");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn a_bridge_leg_burns_waits_for_circle_and_mints_on_the_other_chain(pool: PgPool) {
        let (repo, withdrawal, _) = authorized_leg(
            &pool,
            LegKind::Bridge,
            CHAIN_ID,
            OTHER_CHAIN_ID,
            far_future(),
        )
        .await;
        let iris = Arc::new(FakeIris::default());
        let source = Arc::new(MockChain::new(10));
        let source_worker = indexer(&pool, source.clone(), CHAIN_ID, iris.clone());
        let destination = Arc::new(MockChain::new(10));
        let destination_worker = indexer(&pool, destination.clone(), OTHER_CHAIN_ID, iris.clone());

        // The destination chain has nothing to do yet.
        destination_worker.sweep_tick().await.unwrap();
        assert!(destination.submissions().is_empty());

        source_worker.sweep_tick().await.unwrap();
        let burn = source.submissions().remove(0);
        assert_eq!(burn.to, Some(FORWARDER));
        assert_eq!(burn.gas_limit, BRIDGE_GAS);
        let call = bridgeCall::abi_decode(&burn.calldata).unwrap();
        assert_eq!(call.token, usdc());
        assert_eq!(call.from, WALLET);
        assert_eq!(call.destinationDomain, 6, "the destination's CCTP domain");
        assert_eq!(call.mintRecipient, DESTINATION.into_word());
        assert_eq!(call.salt, B256::repeat_byte(0x55));
        assert_eq!(call.value, U256::from(2_500_000u64));

        source_worker.sweep_tick().await.unwrap();
        let leg = leg_state(&repo, withdrawal).await;
        assert_eq!(leg.state, LegState::Burned);
        assert_eq!(leg.burn_tx_hash, Some(burn.tx_hash));

        // Circle has not indexed it: polled from the source domain, deferred.
        tokio::time::sleep(Duration::from_millis(15)).await;
        source_worker.sweep_tick().await.unwrap();
        assert_eq!(iris.polls.lock().unwrap().as_slice(), &[(15, burn.tx_hash)]);
        assert_eq!(leg_state(&repo, withdrawal).await.state, LegState::Burned);

        let message_nonce = B256::repeat_byte(0x77);
        iris.answers.lock().unwrap().insert(
            burn.tx_hash,
            AttestationStatus::Complete {
                message: message_with_nonce(message_nonce).into(),
                attestation: vec![0xAA; 65].into(),
            },
        );
        tokio::time::sleep(Duration::from_millis(15)).await;
        source_worker.sweep_tick().await.unwrap();
        let leg = leg_state(&repo, withdrawal).await;
        assert_eq!(leg.state, LegState::Attested);
        assert_eq!(
            source.submissions().len(),
            1,
            "the source chain does not mint"
        );

        destination_worker.sweep_tick().await.unwrap();
        let mint = destination.submissions().remove(0);
        assert_eq!(mint.to, Some(TRANSMITTER));
        assert_eq!(mint.gas_limit, MINT_GAS);
        let call = receiveMessageCall::abi_decode(&mint.calldata).unwrap();
        assert_eq!(
            call.message.as_ref(),
            message_with_nonce(message_nonce).as_slice()
        );
        assert_eq!(call.attestation.as_ref(), &[0xAA; 65][..]);
        assert_eq!(leg_state(&repo, withdrawal).await.state, LegState::Minting);

        destination_worker.sweep_tick().await.unwrap();
        let leg = leg_state(&repo, withdrawal).await;
        assert_eq!(leg.state, LegState::Completed);
        assert_eq!(leg.mint_tx_hash, Some(mint.tx_hash));
        assert!(
            repo.by_id(withdrawal)
                .await
                .unwrap()
                .unwrap()
                .completed_at
                .is_some()
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn a_reverted_step_fails_the_leg_and_the_withdrawal(pool: PgPool) {
        let (repo, withdrawal, _) =
            authorized_leg(&pool, LegKind::Transfer, CHAIN_ID, CHAIN_ID, far_future()).await;
        let chain = Arc::new(MockChain::new(10).with(|state| state.next_receipt_succeeds = false));
        let worker = indexer(
            &pool,
            chain.clone(),
            CHAIN_ID,
            Arc::new(FakeIris::default()),
        );

        worker.sweep_tick().await.unwrap();
        worker.sweep_tick().await.unwrap();
        let leg = leg_state(&repo, withdrawal).await;
        assert_eq!(leg.state, LegState::Failed);
        assert_eq!(
            leg.failure_reason.as_deref(),
            Some("the transfer transaction reverted")
        );
        assert!(leg.step.is_none());
        assert!(
            repo.by_id(withdrawal)
                .await
                .unwrap()
                .unwrap()
                .failed_at
                .is_some()
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn a_mint_somebody_else_delivered_completes_without_a_transaction(pool: PgPool) {
        let (repo, withdrawal, leg_id) = authorized_leg(
            &pool,
            LegKind::Bridge,
            OTHER_CHAIN_ID,
            CHAIN_ID,
            far_future(),
        )
        .await;
        // Drive the leg to attested by hand: the burn happened on the other chain.
        assert!(
            repo.record_step_submission(
                leg_id,
                OTHER_CHAIN_ID,
                1,
                1,
                1,
                1,
                B256::repeat_byte(1),
                b"x"
            )
            .await
            .unwrap()
        );
        assert!(
            repo.complete_step(
                leg_id,
                StepOutcome::Burned {
                    tx_hash: B256::repeat_byte(1),
                    next_check_at: Utc::now(),
                }
            )
            .await
            .unwrap()
        );
        let message_nonce = B256::repeat_byte(0x77);
        assert!(
            repo.record_attestation(leg_id, &message_with_nonce(message_nonce), &[0xAA; 65])
                .await
                .unwrap()
        );
        let chain = Arc::new(MockChain::new(10).with(|state| {
            state.view_results.insert(
                (
                    TRANSMITTER,
                    usedNoncesCall {
                        nonce: message_nonce,
                    }
                    .abi_encode()
                    .into(),
                ),
                B256::with_last_byte(1).to_vec().into(),
            );
        }));
        let worker = indexer(
            &pool,
            chain.clone(),
            CHAIN_ID,
            Arc::new(FakeIris::default()),
        );

        worker.sweep_tick().await.unwrap();
        assert!(chain.submissions().is_empty(), "no mint of our own");
        let leg = leg_state(&repo, withdrawal).await;
        assert_eq!(leg.state, LegState::Completed);
        assert_eq!(leg.mint_tx_hash, None);
        assert!(
            repo.by_id(withdrawal)
                .await
                .unwrap()
                .unwrap()
                .completed_at
                .is_some()
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn an_authorization_about_to_lapse_is_expired_instead_of_relayed(pool: PgPool) {
        let soon = far_future() - 86_400 + 60;
        let (repo, withdrawal, _) =
            authorized_leg(&pool, LegKind::Transfer, CHAIN_ID, CHAIN_ID, soon).await;
        let chain = Arc::new(MockChain::new(10));
        let worker = indexer(
            &pool,
            chain.clone(),
            CHAIN_ID,
            Arc::new(FakeIris::default()),
        );

        worker.sweep_tick().await.unwrap();
        assert!(chain.submissions().is_empty());
        let leg = leg_state(&repo, withdrawal).await;
        assert_eq!(leg.state, LegState::Expired);
        assert!(
            repo.by_id(withdrawal)
                .await
                .unwrap()
                .unwrap()
                .failed_at
                .is_some()
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn an_unmined_step_is_bumped_and_a_consumed_nonce_requeues_the_leg(pool: PgPool) {
        let (repo, withdrawal, _) =
            authorized_leg(&pool, LegKind::Transfer, CHAIN_ID, CHAIN_ID, far_future()).await;
        let chain = Arc::new(MockChain::new(10).with(|state| state.mine_at = None));
        let worker = indexer(
            &pool,
            chain.clone(),
            CHAIN_ID,
            Arc::new(FakeIris::default()),
        );

        worker.sweep_tick().await.unwrap();
        let first = chain.submissions().remove(0);
        // Not yet timed out: nothing happens.
        worker.sweep_tick().await.unwrap();
        assert_eq!(chain.submissions().len(), 1);

        sqlx::query("UPDATE withdrawal_legs SET step_submitted_at = now() - interval '1 hour'")
            .execute(&pool)
            .await
            .unwrap();
        worker.sweep_tick().await.unwrap();
        let submissions = chain.submissions();
        assert_eq!(submissions.len(), 2, "a same-nonce replacement");
        assert_eq!(submissions[1].nonce, first.nonce);
        assert!(submissions[1].fees.max_fee_per_gas > first.fees.max_fee_per_gas);
        let step = leg_state(&repo, withdrawal).await.step.unwrap();
        assert_eq!(step.tx_hashes, vec![first.tx_hash, submissions[1].tx_hash]);

        // The nonce mined elsewhere without a receipt for either: back to the queue.
        sqlx::query("UPDATE withdrawal_legs SET step_submitted_at = now() - interval '1 hour'")
            .execute(&pool)
            .await
            .unwrap();
        chain.set(|state| state.mined_nonce = first.nonce + 1);
        worker.sweep_tick().await.unwrap();
        let leg = leg_state(&repo, withdrawal).await;
        assert_eq!(leg.state, LegState::Authorized);
        assert!(leg.step.is_none());
    }
}
