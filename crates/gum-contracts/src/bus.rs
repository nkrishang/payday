//! Durable asynchronous messages.
//!
//! Topics are static: the set of producers and consumers is the three
//! services, so the routing table lives in the schema (`bus.subscriptions`)
//! rather than in configuration. Each topic carries one Rust enum; the
//! variant name is the message `kind` stored alongside the payload so
//! operators can query the bus without decoding JSON.
//!
//! Delivery is at least once. Every consumer must be idempotent on the
//! message's deduplication key, which producers choose so that a retry of
//! the same intent produces the same key:
//!
//! * `ExecutionCommand`: the job id (one job, one command, forever).
//! * `ExecutionEvent`: `{job_id}:{event kind}[:{replacement}]`.
//! * `ChainControl`: `{chain_id}:{halted|resumed}:{fault id}`.

use alloy_primitives::{Address, B256, Bytes, U256};
use gum_core::Currency;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Commands the server publishes for `gum-signers`.
pub const EXECUTION_COMMANDS_TOPIC: &str = "execution.commands";
/// Facts `gum-signers` publishes about executed commands, for the server.
pub const EXECUTION_EVENTS_TOPIC: &str = "execution.events";
/// Chain safety notices the server publishes for `gum-signers`.
pub const CHAIN_CONTROL_TOPIC: &str = "chain.control";

/// A message type that belongs to exactly one topic.
pub trait BusMessage: Serialize + DeserializeOwned + Send + Sync + 'static {
    const TOPIC: &'static str;
    /// Schema version of this topic's payloads. Bump when a variant changes
    /// incompatibly; consumers refuse versions above the one they compile.
    const SCHEMA_VERSION: u16;
    /// The variant name, stored next to the payload for querying and logging.
    fn kind(&self) -> &'static str;
    /// The producer-chosen idempotency key (unique within the topic).
    fn deduplication_key(&self) -> String;
}

// ---------------------------------------------------------------------------
// Commands: server -> signers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExecutionCommand {
    /// Deploy/drain a batch of payment addresses through `BatchSweeper`.
    SweepBatch(SweepBatchCommand),
    /// One transaction of a withdrawal leg: the EIP-3009 transfer, the CCTP
    /// burn, or the CCTP mint.
    WithdrawalStep(WithdrawalStepCommand),
    /// The onboarding walkthrough's one USDC payment, from its dedicated payer.
    OnboardingPayment(OnboardingPaymentCommand),
}

impl ExecutionCommand {
    pub fn job_id(&self) -> Uuid {
        match self {
            Self::SweepBatch(command) => command.job_id,
            Self::WithdrawalStep(command) => command.job_id,
            Self::OnboardingPayment(command) => command.job_id,
        }
    }

    pub fn chain_id(&self) -> u64 {
        match self {
            Self::SweepBatch(command) => command.chain_id,
            Self::WithdrawalStep(command) => command.chain_id,
            Self::OnboardingPayment(command) => command.chain_id,
        }
    }
}

impl BusMessage for ExecutionCommand {
    const TOPIC: &'static str = EXECUTION_COMMANDS_TOPIC;
    const SCHEMA_VERSION: u16 = 1;

    fn kind(&self) -> &'static str {
        match self {
            Self::SweepBatch(_) => "sweep_batch",
            Self::WithdrawalStep(_) => "withdrawal_step",
            Self::OnboardingPayment(_) => "onboarding_payment",
        }
    }

    fn deduplication_key(&self) -> String {
        self.job_id().to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnboardingPaymentCommand {
    pub job_id: Uuid,
    pub chain_id: u64,
    /// The only signer permitted to execute this command.
    pub payer: Address,
    pub token: Address,
    pub recipient: Address,
    pub amount: U256,
}

/// Everything the signers need to build, submit and *classify* one batch.
/// The item context is a snapshot taken when the job was scheduled; the
/// server treats the resulting evidence against its current state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SweepBatchCommand {
    pub job_id: Uuid,
    pub chain_id: u64,
    pub items: Vec<SweepItem>,
}

