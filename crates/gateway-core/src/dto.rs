//! Merchant- and payer-facing HTTP types. The API speaks of deposit requests
//! and deposits; the internal invoice terminology stops at this seam.

use alloy_primitives::utils::format_units;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{Invoice, InvoiceStatus, Party, PayerPolicy, PayerPolicyMode, USDC_DECIMALS};

/// The only attachment type Payday accepts (product plan §4.2).
pub const PDF_MIME_TYPE: &str = "application/pdf";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateDepositRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_address: Option<String>,
    pub payout_address: String,
    pub amount: String,
    pub issuer: Party,
    pub payer: Party,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub customer_id: Option<Uuid>,
    /// The issuer identity this is issued under. The `issuer` party above is
    /// still the snapshot the document carries; this only records which saved
    /// identity it came from, and survives that identity being renamed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(default = "empty_metadata")]
    pub metadata: serde_json::Value,
    pub payer_policy: PayerPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_id: Option<Uuid>,
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
    pub id: Uuid,
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
    pub address: String,
    pub address_explorer_url: Option<String>,
    pub payout_address: String,
    /// Payday's custodial recovery wallet, committed into the deposit address.
    /// Overpayments, expired balances, and late transfers land here and are
    /// returned by the operator. Merchant-visible; never in the payer response.
    pub recovery_address: String,
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
    pub token: TokenDto,
    pub chain: ChainDto,
    pub settlement_tx_hash: Option<String>,
    pub settlement_explorer_url: Option<String>,
    pub settled_at: Option<String>,
    pub settled_block: Option<String>,
    pub self_settlement: SelfSettlementDto,
    pub attention: Option<AttentionDto>,
    pub issuer: Party,
    pub payer: Party,
    pub notes: Option<String>,
    pub heading: Option<String>,
    pub reference: Option<String>,
    pub customer_id: Option<String>,
    pub issuer_id: Option<String>,
    /// The complete policy including the merchant's assertions: merchant-only.
    pub payer_policy: PayerPolicy,
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

/// What the payer learns about the policy: the mode and a hint at whose
/// mailbox is expected, never the assertion itself. A merchant-session
/// policy's `payer_reference` is the merchant's own identifier and is never
/// shown to the payer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PayerPolicyResponse {
    pub mode: PayerPolicyMode,
    pub expected_email_hint: Option<String>,
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

/// The facts behind the modes (product plan §3.2), each reported on its own
/// so the checkout can show what is still outstanding.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerificationRequirementsResponse {
    pub email: VerificationFactStatus,
    /// The merchant's own application opened this checkout for the payer it
    /// authenticated, by exchanging a single-use client secret.
    pub merchant_session: VerificationFactStatus,
    pub complete: bool,
}

/// Which facts one payer session has established; the policy mode decides
/// which ones matter.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VerificationFacts {
    pub email: bool,
    pub merchant_session: bool,
}

impl VerificationFacts {
    /// Every fact at once: what an invoice-level completion implies.
    pub const ALL: Self = Self {
        email: true,
        merchant_session: true,
    };

    /// Whether these facts satisfy `mode`.
    pub fn satisfy(self, mode: PayerPolicyMode) -> bool {
        VerificationRequirementsResponse::from_facts(mode, self).complete
    }
}

impl VerificationRequirementsResponse {
    /// The requirements a mode imposes, all at one status. `completed` is
    /// whether the invoice's verification has finished; a payer session that
    /// satisfies only part of the policy refines individual facts.
    pub fn for_mode(mode: PayerPolicyMode, completed: bool) -> Self {
        Self::from_facts(
            mode,
            if completed {
                VerificationFacts::ALL
            } else {
                VerificationFacts::default()
            },
        )
    }

