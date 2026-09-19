//! Merchant- and payer-facing HTTP types. The API speaks of deposit requests
//! and deposits; the internal invoice terminology stops at this seam.

use alloy_primitives::utils::format_units;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    AttachmentId, Currency, CustomerId, Invoice, InvoiceStatus, IssuerId, NetworkTerms, Party,
    PayerVerification, chain_name, native_symbol,
};

/// The only attachment type Gum accepts (product plan §4.2).
pub const PDF_MIME_TYPE: &str = "application/pdf";

/// Every timestamp the API emits, in one shape: RFC 3339, UTC, whole
/// seconds, `Z` suffix (`2026-09-06T12:00:00Z`). The signed artifacts (the
/// wallet binding, the Proof of Payment) already use it, so nothing on the
/// wire needs a second parser.
pub fn rfc3339(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// A request names a currency (USDC unless told otherwise) and the amount
/// of it that settles at `payout_address`. For USDC the payer chooses the
/// network they pay on among the deployment's supported ones, unless the
/// merchant pins one with `chain_id`; a currency without a 1:1 bridge for
/// the merchant (USDT) must be pinned, so the merchant is paid where they
/// asked to be.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateDepositRequest {
    /// Where exactly `amount` settles. Optional when `issuer_id` names an
    /// identity with a saved payout address: the first one is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payout_address: Option<String>,
    pub amount: String,
    /// `USDC` (the default) or `USDT`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    /// The one network the payer must pay on, as a decimal chain id string.
    /// Absent, the payer picks among every network the deployment offers
    /// the currency on; required for a currency that must be pinned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain_id: Option<String>,
    /// The issuing party as the document will carry it. Optional when
    /// `issuer_id` is given: the saved identity's name, contact address, and
    /// details are snapshotted in its place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer: Option<Party>,
    /// The paying party. Optional when `customer_id` is given: the saved
    /// customer is snapshotted in its place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payer: Option<Party>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub customer_id: Option<CustomerId>,
    /// The issuer identity this is issued under. The `issuer` party above is
    /// still the snapshot the document carries; this only records which saved
    /// identity it came from, and survives that identity being renamed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer_id: Option<IssuerId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(default = "empty_metadata")]
    pub metadata: serde_json::Value,
    /// The verification add-ons attached to this request; omitted or empty
    /// means none, the default. A request may attach any combination of
    /// email verification, merchant authentication, and wallet attestation.
    #[serde(default)]
    pub verification: PayerVerification,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_id: Option<AttachmentId>,
    /// Lifetime in seconds. Idempotent retries retain the original deadline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

fn empty_metadata() -> serde_json::Value {
    serde_json::json!({})
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransferDto {
    pub timestamp: String,
    pub amount: String,
    pub amount_base_units: String,
    pub sender: String,
    pub transaction_hash: String,
    pub explorer_url: Option<String>,
    pub block: String,
    pub disposition: String,
    pub collected: bool,
    /// Present when Relay's solver sent this transfer for a cross-chain
    /// payment the attested wallet made from another chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay: Option<TransferRelayDto>,
}

/// The origin of a transfer Relay delivered: what the attested wallet sent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransferRelayDto {
    /// Relay's request id, `0x` hex, 32 bytes.
    pub request_id: String,
    /// Decimal chain id the wallet paid on.
    pub origin_chain_id: String,
    /// The transaction the wallet sent there, `0x` hex, 32 bytes.
    pub origin_transaction_hash: Option<String>,
}

/// A stablecoin a payer may send from a Relay origin chain.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RelayOriginTokenDto {
    /// The currency it is: `USDC` or `USDT`.
    pub currency: String,
    /// The contract's own symbol, as the payer's wallet shows it.
    pub symbol: String,
    pub address: String,
    pub decimals: u8,
}

/// A chain a payer may pay from through Relay, as the checkout lists it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RelayOriginChainDto {
    /// Decimal chain id.
    pub chain_id: String,
    pub name: String,
    pub native_symbol: Option<String>,
    /// What the payer may send from this chain; Relay swaps it into the
    /// request's currency on the way. At least one entry.
    pub tokens: Vec<RelayOriginTokenDto>,
    pub explorer_url: Option<String>,
    pub icon_url: Option<String>,
    /// A public RPC, so a wallet that lacks the chain can be asked to add it.
    pub rpc_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RelayOriginChainsResponse {
    pub chains: Vec<RelayOriginChainDto>,
}

