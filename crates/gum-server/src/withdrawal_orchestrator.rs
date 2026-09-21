//! Drives merchant withdrawals from `authorized` to `completed`.
//!
//! The ledger owns each leg's state machine (`gum_ledger::withdrawals`);
//! the signers own the transactions. This loop is the policy between them:
//! it decides *which* step a leg needs next, builds the calldata from the
//! stored authorization, opens the step in the ledger and publishes the
//! `WithdrawalStep` command in the same transaction, and between a bridge
//! leg's burn and mint it polls Circle for the attestation the mint needs.
//! It never signs and never talks to a wallet.
//!
//! ```text
//! authorized ──(pass: begin_step + publish)──▶ relaying ──(signers' event)──▶ completed | burned
//! burned ──(pass: Iris says complete)──▶ attested ──(pass: begin_step + publish)──▶ minting ──▶ completed
//! ```
//!
//! # Crash boundaries
//!
//! * Between reading a relayable leg and opening its step: nothing was
//!   written; the next pass sees the leg again.
//! * Opening the step and publishing the command share one transaction, so
//!   a crash there leaves the leg `authorized` with no command on the bus,
//!   or `relaying` with the command committed. Never one without the other.
//! * A step the signers have taken but not reported keeps its `step_job_id`
//!   in the ledger; `relayable` excludes it, so no second command is ever
//!   built for a step in flight, and the signers reconcile the transaction
//!   themselves however long that takes.
//! * Attestation polling writes only `attested` (with the message) or a new
//!   `attestation_next_check_at`; a crash mid-poll costs one poll interval.
//!
//! A leg whose stored authorization cannot be turned into a call (a
//! malformed address, a missing salt) is failed with the reason rather than
//! retried forever, because no later pass would build it differently.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use alloy_primitives::{Address, B256, Bytes, U256};
use alloy_sol_types::{SolCall, sol};
use chrono::Utc;
use gum_bus::Publisher;
use gum_chain::ChainReader;
use gum_contracts::{
    CorrelationId, ExecutionCommand, StepPrecondition, WithdrawalStepCommand, WithdrawalStepKind,
};
use gum_core::{CctpConfig, ChainConfig, ChainRegistry};
use gum_ledger::{DbWithdrawal, DbWithdrawalLeg, StepPolicy, WithdrawalRepository};
use sqlx::PgPool;
use tokio::sync::watch;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::iris::{AttestationSource, AttestationStatus};

sol! {
    function transferWithAuthorization(address from, address to, uint256 value, uint256 validAfter, uint256 validBefore, bytes32 nonce, bytes signature);
    function bridge(address token, address from, uint256 value, uint32 destinationDomain, bytes32 mintRecipient, bytes32 salt, uint256 validBefore, bytes signature);
    function receiveMessage(bytes message, bytes attestation) returns (bool);
}

/// An authorization with less than this left is not relayed: by the time
/// the signers mine it, `validBefore` could have passed and the revert
/// would cost gas for nothing. Expiry housekeeping retires such legs.
pub const MIN_AUTHORIZATION_VALIDITY: Duration = Duration::from_secs(10 * 60);
/// `not_after` handed to the signers: the authorization's `validBefore`
/// minus this, so a step is never submitted into its last minute.
const NOT_AFTER_MARGIN: u64 = 60;
/// How often the loop runs when nothing else drives it.
const PASS_INTERVAL: Duration = Duration::from_secs(3);
/// Steps opened per chain per pass.
const STEPS_PER_PASS: i64 = 8;

pub const TRANSFER_GAS: u64 = 120_000;
pub const BRIDGE_GAS: u64 = 350_000;
pub const MINT_GAS: u64 = 350_000;

/// Byte layout of a CCTP V2 `BurnMessage` inside its `Message` envelope
/// (header: version 0..4, source domain 4..8, destination domain 8..12,
/// nonce 12..44, sender 44..76, recipient 76..108, destination caller
/// 108..140, min finality 140..144, finality executed 144..148, body from
/// 148: version, burn token, mint recipient, amount, message sender, max
/// fee, fee executed, expiration, hook data).
pub(crate) const MESSAGE_NONCE_OFFSET: usize = 12;
pub(crate) const MESSAGE_SENDER_OFFSET: usize = 44;
pub(crate) const DESTINATION_CALLER_OFFSET: usize = 108;
pub(crate) const BURN_TOKEN_OFFSET: usize = 152;
pub(crate) const MINT_RECIPIENT_OFFSET: usize = 184;
pub(crate) const AMOUNT_OFFSET: usize = 216;
pub(crate) const BODY_MESSAGE_SENDER_OFFSET: usize = 248;
pub(crate) const MAX_FEE_OFFSET: usize = 280;
pub(crate) const MIN_BURN_MESSAGE_LEN: usize = 376;

/// What one pass over one chain did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PassReport {
    /// Burned legs whose attestation arrived and was recorded.
    pub attested: u32,
    /// Burned legs still waiting on Circle.
    pub deferred: u32,
    /// `WithdrawalStep` commands published.
    pub steps_opened: u32,
    /// Legs failed because their stored authorization cannot be executed.
    pub failed: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum OrchestratorError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Bus(#[from] gum_bus::BusError),
    #[error("chain {0} is not configured")]
    UnknownChain(u64),
}

pub struct WithdrawalOrchestrator {
    withdrawals: WithdrawalRepository,
    pool: PgPool,
    registry: Arc<ChainRegistry>,
    publisher: Publisher,
    attestations: Arc<dyn AttestationSource>,
    policy: StepPolicy,
    /// Read-only chain access per chain, for the finalized head the
    /// authorization search is anchored to. A chain without one anchors
    /// at its deployment block, which is correct and merely slower for
    /// the signers.
    chains: HashMap<u64, Arc<dyn ChainReader>>,
}