    /// The requirements a mode imposes, each reported against the facts one
    /// payer session has established. `complete` is true exactly when every
    /// fact the mode needs is present.
    pub fn from_facts(mode: PayerPolicyMode, facts: VerificationFacts) -> Self {
        let status = |needed: bool, established: bool| match (needed, established) {
            (false, _) => VerificationFactStatus::NotRequired,
            (true, true) => VerificationFactStatus::Approved,
            (true, false) => VerificationFactStatus::Pending,
        };
        let needs_email = mode == PayerPolicyMode::VerifiedEmail;
        let needs_merchant_session = mode == PayerPolicyMode::MerchantSession;
        let complete =
            (!needs_email || facts.email) && (!needs_merchant_session || facts.merchant_session);
        Self {
            email: status(needs_email, facts.email),
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
    pub payer_policy_mode: PayerPolicyMode,
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
    pub payer_policy: PayerPolicyResponse,
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
    pub chain: Option<ChainDto>,
    pub token: Option<TokenDto>,
    pub amount: Option<String>,
    pub amount_base_units: Option<String>,
    pub received: Option<String>,
    pub received_base_units: Option<String>,
    pub remaining: Option<String>,
    pub remaining_base_units: Option<String>,
    pub address: Option<String>,
    pub address_explorer_url: Option<String>,
    /// EIP-681 request for the amount still due, absent after the deadline or
    /// after the payment has left the payable state.
    pub deposit_uri: Option<String>,
    pub details: Option<PayerDepositRequestDetails>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepositRequestSummaryResponse {
    pub id: String,
    pub heading: Option<String>,
    pub payer_name: String,
    pub reference: Option<String>,
    pub metadata: serde_json::Value,
    pub created_at: String,
    pub status: DepositRequestStatus,
    pub amount: String,
    pub received: String,
    pub payer_policy_mode: PayerPolicyMode,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelDepositRequestResponse {
    pub deposit_request: DepositRequestResponse,
    pub advisory: String,
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
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfSettlementDto {
    pub factory: String,
    pub salt: String,
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
        let human = |units| format_units(units, USDC_DECIMALS).unwrap_or_default();
        let remaining = inv.amount.0.saturating_sub(inv.received.0);
        let status = payment_status(&inv);
        let attention = inv.blocked_reason.as_deref().map(attention);
        let snapshot = inv.issuance_snapshot;
        Self {
            id: inv.id.to_string(),
            deposit_url: String::new(),
            address: inv.payment_address.0.to_checksum(None),
            address_explorer_url: None,
            payout_address: inv.beneficiary.0.to_checksum(None),
            recovery_address: inv.recovery.0.to_checksum(None),
            expires_at: DateTime::<Utc>::from_timestamp(inv.expiration_timestamp as i64, 0)
                .expect("validated timestamp")
                .to_rfc3339_opts(SecondsFormat::Secs, true),
            expires_in,
            amount: human(inv.amount.0),
            amount_base_units: inv.amount.0.to_string(),
            received: human(inv.received.0),
            received_base_units: inv.received.0.to_string(),
            remaining: human(remaining),
            remaining_base_units: remaining.to_string(),
            currency: "USDC".into(),
            fee_amount: "0".into(),
            fee_amount_base_units: "0".into(),
            net_amount: human(inv.amount.0),
            net_amount_base_units: inv.amount.0.to_string(),
            status,
            token: TokenDto {
                symbol: "USDC".into(),
                address: inv.token.0.to_checksum(None),
                decimals: USDC_DECIMALS,
            },
            chain: ChainDto {
                id: inv.chain_id.0.to_string(),
                name: chain_name(inv.chain_id.0).into(),
            },
            settlement_tx_hash: inv.execute_tx_hash.map(|h| h.to_string()),
            settlement_explorer_url: None,
            settled_at: inv.settled_at_timestamp.and_then(|t| {
                DateTime::<Utc>::from_timestamp(t as i64, 0)
                    .map(|d| d.to_rfc3339_opts(SecondsFormat::Secs, true))
            }),
            settled_block: inv.resolved_at_block.map(|b| b.to_string()),
            self_settlement: SelfSettlementDto {
                factory: inv.factory.0.to_checksum(None),
                salt: inv.salt.0.to_string(),
            },
            attention,
            issuer: snapshot.issuer,
            payer: snapshot.bill_to,
            notes: snapshot.notes,
            heading: snapshot.heading,
            reference: snapshot.reference,
            customer_id: None,
            issuer_id: None,
            payer_policy: snapshot.payer_policy,
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
    if inv.blocked_reason.is_some() || inv.status == InvoiceStatus::Blocked {
        return DepositRequestStatus::NeedsAttention;
    }
    match inv.status {
        InvoiceStatus::Created if inv.received.0.is_zero() => DepositRequestStatus::AwaitingDeposit,
        InvoiceStatus::Created => DepositRequestStatus::PartiallyDeposited,
        InvoiceStatus::Funded | InvoiceStatus::Deploying => DepositRequestStatus::Deposited,
        InvoiceStatus::Fulfilled => DepositRequestStatus::Settled,
        InvoiceStatus::Expired => DepositRequestStatus::Expired,
        InvoiceStatus::Recovered => DepositRequestStatus::Returned,
        InvoiceStatus::Blocked => DepositRequestStatus::NeedsAttention,
    }
}
fn chain_name(id: u64) -> &'static str {
    match id {
        1 => "Ethereum",
        143 => "Monad",
        10_143 => "Monad Testnet",
        31_337 => "Local",
        _ => "Unknown",
    }
}
fn attention(code: &str) -> AttentionDto {
    let (message, action) = match code {
        "beneficiary_blacklisted" => (
            "Circle has blacklisted the payout address.",
            "Contact support to provide a compliant payout address.",
        ),
        "recovery_blacklisted" => (
            "The Payday recovery wallet is restricted by the USDC issuer.",
            "Payday is resolving it; no merchant action is needed. Contact Payday support only if the deposit request stays paused.",
        ),
        "payment_address_blacklisted" => (
            "Circle has blacklisted the deposit address.",
            "Contact support; do not send additional funds.",
        ),
        "balance_below_amount" => (
            "The deposit address balance is lower than the confirmed amount.",
            "Contact support so Payday can investigate safely.",
        ),
        _ => (
            "Automatic settlement has paused.",
            "Funds remain safe at the deposit address; contact support.",
        ),
    };
    AttentionDto {
        code: code.into(),
        message: message.into(),
        action: action.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Amount, BeneficiaryAddress, CanonicalIssuanceSnapshot, ChainId, FactoryAddress,
        RecoveryAddress, TokenAddress,
    };
    use alloy_primitives::{U256, address};

    fn party(name: &str) -> Party {
        Party {
            name: name.into(),
            email: None,
            details: None,
        }
    }

    fn invoice(policy: PayerPolicy) -> Invoice {
        let factory = FactoryAddress(address!("0000000000000000000000000000000000000001"));
        let chain_id = ChainId(143);
        let token = TokenAddress(address!("754704Bc059F8C67012fEd69BC8A327a5aafb603"));
        let beneficiary = BeneficiaryAddress(address!("70997970C51812dc3A010C7d01b50e0d17dc79C8"));
        let amount = Amount(U256::from(1_000_000));
        let recovery = RecoveryAddress(address!("3C44CdDdB6a900fa2b585dd299e03d12FA4293BC"));
        let mut snapshot = CanonicalIssuanceSnapshot::new(
            party("Acme"),
            party("Globex"),
            policy,
            factory,
            chain_id,
            token,
            beneficiary,
            amount,
            1_900_000_000,
            recovery,
        );
        snapshot.heading = Some("March retainer".into());
        snapshot.reference = Some("INV-7".into());
        Invoice::issue(
            factory,
            chain_id,
            token,
            beneficiary,
            amount,
            1_900_000_000,
            recovery,
            snapshot,
        )
        .unwrap()
    }

    fn payment(
        status: InvoiceStatus,
        received: u64,
        blocked: Option<&str>,
    ) -> DepositRequestResponse {
        let mut invoice = invoice(PayerPolicy::Permissionless);
        invoice.status = status;
        invoice.received = Amount(U256::from(received));
        invoice.blocked_reason = blocked.map(str::to_owned);
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
                InvoiceStatus::Deploying,
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
            (
                InvoiceStatus::Blocked,
                1,
                DepositRequestStatus::NeedsAttention,
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
        assert_eq!(
            json["recovery_address"],
            "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC"
        );
        assert!(json.get("refund_address").is_none());
        assert_eq!(json["issuer"]["name"], "Acme");
        assert_eq!(json["payer"]["name"], "Globex");
        assert!(json.get("bill_to").is_none());
        assert_eq!(json["heading"], "March retainer");
        assert_eq!(json["reference"], "INV-7");
        assert_eq!(json["payer_policy"]["mode"], "permissionless");
        assert_eq!(json["attribution"]["version"], 1);
        assert!(
            json["attribution"]["hash"]
                .as_str()
                .unwrap()
                .starts_with("0x")
        );

        let summary = DepositRequestSummaryResponse {
            id: payment.id,
            heading: payment.heading,
            payer_name: payment.payer.name,
            issuer_id: None,
            reference: None,
            metadata: serde_json::json!({}),
            created_at: String::new(),
            status: payment.status,
            amount: payment.amount,
            received: payment.received,
            payer_policy_mode: payment.payer_policy.mode(),
            customer_id: None,
            has_attachment: false,
            verification_completed_at: None,
            likely_unsolicited_at: None,
            cancellation_requested_at: None,
        };
        let summary = serde_json::to_value(summary).unwrap();
        assert!(summary.get("transfers").is_none());
        assert!(summary.get("self_settlement").is_none());
        assert!(summary.get("payer_policy").is_none());
        assert_eq!(summary["payer_policy_mode"], "permissionless");
    }

    #[test]
    fn create_request_requires_the_document_and_rejects_retired_fields() {
        let accepted = serde_json::json!({
            "payout_address": "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
            "amount": "1",
            "issuer": {"name": "Acme"},
            "payer": {"name": "Globex"},
            "payer_policy": {"mode": "permissionless"}
        });
        let request: CreateDepositRequest = serde_json::from_value(accepted.clone()).unwrap();
        assert_eq!(request.metadata, serde_json::json!({}));
        assert!(request.attachment_id.is_none());

        for (field, value) in [
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
        for required in ["issuer", "payer", "payer_policy"] {
            let mut missing = accepted.clone();
            missing.as_object_mut().unwrap().remove(required);
            assert!(
                serde_json::from_value::<CreateDepositRequest>(missing).is_err(),
                "{required} must be required"
            );
        }
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

    #[test]
    fn requirements_follow_the_mode_and_completion() {
        let open =
            VerificationRequirementsResponse::for_mode(PayerPolicyMode::Permissionless, false);
        assert!(open.complete);
        assert_eq!(open.email, VerificationFactStatus::NotRequired);

        let email =
            VerificationRequirementsResponse::for_mode(PayerPolicyMode::VerifiedEmail, false);
        assert!(!email.complete);
        assert_eq!(email.email, VerificationFactStatus::Pending);
        assert_eq!(serde_json::to_value(email.email).unwrap(), "pending");

        let verified =
            VerificationRequirementsResponse::for_mode(PayerPolicyMode::VerifiedEmail, true);
        assert!(verified.complete);
        assert_eq!(verified.email, VerificationFactStatus::Approved);
        assert_eq!(
            verified.merchant_session,
            VerificationFactStatus::NotRequired
        );

        let merchant =
            VerificationRequirementsResponse::for_mode(PayerPolicyMode::MerchantSession, false);
        assert!(!merchant.complete);
        assert_eq!(merchant.email, VerificationFactStatus::NotRequired);
        assert_eq!(merchant.merchant_session, VerificationFactStatus::Pending);
        let opened =
            VerificationRequirementsResponse::for_mode(PayerPolicyMode::MerchantSession, true);
        assert!(opened.complete);
        assert_eq!(opened.merchant_session, VerificationFactStatus::Approved);
        assert_eq!(
            serde_json::to_value(&opened).unwrap(),
            serde_json::json!({
                "email": "not_required",
                "merchant_session": "approved",
                "complete": true
            })
        );
    }

    #[test]
    fn session_facts_refine_each_requirement_and_complete_only_when_the_mode_is_met() {
        let email_only = VerificationFacts {
            email: true,
            merchant_session: false,
        };
        let merchant_only = VerificationFacts {
            email: false,
            merchant_session: true,
        };
        // One mode's fact never satisfies the other's.
        assert!(!merchant_only.satisfy(PayerPolicyMode::VerifiedEmail));
        assert!(!email_only.satisfy(PayerPolicyMode::MerchantSession));
        assert!(merchant_only.satisfy(PayerPolicyMode::MerchantSession));
        let by_merchant = VerificationRequirementsResponse::from_facts(
            PayerPolicyMode::MerchantSession,
            merchant_only,
        );
        assert_eq!(
            by_merchant.merchant_session,
            VerificationFactStatus::Approved
        );
        assert_eq!(by_merchant.email, VerificationFactStatus::NotRequired);
        assert!(by_merchant.complete);

        let by_email = VerificationRequirementsResponse::from_facts(
            PayerPolicyMode::VerifiedEmail,
            email_only,
        );
        assert_eq!(by_email.email, VerificationFactStatus::Approved);
        assert!(by_email.complete);

        let nothing = VerificationRequirementsResponse::from_facts(
            PayerPolicyMode::VerifiedEmail,
            VerificationFacts::default(),
        );
        assert_eq!(nothing.email, VerificationFactStatus::Pending);
        assert!(!nothing.complete);
        assert!(email_only.satisfy(PayerPolicyMode::VerifiedEmail));
        assert!(!VerificationFacts::default().satisfy(PayerPolicyMode::VerifiedEmail));
        assert!(VerificationFacts::default().satisfy(PayerPolicyMode::Permissionless));
        assert_eq!(
            VerificationRequirementsResponse::for_mode(PayerPolicyMode::VerifiedEmail, true),
            VerificationRequirementsResponse::from_facts(
                PayerPolicyMode::VerifiedEmail,
                VerificationFacts::ALL
            )
        );
    }

    #[test]
    fn merchant_response_carries_the_full_policy_for_the_verified_mode() {
        let response = DepositRequestResponse::from_invoice(
            invoice(PayerPolicy::VerifiedEmail {
                expected_email: "alice@example.com".into(),
            }),
            None,
        );
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["payer_policy"]["mode"], "verified_email");
        assert_eq!(json["payer_policy"]["expected_email"], "alice@example.com");
        assert!(json["payer_policy"].get("expected_identity").is_none());
    }

    #[test]
    fn merchant_response_carries_the_payer_reference_and_no_secret_unless_minted() {
        let mut response = DepositRequestResponse::from_invoice(
            invoice(PayerPolicy::MerchantSession {
                payer_reference: "user_123".into(),
            }),
            None,
        );
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["payer_policy"]["mode"], "merchant_session");
        assert_eq!(json["payer_policy"]["payer_reference"], "user_123");
        assert!(json["payer_policy"].get("expected_email").is_none());
        // Absent, not null: the secret exists only in the response that minted it.
        assert!(json.get("client_secret").is_none());
        assert!(json.get("client_secret_expires_at").is_none());

        response.client_secret = Some("cs_secret".into());
        response.client_secret_expires_at = Some("2026-01-01T00:00:00Z".into());
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["client_secret"], "cs_secret");
        assert_eq!(json["client_secret_expires_at"], "2026-01-01T00:00:00Z");
    }
}