/// A quote for paying the amount still due from another chain. The steps
/// are transactions for the attested wallet to send on the origin chain, in
/// order; the last one is the deposit Relay fills against.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RelayQuoteResponse {
    /// The `rli_` id to report the origin transaction against.
    pub id: String,
    /// Relay's request id.
    pub request_id: String,
    pub origin: RelayOriginChainDto,
    /// The stablecoin the payer sends on the origin chain.
    pub origin_token: RelayOriginTokenDto,
    /// What the payer sends on the origin chain, in `origin_token`.
    pub amount_in: String,
    pub amount_in_base_units: String,
    /// What lands on the payment address: exactly the amount still due.
    pub amount_out: String,
    pub amount_out_base_units: String,
    /// Relay's fee in USD, as it reports it.
    pub relayer_fee_usd: Option<String>,
    pub time_estimate_seconds: u64,
    /// When the quote is no longer worth sending; ask for another after.
    pub expires_at: String,
    pub steps: Vec<RelayQuoteStepDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RelayQuoteStepDto {
    /// `approve` or `deposit`.
    pub id: String,
    pub transaction: RelayTransactionDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RelayTransactionDto {
    /// Decimal chain id the transaction is for: the origin chain.
    pub chain_id: String,
    pub to: String,
    /// `0x` hex calldata.
    pub data: String,
    /// Decimal wei.
    pub value: String,
    /// Relay's gas estimate, when it gives one.
    pub gas: Option<String>,
}

/// The cross-chain payment the page is following, on the payer view.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PayerRelayIntentDto {
    pub id: String,
    /// `quoted`, `sent`, `filled`, `failed`, `refunded`, or `expired`.
    pub status: String,
    pub origin_chain_id: String,
    pub origin_transaction_hash: Option<String>,
    /// The destination transaction that delivered the funds, once filled.
    pub fill_transaction_hash: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IndexerFreshnessDto {
    pub last_indexed_block: Option<String>,
    pub last_finalized_block: Option<String>,
    pub cursor_updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AsOfDto {
    pub block: String,
    pub at: String,
}

/// The attached PDF as presented to whoever may see the invoice. The
/// `download_url` is a short-lived signed link, filled in by the route that
/// serves it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttachmentDescriptor {
    /// The `att_` id. The canonical issuance snapshot's `attachment.id` is
    /// the UUID inside it, since that document's schema is frozen.
    pub id: AttachmentId,
    pub filename: String,
    pub mime_type: String,
    pub byte_length: String,
    /// `0x`-prefixed lowercase hex.
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttributionDto {
    pub version: u16,
    pub hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepositRequestResponse {
    pub id: String,
    /// Shareable, request-scoped page for the payer: the hosted deposit checkout.
    pub deposit_url: String,
    /// The one-time payment address. Present as soon as it is registered:
    /// at creation for a request without wallet attestation whose network is
    /// known, and after the payer's attestation or network choice otherwise.
    pub address: Option<String>,
    pub address_explorer_url: Option<String>,
    pub payout_address: String,
    /// The wallet the payer attested, once they have — only a request that
    /// attached the wallet-attestation add-on ever has one.
    pub payer_wallet: Option<String>,
    /// Always present: Gum's recovery wallet, the payment contract's
    /// recovery term. Funds Gum recovers land here and are returned to
    /// the payer manually.
    pub recovery_address: Option<String>,
    /// When the payer's attestation was accepted, for a request with the
    /// wallet-attestation add-on.
    pub wallet_bound_at: Option<String>,
    /// When the payment address was registered and became payable.
    pub ready_at: Option<String>,
    pub expires_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_in: Option<u64>,
    pub amount: String,
    pub amount_base_units: String,
    pub received: String,
    pub received_base_units: String,
    pub remaining: String,
    pub remaining_base_units: String,
    pub currency: String,
    pub fee_amount: String,
    pub fee_amount_base_units: String,
    pub net_amount: String,
    pub net_amount_base_units: String,
    pub status: DepositRequestStatus,
    /// Every network the payer may pay on; the request commits to all of them.
    /// One entry when the merchant pinned the network.
    pub networks: Vec<NetworkDto>,
    /// The network the payment is on: the pinned one from issuance, or the
    /// one the payer chose once a wallet is bound; `None` before either.
    pub token: Option<TokenDto>,
    pub chain: Option<ChainDto>,
    pub settlement_tx_hash: Option<String>,
    pub settlement_explorer_url: Option<String>,
    pub settled_at: Option<String>,
    pub settled_block: Option<String>,
    /// What a third party needs to execute the payment contract themselves;
    /// absent until the address exists.
    pub self_settlement: Option<SelfSettlementDto>,
    pub attention: Option<AttentionDto>,
    pub issuer: Party,
    pub payer: Party,
    pub notes: Option<String>,
    pub heading: Option<String>,
    pub reference: Option<String>,
    pub customer_id: Option<String>,
    pub issuer_id: Option<String>,
    /// The verification add-ons attached, including the merchant's
    /// assertions: merchant-only. Always present, so consumers can read it
    /// unconditionally; a permissionless request serializes the empty
    /// default.
    pub verification: PayerVerification,
    pub attachment: Option<AttachmentDescriptor>,
    pub verification_completed_at: Option<String>,
    pub likely_unsolicited_at: Option<String>,
    /// Merchant-session mode only, and only in the response that minted it:
    /// the single-use secret that opens the hosted checkout for the payer the
    /// merchant authenticated. Stored hashed, so no later read returns it;
    /// `POST /v1/deposit-requests/{id}/client-secret` mints another.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret_expires_at: Option<String>,
    pub attribution: AttributionDto,
    pub metadata: serde_json::Value,
    pub created_at: String,
    pub updated_at: String,
    pub deposited_at: Option<String>,
    pub deposited_at_block: Option<String>,
    pub expired_at: Option<String>,
    pub cancellation_requested_at: Option<String>,
    pub transfers: Vec<TransferDto>,
    pub indexer_freshness: IndexerFreshnessDto,
    pub as_of: Option<AsOfDto>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationFactStatus {
    NotRequired,
    Pending,
    Approved,
    Declined,
}

impl VerificationFactStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotRequired => "not_required",
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Declined => "declined",
        }
    }
}

/// The verification add-ons' facts, each reported on its own so the checkout
/// can show what is still outstanding.
///
/// `email` and `merchant_session` are identity facts and belong to a payer
/// session; `complete` is true once the presented session satisfies every
/// attached identity add-on, which unlocks the request's content. `wallet`
/// is `not_required` unless the request attaches wallet attestation, and it
/// is a fact about the request (one wallet is bound to it), not the session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerificationRequirementsResponse {
    pub email: VerificationFactStatus,
    pub wallet: VerificationFactStatus,
    /// The merchant's own application opened this checkout for the payer it
    /// authenticated, by exchanging a single-use client secret.
    pub merchant_session: VerificationFactStatus,
    pub complete: bool,
}

/// Which facts have been established: the session's identity facts and the
/// request's wallet binding.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VerificationFacts {
    pub email: bool,
    pub wallet: bool,
    pub merchant_session: bool,
}

impl VerificationFacts {
    /// Every fact at once.
    pub const ALL: Self = Self {
        email: true,
        wallet: true,
        merchant_session: true,
    };

