//! The withdrawal relayer: the signer pool's other job besides sweeping.
//!
//! A merchant's signed legs (`gateway_db::WithdrawalRepository`) are relayed
//! by the chain worker that owns the chain they act on, with the very same
//! transaction discipline as a sweep batch: one nonce of one pool signer
//! per step, signed-then-persisted-then-broadcast, same-nonce fee bumps, and
//! a finalized receipt before anything is concluded. A signer owns at most
//! one open step or batch; `Indexer::sweep_pass` gives a free signer a relay
//! step before a new sweep batch, and steps and batches run side by side on
//! different signers.
//!
//! Per chain, each pass:
//! - housekeeping: expire authorizations that ran out unsigned or unrelayed;
//!   poll Circle for the attestation of every burn made from this chain;
//! - lanes: reconcile every in-flight step, one per signer;
//! - submission: a free signer signs and broadcasts the next step: a mint
//!   for an attested leg whose destination is this chain, else a transfer or
//!   burn for an authorized leg whose source is this chain.
//!
//! The relayer decides nothing about where funds go. A transfer leg's
//! authorization names the destination; a bridge leg's names the forwarder
//! and commits to the CCTP recipient through its nonce; the mint's recipient
//! is inside Circle's attested message.

use std::time::Duration;

use alloy_primitives::{Address, B256, Bytes, U256};
use alloy_sol_types::{SolCall, sol};
use chrono::Utc;
use gateway_db::{
    DbWithdrawal, DbWithdrawalLeg, LegKind, LegState, MinedStep, RetryStep, StepOutcome,
};
use tokio::sync::OnceCell;
use tracing::{info, warn};

use crate::chain::{FeeEstimate, PreparedSweepTransaction};
use crate::indexer::{Indexer, IndexerError, LaneOutcome, RANGE_RETRY_BACKOFF};
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

/// A reverted step whose obligation survives — an unused authorization, an
/// attestation Circle has already signed — is retried on this base backoff,
/// which doubles with every consecutive revert; only a run of
/// `MAX_STEP_REVERTS` reverts makes the failure permanent.
const STEP_RETRY_BACKOFF: Duration = Duration::from_secs(30);
const MAX_STEP_REVERTS: u32 = 5;

/// How long a step whose nonce was spent without any visible receipt or
/// consuming transaction may wait for the execution to surface before the
/// worker reports itself stalled for an operator. The step and its history
/// are untouched either way; the wait only keeps reporting honest.
const UNRESOLVED_EXECUTION_LIMIT: Duration = Duration::from_secs(24 * 60 * 60);

/// How many legs a free signer will look at in one pass before giving up
/// on finding one that needs a transaction: a leg can leave the queue
/// without one (minted by somebody else, cancelled between the read and the
/// write), and the next leg may still need the signer.
const MAX_RELAY_CANDIDATES: usize = 8;

/// Offset of the message nonce in a CCTP V2 message header
/// (version 4, sourceDomain 4, destinationDomain 4, then the nonce).
const MESSAGE_NONCE_OFFSET: usize = 12;