impl WithdrawalOrchestrator {
    pub fn new(
        withdrawals: WithdrawalRepository,
        pool: PgPool,
        registry: Arc<ChainRegistry>,
        publisher: Publisher,
        attestations: Arc<dyn AttestationSource>,
        policy: StepPolicy,
        chains: HashMap<u64, Arc<dyn ChainReader>>,
    ) -> Self {
        Self {
            withdrawals,
            pool,
            registry,
            publisher,
            attestations,
            policy,
            chains,
        }
    }

    pub async fn run(self, mut shutdown: watch::Receiver<bool>) {
        info!("withdrawal orchestrator started");
        loop {
            match self
                .withdrawals
                .expire_stale(MIN_AUTHORIZATION_VALIDITY)
                .await
            {
                Ok(0) => {}
                Ok(expired) => info!(expired, "withdrawal legs expired unrelayed"),
                Err(error) => warn!(error = %error, "withdrawal expiry failed; retrying next pass"),
            }
            for chain in self.registry.chains() {
                match self.pass(chain.chain_id).await {
                    Ok(report) if report != PassReport::default() => {
                        info!(chain_id = chain.chain_id, ?report, "withdrawal pass");
                    }
                    Ok(_) => {}
                    Err(error) => {
                        warn!(chain_id = chain.chain_id, error = %error, "withdrawal pass failed; retrying next pass");
                    }
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(PASS_INTERVAL) => {}
                result = shutdown.changed() => {
                    if result.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
            }
        }
    }

    /// One pass over one chain: poll attestations for burns made here,
    /// then open the next steps due here.
    pub async fn pass(&self, chain_id: u64) -> Result<PassReport, OrchestratorError> {
        let config = self
            .registry
            .get(chain_id)
            .ok_or(OrchestratorError::UnknownChain(chain_id))?;
        let mut report = PassReport::default();
        if let Some(cctp) = &config.cctp {
            self.poll_attestations(chain_id, cctp, &mut report).await?;
        }
        self.open_steps(config, &mut report).await?;
        Ok(report)
    }

    async fn poll_attestations(
        &self,
        chain_id: u64,
        cctp: &CctpConfig,
        report: &mut PassReport,
    ) -> Result<(), OrchestratorError> {
        for leg in self.withdrawals.legs_awaiting_attestation(chain_id).await? {
            let Some(burn_tx_hash) = leg.burn_tx_hash else {
                error!(leg_id = %leg.id, withdrawal_id = %leg.withdrawal_id, chain_id, "burned leg without a burn transaction; failing it");
                self.withdrawals
                    .fail_leg(leg.id, "burned leg has no burn transaction hash")
                    .await?;
                report.failed += 1;
                continue;
            };
            let next_check = Utc::now()
                + chrono::Duration::from_std(self.policy.attestation_poll).unwrap_or_default();
            match self
                .attestations
                .attestation(cctp.domain, burn_tx_hash)
                .await
            {
                Ok(AttestationStatus::Complete { messages }) => {
                    let Some(withdrawal) = self.withdrawals.by_id(leg.withdrawal_id).await? else {
                        // Legs are deleted with their withdrawal; a row read
                        // moments ago is gone only under a race with an
                        // operator. Nothing to record.
                        continue;
                    };
                    let ours = messages.iter().find(|(message, _)| {
                        message_describes_burn(&leg, &withdrawal, cctp, &self.registry, message)
                            .is_ok()
                    });
                    match ours {
                        Some((message, attestation)) => {
                            self.withdrawals
                                .record_attestation(leg.id, message, attestation)
                                .await?;
                            info!(leg_id = %leg.id, withdrawal_id = %leg.withdrawal_id, chain_id, tx_hash = %burn_tx_hash, "withdrawal burn attested");
                            report.attested += 1;
                        }
                        None => {
                            self.withdrawals
                                .defer_attestation(leg.id, next_check)
                                .await?;
                            warn!(leg_id = %leg.id, chain_id, tx_hash = %burn_tx_hash, messages = messages.len(), "none of the attestation service's messages describes this burn; polling on");
                            report.deferred += 1;
                        }
                    }
                }
                Ok(status) => {
                    self.withdrawals
                        .defer_attestation(leg.id, next_check)
                        .await?;
                    debug!(leg_id = %leg.id, chain_id, tx_hash = %burn_tx_hash, ?status, "withdrawal attestation not ready");
                    report.deferred += 1;
                }
                Err(error) => {
                    self.withdrawals
                        .defer_attestation(leg.id, next_check)
                        .await?;
                    warn!(leg_id = %leg.id, chain_id, tx_hash = %burn_tx_hash, error = %error, "attestation poll failed; retrying");
                    report.deferred += 1;
                }
            }
        }
        Ok(())
    }

    async fn open_steps(
        &self,
        config: &ChainConfig,
        report: &mut PassReport,
    ) -> Result<(), OrchestratorError> {
        let chain_id = config.chain_id;
        let legs = self
            .withdrawals
            .relayable(chain_id, MIN_AUTHORIZATION_VALIDITY, STEPS_PER_PASS)
            .await?;
        for leg in legs {
            let Some(withdrawal) = self.withdrawals.by_id(leg.withdrawal_id).await? else {
                continue;
            };
            let command = match self.command(config, &leg, &withdrawal).await {
                Ok(command) => command,
                Err(reason) => {
                    error!(leg_id = %leg.id, withdrawal_id = %withdrawal.id, chain_id, error = %reason, "withdrawal leg cannot be executed; failing it");
                    self.withdrawals.fail_leg(leg.id, &reason).await?;
                    report.failed += 1;
                    continue;
                }
            };
            let mut tx = self.pool.begin().await?;
            let opened = self
                .withdrawals
                .begin_step(&mut tx, leg.id, command.job_id, chain_id)
                .await?;
            if !opened {
                // Cancelled or already taken between the read and the lock.
                tx.rollback().await?;
                continue;
            }
            self.publisher
                .publish(
                    &mut tx,
                    &ExecutionCommand::WithdrawalStep(command.clone()),
                    CorrelationId::from(withdrawal.id),
                    None,
                )
                .await?;
            tx.commit().await?;
            info!(
                leg_id = %leg.id, withdrawal_id = %withdrawal.id, correlation_id = %withdrawal.id,
                chain_id, job_id = %command.job_id, step = ?command.step, to = %command.to,
                "withdrawal step requested"
            );
            report.steps_opened += 1;
        }
        Ok(())
    }

    /// The command a leg's current state calls for, from the stored
    /// authorization alone. `Err` names what makes the leg unexecutable.
    async fn command(
        &self,
        config: &ChainConfig,
        leg: &DbWithdrawalLeg,
        withdrawal: &DbWithdrawal,
    ) -> Result<WithdrawalStepCommand, String> {
        let chain_id = config.chain_id;
        let step = leg
            .step_kind()
            .ok_or_else(|| format!("leg in state {} has no step to run", leg.state.as_str()))?;
        let from: Address = withdrawal
            .wallet_address
            .parse()
            .map_err(|_| "the withdrawal's wallet address is malformed".to_owned())?;
        let token: Address = leg
            .token_address
            .parse()
            .map_err(|_| "the leg's token address is malformed".to_owned())?;
        let signature = || -> Result<Bytes, String> {
            leg.signature
                .clone()
                .map(Bytes::from)
                .ok_or_else(|| "the leg is unsigned".to_owned())
        };
        let not_after = Some(leg.valid_before.saturating_sub(NOT_AFTER_MARGIN));
        let (to, calldata, gas_limit, precondition, not_after) = match step {
            WithdrawalStepKind::Transfer => {
                let to: Address = leg
                    .authorization_to
                    .parse()
                    .map_err(|_| "the leg's payee is malformed".to_owned())?;
                let calldata = transferWithAuthorizationCall {
                    from,
                    to,
                    value: leg.amount,
                    validAfter: U256::ZERO,
                    validBefore: U256::from(leg.valid_before),
                    nonce: leg.nonce,
                    signature: signature()?,
                }
                .abi_encode();
                let precondition = StepPrecondition::AuthorizationUnused {
                    token,
                    authorizer: from,
                    nonce: leg.nonce,
                    search_from_block: self.search_start(config, leg).await,
                };
                (token, calldata, TRANSFER_GAS, Some(precondition), not_after)
            }
            WithdrawalStepKind::Burn => {
                let forwarder: Address = leg
                    .authorization_to
                    .parse()
                    .map_err(|_| "the leg's forwarder is malformed".to_owned())?;
                let destination: Address = withdrawal
                    .destination_address
                    .parse()
                    .map_err(|_| "the withdrawal's destination address is malformed".to_owned())?;
                let destination_domain = self
                    .registry
                    .cctp(leg.destination_chain_id)
                    .map(|cctp| cctp.domain)
                    .ok_or_else(|| {
                        format!(
                            "the leg bridges to chain {} which has no CCTP configured",
                            leg.destination_chain_id
                        )
                    })?;
                let calldata = bridgeCall {
                    token,
                    from,
                    value: leg.amount,
                    destinationDomain: destination_domain,
                    mintRecipient: destination.into_word(),
                    salt: leg
                        .salt
                        .ok_or_else(|| "the bridge leg has no salt".to_owned())?,
                    validBefore: U256::from(leg.valid_before),
                    signature: signature()?,
                }
                .abi_encode();
                let precondition = StepPrecondition::AuthorizationUnused {
                    token,
                    authorizer: from,
                    nonce: leg.nonce,
                    search_from_block: self.search_start(config, leg).await,
                };
                (
                    forwarder,
                    calldata,
                    BRIDGE_GAS,
                    Some(precondition),
                    not_after,
                )
            }
            WithdrawalStepKind::Mint => {
                let cctp = config
                    .cctp
                    .as_ref()
                    .ok_or_else(|| format!("chain {chain_id} mints but has no CCTP configured"))?;
                let source_domain = self
                    .registry
                    .cctp(leg.source_chain_id)
                    .map(|cctp| cctp.domain)
                    .ok_or_else(|| {
                        format!(
                            "the leg burned on chain {} which has no CCTP configured",
                            leg.source_chain_id
                        )
                    })?;
                let message = leg
                    .attestation_message
                    .clone()
                    .ok_or_else(|| "the attested leg has no message".to_owned())?;
                let attestation = leg
                    .attestation
                    .clone()
                    .ok_or_else(|| "the attested leg has no attestation".to_owned())?;
                let nonce = message
                    .get(MESSAGE_NONCE_OFFSET..MESSAGE_NONCE_OFFSET + 32)
                    .map(B256::from_slice)
                    .ok_or_else(|| "the attestation message is too short".to_owned())?;
                let calldata = receiveMessageCall {
                    message: message.into(),
                    attestation: attestation.into(),
                }
                .abi_encode();
                let precondition = StepPrecondition::MessageNonceUnused {
                    message_transmitter: cctp.message_transmitter,
                    source_domain,
                    nonce,
                };
                (
                    cctp.message_transmitter,
                    calldata,
                    MINT_GAS,
                    Some(precondition),
                    None,
                )
            }
        };
        Ok(WithdrawalStepCommand {
            job_id: Uuid::now_v7(),
            leg_id: leg.id,
            chain_id,
            step,
            to,
            calldata: calldata.into(),
            gas_limit,
            precondition,
            not_after,
        })
    }

    /// Where the signers may start a backward search for a transaction that
    /// consumed the leg's authorization: the block the leg was created
    /// around, minus a margin for the block-time estimate, never before the
    /// deployment. The finalized head anchors the estimate; without a chain
    /// client (or with the node down) the deployment block does, which only
    /// makes the search longer.
    async fn search_start(&self, config: &ChainConfig, leg: &DbWithdrawalLeg) -> u64 {
        let finalized = match self.chains.get(&config.chain_id) {
            Some(chain) => match chain.finalized_header().await {
                Ok(header) => header.number,
                Err(error) => {
                    warn!(chain_id = config.chain_id, error = %error, "finalized header unavailable; anchoring the authorization search at the deployment block");
                    config.start_block
                }
            },
            None => config.start_block,
        };
        let elapsed = Utc::now()
            .signed_duration_since(leg.created_at)
            .to_std()
            .unwrap_or_default();
        let blocks_since =
            u64::try_from(elapsed.as_millis() / u128::from(config.block_time_ms.max(1)) + 120)
                .unwrap_or(u64::MAX);
        finalized
            .saturating_sub(blocks_since)
            .max(config.start_block)
    }
}

/// Whether an attested message is this leg's burn. It must leave the
/// source domain this chain burns from, head for the leg's destination
/// domain, carry a nonce the transmitter could consume (`usedNonces[0]`
/// starts used, so a zero-nonce message would look already-minted without
/// ever minting), and its body must be exactly what the forwarder's burn
/// produces: the source token messenger as sender, no designated
/// destination caller, the leg's token, the withdrawal's recipient, the
/// leg's amount, the forwarder as depositor, and no fee. The body checks
/// matter because one transaction can carry several CCTP messages with
/// identical headers (a helper burning for itself and the merchant in one
/// call); only the full envelope separates this burn from one Gum could
/// never mint.
pub fn message_describes_burn(
    leg: &DbWithdrawalLeg,
    withdrawal: &DbWithdrawal,
    cctp: &CctpConfig,
    registry: &ChainRegistry,
    message: &[u8],
) -> Result<(), &'static str> {
    let destination_domain = registry
        .cctp(leg.destination_chain_id)
        .map(|cctp| cctp.domain)
        .ok_or("the destination chain has no CCTP configured")?;
    if message.len() < MIN_BURN_MESSAGE_LEN {
        return Err("the message is shorter than a CCTP V2 burn message");
    }
    let domain = |at: usize| u32::from_be_bytes(message[at..at + 4].try_into().expect("4 bytes"));
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
        return Err("the message names a designated destination caller Gum cannot satisfy");
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::address;
    use async_trait::async_trait;
    use gum_bus::{Consumer, ConsumerOptions};
    use gum_contracts::StepResult;
    use gum_core::{Currency, FinalitySource, TokenConfig};
    use gum_ledger::{AccountId, LegKind, LegState, NewWithdrawal, NewWithdrawalLeg};
    use std::sync::Mutex;

    const SOURCE: u64 = 8453;
    const DESTINATION_CHAIN: u64 = 42161;
    const WALLET: Address = address!("0x1111111111111111111111111111111111111111");
    const DESTINATION: Address = address!("0x2222222222222222222222222222222222222222");
    const FORWARDER: Address = address!("0x3333333333333333333333333333333333333333");
    const TRANSMITTER: Address = address!("0x4444444444444444444444444444444444444444");
    const MESSENGER: Address = address!("0x5555555555555555555555555555555555555555");
    const USDC: Address = address!("0x754704Bc059F8C67012fEd69BC8A327a5aafb603");
    const AMOUNT: u64 = 2_500_000;

    fn cctp(domain: u32) -> CctpConfig {
        CctpConfig {
            domain,
            token_messenger: MESSENGER,
            message_transmitter: TRANSMITTER,
            forwarder: FORWARDER,
            forwarder_code_hash: B256::repeat_byte(0xFF),
        }
    }

    fn chain(chain_id: u64, domain: u32) -> ChainConfig {
        ChainConfig {
            chain_id,
            tokens: vec![TokenConfig {
                currency: Currency::Usdc,
                address: USDC,
            }],
            factory: Address::repeat_byte(0xfa),
            batch_sweeper: Address::repeat_byte(0x55),
            factory_code_hash: B256::ZERO,
            batch_sweeper_code_hash: B256::ZERO,
            start_block: 100,
            finality_source: FinalitySource::Finalized,
            finality_confirmations: 0,
            block_time_ms: 1000,
            log_range_size: 100,
            signer_low_balance_wei: None,
            explorer_base_url: None,
            cctp: Some(cctp(domain)),
            block_gas_limit: None,
            transaction_gas_limit: None,
            sweep_batch_size: None,
        }
    }

    fn registry() -> Arc<ChainRegistry> {
        Arc::new(ChainRegistry::new(vec![chain(SOURCE, 6), chain(DESTINATION_CHAIN, 3)]).unwrap())
    }

    /// Answers by burn hash; unknown hashes are not indexed.
    #[derive(Default)]
    struct FakeIris {
        answers: Mutex<HashMap<B256, AttestationStatus>>,
        polls: Mutex<Vec<(u32, B256)>>,
    }

    impl FakeIris {
        fn answer(&self, tx_hash: B256, status: AttestationStatus) {
            self.answers.lock().unwrap().insert(tx_hash, status);
        }
    }

    #[async_trait]
    impl AttestationSource for FakeIris {
        async fn attestation(
            &self,
            source_domain: u32,
            burn_tx_hash: B256,
        ) -> Result<AttestationStatus, crate::iris::IrisError> {
            self.polls
                .lock()
                .unwrap()
                .push((source_domain, burn_tx_hash));
            Ok(self
                .answers
                .lock()
                .unwrap()
                .get(&burn_tx_hash)
                .cloned()
                .unwrap_or(AttestationStatus::NotIndexed))
        }
    }

    struct Harness {
        orchestrator: WithdrawalOrchestrator,
        repo: WithdrawalRepository,
        iris: Arc<FakeIris>,
        signers: Consumer,
    }

    impl Harness {
        fn new(pool: &PgPool) -> Self {
            Self::with_registry(pool, registry())
        }

        fn with_registry(pool: &PgPool, registry: Arc<ChainRegistry>) -> Self {
            let iris = Arc::new(FakeIris::default());
            let repo = WithdrawalRepository::new(pool.clone());
            let orchestrator = WithdrawalOrchestrator::new(
                repo.clone(),
                pool.clone(),
                registry,
                Publisher::new("gum-server"),
                iris.clone(),
                StepPolicy::default(),
                HashMap::new(),
            );
            Self {
                orchestrator,
                repo,
                iris,
                signers: Consumer::new("gum-signers", pool.clone(), ConsumerOptions::default()),
            }
        }

        /// Commands published for the signers since the last call.
        async fn commands(&self) -> Vec<WithdrawalStepCommand> {
            let deliveries = self.signers.claim::<ExecutionCommand>().await.unwrap();
            let mut commands = Vec::new();
            for delivery in deliveries {
                let mut tx = self.signers.pool().begin().await.unwrap();
                self.signers.ack(&mut tx, &delivery).await.unwrap();
                tx.commit().await.unwrap();
                match delivery.message {
                    ExecutionCommand::WithdrawalStep(command) => commands.push(command),
                    other => panic!("unexpected command {other:?}"),
                }
            }
            commands
        }

        async fn leg(&self, withdrawal: Uuid) -> DbWithdrawalLeg {
            self.repo.legs(withdrawal).await.unwrap().remove(0)
        }

        /// The signers report the step `job_id` runs as done.
        async fn step_succeeded(&self, job_id: Uuid, step: WithdrawalStepKind, tx_hash: B256) {
            let mut tx = self.signers.pool().begin().await.unwrap();
            self.repo
                .apply_step_result(
                    &mut tx,
                    job_id,
                    step,
                    &StepResult::Succeeded {
                        tx_hash,
                        block: 200,
                        block_hash: B256::repeat_byte(0xb0),
                    },
                    &StepPolicy::default(),
                )
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }
    }

    async fn account(pool: &PgPool) -> AccountId {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO accounts (id, api_key_hash, api_key_hint) VALUES ($1, $2, 'hint')",
        )
        .bind(id)
        .bind(id.as_bytes().repeat(2))
        .execute(pool)
        .await
        .unwrap();
        AccountId(id)
    }