    /// Whether these facts satisfy the request's identity add-ons (the gate
    /// on its content). The wallet binding is a separate step.
    pub fn satisfy(self, verification: &PayerVerification) -> bool {
        (!verification.email.is_some() || self.email)
            && (!verification.merchant_auth.is_some() || self.merchant_session)
    }
}

impl VerificationRequirementsResponse {
    /// The requirements the attached add-ons impose at the invoice level:
    /// `completed` is whether the identity add-ons' verification has
    /// finished, `wallet_bound` whether a payer wallet is bound. A payer
    /// session that satisfies only part of the policy refines individual
    /// facts through `from_facts`.
    pub fn for_verification(
        verification: &PayerVerification,
        completed: bool,
        wallet_bound: bool,
    ) -> Self {
        Self::from_facts(
            verification,
            VerificationFacts {
                email: completed,
                wallet: wallet_bound,
                merchant_session: completed,
            },
        )
    }

    /// The requirements the attached add-ons impose, each reported against
    /// the facts established. `complete` is true exactly when every identity
    /// fact an attached add-on needs is present; the wallet is reported
    /// alongside.
    pub fn from_facts(verification: &PayerVerification, facts: VerificationFacts) -> Self {
        let status = |needed: bool, established: bool| match (needed, established) {
            (false, _) => VerificationFactStatus::NotRequired,
            (true, true) => VerificationFactStatus::Approved,
            (true, false) => VerificationFactStatus::Pending,
        };
        let needs_email = verification.email.is_some();
        let needs_merchant_session = verification.merchant_auth.is_some();
        let complete = facts.satisfy(verification);
        Self {
            email: status(needs_email, facts.email),
            wallet: status(verification.wallet_attestation, facts.wallet),
            merchant_session: status(needs_merchant_session, facts.merchant_session),
            complete,
        }
    }
}

/// One verification attempt as the merchant sees it: what was attempted and
/// where it stands, never the code or the payer's session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerificationAttemptResponse {
    pub id: String,
    /// `email`.
    pub kind: String,
    /// `pending`, `approved`, or `abandoned`.
    pub status: String,
    pub verified_at: Option<String>,
    pub created_at: String,
}

/// The merchant's verification view of one invoice: each fact on its own
/// and every attempt (product plan §7.2).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerificationDetailResponse {
    pub verification: PayerVerification,
    pub verification_completed_at: Option<String>,
    pub likely_unsolicited_at: Option<String>,
    pub facts: VerificationRequirementsResponse,
    pub attempts: Vec<VerificationAttemptResponse>,
}

/// Deposit request content revealed only once the payer may see it (product plan §4.3).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PayerDepositRequestDetails {
    pub amount: String,
    pub amount_base_units: String,
    pub payer: Party,
    pub notes: Option<String>,
    pub reference: Option<String>,
    pub attachment: Option<AttachmentDescriptor>,
}

/// Progressive disclosure for one payer. Every deposit mechanic and request
/// detail — including the settlement transaction, which would reveal the
/// address and amount on chain — is present only when `content_unlocked` is
/// true; a gated deposit request shows the issuer, the heading, the policy, the
/// lifecycle status, and what verification remains.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PayerDepositRequestResponse {
    pub id: String,
    pub issuer_name: String,
    pub heading: Option<String>,
    /// Masked hint at the expected mailbox, when the request attaches email
    /// verification; never the mailbox itself.
    pub expected_email_hint: Option<String>,
    pub requirements: VerificationRequirementsResponse,
    pub status: DepositRequestStatus,
    /// Whether the gateway still considers this address payable.
    pub payable: bool,
    pub expires_at: String,
    /// Gateway wall-clock time used by clients to render the deadline without
    /// trusting the payer device's clock.
    pub server_timestamp: String,
    pub settlement_tx_hash: Option<String>,
    pub settlement_explorer_url: Option<String>,
    /// Safety guidance shown only when payout needs operator attention.
    pub payer_message: Option<String>,
    pub content_unlocked: bool,
    /// The request's currency (`USDC`, `USDT`); gated with the content.
    pub currency: Option<String>,
    /// The networks the payer may choose from; gated with the content. One
    /// entry when the merchant pinned the network.
    pub networks: Option<Vec<NetworkDto>>,
    /// The payment's network once known (pinned at issuance, or chosen when
    /// a wallet is bound) and the content is unlocked.
    pub chain: Option<ChainDto>,
    pub token: Option<TokenDto>,
    pub amount: Option<String>,
    pub amount_base_units: Option<String>,
    pub received: Option<String>,
    pub received_base_units: Option<String>,
    pub remaining: Option<String>,
    pub remaining_base_units: Option<String>,
    /// The wallet bound to this request, once a payer has attested one.
    /// Only transfers from it count; anything Gum returns goes to it.
    pub payer_wallet: Option<String>,
    /// Present once unlocked and a wallet is bound; the address does not
    /// exist before the attestation it commits to.
    pub address: Option<String>,
    pub address_explorer_url: Option<String>,
    /// EIP-681 request for the amount still due, absent after the deadline or
    /// after the payment has left the payable state.
    pub deposit_uri: Option<String>,
    pub details: Option<PayerDepositRequestDetails>,
    /// Whether the payer may pay from another chain through Relay: the
    /// deployment offers it, the address exists, and the request is payable.
    pub relay_available: bool,
    /// The newest cross-chain payment for this request, if any was quoted;
    /// gated with the content.
    pub relay: Option<PayerRelayIntentDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepositRequestSummaryResponse {
    pub id: String,
    pub deposit_url: String,
    pub heading: Option<String>,
    pub payer_name: String,
    pub reference: Option<String>,
    pub metadata: serde_json::Value,
    pub created_at: String,
    pub updated_at: String,
    pub expires_at: String,
    pub status: DepositRequestStatus,
    pub amount: String,
    pub received: String,
    pub currency: String,
    pub verification: PayerVerification,
    pub customer_id: Option<String>,
    pub issuer_id: Option<String>,
    pub has_attachment: bool,
    pub verification_completed_at: Option<String>,
    pub likely_unsolicited_at: Option<String>,
    pub cancellation_requested_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepositRequestListResponse {
    pub deposit_requests: Vec<DepositRequestSummaryResponse>,
    pub next_cursor: Option<String>,
}

/// `GET /v1/deposit-requests/{id}/transfers`: an envelope like every other
/// list, so it can grow a field without breaking a reader.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferListResponse {
    pub transfers: Vec<TransferDto>,
}