/// A CCTP V2 message's header is 148 bytes (version, the two domains, the
/// nonce, sender, recipient, destinationCaller, and the two finality fields),
/// and its packed burn body follows: a body version, then the burn token,
/// the mint recipient, the amount, the depositor, the fee, and the fee
/// execution, as 32-byte big-endian words.
const MESSAGE_SENDER_OFFSET: usize = 44;
const DESTINATION_CALLER_OFFSET: usize = 108;
const BURN_TOKEN_OFFSET: usize = 152;
const MINT_RECIPIENT_OFFSET: usize = 184;
const AMOUNT_OFFSET: usize = 216;
const BODY_MESSAGE_SENDER_OFFSET: usize = 248;
const MAX_FEE_OFFSET: usize = 280;
const MIN_BURN_MESSAGE_LEN: usize = 376;

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
                Ok(AttestationStatus::Complete { messages }) => {
                    let withdrawal = self.withdrawal_of(&leg).await?;
                    match messages.iter().find(|(message, _)| {
                        self.message_describes_burn(&leg, &withdrawal, cctp, message)
                            .is_ok()
                    }) {
                        Some((message, attestation)) => {
                            self.withdrawals
                                .record_attestation(leg.id, message, attestation)
                                .await?;
                            info!(leg_id = %leg.id, %burn_tx_hash, "withdrawal burn attested");
                        }
                        None => {
                            self.withdrawals
                                .defer_attestation(leg.id, next_check)
                                .await?;
                            warn!(leg_id = %leg.id, %burn_tx_hash, messages = messages.len(), "none of the attestation service's messages describes this burn; none is recorded and the poll retries");
                        }
                    }
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

    /// The message an attestation answer carries must be this leg's: it
    /// leaves the source domain this worker burns from, heads for the leg's
    /// destination domain, and carries a nonce the transmitter could have
    /// consumed (`usedNonces[0]` starts used, so a zero-nonce message would
    /// look already-minted without ever minting), and its burn body is
    /// exactly what this leg's forwarder burn produces: the source token
    /// messenger as the sender, no designated destination caller, the leg's
    /// token, the withdrawal's recipient, the leg's amount, the forwarder
    /// itself as the depositor, and no fee. The body checks matter because
    /// one transaction can carry several CCTP messages with identical
    /// domains — a helper that burns for itself and for the merchant in one
    /// call — so only the full envelope separates this burn from a foreign
    /// one Payday could never mint. A mangled or foreign answer is refused,
    /// and the leg waits for the right one.
    fn message_describes_burn(
        &self,
        leg: &DbWithdrawalLeg,
        withdrawal: &DbWithdrawal,
        cctp: &gateway_core::CctpConfig,
        message: &[u8],
    ) -> Result<(), &'static str> {
        let destination_domain = self
            .registry
            .cctp(leg.destination_chain_id)
            .map(|cctp| cctp.domain)
            .ok_or("the destination chain has no CCTP configured")?;
        if message.len() < MIN_BURN_MESSAGE_LEN {
            return Err("the message is shorter than a CCTP V2 burn message");
        }
        let domain =
            |at: usize| u32::from_be_bytes(message[at..at + 4].try_into().expect("4 bytes"));
        if domain(4) != cctp.domain {
            return Err("the message left another source domain");
        }
        if domain(8) != destination_domain {
            return Err("the message heads for another destination domain");
        }
        if message[MESSAGE_NONCE_OFFSET..MESSAGE_NONCE_OFFSET + 32]
            .iter()
            .all(|&byte| byte == 0)
        {
            return Err("the message's nonce is zero, which no transmitter can consume");
        }
        let word = |at: usize| B256::from_slice(&message[at..at + 32]);
        if word(MESSAGE_SENDER_OFFSET) != cctp.token_messenger.into_word() {
            return Err("the message was not sent by this chain's token messenger");
        }
        if !word(DESTINATION_CALLER_OFFSET).is_zero() {
            return Err("the message names a designated destination caller Payday cannot satisfy");
        }
        let token: Address = leg
            .token_address
            .parse()
            .map_err(|_| "the leg's token address is malformed")?;
        if word(BURN_TOKEN_OFFSET) != token.into_word() {
            return Err("the message burns another token");
        }
        let recipient: Address = withdrawal
            .destination_address
            .parse()
            .map_err(|_| "the withdrawal's destination address is malformed")?;
        if word(MINT_RECIPIENT_OFFSET) != recipient.into_word() {
            return Err("the message mints to another recipient");
        }
        if U256::from_be_bytes::<32>(
            message[AMOUNT_OFFSET..AMOUNT_OFFSET + 32]
                .try_into()
                .expect("32 bytes"),
        ) != leg.amount
        {
            return Err("the message carries another amount");
        }
        if word(BODY_MESSAGE_SENDER_OFFSET) != cctp.forwarder.into_word() {
            return Err("the message was not deposited by this chain's forwarder");
        }
        if !word(MAX_FEE_OFFSET).is_zero() {
            return Err("the message carries a fee the forwarder never pays");
        }
        Ok(())
    }

    /// Advance one open step: broadcast it if its newest signed transaction
    /// never left, otherwise reconcile it against the chain.
    pub(crate) async fn step_lane(
        &self,
        leg: DbWithdrawalLeg,
        fees: &OnceCell<FeeEstimate>,
    ) -> Result<LaneOutcome, IndexerError> {
        let step = leg.step.clone().ok_or_else(|| {
            IndexerError::Configuration(format!("open relay step on leg {} has no step", leg.id))
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
            return Ok(LaneOutcome::Open);
        }
        self.reconcile_step(&leg, &step, fees).await
    }

    /// Give a free signer the next leg to relay. Returns true when a step
    /// was signed and broadcast on it, false when nothing needed one.
    pub(crate) async fn take_relay_step(
        &self,
        signer: Address,
        fees: &OnceCell<FeeEstimate>,
    ) -> Result<bool, IndexerError> {
        for _ in 0..MAX_RELAY_CANDIDATES {
            let Some(leg) = self
                .withdrawals
                .next_relayable(self.cfg.chain_id.0, MIN_AUTHORIZATION_VALIDITY)
                .await?
            else {
                return Ok(false);
            };
            if self.submit_step(signer, &leg, fees).await? {
                return Ok(true);
            }
        }
        warn!(
            chain_id = self.cfg.chain_id.0,
            "relayable legs kept leaving the queue without a transaction; trying again next pass"
        );
        Ok(false)
    }

    /// Sign and broadcast `leg`'s next step from `signer`. False when the
    /// leg needed no transaction after all (already minted by another party,
    /// or gone from the queue before the step was recorded).
    async fn submit_step(
        &self,
        signer: Address,
        leg: &DbWithdrawalLeg,
        fees: &OnceCell<FeeEstimate>,
    ) -> Result<bool, IndexerError> {
        if leg.state == LegState::Attested {
            // `receiveMessage` is permissionless: somebody may have minted
            // already, in which case submitting our own would only revert
            // once it finalizes. The check reads at the finalized boundary,
            // so it can only ever complete a leg whose mint is itself final.
            if self.message_nonce_used(leg).await? {
                self.withdrawals.complete_minted_elsewhere(leg.id).await?;
                info!(leg_id = %leg.id, "withdrawal mint already executed by another party; leg complete");
                return Ok(false);
            }
        }
        let withdrawal = self.withdrawal_of(leg).await?;
        let call = self.step_call(leg, &withdrawal)?;
        let nonce = self.chain.signer_nonce(signer, true).await?;
        let fees = self.tick_fees(fees).await?;
        let transaction = self
            .chain
            .prepare_call(signer, call.to, call.calldata, nonce, call.gas_limit, fees)
            .await?;
        let recorded = self
            .withdrawals
            .record_step_submission(
                leg.id,
                self.cfg.chain_id.0,
                signer,
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
            return Ok(false);
        }
        self.broadcast_step(leg, &transaction).await?;
        info!(leg_id = %leg.id, state = leg.state.as_str(), tx_hash = %transaction.hash, %signer, nonce, "withdrawal step broadcast");
        Ok(true)
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
        fees: &OnceCell<FeeEstimate>,
    ) -> Result<LaneOutcome, IndexerError> {
        for &tx_hash in step.tx_hashes.iter().rev() {
            if let Some(receipt) = self.chain.transaction_receipt(tx_hash).await? {
                return self.apply_step_receipt(leg, step, tx_hash, receipt).await;
            }
        }
        self.handle_unmined_step(leg, step, fees).await
    }

    async fn apply_step_receipt(
        &self,
        leg: &DbWithdrawalLeg,
        step: &gateway_db::RelayStep,
        tx_hash: B256,
        receipt: crate::chain::TransactionOutcome,
    ) -> Result<LaneOutcome, IndexerError> {
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
            return Ok(LaneOutcome::Open);
        }
        let header = self
            .retried_header(receipt.block, &mut attempt, &mut backoff)
            .await?;
        if header.hash != receipt.block_hash {
            self.withdrawals.clear_step_mined(leg.id).await?;
            warn!(leg_id = %leg.id, %tx_hash, block = receipt.block, "withdrawal step receipt is no longer canonical; waiting for a new receipt");
            return Ok(LaneOutcome::Open);
        }
        if !receipt.succeeded {
            // Interpret the revert only once it is final: the reads below
            // see the world at the finalized boundary, so an execution by
            // another party near the reverted transaction is invisible
            // until then, and counting the revert meanwhile could exhaust
            // the retries of a step somebody else already completed.
            let finalized = self.chain.finalized_header().await?;
            if receipt.block > finalized.number {
                return Ok(LaneOutcome::Open);
            }
            if self.reconcile_executed_elsewhere(leg).await? {
                return Ok(LaneOutcome::Resolved);
            }
            let reason = match leg.state {
                LegState::Minting => "the mint transaction reverted",
                _ if leg.kind == LegKind::Transfer => "the transfer transaction reverted",
                _ => "the burn transaction reverted",
            };
            self.retry_or_fail(leg, reason).await?;
            return Ok(LaneOutcome::Resolved);
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
        Ok(LaneOutcome::Resolved)
    }

    async fn handle_unmined_step(
        &self,
        leg: &DbWithdrawalLeg,
        step: &gateway_db::RelayStep,
        fees: &OnceCell<FeeEstimate>,
    ) -> Result<LaneOutcome, IndexerError> {
        let age = Utc::now()
            .signed_duration_since(step.submitted_at)
            .to_std()
            .unwrap_or_default();
        if age < self.cfg.sweep_pending_timeout {
            return Ok(LaneOutcome::Open);
        }
        if self.chain.signer_nonce(step.signer, false).await? > step.nonce {
            // The nonce was spent without a visible receipt: one of our
            // submissions mined (this RPC lost its receipt) or somebody
            // consumed the authorization first. Establish where the funds
            // are before touching the step. While the evidence is not final
            // yet — or the consuming transaction has not been located — the
            // step stays exactly as it is, transaction history included, and
            // the next tick reconciles again: the consuming transaction
            // enters the finalized window the moment it is final.
            if self.reconcile_executed_elsewhere(leg).await? {
                return Ok(LaneOutcome::Resolved);
            }
            let unresolved_for = Utc::now()
                .signed_duration_since(step.submitted_at)
                .to_std()
                .unwrap_or_default();
            if unresolved_for < UNRESOLVED_EXECUTION_LIMIT {
                tracing::debug!(leg_id = %leg.id, signer = %step.signer, nonce = step.nonce, "withdrawal step nonce was spent without a visible receipt; waiting for the execution to surface");
                return Ok(LaneOutcome::Open);
            }
            // The wait has run out of patience: report the worker stalled —
            // it retries every tick — without touching the step, whose
            // history is the operator's evidence.
            return Err(IndexerError::SweepStalled(format!(
                "withdrawal leg {} step (signer {}, nonce {}) has been unexplained for {} s: the nonce is spent but no receipt or consuming transaction has surfaced; check the signer's nonce history and the leg, then resolve manually",
                leg.id,
                step.signer,
                step.nonce,
                unresolved_for.as_secs()
            )));
        }
        if step.tx_hashes.len() as u32 >= self.cfg.sweep_max_submissions {
            return Err(IndexerError::SweepStalled(format!(
                "withdrawal leg {} step (signer {}, nonce {}) is unconfirmed after {} submissions; check the signer balance and fee market, then replace or cancel nonce {} manually",
                leg.id,
                step.signer,
                step.nonce,
                step.tx_hashes.len(),
                step.nonce
            )));
        }
        let previous = FeeEstimate {
            max_fee_per_gas: step.max_fee_per_gas,
            max_priority_fee_per_gas: step.max_priority_fee_per_gas,
        };
        let fees = self.tick_fees(fees).await?.max(previous.bumped());
        let withdrawal = self.withdrawal_of(leg).await?;
        let call = self.step_call(leg, &withdrawal)?;
        let transaction = self
            .chain
            .prepare_call(
                step.signer,
                call.to,
                call.calldata,
                step.nonce,
                step.gas_limit,
                fees,
            )
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
        warn!(leg_id = %leg.id, signer = %step.signer, nonce = step.nonce, tx_hash = %transaction.hash, submission = step.tx_hashes.len() + 1, max_fee_per_gas = fees.max_fee_per_gas, "replaced unconfirmed withdrawal step");
        Ok(LaneOutcome::Open)
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

    /// Whether this leg's work was already done by another party: a mint
    /// whose nonce Circle's transmitter has consumed (finalized, so a
    /// competitor's block cannot later leave the chain), or a source
    /// authorization somebody else executed. The forwarder is permissionless
    /// and a transfer leg's payee is the destination itself, so a competing
    /// transaction moves the funds exactly where the signature commits them;
    /// the leg is then complete, with the competitor's transaction as its
    /// evidence. Returns false when the leg is still ours to do.
    async fn reconcile_executed_elsewhere(
        &self,
        leg: &DbWithdrawalLeg,
    ) -> Result<bool, IndexerError> {
        if leg.state == LegState::Minting {
            if !self.message_nonce_used(leg).await? {
                return Ok(false);
            }
            self.withdrawals
                .complete_step(leg.id, StepOutcome::Minted { tx_hash: None })
                .await?;
            info!(leg_id = %leg.id, "withdrawal mint already executed by another party; leg complete");
            return Ok(true);
        }
        let Some(tx_hash) = self.authorization_consumed_elsewhere(leg).await? else {
            return Ok(false);
        };
        let outcome = match leg.kind {
            LegKind::Transfer => StepOutcome::Transferred { tx_hash },
            LegKind::Bridge => StepOutcome::Burned {
                tx_hash,
                next_check_at: Utc::now()
                    + chrono::Duration::from_std(self.cfg.attestation_poll).unwrap_or_default(),
            },
        };
        self.withdrawals.complete_step(leg.id, outcome).await?;
        info!(leg_id = %leg.id, %tx_hash, "withdrawal authorization executed by another party; leg complete");
        Ok(true)
    }

    /// The transaction that consumed the leg's authorization, when it was
    /// not ours: `None` while the authorization is still unconsumed. The
    /// consuming transaction went through the payee — the forwarder for a
    /// bridge leg, the destination for a transfer leg — because USDC refuses
    /// `receiveWithAuthorization` from anyone else, so its burn/transfer
    /// landed where the signature commits it and Circle can attest it.
    async fn authorization_consumed_elsewhere(
        &self,
        leg: &DbWithdrawalLeg,
    ) -> Result<Option<B256>, IndexerError> {
        let withdrawal = self.withdrawal_of(leg).await?;
        let authorizer: Address = withdrawal.wallet_address.parse().map_err(|_| {
            IndexerError::Configuration(format!("withdrawal leg {} has a malformed wallet", leg.id))
        })?;
        let token: Address = leg.token_address.parse().map_err(|_| {
            IndexerError::Configuration(format!("withdrawal leg {} has a malformed token", leg.id))
        })?;
        let finalized = self.chain.finalized_header().await?;
        if !self
            .chain
            .authorization_state(token, authorizer, leg.nonce, finalized.number)
            .await?
        {
            return Ok(None);
        }
        // The nonce is spent, so our reverted transaction did not do it.
        // Find the transaction that did: newest-first in provider-sized
        // chunks, down to the block the leg was created in — the
        // authorization is at most one TTL old.
        let start = self.authorization_search_start(leg, finalized.number);
        let mut upper = finalized.number;
        loop {
            let lower = upper.saturating_sub(self.cfg.log_range_size - 1).max(start);
            match self
                .chain
                .authorization_used_tx(token, authorizer, leg.nonce, lower, upper)
                .await?
            {
                Some((tx_hash, receipt)) => {
                    if !receipt.succeeded {
                        return Err(IndexerError::Configuration(format!(
                            "AuthorizationUsed in {} belongs to a reverted transaction",
                            tx_hash
                        )));
                    }
                    return Ok(Some(tx_hash));
                }
                None if lower == start => {
                    warn!(leg_id = %leg.id, nonce = %leg.nonce, "an authorization was consumed but its transaction is outside the searched history");
                    return Ok(None);
                }
                None => upper = lower - 1,
            }
        }
    }

    /// Where the backward search for a consuming transaction may start: the
    /// block the leg was created in, minus a margin for the block-time
    /// estimate, never before the deployment.
    fn authorization_search_start(&self, leg: &DbWithdrawalLeg, finalized: u64) -> u64 {
        let elapsed = Utc::now()
            .signed_duration_since(leg.created_at)
            .to_std()
            .unwrap_or_default();
        let blocks_since =
            u64::try_from(elapsed.as_millis() / self.cfg.block_time.as_millis().max(1) + 120)
                .unwrap_or(u64::MAX);
        finalized
            .saturating_sub(blocks_since)
            .max(self.cfg.start_block)
    }

    /// A reverted step whose obligation survives is retried with a backoff;
    /// a run of reverts is a real failure and ends the leg.
    async fn retry_or_fail(&self, leg: &DbWithdrawalLeg, reason: &str) -> Result<(), IndexerError> {
        match self
            .withdrawals
            .retry_step(leg.id, STEP_RETRY_BACKOFF, MAX_STEP_REVERTS, reason)
            .await?
        {
            RetryStep::Retried => {
                info!(leg_id = %leg.id, reason, "withdrawal step reverted recoverably; leg returns to the queue after a backoff");
            }
            RetryStep::Failed => {
                warn!(leg_id = %leg.id, reason, "withdrawal step reverted once too often; leg failed");
            }
        }
        Ok(())
    }

    /// Whether the destination transmitter has already consumed the leg's
    /// attested message, read at the finalized boundary.
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
            .finalized_view_call(
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
        CHAIN_ID, FakeIris, MockChain, OTHER_CHAIN_ID, config, indexer_with_relay, insert_funded,
        make_invoice, mock_authorization_tx_hash, mock_signer, usdc,
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
            currency: "USDC".into(),
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

    /// A complete CCTP V2 burn message exactly as this chain's forwarder
    /// produces it: the given source and destination domains and nonce in
    /// its header, and the test's messenger, token, recipient, amount, and
    /// forwarder in the fields the validator pins.
    fn message_with_nonce(source_domain: u32, destination_domain: u32, nonce: B256) -> Vec<u8> {
        let mut message = vec![0u8; MIN_BURN_MESSAGE_LEN];
        message[4..8].copy_from_slice(&source_domain.to_be_bytes());
        message[8..12].copy_from_slice(&destination_domain.to_be_bytes());
        message[MESSAGE_NONCE_OFFSET..MESSAGE_NONCE_OFFSET + 32].copy_from_slice(nonce.as_slice());
        message[MESSAGE_SENDER_OFFSET..MESSAGE_SENDER_OFFSET + 32]
            .copy_from_slice(MESSENGER.into_word().as_slice());
        message[BURN_TOKEN_OFFSET..BURN_TOKEN_OFFSET + 32]
            .copy_from_slice(usdc().into_word().as_slice());
        message[MINT_RECIPIENT_OFFSET..MINT_RECIPIENT_OFFSET + 32]
            .copy_from_slice(DESTINATION.into_word().as_slice());
        message[AMOUNT_OFFSET..AMOUNT_OFFSET + 32]
            .copy_from_slice(&U256::from(2_500_000u64).to_be_bytes::<32>());
        message[BODY_MESSAGE_SENDER_OFFSET..BODY_MESSAGE_SENDER_OFFSET + 32]
            .copy_from_slice(FORWARDER.into_word().as_slice());
        message
    }

    fn complete_attestation(message: Vec<u8>) -> AttestationStatus {
        AttestationStatus::Complete {
            messages: vec![(message.into(), vec![0xAA; 65].into())],
        }
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

    /// The invoice's row, for the sweep side of a mixed pass.
    async fn invoice_status(pool: &PgPool, invoice: &gateway_core::Invoice) -> String {
        sqlx::query_scalar("SELECT status FROM invoices WHERE id = $1")
            .bind(invoice.id.0)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn a_relay_step_and_a_sweep_batch_share_a_pass_on_different_signers(pool: PgPool) {
        let (repo, withdrawal, _) =
            authorized_leg(&pool, LegKind::Transfer, CHAIN_ID, CHAIN_ID, far_future()).await;
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1", 1).await;
        let chain = Arc::new(
            MockChain::new(10)
                .with(|state| state.mine_at = None)
                .with_signers(2),
        );
        let worker = indexer(
            &pool,
            chain.clone(),
            CHAIN_ID,
            Arc::new(FakeIris::default()),
        );

        worker.sweep_tick().await.unwrap();
        let submissions = chain.submissions();
        assert_eq!(submissions.len(), 2);
        // The first free signer relays; the next one sweeps.
        assert_eq!(submissions[0].to, Some(usdc()));
        assert_eq!(submissions[0].signer, mock_signer(0));
        assert_eq!(submissions[1].to, None);
        assert_eq!(submissions[1].signer, mock_signer(1));
        assert_eq!(leg_state(&repo, withdrawal).await.state, LegState::Relaying);
        assert_eq!(invoice_status(&pool, &invoice).await, "deploying");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn with_one_signer_the_relay_step_takes_priority_and_the_batch_waits(pool: PgPool) {
        let (repo, withdrawal, _) =
            authorized_leg(&pool, LegKind::Transfer, CHAIN_ID, CHAIN_ID, far_future()).await;
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1", 1).await;
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
        assert_eq!(invoice_status(&pool, &invoice).await, "funded");

        // The step finalizes and frees the signer, which sweeps in the same pass.
        worker.sweep_tick().await.unwrap();
        assert_eq!(
            leg_state(&repo, withdrawal).await.state,
            LegState::Completed
        );
        let submissions = chain.submissions();
        assert_eq!(submissions.len(), 2);
        assert_eq!(submissions[1].to, None);
        assert_eq!(invoice_status(&pool, &invoice).await, "deploying");
        worker.sweep_tick().await.unwrap();
        assert_eq!(invoice_status(&pool, &invoice).await, "fulfilled");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn a_signer_with_an_open_batch_and_an_open_step_halts(pool: PgPool) {
        let (_repo, _withdrawal, _) =
            authorized_leg(&pool, LegKind::Transfer, CHAIN_ID, CHAIN_ID, far_future()).await;
        let invoice = make_invoice(100);
        insert_funded(&pool, &invoice, "key-1", 1).await;
        let chain = Arc::new(
            MockChain::new(10)
                .with(|state| state.mine_at = None)
                .with_signers(2),
        );
        let worker = indexer(
            &pool,
            chain.clone(),
            CHAIN_ID,
            Arc::new(FakeIris::default()),
        );
        worker.sweep_tick().await.unwrap();
        assert_eq!(chain.submissions().len(), 2);

        // Corrupt the ledger: the batch now claims the step's signer.
        sqlx::query("UPDATE sweep_batches SET signer = $1")
            .bind(mock_signer(0).as_slice())
            .execute(&pool)
            .await
            .unwrap();
        let error = worker.sweep_tick().await.unwrap_err();
        assert!(matches!(error, IndexerError::Configuration(_)));
        assert!(
            error
                .to_string()
                .contains("more than one open helper transaction")
        );
        assert_eq!(
            chain.submissions().len(),
            2,
            "nothing is signed while the ledger is inconsistent"
        );
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

    async fn burned_bridge_leg(
        pool: &PgPool,
        iris: Arc<FakeIris>,
    ) -> (WithdrawalRepository, Uuid, B256, Indexer) {
        let (repo, withdrawal, _) = authorized_leg(
            pool,
            LegKind::Bridge,
            CHAIN_ID,
            OTHER_CHAIN_ID,
            far_future(),
        )
        .await;
        let source = Arc::new(MockChain::new(10));
        let worker = indexer(pool, source.clone(), CHAIN_ID, iris);
        worker.sweep_tick().await.unwrap();
        let burn = source.submissions().remove(0);
        worker.sweep_tick().await.unwrap();
        assert_eq!(leg_state(&repo, withdrawal).await.state, LegState::Burned);
        (repo, withdrawal, burn.tx_hash, worker)
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn an_attestation_message_for_another_burn_is_not_recorded(pool: PgPool) {
        // A transaction can carry several CCTP messages, and a helper's burn
        // may share this leg's domains and header shape. Only a message whose
        // body is this leg's — token, recipient, amount — may be recorded.
        let mut foreign = message_with_nonce(15, 6, B256::repeat_byte(0x77));
        foreign[MINT_RECIPIENT_OFFSET..MINT_RECIPIENT_OFFSET + 32]
            .copy_from_slice(B256::repeat_byte(0x99).as_slice());
        let ours = message_with_nonce(15, 6, B256::repeat_byte(0x77));

        // A foreign-only answer is refused: the leg stays burned and polls on.
        let iris = Arc::new(FakeIris::default());
        let (repo, withdrawal, burn, worker) = burned_bridge_leg(&pool, iris.clone()).await;
        iris.answers
            .lock()
            .unwrap()
            .insert(burn, complete_attestation(foreign.clone()));
        tokio::time::sleep(Duration::from_millis(15)).await;
        worker.sweep_tick().await.unwrap();
        let leg = leg_state(&repo, withdrawal).await;
        assert_eq!(
            leg.state,
            LegState::Burned,
            "a foreign answer is not recorded"
        );
        assert_eq!(leg.attestation_message, None);

        // Among several messages, the leg's own is the one recorded.
        let iris = Arc::new(FakeIris::default());
        let (repo, withdrawal, burn, worker) = burned_bridge_leg(&pool, iris.clone()).await;
        iris.answers.lock().unwrap().insert(
            burn,
            AttestationStatus::Complete {
                messages: vec![
                    (foreign.into(), vec![0xAA; 65].into()),
                    (ours.clone().into(), vec![0xAA; 65].into()),
                ],
            },
        );
        tokio::time::sleep(Duration::from_millis(15)).await;
        worker.sweep_tick().await.unwrap();
        let leg = leg_state(&repo, withdrawal).await;
        assert_eq!(
            leg.state,
            LegState::Attested,
            "the leg's own message is the one recorded"
        );
        assert_eq!(
            leg.attestation_message.as_deref(),
            Some(ours.as_slice()),
            "the leg's own message is the one recorded"
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn an_attested_leg_somebody_else_minted_completes_without_submitting(pool: PgPool) {
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
        source_worker.sweep_tick().await.unwrap();
        let burn = source.submissions().remove(0);
        source_worker.sweep_tick().await.unwrap();
        let message_nonce = B256::repeat_byte(0x77);
        iris.answers.lock().unwrap().insert(
            burn.tx_hash,
            complete_attestation(message_with_nonce(15, 6, message_nonce)),
        );
        tokio::time::sleep(Duration::from_millis(15)).await;
        source_worker.sweep_tick().await.unwrap();
        assert_eq!(leg_state(&repo, withdrawal).await.state, LegState::Attested);

        // `receiveMessage` is permissionless: another party's mint consumed
        // the message, finality included. The relayer must complete the leg
        // on the very tick it would have submitted its own mint, and submit
        // nothing.
        let destination = Arc::new(MockChain::new(10).with(|state| {
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
        let destination_worker = indexer(&pool, destination.clone(), OTHER_CHAIN_ID, iris);
        destination_worker.sweep_tick().await.unwrap();
        let leg = leg_state(&repo, withdrawal).await;
        assert_eq!(leg.state, LegState::Completed);
        assert_eq!(leg.mint_tx_hash, None, "no mint of our own landed");
        assert!(
            destination.submissions().is_empty(),
            "an already-minted attested leg submits nothing"
        );
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
            complete_attestation(message_with_nonce(15, 6, message_nonce)),
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
            message_with_nonce(15, 6, message_nonce).as_slice()
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
    async fn a_reverted_step_returns_to_the_queue_instead_of_failing(pool: PgPool) {
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
        // The transfer reverted, but its authorization is unconsumed and the
        // merchant's signature still stands: the leg is queued again with a
        // backoff, not failed.
        let leg = leg_state(&repo, withdrawal).await;
        assert_eq!(leg.state, LegState::Authorized);
        assert!(leg.step.is_none());
        assert!(
            repo.by_id(withdrawal)
                .await
                .unwrap()
                .unwrap()
                .failed_at
                .is_none()
        );
        let (reverts, retry_at) = sqlx::query_as::<_, (i16, Option<chrono::DateTime<Utc>>)>(
            "SELECT step_reverts, step_retry_at FROM withdrawal_legs WHERE id = $1",
        )
        .bind(leg.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(reverts, 1);
        assert!(retry_at.is_some());
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn a_mint_somebody_else_delivered_completes_without_minting(pool: PgPool) {
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
                crate::indexer::tests::mock_signer(0),
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
            repo.record_attestation(
                leg_id,
                &message_with_nonce(15, 6, message_nonce),
                &[0xAA; 65]
            )
            .await
            .unwrap()
        );
        let chain = Arc::new(MockChain::new(10).with(|state| {
            state.next_receipt_succeeds = false;
            // Another party's mint consumed the message: our own reverts.
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
        worker.sweep_tick().await.unwrap();
        let leg = leg_state(&repo, withdrawal).await;
        assert_eq!(leg.state, LegState::Completed);
        assert_eq!(leg.mint_tx_hash, None, "no mint of our own landed");
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
    async fn a_burn_somebody_else_executed_is_found_and_still_mints(pool: PgPool) {
        let (repo, withdrawal, _) = authorized_leg(
            &pool,
            LegKind::Bridge,
            CHAIN_ID,
            OTHER_CHAIN_ID,
            far_future(),
        )
        .await;
        // The forwarder is permissionless: whoever replays our calldata
        // consumes the authorization first, our transaction reverts on the
        // spent nonce, and the burn that succeeded is someone else's. Its
        // transaction is what Circle must attest.
        let chain = Arc::new(MockChain::new(10).with(|state| {
            state.next_receipt_succeeds = false;
            state
                .consumed_authorizations
                .insert((WALLET, B256::repeat_byte(0x42)));
            state
                .authorization_events
                .insert(B256::repeat_byte(0x42), 9);
        }));
        let iris = Arc::new(FakeIris::default());
        let source_worker = indexer(&pool, chain.clone(), CHAIN_ID, iris.clone());
        let destination = Arc::new(MockChain::new(10));
        let destination_worker = indexer(&pool, destination.clone(), OTHER_CHAIN_ID, iris.clone());

        source_worker.sweep_tick().await.unwrap();
        source_worker.sweep_tick().await.unwrap();
        let leg = leg_state(&repo, withdrawal).await;
        assert_eq!(leg.state, LegState::Burned);
        assert_eq!(
            leg.burn_tx_hash,
            Some(mock_authorization_tx_hash(B256::repeat_byte(0x42))),
            "the competitor's transaction is the burn Circle attests"
        );

        // Circle attests the burn that actually happened, and the mint lands
        // on the destination chain: nothing stranded.
        let message_nonce = B256::repeat_byte(0x77);
        iris.answers.lock().unwrap().insert(
            mock_authorization_tx_hash(B256::repeat_byte(0x42)),
            complete_attestation(message_with_nonce(15, 6, message_nonce)),
        );
        tokio::time::sleep(Duration::from_millis(15)).await;
        source_worker.sweep_tick().await.unwrap();
        destination_worker.sweep_tick().await.unwrap();
        destination_worker.sweep_tick().await.unwrap();
        let leg = leg_state(&repo, withdrawal).await;
        assert_eq!(leg.state, LegState::Completed);
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
    async fn a_transfer_leg_the_destination_executed_itself_completes(pool: PgPool) {
        let (repo, withdrawal, _) =
            authorized_leg(&pool, LegKind::Transfer, CHAIN_ID, CHAIN_ID, far_future()).await;
        // The payee of a transfer leg is the destination address itself, and
        // USDC lets only the payee consume a receive-less authorization: the
        // destination may race us and win. Their transaction did exactly
        // what ours would have, and is kept as the leg's evidence.
        let chain = Arc::new(MockChain::new(10).with(|state| {
            state.next_receipt_succeeds = false;
            state
                .consumed_authorizations
                .insert((WALLET, B256::repeat_byte(0x42)));
            state
                .authorization_events
                .insert(B256::repeat_byte(0x42), 9);
        }));
        let worker = indexer(
            &pool,
            chain.clone(),
            CHAIN_ID,
            Arc::new(FakeIris::default()),
        );

        worker.sweep_tick().await.unwrap();
        worker.sweep_tick().await.unwrap();
        let leg = leg_state(&repo, withdrawal).await;
        assert_eq!(leg.state, LegState::Completed);
        assert_eq!(
            leg.transfer_tx_hash,
            Some(mock_authorization_tx_hash(B256::repeat_byte(0x42)))
        );
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
    async fn an_unmined_step_is_bumped_and_a_consumed_nonce_is_reconciled(pool: PgPool) {
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

        // The nonce mined elsewhere without a receipt for either: the step
        // and its history stay in flight while reconciliation looks.
        sqlx::query("UPDATE withdrawal_legs SET step_submitted_at = now() - interval '1 hour'")
            .execute(&pool)
            .await
            .unwrap();
        chain.set(|state| {
            state.mined_nonces.insert(first.signer, first.nonce + 1);
        });
        worker.sweep_tick().await.unwrap();
        let leg = leg_state(&repo, withdrawal).await;
        assert_eq!(leg.state, LegState::Relaying, "no evidence yet, no touch");
        let step = leg.step.clone().unwrap();
        assert_eq!(
            step.tx_hashes,
            vec![first.tx_hash, submissions[1].tx_hash],
            "the attempted transactions are kept while the evidence is missing"
        );
        assert_eq!(step.raw_transactions.len(), 2);

        // Long past the wait, still unexplained: the worker reports itself
        // stalled for an operator — and the step still stays untouched.
        sqlx::query("UPDATE withdrawal_legs SET step_submitted_at = now() - interval '25 hours'")
            .execute(&pool)
            .await
            .unwrap();
        let error = worker.sweep_tick().await.unwrap_err();
        assert!(matches!(error, IndexerError::SweepStalled(_)));
        let leg = leg_state(&repo, withdrawal).await;
        assert_eq!(leg.state, LegState::Relaying);
        assert_eq!(
            leg.step.unwrap().tx_hashes,
            vec![first.tx_hash, submissions[1].tx_hash]
        );

        // The consuming transaction surfaces, finality included: the same
        // step resolves into the completed leg.
        chain.set(|state| {
            state
                .consumed_authorizations
                .insert((WALLET, B256::repeat_byte(0x42)));
            state
                .authorization_events
                .insert(B256::repeat_byte(0x42), 9);
        });
        worker.sweep_tick().await.unwrap();
        let leg = leg_state(&repo, withdrawal).await;
        assert_eq!(
            leg.state,
            LegState::Completed,
            "the consuming transaction is found and completes the leg"
        );
        assert_eq!(
            leg.transfer_tx_hash,
            Some(mock_authorization_tx_hash(B256::repeat_byte(0x42)))
        );
        assert!(leg.step.is_none());
    }
}