/// Whether the deposit request was still open when the job was scheduled.
/// It decides how a `Collected` receipt (the contract already existed) is
/// investigated: an open request needs its third-party settlement found,
/// a closed one already has it ledgered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SweepItemStatus {
    Open,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SweepItem {
    pub invoice_id: Uuid,
    pub payment_address: Address,
    pub currency: Currency,
    pub token: Address,
    pub amount: U256,
    pub receiver: Address,
    pub expiration_timestamp: u64,
    pub recovery: Address,
    pub salt: B256,
    pub status: SweepItemStatus,
    /// The ledger already holds a settlement transaction for this request.
    pub settlement_recorded: bool,
    /// Earliest block a payment to this address was observed at, the lower
    /// bound of any settlement-log search. `None` when the ledger holds no
    /// observation; the chain's configured start block applies.
    pub first_observed_block: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WithdrawalStepKind {
    /// `transferWithAuthorization` on the destination chain.
    Transfer,
    /// `WithdrawalForwarder.bridge` on the source chain (CCTP burn).
    Burn,
    /// `MessageTransmitter.receiveMessage` on the destination chain.
    Mint,
}

/// A fact whose truth on chain means the step already happened (somebody
/// else executed it, or a previous run of ours did before crashing). The
/// signers check it before submitting and again after a revert.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StepPrecondition {
    /// The EIP-3009 authorization is still unused. When it is found used,
    /// the transaction that consumed it is searched from `search_from_block`
    /// so the server learns the burn/transfer hash.
    AuthorizationUnused {
        token: Address,
        authorizer: Address,
        nonce: B256,
        search_from_block: u64,
    },
    /// The CCTP message nonce has not been received on the destination
    /// `MessageTransmitter`. A used nonce means the mint already landed.
    MessageNonceUnused {
        message_transmitter: Address,
        source_domain: u32,
        nonce: B256,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WithdrawalStepCommand {
    pub job_id: Uuid,
    pub leg_id: Uuid,
    pub chain_id: u64,
    pub step: WithdrawalStepKind,
    pub to: Address,
    pub calldata: Bytes,
    pub gas_limit: u64,
    pub precondition: Option<StepPrecondition>,
    /// Unix time after which the step must not be submitted (the
    /// authorization's `validBefore` minus a safety margin). `None` for
    /// steps without a deadline (the mint).
    pub not_after: Option<u64>,
}

// ---------------------------------------------------------------------------
// Events: signers -> server
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExecutionEvent {
    /// The durable onboarding transaction was accepted by a node.
    OnboardingPaymentSubmitted {
        job_id: Uuid,
        chain_id: u64,
        tx_hash: B256,
    },
    /// A helper transaction for the job left the node. Informational: the
    /// server records the hash for the dashboard, nothing else depends on it.
    SweepSubmitted {
        job_id: Uuid,
        chain_id: u64,
        signer: Address,
        nonce: u64,
        tx_hash: B256,
        /// 0 for the first submission, incremented per same-nonce replacement.
        replacement: u32,
    },
    /// The batch mined, reached finality and every item's receipt was
    /// classified. The server applies the lifecycle transitions.
    SweepFinalized {
        job_id: Uuid,
        chain_id: u64,
        tx_hash: B256,
        block: u64,
        block_hash: B256,
        block_timestamp: u64,
        transaction_index: u64,
        items: Vec<SweepItemResult>,
    },
    /// The job did not produce a finalized successful receipt and will not
    /// be retried by the signers; the server returns its items to the queue.
    SweepAbandoned {
        job_id: Uuid,
        chain_id: u64,
        reason: AbandonReason,
    },
    /// A withdrawal step reached a conclusion.
    WithdrawalStepFinalized {
        job_id: Uuid,
        leg_id: Uuid,
        chain_id: u64,
        step: WithdrawalStepKind,
        result: StepResult,
    },
    /// A job's signer lane has replaced its transaction more times than the
    /// configured limit without mining. Work continues; this is an alert.
    ExecutionStalled {
        job_id: Uuid,
        chain_id: u64,
        signer: Address,
        submissions: u32,
    },
    /// The command could not be executed and never will be: malformed,
    /// unknown chain, or rejected by policy. Terminal for the job.
    ExecutionRejected {
        job_id: Uuid,
        chain_id: u64,
        reason: String,
    },
}

impl ExecutionEvent {
    pub fn job_id(&self) -> Uuid {
        match self {
            Self::OnboardingPaymentSubmitted { job_id, .. }
            | Self::SweepSubmitted { job_id, .. }
            | Self::SweepFinalized { job_id, .. }
            | Self::SweepAbandoned { job_id, .. }
            | Self::WithdrawalStepFinalized { job_id, .. }
            | Self::ExecutionStalled { job_id, .. }
            | Self::ExecutionRejected { job_id, .. } => *job_id,
        }
    }

    pub fn chain_id(&self) -> u64 {
        match self {
            Self::OnboardingPaymentSubmitted { chain_id, .. }
            | Self::SweepSubmitted { chain_id, .. }
            | Self::SweepFinalized { chain_id, .. }
            | Self::SweepAbandoned { chain_id, .. }
            | Self::WithdrawalStepFinalized { chain_id, .. }
            | Self::ExecutionStalled { chain_id, .. }
            | Self::ExecutionRejected { chain_id, .. } => *chain_id,
        }
    }
}

impl BusMessage for ExecutionEvent {
    const TOPIC: &'static str = EXECUTION_EVENTS_TOPIC;
    const SCHEMA_VERSION: u16 = 1;

    fn kind(&self) -> &'static str {
        match self {
            Self::OnboardingPaymentSubmitted { .. } => "onboarding_payment_submitted",
            Self::SweepSubmitted { .. } => "sweep_submitted",
            Self::SweepFinalized { .. } => "sweep_finalized",
            Self::SweepAbandoned { .. } => "sweep_abandoned",
            Self::WithdrawalStepFinalized { .. } => "withdrawal_step_finalized",
            Self::ExecutionStalled { .. } => "execution_stalled",
            Self::ExecutionRejected { .. } => "execution_rejected",
        }
    }

    fn deduplication_key(&self) -> String {
        match self {
            Self::SweepSubmitted {
                job_id,
                replacement,
                ..
            } => format!("{job_id}:sweep_submitted:{replacement}"),
            Self::ExecutionStalled {
                job_id,
                submissions,
                ..
            } => format!("{job_id}:execution_stalled:{submissions}"),
            other => format!("{}:{}", other.job_id(), other.kind()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SweepItemResult {
    pub invoice_id: Uuid,
    pub outcome: SweepItemOutcome,
}

/// What the finalized receipt proved about one payment address. Evidence,
/// not policy: the server decides what each outcome does to the request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SweepItemOutcome {
    /// `Payment` was deployed and paid the receiver the invoice amount; any
    /// overpayment went to the recovery wallet in the same transaction.
    Settled { overpayment_recovered: U256 },
    /// `Payment` was deployed after expiry and its whole balance went to the
    /// recovery wallet.
    Returned { amount: U256 },
    /// `Payment` already existed; `recover` forwarded `amount` (possibly
    /// zero). `settlement` describes the deployment somebody else made,
    /// when the item was open and had no recorded settlement; `None` when
    /// the ledger already held one.
    LateCollected {
        amount: U256,
        settlement: Option<SettlementEvidence>,
    },
    /// The item's call reverted. The cause is what pinned reads at the
    /// receipt block could establish.
    Failed { cause: SweepFailureCause },
}

/// A third party's deployment of a `Payment`, with what its constructor
/// routed: `settled` is the amount paid to the receiver (`None` when the
/// deployment was after expiry), `recovered` what went to the recovery wallet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettlementEvidence {
    pub tx_hash: B256,
    pub block: u64,
    pub transaction_index: u64,
    pub block_timestamp: u64,
    pub settled: Option<U256>,
    pub recovered: U256,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SweepFailureCause {
    /// The token contract is paused; transient.
    TokenPaused,
    PaymentAddressBlacklisted,
    RecoveryBlacklisted,
    BeneficiaryBlacklisted,
    /// The address holds less than the invoice amount (and the request is
    /// still live, so the contract refused to deploy).
    BalanceBelowAmount,
    /// Nothing the probes read explains the revert.
    Unknown,
}

impl SweepFailureCause {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TokenPaused => "token_paused",
            Self::PaymentAddressBlacklisted => "payment_address_blacklisted",
            Self::RecoveryBlacklisted => "recovery_blacklisted",
            Self::BeneficiaryBlacklisted => "beneficiary_blacklisted",
            Self::BalanceBelowAmount => "balance_below_amount",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AbandonReason {
    /// The finalized helper transaction reverted as a whole.
    Reverted { tx_hash: B256 },
    /// The signer's nonce was consumed by a transaction that is none of the
    /// job's submissions; the job's own transactions can never mine.
    NonceConsumed { signer: Address, nonce: u64 },
    /// Signing or building the transaction failed permanently.
    Rejected { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StepResult {
    /// Our transaction mined, succeeded and is final.
    Succeeded {
        tx_hash: B256,
        block: u64,
        block_hash: B256,
    },
    /// The precondition proved somebody else (or a lost previous run) did
    /// the step. `tx_hash` is the transaction found consuming the
    /// authorization, when the search located it.
    ExecutedElsewhere { tx_hash: Option<B256> },
    /// Our transaction mined and reverted, and the precondition still holds.
    /// The server decides whether to schedule another attempt.
    Reverted { tx_hash: B256 },
    /// `not_after` passed before the step was submitted or mined.
    Expired,
    /// The transaction was never going to mine (nonce consumed elsewhere,
    /// signing rejected); nothing on chain changed as far as we can tell.
    Abandoned { reason: AbandonReason },
}

// ---------------------------------------------------------------------------
// Chain control: server -> signers
// ---------------------------------------------------------------------------

/// The indexer reports a finality violation to the server, which stops
/// scheduling work on the chain and tells the signers to stop signing new
/// transactions for it. In-flight transactions keep being reconciled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChainControl {
    Halted {
        fault_id: Uuid,
        chain_id: u64,
        reason: String,
    },
    Resumed {
        fault_id: Uuid,
        chain_id: u64,
    },
}

impl BusMessage for ChainControl {
    const TOPIC: &'static str = CHAIN_CONTROL_TOPIC;
    const SCHEMA_VERSION: u16 = 1;

    fn kind(&self) -> &'static str {
        match self {
            Self::Halted { .. } => "halted",
            Self::Resumed { .. } => "resumed",
        }
    }

    fn deduplication_key(&self) -> String {
        match self {
            Self::Halted {
                fault_id, chain_id, ..
            } => format!("{chain_id}:halted:{fault_id}"),
            Self::Resumed { fault_id, chain_id } => format!("{chain_id}:resumed:{fault_id}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_round_trip_and_carry_their_kind() {
        let command = ExecutionCommand::SweepBatch(SweepBatchCommand {
            job_id: Uuid::nil(),
            chain_id: 143,
            items: vec![SweepItem {
                invoice_id: Uuid::nil(),
                payment_address: Address::repeat_byte(1),
                currency: Currency::Usdc,
                token: Address::repeat_byte(2),
                amount: U256::from(1_000_000u64),
                receiver: Address::repeat_byte(3),
                expiration_timestamp: 1_800_000_000,
                recovery: Address::repeat_byte(4),
                salt: B256::repeat_byte(5),
                status: SweepItemStatus::Open,
                settlement_recorded: false,
                first_observed_block: Some(10),
            }],
        });
        let json = serde_json::to_value(&command).unwrap();
        assert_eq!(json["kind"], "sweep_batch");
        assert_eq!(command.kind(), "sweep_batch");
        assert_eq!(command.deduplication_key(), Uuid::nil().to_string());
        let decoded: ExecutionCommand = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, command);
    }

    #[test]
    fn replacement_submissions_have_distinct_keys() {
        let first = ExecutionEvent::SweepSubmitted {
            job_id: Uuid::nil(),
            chain_id: 1,
            signer: Address::ZERO,
            nonce: 7,
            tx_hash: B256::ZERO,
            replacement: 0,
        };
        let second = ExecutionEvent::SweepSubmitted {
            job_id: Uuid::nil(),
            chain_id: 1,
            signer: Address::ZERO,
            nonce: 7,
            tx_hash: B256::repeat_byte(1),
            replacement: 1,
        };
        assert_ne!(first.deduplication_key(), second.deduplication_key());
        let finalized = ExecutionEvent::SweepFinalized {
            job_id: Uuid::nil(),
            chain_id: 1,
            tx_hash: B256::ZERO,
            block: 1,
            block_hash: B256::ZERO,
            block_timestamp: 0,
            transaction_index: 0,
            items: vec![],
        };
        assert_eq!(
            finalized.deduplication_key(),
            format!("{}:sweep_finalized", Uuid::nil())
        );
    }
}