/// The onboarding walkthrough's one real demo transfer: a payer session
/// already proven to have verified the reserved onboarding mailbox (so the
/// dashboard can unlock the embedded payer view immediately), and the hash
/// of the transfer that was just broadcast.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnboardingDepositResponse {
    pub payer_session: String,
    pub tx_hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DepositRequestStatus {
    AwaitingDeposit,
    PartiallyDeposited,
    Deposited,
    Settled,
    Expired,
    Returned,
    NeedsAttention,
}
impl DepositRequestStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AwaitingDeposit => "awaiting_deposit",
            Self::PartiallyDeposited => "partially_deposited",
            Self::Deposited => "deposited",
            Self::Settled => "settled",
            Self::Expired => "expired",
            Self::Returned => "returned",
            Self::NeedsAttention => "needs_attention",
        }
    }
}

impl std::str::FromStr for DepositRequestStatus {
    type Err = ();
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        [
            Self::AwaitingDeposit,
            Self::PartiallyDeposited,
            Self::Deposited,
            Self::Settled,
            Self::Expired,
            Self::Returned,
            Self::NeedsAttention,
        ]
        .into_iter()
        .find(|status| status.as_str() == value)
        .ok_or(())
    }
}

/// The request's currency as one chain's contract: `symbol` is what the
/// payer's wallet shows there (USDT0 for USDT on Monad), never a second
/// currency.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenDto {
    pub symbol: String,
    pub address: String,
    pub decimals: u8,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainDto {
    pub id: String,
    pub name: String,
    /// The gas token's symbol on this chain.
    pub native_symbol: String,
}
/// One network a request can be paid on: the chain and the request's
/// currency's contract there.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkDto {
    pub chain: ChainDto,
    pub token: TokenDto,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfSettlementDto {
    pub chain_id: String,
    pub factory: String,
    pub salt: String,
}