    fn far_future() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 86_400
    }

    /// A signed, authorized withdrawal with one leg. Returns the
    /// withdrawal id.
    async fn authorized_leg(
        pool: &PgPool,
        repo: &WithdrawalRepository,
        kind: LegKind,
        valid_before: u64,
    ) -> Uuid {
        let account = account(pool).await;
        let leg = NewWithdrawalLeg {
            id: Uuid::now_v7(),
            position: 0,
            kind,
            source_chain_id: SOURCE,
            amount: U256::from(AMOUNT),
            token_address: USDC.to_checksum(None),
            domain_name: "USDC".into(),
            domain_version: "2".into(),
            authorization_to: match kind {
                LegKind::Transfer => DESTINATION.to_checksum(None),
                LegKind::Bridge => FORWARDER.to_checksum(None),
            },
            nonce: B256::repeat_byte(0x42),
            salt: matches!(kind, LegKind::Bridge).then(|| B256::repeat_byte(0x55)),
            valid_before,
        };
        let leg_id = leg.id;
        let input = NewWithdrawal {
            id: Uuid::now_v7(),
            account_id: account,
            idempotency_key: "k".into(),
            wallet_address: WALLET.to_checksum(None),
            currency: "USDC".into(),
            destination_chain_id: match kind {
                LegKind::Transfer => SOURCE,
                LegKind::Bridge => DESTINATION_CHAIN,
            },
            destination_address: DESTINATION.to_checksum(None),
            legs: vec![leg],
        };
        repo.create(&input).await.unwrap();
        repo.authorize(account, input.id, leg_id, &[0x11u8; 65])
            .await
            .unwrap();
        input.id
    }

    /// A complete CCTP V2 burn message exactly as the forwarder produces
    /// it for the test leg.
    fn burn_message(source_domain: u32, destination_domain: u32, nonce: B256) -> Vec<u8> {
        let mut message = vec![0u8; MIN_BURN_MESSAGE_LEN];
        message[4..8].copy_from_slice(&source_domain.to_be_bytes());
        message[8..12].copy_from_slice(&destination_domain.to_be_bytes());
        message[MESSAGE_NONCE_OFFSET..MESSAGE_NONCE_OFFSET + 32].copy_from_slice(nonce.as_slice());
        message[MESSAGE_SENDER_OFFSET..MESSAGE_SENDER_OFFSET + 32]
            .copy_from_slice(MESSENGER.into_word().as_slice());
        message[BURN_TOKEN_OFFSET..BURN_TOKEN_OFFSET + 32]
            .copy_from_slice(USDC.into_word().as_slice());
        message[MINT_RECIPIENT_OFFSET..MINT_RECIPIENT_OFFSET + 32]
            .copy_from_slice(DESTINATION.into_word().as_slice());
        message[AMOUNT_OFFSET..AMOUNT_OFFSET + 32]
            .copy_from_slice(&U256::from(AMOUNT).to_be_bytes::<32>());
        message[BODY_MESSAGE_SENDER_OFFSET..BODY_MESSAGE_SENDER_OFFSET + 32]
            .copy_from_slice(FORWARDER.into_word().as_slice());
        message
    }

    fn complete(messages: Vec<Vec<u8>>) -> AttestationStatus {
        AttestationStatus::Complete {
            messages: messages
                .into_iter()
                .map(|message| (message.into(), vec![0xAA; 65].into()))
                .collect(),
        }
    }

    /// Burn the bridge leg: open the step, let the signers report success.
    async fn burned_leg(pool: &PgPool, h: &Harness) -> (Uuid, B256) {
        let withdrawal = authorized_leg(pool, &h.repo, LegKind::Bridge, far_future()).await;
        h.orchestrator.pass(SOURCE).await.unwrap();
        let command = h.commands().await.remove(0);
        assert_eq!(command.step, WithdrawalStepKind::Burn);
        let burn_hash = B256::repeat_byte(0xb1);
        h.step_succeeded(command.job_id, WithdrawalStepKind::Burn, burn_hash)
            .await;
        assert_eq!(h.leg(withdrawal).await.state, LegState::Burned);
        (withdrawal, burn_hash)
    }

    #[sqlx::test(migrator = "gum_schema::MIGRATOR")]
    async fn a_transfer_leg_becomes_one_transfer_step_command(pool: PgPool) {
        let h = Harness::new(&pool);
        let withdrawal = authorized_leg(&pool, &h.repo, LegKind::Transfer, far_future()).await;

        let report = h.orchestrator.pass(SOURCE).await.unwrap();
        assert_eq!(report.steps_opened, 1);
        let commands = h.commands().await;
        assert_eq!(commands.len(), 1);
        let command = &commands[0];
        assert_eq!(command.step, WithdrawalStepKind::Transfer);
        assert_eq!(command.chain_id, SOURCE);
        assert_eq!(command.to, USDC);
        assert_eq!(command.gas_limit, TRANSFER_GAS);
        let call = transferWithAuthorizationCall::abi_decode(&command.calldata).unwrap();
        assert_eq!(call.from, WALLET);
        assert_eq!(call.to, DESTINATION);
        assert_eq!(call.value, U256::from(AMOUNT));
        assert_eq!(call.validAfter, U256::ZERO);
        assert_eq!(call.nonce, B256::repeat_byte(0x42));
        assert_eq!(call.signature.as_ref(), &[0x11u8; 65]);
        assert_eq!(
            command.precondition,
            Some(StepPrecondition::AuthorizationUnused {
                token: USDC,
                authorizer: WALLET,
                nonce: B256::repeat_byte(0x42),
                search_from_block: 100,
            }),
            "without a chain client the search anchors at the deployment block"
        );
        let leg = h.leg(withdrawal).await;
        assert_eq!(leg.state, LegState::Relaying);
        assert_eq!(leg.step.map(|step| step.job_id), Some(command.job_id));
        assert_eq!(command.not_after, Some(leg.valid_before - NOT_AFTER_MARGIN));

        // The step is in flight: another pass builds nothing for it.
        let report = h.orchestrator.pass(SOURCE).await.unwrap();
        assert_eq!(report.steps_opened, 0);
        assert!(h.commands().await.is_empty());

        // The signers conclude; the leg completes and stays quiet.
        h.step_succeeded(
            command.job_id,
            WithdrawalStepKind::Transfer,
            B256::repeat_byte(0x71),
        )
        .await;
        let leg = h.leg(withdrawal).await;
        assert_eq!(leg.state, LegState::Completed);
        assert!(leg.step.is_none());
        h.orchestrator.pass(SOURCE).await.unwrap();
        assert!(h.commands().await.is_empty());
    }

    #[sqlx::test(migrator = "gum_schema::MIGRATOR")]
    async fn a_bridge_leg_burns_waits_for_circle_and_mints_on_the_other_chain(pool: PgPool) {
        let h = Harness::new(&pool);
        let (withdrawal, burn_hash) = burned_leg(&pool, &h).await;
        let leg = h.leg(withdrawal).await;
        assert!(
            leg.attestation_next_check_at.unwrap() > Utc::now(),
            "a fresh burn is not polled before Circle could have seen it"
        );
        let due = || Utc::now() - chrono::Duration::seconds(1);

        // Not indexed yet: the leg waits and the poll is deferred.
        h.repo.defer_attestation(leg.id, due()).await.unwrap();
        let report = h.orchestrator.pass(SOURCE).await.unwrap();
        assert_eq!((report.attested, report.deferred), (0, 1));
        assert_eq!(
            h.iris.polls.lock().unwrap().as_slice(),
            &[(6, burn_hash)],
            "polled once, with the source domain"
        );
        let leg = h.leg(withdrawal).await;
        assert_eq!(leg.state, LegState::Burned);
        assert!(leg.attestation_next_check_at.unwrap() > Utc::now());

        // Deferred: the next pass does not poll again before the backoff.
        h.orchestrator.pass(SOURCE).await.unwrap();
        assert_eq!(h.iris.polls.lock().unwrap().len(), 1);

        // Once the attestation is due and complete, the leg is attested.
        let message = burn_message(6, 3, B256::repeat_byte(0x77));
        h.iris.answer(burn_hash, complete(vec![message.clone()]));
        h.repo.defer_attestation(leg.id, due()).await.unwrap();
        let report = h.orchestrator.pass(SOURCE).await.unwrap();
        assert_eq!(report.attested, 1);
        let leg = h.leg(withdrawal).await;
        assert_eq!(leg.state, LegState::Attested);
        assert_eq!(leg.attestation_message.as_deref(), Some(message.as_slice()));
        assert!(
            h.commands().await.is_empty(),
            "the mint belongs to the destination chain's pass"
        );

        // The destination chain's pass opens the mint.
        let report = h.orchestrator.pass(DESTINATION_CHAIN).await.unwrap();
        assert_eq!(report.steps_opened, 1);
        let command = h.commands().await.remove(0);
        assert_eq!(command.step, WithdrawalStepKind::Mint);
        assert_eq!(command.chain_id, DESTINATION_CHAIN);
        assert_eq!(command.to, TRANSMITTER);
        assert_eq!(command.gas_limit, MINT_GAS);
        assert_eq!(command.not_after, None);
        let call = receiveMessageCall::abi_decode(&command.calldata).unwrap();
        assert_eq!(call.message.as_ref(), message.as_slice());
        assert_eq!(call.attestation.as_ref(), &[0xAA; 65]);
        assert_eq!(
            command.precondition,
            Some(StepPrecondition::MessageNonceUnused {
                message_transmitter: TRANSMITTER,
                source_domain: 6,
                nonce: B256::repeat_byte(0x77),
            })
        );
        assert_eq!(h.leg(withdrawal).await.state, LegState::Minting);

        h.step_succeeded(
            command.job_id,
            WithdrawalStepKind::Mint,
            B256::repeat_byte(0xa1),
        )
        .await;
        let leg = h.leg(withdrawal).await;
        assert_eq!(leg.state, LegState::Completed);
        assert_eq!(leg.burn_tx_hash, Some(burn_hash));
        assert_eq!(leg.mint_tx_hash, Some(B256::repeat_byte(0xa1)));
        assert!(
            h.repo
                .by_id(withdrawal)
                .await
                .unwrap()
                .unwrap()
                .completed_at
                .is_some()
        );
    }

    #[sqlx::test(migrator = "gum_schema::MIGRATOR")]
    async fn only_the_message_describing_this_burn_is_recorded(pool: PgPool) {
        // One transaction can carry several CCTP messages, and a helper's
        // burn may share this leg's domains and header shape. Only the
        // message whose body is this leg's (token, recipient, amount) may
        // be recorded; a foreign-only answer leaves the leg polling.
        let ours = burn_message(6, 3, B256::repeat_byte(0x77));
        let mut foreign = ours.clone();
        foreign[MINT_RECIPIENT_OFFSET..MINT_RECIPIENT_OFFSET + 32]
            .copy_from_slice(B256::repeat_byte(0x99).as_slice());

        let h = Harness::new(&pool);
        let (withdrawal, burn_hash) = burned_leg(&pool, &h).await;
        let leg_id = h.leg(withdrawal).await.id;
        let due = || Utc::now() - chrono::Duration::seconds(1);

        h.iris.answer(burn_hash, complete(vec![foreign.clone()]));
        h.repo.defer_attestation(leg_id, due()).await.unwrap();
        let report = h.orchestrator.pass(SOURCE).await.unwrap();
        assert_eq!((report.attested, report.deferred), (0, 1));
        let leg = h.leg(withdrawal).await;
        assert_eq!(leg.state, LegState::Burned, "a foreign answer is refused");
        assert_eq!(leg.attestation_message, None);

        h.iris
            .answer(burn_hash, complete(vec![foreign, ours.clone()]));
        h.repo.defer_attestation(leg_id, due()).await.unwrap();
        let report = h.orchestrator.pass(SOURCE).await.unwrap();
        assert_eq!(report.attested, 1);
        let leg = h.leg(withdrawal).await;
        assert_eq!(leg.state, LegState::Attested);
        assert_eq!(leg.attestation_message.as_deref(), Some(ours.as_slice()));
    }

    #[test]
    fn a_burn_message_is_pinned_field_by_field() {
        let registry = registry();
        let cctp = cctp(6);
        let now = Utc::now();
        let leg = DbWithdrawalLeg {
            id: Uuid::nil(),
            withdrawal_id: Uuid::nil(),
            position: 0,
            kind: LegKind::Bridge,
            source_chain_id: SOURCE,
            destination_chain_id: DESTINATION_CHAIN,
            amount: U256::from(AMOUNT),
            state: LegState::Burned,
            token_address: USDC.to_checksum(None),
            domain_name: "USDC".into(),
            domain_version: "2".into(),
            authorization_to: FORWARDER.to_checksum(None),
            nonce: B256::repeat_byte(0x42),
            salt: Some(B256::repeat_byte(0x55)),
            valid_before: far_future(),
            signature: Some(vec![0x11; 65]),
            authorized_at: Some(now),
            step: None,
            step_reverts: 0,
            step_retry_at: None,
            transfer_tx_hash: None,
            burn_tx_hash: Some(B256::repeat_byte(0xb1)),
            mint_tx_hash: None,
            attestation_message: None,
            attestation: None,
            attestation_next_check_at: None,
            failure_reason: None,
            created_at: now,
            updated_at: now,
        };
        let withdrawal = DbWithdrawal {
            id: Uuid::nil(),
            account_id: Uuid::nil(),
            idempotency_key: "k".into(),
            wallet_address: WALLET.to_checksum(None),
            currency: "USDC".into(),
            destination_chain_id: DESTINATION_CHAIN as i64,
            destination_address: DESTINATION.to_checksum(None),
            created_at: now,
            completed_at: None,
            cancelled_at: None,
            failed_at: None,
        };
        let check =
            |message: &[u8]| message_describes_burn(&leg, &withdrawal, &cctp, &registry, message);
        let ours = burn_message(6, 3, B256::repeat_byte(0x77));
        assert_eq!(check(&ours), Ok(()));

        let mutate = |at: usize, byte: u8| {
            let mut message = ours.clone();
            message[at] = byte;
            message
        };
        assert!(
            check(&ours[..MIN_BURN_MESSAGE_LEN - 1]).is_err(),
            "too short"
        );
        assert!(
            check(&burn_message(3, 3, B256::repeat_byte(0x77))).is_err(),
            "source domain"
        );
        assert!(
            check(&burn_message(6, 6, B256::repeat_byte(0x77))).is_err(),
            "destination domain"
        );
        assert!(
            check(&burn_message(6, 3, B256::ZERO)).is_err(),
            "zero nonce"
        );
        assert!(
            check(&mutate(MESSAGE_SENDER_OFFSET + 31, 0x01)).is_err(),
            "sender"
        );
        assert!(
            check(&mutate(DESTINATION_CALLER_OFFSET + 31, 0x01)).is_err(),
            "destination caller"
        );
        assert!(
            check(&mutate(BURN_TOKEN_OFFSET + 31, 0x01)).is_err(),
            "token"
        );
        assert!(
            check(&mutate(MINT_RECIPIENT_OFFSET + 31, 0x01)).is_err(),
            "recipient"
        );
        assert!(check(&mutate(AMOUNT_OFFSET + 31, 0x01)).is_err(), "amount");
        assert!(
            check(&mutate(BODY_MESSAGE_SENDER_OFFSET + 31, 0x01)).is_err(),
            "depositor"
        );
        assert!(check(&mutate(MAX_FEE_OFFSET + 31, 0x01)).is_err(), "fee");
    }

    #[sqlx::test(migrator = "gum_schema::MIGRATOR")]
    async fn an_authorization_about_to_lapse_is_expired_instead_of_relayed(pool: PgPool) {
        let h = Harness::new(&pool);
        let soon = far_future() - 86_400 + MIN_AUTHORIZATION_VALIDITY.as_secs() / 2;
        let withdrawal = authorized_leg(&pool, &h.repo, LegKind::Transfer, soon).await;

        let report = h.orchestrator.pass(SOURCE).await.unwrap();
        assert_eq!(report.steps_opened, 0, "not worth the gas");
        assert!(h.commands().await.is_empty());
        assert_eq!(
            h.repo
                .expire_stale(MIN_AUTHORIZATION_VALIDITY)
                .await
                .unwrap(),
            1
        );
        assert_eq!(h.leg(withdrawal).await.state, LegState::Expired);
    }

    #[sqlx::test(migrator = "gum_schema::MIGRATOR")]
    async fn a_cancelled_withdrawal_is_not_relayed(pool: PgPool) {
        let h = Harness::new(&pool);
        let withdrawal = authorized_leg(&pool, &h.repo, LegKind::Transfer, far_future()).await;
        let account = AccountId(h.repo.by_id(withdrawal).await.unwrap().unwrap().account_id);
        h.repo.cancel(account, withdrawal).await.unwrap();

        let report = h.orchestrator.pass(SOURCE).await.unwrap();
        assert_eq!(report.steps_opened, 0);
        assert!(h.commands().await.is_empty());
        assert_eq!(h.leg(withdrawal).await.state, LegState::Cancelled);
    }

    #[sqlx::test(migrator = "gum_schema::MIGRATOR")]
    async fn a_leg_whose_authorization_cannot_be_executed_fails_with_the_reason(pool: PgPool) {
        // The schema pins every stored shape (addresses, the bridge salt),
        // so the reachable case is a configuration that no longer matches
        // the data: the destination chain lost its CCTP configuration
        // after the leg was created. No later pass builds it differently.
        let mut destination = chain(DESTINATION_CHAIN, 3);
        destination.cctp = None;
        let registry = Arc::new(ChainRegistry::new(vec![chain(SOURCE, 6), destination]).unwrap());
        let h = Harness::with_registry(&pool, registry);
        let withdrawal = authorized_leg(&pool, &h.repo, LegKind::Bridge, far_future()).await;

        let report = h.orchestrator.pass(SOURCE).await.unwrap();
        assert_eq!((report.steps_opened, report.failed), (0, 1));
        assert!(h.commands().await.is_empty(), "nothing reaches the signers");
        let leg = h.leg(withdrawal).await;
        assert_eq!(leg.state, LegState::Failed);
        assert_eq!(
            leg.failure_reason.as_deref(),
            Some("the leg bridges to chain 42161 which has no CCTP configured")
        );
        assert!(
            h.repo
                .by_id(withdrawal)
                .await
                .unwrap()
                .unwrap()
                .failed_at
                .is_some(),
            "the withdrawal settles as failed"
        );
    }
}