impl NetworkDto {
    pub fn from_terms(network: &NetworkTerms, currency: Currency) -> Self {
        Self {
            chain: ChainDto {
                id: network.chain_id.to_string(),
                name: chain_name(network.chain_id.0).into(),
                native_symbol: native_symbol(network.chain_id.0).into(),
            },
            token: TokenDto {
                symbol: currency.symbol_on(network.chain_id.0).into(),
                address: network.token.0.to_checksum(None),
                decimals: currency.decimals(),
            },
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttentionDto {
    pub code: String,
    pub message: String,
    pub action: String,
}

/// The public hint for an expected email: `a****@e***.com`. The mask is a
/// fixed shape so it reveals neither the length of the local part nor any
/// domain label beyond its first character and the TLD.
pub fn masked_email(value: &str) -> String {
    let first = |part: &str| part.chars().next().map(String::from).unwrap_or_default();
    let (local, domain) = value.split_once('@').unwrap_or((value, ""));
    let (label, tld) = match domain.rsplit_once('.') {
        Some((head, tld)) => (head.split('.').next().unwrap_or(head), Some(tld)),
        None => (domain, None),
    };
    let mut masked = format!("{}****@{}***", first(local), first(label));
    if let Some(tld) = tld {
        masked.push('.');
        masked.push_str(tld);
    }
    masked
}

impl DepositRequestResponse {
    /// Project the domain model. Fields that live only in the database row
    /// (timestamps, transfers, the customer link, the attachment's filename)
    /// are filled in by the handler.
    pub fn from_invoice(inv: Invoice, expires_in: Option<u64>) -> Self {
        let currency = inv.currency();
        let human = |units| format_units(units, currency.decimals()).unwrap_or_default();
        let remaining = inv.amount.0.saturating_sub(inv.received.0);
        let status = payment_status(&inv);
        let attention = inv
            .attention_reason
            .as_deref()
            .map(|code| attention(code, currency));
        let snapshot = inv.issuance_snapshot;
        let binding = inv.binding.as_ref();
        let payer_wallet = binding.as_ref().and_then(|b| b.wallet.as_ref());
        // A request offering one network is on it before any wallet binds;
        // the address still waits for the binding.
        let chosen = binding
            .map(|b| NetworkDto::from_terms(&b.network, currency))
            .or_else(|| match inv.networks.as_slice() {
                [only] => Some(NetworkDto::from_terms(only, currency)),
                _ => None,
            });
        Self {
            id: inv.id.to_string(),
            deposit_url: String::new(),
            address: binding.map(|b| b.payment_address.0.to_checksum(None)),
            address_explorer_url: None,
            payout_address: inv.beneficiary.0.to_checksum(None),
            payer_wallet: payer_wallet.map(|w| w.payer_wallet.to_checksum(None)),
            recovery_address: Some(snapshot.recovery_address.clone()),
            wallet_bound_at: payer_wallet.map(|w| w.bound_at.clone()),
            ready_at: binding.map(|b| b.ready_at.clone()),
            expires_at: rfc3339(
                DateTime::<Utc>::from_timestamp(inv.expiration_timestamp as i64, 0)
                    .expect("validated timestamp"),
            ),
            expires_in,
            amount: human(inv.amount.0),
            amount_base_units: inv.amount.0.to_string(),
            received: human(inv.received.0),
            received_base_units: inv.received.0.to_string(),
            remaining: human(remaining),
            remaining_base_units: remaining.to_string(),
            currency: currency.code().into(),
            fee_amount: "0".into(),
            fee_amount_base_units: "0".into(),
            net_amount: human(inv.amount.0),
            net_amount_base_units: inv.amount.0.to_string(),
            status,
            networks: inv
                .networks
                .iter()
                .map(|network| NetworkDto::from_terms(network, currency))
                .collect(),
            token: chosen.as_ref().map(|network| network.token.clone()),
            chain: chosen.map(|network| network.chain),
            settlement_tx_hash: inv.execute_tx_hash.map(|h| h.to_string()),
            settlement_explorer_url: None,
            settled_at: inv
                .settled_at_timestamp
                .and_then(|t| DateTime::<Utc>::from_timestamp(t as i64, 0).map(rfc3339)),
            settled_block: inv.resolved_at_block.map(|b| b.to_string()),
            self_settlement: binding.map(|b| SelfSettlementDto {
                chain_id: b.network.chain_id.to_string(),
                factory: b.network.factory.0.to_checksum(None),
                salt: b.salt.0.to_string(),
            }),
            attention,
            issuer: snapshot.issuer,
            payer: snapshot.bill_to,
            notes: snapshot.notes,
            heading: snapshot.heading,
            reference: snapshot.reference,
            customer_id: None,
            issuer_id: None,
            verification: snapshot.payer_verification,
            client_secret: None,
            client_secret_expires_at: None,
            attachment: None,
            verification_completed_at: None,
            likely_unsolicited_at: None,
            attribution: AttributionDto {
                version: inv.attribution_version,
                hash: inv.attribution_hash.to_string(),
            },
            metadata: serde_json::json!({}),
            created_at: String::new(),
            updated_at: String::new(),
            deposited_at: None,
            deposited_at_block: None,
            expired_at: None,
            cancellation_requested_at: inv.cancellation_requested_at,
            transfers: Vec::new(),
            indexer_freshness: IndexerFreshnessDto {
                last_indexed_block: None,
                last_finalized_block: None,
                cursor_updated_at: None,
            },
            as_of: None,
        }
    }
}

fn payment_status(inv: &Invoice) -> DepositRequestStatus {
    if inv.attention_reason.is_some() {
        return DepositRequestStatus::NeedsAttention;
    }
    match inv.status {
        InvoiceStatus::Created if inv.received.0.is_zero() => DepositRequestStatus::AwaitingDeposit,
        InvoiceStatus::Created => DepositRequestStatus::PartiallyDeposited,
        InvoiceStatus::Funded => DepositRequestStatus::Deposited,
        InvoiceStatus::Fulfilled => DepositRequestStatus::Settled,
        InvoiceStatus::Expired => DepositRequestStatus::Expired,
        InvoiceStatus::Recovered => DepositRequestStatus::Returned,
    }
}
/// The merchant-facing explanation of a `attention_reason`, naming the
/// currency's issuer where an address restriction is the cause.
pub fn attention(code: &str, currency: Currency) -> AttentionDto {
    let issuer = currency.issuer_name();
    let (message, action) = match code {
        "beneficiary_blacklisted" => (
            format!("{issuer} has blacklisted the payout address."),
            "Contact support to provide a compliant payout address.",
        ),
        "recovery_blacklisted" => (
            format!(
                "The payer's wallet, where excess funds return, is restricted by {issuer}, the {currency} issuer."
            ),
            "Contact Gum support with the deposit request ID; the payer may need to be contacted.",
        ),
        "payment_address_blacklisted" => (
            format!("{issuer} has blacklisted the deposit address."),
            "Contact support; do not send additional funds.",
        ),
        "balance_below_amount" => (
            "The deposit address balance is lower than the confirmed amount.".into(),
            "Contact support so Gum can investigate safely.",
        ),
        _ => (
            "Automatic settlement has paused.".into(),
            "Funds remain safe at the deposit address; contact support.",
        ),
    };
    AttentionDto {
        code: code.into(),
        message,
        action: action.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Amount, BeneficiaryAddress, CanonicalIssuanceSnapshot, ChainId, EmailVerification,
        FactoryAddress, MerchantAuth, PayerAttestation, PayerVerification, RecoveryAddress,
        TokenAddress, sign_payer_attestation, wallet_of,
    };

    const MONAD: ChainId = ChainId(143);

    fn networks() -> Vec<NetworkTerms> {
        vec![
            NetworkTerms {
                chain_id: MONAD,
                token: TokenAddress(address!("754704Bc059F8C67012fEd69BC8A327a5aafb603")),
                factory: FactoryAddress(address!("0000000000000000000000000000000000000001")),
            },
            NetworkTerms {
                chain_id: ChainId(8453),
                token: TokenAddress(address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913")),
                factory: FactoryAddress(address!("0000000000000000000000000000000000000001")),
            },
        ]
    }
    use alloy_primitives::{Address, B256, U256, address};

    const PAYER_KEY: [u8; 32] = [7u8; 32];

    fn party(name: &str) -> Party {
        Party {
            name: name.into(),
            email: None,
            details: None,
        }
    }

    const RECOVERY: Address = address!("14dC79964da2C08b23698B3D3cc7Ca32193d9955");

    fn invoice(verification: PayerVerification) -> Invoice {
        let beneficiary = BeneficiaryAddress(address!("70997970C51812dc3A010C7d01b50e0d17dc79C8"));
        let amount = Amount(U256::from(1_000_000));
        let mut snapshot = CanonicalIssuanceSnapshot::new(
            party("Acme"),
            party("Globex"),
            verification,
            Currency::Usdc,
            &networks(),
            beneficiary,
            RecoveryAddress(RECOVERY),
            amount,
            1_900_000_000,
        );
        snapshot.heading = Some("March retainer".into());
        snapshot.reference = Some("INV-7".into());
        Invoice::issue(
            Currency::Usdc,
            &networks(),
            beneficiary,
            RecoveryAddress(RECOVERY),
            amount,
            1_900_000_000,
            snapshot,
        )
        .unwrap()
    }

    fn payment(
        status: InvoiceStatus,
        received: u64,
        blocked: Option<&str>,
    ) -> DepositRequestResponse {
        // The wallet step still exists in every sampled response: the sample
        // request attaches wallet attestation and binds through it.
        let mut invoice = invoice(PayerVerification {
            email: None,
            merchant_auth: None,
            wallet_attestation: true,
        });
        let message = PayerAttestation::new(
            invoice.attribution_hash,
            wallet_of(&PAYER_KEY),
            B256::repeat_byte(0x11),
            invoice.expiration_timestamp,
        );
        let factory = invoice.network_for(MONAD).unwrap().factory;
        let attestation = sign_payer_attestation(&PAYER_KEY, &message, MONAD.0, factory.0);
        let binding = invoice
            .bind_payer_wallet(MONAD, attestation, "2026-09-06T00:00:00Z".into())
            .unwrap();
        invoice.binding = Some(binding);
        invoice.status = status;
        invoice.received = Amount(U256::from(received));
        invoice.attention_reason = blocked.map(str::to_owned);
        DepositRequestResponse::from_invoice(invoice, None)
    }

    #[test]
    fn projects_every_customer_status_and_blocking_takes_precedence() {
        for (internal, received, expected) in [
            (
                InvoiceStatus::Created,
                0,
                DepositRequestStatus::AwaitingDeposit,
            ),
            (
                InvoiceStatus::Created,
                1,
                DepositRequestStatus::PartiallyDeposited,
            ),
            (
                InvoiceStatus::Funded,
                1_000_000,
                DepositRequestStatus::Deposited,
            ),
            (
                InvoiceStatus::Fulfilled,
                1_000_000,
                DepositRequestStatus::Settled,
            ),
            (
                InvoiceStatus::Expired,
                400_000,
                DepositRequestStatus::Expired,
            ),
            (
                InvoiceStatus::Recovered,
                400_000,
                DepositRequestStatus::Returned,
            ),
        ] {
            assert_eq!(payment(internal, received, None).status, expected);
        }
        let blocked = payment(
            InvoiceStatus::Fulfilled,
            1_000_000,
            Some("beneficiary_blacklisted"),
        );
        assert_eq!(blocked.status, DepositRequestStatus::NeedsAttention);
        assert!(
            blocked
                .attention
                .unwrap()
                .action
                .contains("compliant payout address")
        );
    }

    #[test]
    fn wire_shape_keeps_customer_vocabulary_and_list_summaries_bounded() {
        let payment = payment(InvoiceStatus::Created, 0, None);
        let json = serde_json::to_value(&payment).unwrap();
        assert!(json["id"].as_str().unwrap().starts_with("dr_"));
        assert_eq!(json["status"], "awaiting_deposit");
        assert_eq!(json["currency"], "USDC");
        assert!(json.get("beneficiary_address").is_none());
        assert!(json.get("memo").is_none());
        // The payer's attested wallet is a fact of the request.
        assert_eq!(
            json["payer_wallet"],
            wallet_of(&PAYER_KEY).to_checksum(None)
        );
        // Recovery is Gum's recovery wallet in every case, never the
        // payer's.
        assert_eq!(json["recovery_address"], RECOVERY.to_checksum(None));
        assert_ne!(json["recovery_address"], json["payer_wallet"]);
        assert_eq!(json["wallet_bound_at"], "2026-09-06T00:00:00Z");
        assert!(json["address"].is_string());
        assert!(json["self_settlement"]["salt"].is_string());
        assert_eq!(json["self_settlement"]["chain_id"], "143");
        assert_eq!(json["chain"]["id"], "143");
        assert_eq!(json["chain"]["name"], "Monad");
        assert_eq!(json["chain"]["native_symbol"], "MON");
        assert_eq!(
            json["token"]["address"],
            "0x754704Bc059F8C67012fEd69BC8A327a5aafb603"
        );
        assert_eq!(json["networks"].as_array().unwrap().len(), 2);
        assert_eq!(json["networks"][1]["chain"]["name"], "Base");
        assert_eq!(json["networks"][1]["chain"]["native_symbol"], "ETH");
        assert!(json.get("refund_address").is_none());

        // Before the network is chosen there is no address and nothing to
        // settle, but the recovery term is already fixed.
        let unbound =
            DepositRequestResponse::from_invoice(invoice(PayerVerification::default()), None);
        let json = serde_json::to_value(&unbound).unwrap();
        assert!(json["address"].is_null());
        assert!(json["payer_wallet"].is_null());
        assert_eq!(json["recovery_address"], RECOVERY.to_checksum(None));
        assert!(json["self_settlement"].is_null());
        assert!(
            json["chain"].is_null(),
            "no network until the payer chooses one"
        );
        assert!(json["token"].is_null());
        assert_eq!(json["networks"].as_array().unwrap().len(), 2);
        assert_eq!(json["status"], "awaiting_deposit");
        assert_eq!(json["issuer"]["name"], "Acme");
        assert_eq!(json["payer"]["name"], "Globex");
        assert!(json.get("bill_to").is_none());
        assert_eq!(json["heading"], "March retainer");
        assert_eq!(json["reference"], "INV-7");
        // The default request carries no add-ons at all, and `verification`
        // is always present so consumers can read it unconditionally.
        assert_eq!(json["verification"]["wallet_attestation"], false);
        assert!(json["verification"]["email"].is_null());
        assert!(json["verification"]["merchant_auth"].is_null());
        assert_eq!(json["attribution"]["version"], 5);
        assert!(
            json["attribution"]["hash"]
                .as_str()
                .unwrap()
                .starts_with("0x")
        );

        let summary = DepositRequestSummaryResponse {
            id: payment.id,
            currency: payment.currency.clone(),
            deposit_url: payment.deposit_url,
            heading: payment.heading,
            payer_name: payment.payer.name,
            issuer_id: None,
            reference: None,
            metadata: serde_json::json!({}),
            created_at: String::new(),
            updated_at: String::new(),
            expires_at: payment.expires_at,
            status: payment.status,
            amount: payment.amount,
            received: payment.received,
            verification: PayerVerification::default(),
            customer_id: None,
            has_attachment: false,
            verification_completed_at: None,
            likely_unsolicited_at: None,
            cancellation_requested_at: None,
        };
        let summary = serde_json::to_value(summary).unwrap();
        assert!(summary.get("transfers").is_none());
        assert!(summary.get("self_settlement").is_none());
        // Always present, empty for the permissionless default.
        assert_eq!(summary["verification"]["wallet_attestation"], false);
    }

    #[test]
    fn create_request_requires_the_document_and_rejects_retired_fields() {
        let accepted = serde_json::json!({
            "payout_address": "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
            "amount": "1",
            "issuer": {"name": "Acme"},
            "payer": {"name": "Globex"}
        });
        let request: CreateDepositRequest = serde_json::from_value(accepted.clone()).unwrap();
        assert_eq!(request.metadata, serde_json::json!({}));
        assert!(request.attachment_id.is_none());
        // Add-ons default to none: omitting `verification` is permissionless.
        assert_eq!(request.verification, PayerVerification::default());

        for (field, value) in [
            // Retired shapes.
            (
                "payer_policy",
                serde_json::json!({"mode": "permissionless"}),
            ),
            (
                "refund_address",
                serde_json::json!("0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC"),
            ),
            ("memo", serde_json::json!("Order 1234")),
            ("line_items", serde_json::json!([])),
            ("bill_to", serde_json::json!({"name": "Globex"})),
        ] {
            let mut rejected = accepted.clone();
            rejected[field] = value;
            let error = serde_json::from_value::<CreateDepositRequest>(rejected).unwrap_err();
            assert!(error.to_string().contains(field), "{field}: {error}");
        }
        let mut missing = accepted.clone();
        missing.as_object_mut().unwrap().remove("amount");
        assert!(
            serde_json::from_value::<CreateDepositRequest>(missing).is_err(),
            "amount must be required"
        );
        // The parties and the payout address may be left to saved records;
        // the handler decides whether the request named any.
        for optional in ["issuer", "payer", "payout_address"] {
            let mut missing = accepted.clone();
            missing.as_object_mut().unwrap().remove(optional);
            assert!(
                serde_json::from_value::<CreateDepositRequest>(missing).is_ok(),
                "{optional} is resolved by the handler"
            );
        }
    }

    #[test]
    fn verification_addons_round_trip_and_default_to_none() {
        for (json, verification) in [
            (serde_json::json!({}), PayerVerification::default()),
            (
                serde_json::json!({"email": {"expected_email": "alice@example.com"}}),
                PayerVerification {
                    email: Some(EmailVerification {
                        expected_email: "alice@example.com".into(),
                    }),
                    merchant_auth: None,
                    wallet_attestation: false,
                },
            ),
            (
                serde_json::json!({"merchant_auth": {"payer_reference": "user_123"}}),
                PayerVerification {
                    email: None,
                    merchant_auth: Some(MerchantAuth {
                        payer_reference: "user_123".into(),
                    }),
                    wallet_attestation: false,
                },
            ),
            (
                serde_json::json!({"wallet_attestation": true}),
                PayerVerification {
                    email: None,
                    merchant_auth: None,
                    wallet_attestation: true,
                },
            ),
        ] {
            let parsed: PayerVerification = serde_json::from_value(json.clone()).unwrap();
            assert_eq!(parsed, verification);
            // Round-trips verbatim once the canonical serialization of the
            // flag is filled in: `wallet_attestation` is always present.
            let mut expected = json.clone();
            if expected.get("wallet_attestation").is_none() {
                expected["wallet_attestation"] = serde_json::json!(false);
            }
            assert_eq!(serde_json::to_value(&parsed).unwrap(), expected);
        }
        // An empty object means no add-ons, and so does omitting the field.
        let empty: PayerVerification = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(empty, PayerVerification::default());
    }

    #[test]
    fn requirements_follow_the_add_ons_and_completion() {
        // No add-ons: nothing to verify, the request is complete as issued.
        let open = VerificationRequirementsResponse::for_verification(
            &PayerVerification::default(),
            false,
            false,
        );
        assert!(open.complete);
        assert_eq!(open.email, VerificationFactStatus::NotRequired);
        assert_eq!(open.merchant_session, VerificationFactStatus::NotRequired);

        let email = VerificationRequirementsResponse::for_verification(
            &PayerVerification {
                email: Some(EmailVerification {
                    expected_email: "alice@example.com".into(),
                }),
                merchant_auth: None,
                wallet_attestation: true,
            },
            false,
            false,
        );
        assert!(!email.complete);
        assert_eq!(email.email, VerificationFactStatus::Pending);
        assert_eq!(serde_json::to_value(email.email).unwrap(), "pending");

        let verified = VerificationRequirementsResponse::from_facts(
            &PayerVerification {
                email: Some(EmailVerification {
                    expected_email: "alice@example.com".into(),
                }),
                merchant_auth: None,
                wallet_attestation: true,
            },
            VerificationFacts {
                email: true,
                merchant_session: false,
                wallet: true,
            },
        );
        assert!(verified.complete);
        assert_eq!(verified.email, VerificationFactStatus::Approved);
        assert_eq!(verified.wallet, VerificationFactStatus::Approved);
        assert_eq!(
            verified.merchant_session,
            VerificationFactStatus::NotRequired
        );

        let merchant = VerificationRequirementsResponse::for_verification(
            &PayerVerification {
                email: None,
                merchant_auth: Some(MerchantAuth {
                    payer_reference: "user_123".into(),
                }),
                wallet_attestation: true,
            },
            false,
            false,
        );
        assert!(!merchant.complete);
        assert_eq!(merchant.email, VerificationFactStatus::NotRequired);
        assert_eq!(merchant.merchant_session, VerificationFactStatus::Pending);
        let opened = VerificationRequirementsResponse::from_facts(
            &PayerVerification {
                email: None,
                merchant_auth: Some(MerchantAuth {
                    payer_reference: "user_123".into(),
                }),
                wallet_attestation: true,
            },
            VerificationFacts {
                email: false,
                merchant_session: true,
                wallet: true,
            },
        );
        assert!(opened.complete);
        assert_eq!(opened.merchant_session, VerificationFactStatus::Approved);
        assert_eq!(
            serde_json::to_value(&opened).unwrap(),
            serde_json::json!({
                "email": "not_required",
                "wallet": "approved",
                "merchant_session": "approved",
                "complete": true
            })
        );
    }

    #[test]
    fn session_facts_refine_each_requirement_and_complete_only_when_add_ons_are_met() {
        let email_only = VerificationFacts {
            email: true,
            wallet: false,
            merchant_session: false,
        };
        let merchant_only = VerificationFacts {
            email: false,
            wallet: false,
            merchant_session: true,
        };
        let email_addon = PayerVerification {
            email: Some(EmailVerification {
                expected_email: "alice@example.com".into(),
            }),
            merchant_auth: None,
            wallet_attestation: false,
        };
        let merchant_addon = PayerVerification {
            email: None,
            merchant_auth: Some(MerchantAuth {
                payer_reference: "user_123".into(),
            }),
            wallet_attestation: false,
        };
        // One add-on's fact never satisfies the other's.
        assert!(!merchant_only.satisfy(&email_addon));
        assert!(!email_only.satisfy(&merchant_addon));
        assert!(merchant_only.satisfy(&merchant_addon));
        let by_merchant =
            VerificationRequirementsResponse::from_facts(&merchant_addon, merchant_only);
        assert_eq!(
            by_merchant.merchant_session,
            VerificationFactStatus::Approved
        );
        assert_eq!(by_merchant.email, VerificationFactStatus::NotRequired);
        assert!(by_merchant.complete);

        let by_email = VerificationRequirementsResponse::from_facts(&email_addon, email_only);
        assert_eq!(by_email.email, VerificationFactStatus::Approved);
        // No wallet attestation attached: no wallet step is required either.
        assert_eq!(by_email.wallet, VerificationFactStatus::NotRequired);
        assert!(
            by_email.complete,
            "the wallet step does not gate the content"
        );

        let nothing = VerificationRequirementsResponse::from_facts(
            &email_addon,
            VerificationFacts::default(),
        );
        assert_eq!(nothing.email, VerificationFactStatus::Pending);
        assert!(!nothing.complete);
        assert!(email_only.satisfy(&email_addon));
        assert!(!VerificationFacts::default().satisfy(&email_addon));
        // With no add-ons attached, no facts are needed at all.
        assert!(VerificationFacts::default().satisfy(&PayerVerification::default()));
        assert_eq!(
            VerificationRequirementsResponse::for_verification(&email_addon, true, true),
            VerificationRequirementsResponse::from_facts(&email_addon, VerificationFacts::ALL)
        );
    }

    #[test]
    fn merchant_response_carries_the_attached_add_ons() {
        let response = DepositRequestResponse::from_invoice(
            invoice(PayerVerification {
                email: Some(EmailVerification {
                    expected_email: "alice@example.com".into(),
                }),
                merchant_auth: None,
                wallet_attestation: false,
            }),
            None,
        );
        let json = serde_json::to_value(&response).unwrap();
        // The flag is always stated explicitly, matching the canonical form.
        assert_eq!(
            json["verification"],
            serde_json::json!({"email": {"expected_email": "alice@example.com"}, "wallet_attestation": false})
        );
        assert!(json["verification"].get("merchant_auth").is_none());
        assert_ne!(json["verification"]["wallet_attestation"], true);
    }

    #[test]
    fn merchant_response_carries_the_payer_reference_and_no_secret_unless_minted() {
        let mut response = DepositRequestResponse::from_invoice(
            invoice(PayerVerification {
                email: None,
                merchant_auth: Some(MerchantAuth {
                    payer_reference: "user_123".into(),
                }),
                wallet_attestation: false,
            }),
            None,
        );
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(
            json["verification"]["merchant_auth"]["payer_reference"],
            "user_123"
        );
        assert!(json["verification"].get("expected_email").is_none());
        // Absent, not null: the secret exists only in the response that minted it.
        assert!(json.get("client_secret").is_none());
        assert!(json.get("client_secret_expires_at").is_none());

        response.client_secret = Some("cs_secret".into());
        response.client_secret_expires_at = Some("2026-01-01T00:00:00Z".into());
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["client_secret"], "cs_secret");
        assert_eq!(json["client_secret_expires_at"], "2026-01-01T00:00:00Z");
    }

    #[test]
    fn expected_email_hint_masks_everything_but_the_shape() {
        assert_eq!(masked_email("alice@example.com"), "a****@e***.com");
        assert_eq!(masked_email("a@b.co"), "a****@b***.co");
        assert_eq!(
            masked_email("alexandra.longname@mail.example.co.uk"),
            "a****@m***.uk"
        );
        assert_eq!(masked_email("x@localhost"), "x****@l***");
        for email in ["alice@example.com", "alexandra.longname@mail.example.co.uk"] {
            let masked = masked_email(email);
            let (local, domain) = email.split_once('@').unwrap();
            assert!(!masked.contains(local), "{masked}");
            assert!(!masked.contains(domain), "{masked}");
            assert_eq!(masked.matches('*').count(), 7);
        }
    }
}
