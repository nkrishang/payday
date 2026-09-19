mod accounts;
mod attachments;
mod chain_faults;
mod cursor;
mod customers;
mod execution_status;
mod invoices;
mod notifications;
mod proofs;
mod relay_intents;
mod sweeps;
mod verifications;
mod webhooks;
mod withdrawals;

pub use accounts::{
    API_KEY_GRACE_HOURS, AccountId, AccountRepository, ApiKeyMetadata, IssueApiKeyError,
    IssuedApiKey, ProvisionAccountError,
};
pub use attachments::{
    AttachInvoiceError, AttachmentRepository, AttachmentStatus, AttachmentStatusParseError,
    CreateAttachmentUpload, DbAttachment,
};
pub use chain_faults::{ChainFaultError, ChainFaultRepository, DbChainFault, RaiseOutcome};
pub use cursor::{CursorRepository, FinalizedHead, IndexerCursor};
pub use customers::{CreateCustomerInput, CustomerRepository, DbCustomer};
pub use execution_status::{ExecutionStatusView, ExecutorStatus, SignerStatus};
pub use invoices::{
    BindPayerWallet, CreateInvoiceInput, CustomerCurrencyStats, DbIndexerFreshness, DbInvoice,
    DbInvoiceError, DbInvoiceTransfer, InsertIssuedInvoice, InsertIssuedInvoiceError,
    InvoiceRepository, IssuanceRequest, PaymentObservation, RangeApplied, ReleasePaymentError,
    WatchFingerprint, same_issuance,
};
pub use notifications::{
    NotificationEvent, NotificationRecipient, NotificationRepository, PAYER_DEPOSIT_REQUEST_ISSUED,
};
pub use proofs::{DbInvoiceSettlement, DbSettlementTransfer, ProofRepository};
pub use relay_intents::{
    DbRelayIntent, FillResolution, MarkSent, NewRelayIntent, RelayIntentRepository,
    RelayIntentStatus, Resolution,
};
pub use sweeps::{
    AppliedSweep, FinalizedSweep, RETRIES_EXHAUSTED, RecoveryReason, SETTLEMENT_UNKNOWN,
    SWEEPABLE_STATUSES, ScheduledSweepJob, SweepError, SweepJobRow, SweepPolicy, SweepQueueStats,
};
pub use verifications::{
    CLIENT_SECRET_PREFIX, CLIENT_SECRET_TTL, ClientSecretExchange, CreatedClientSecret,
    CreatedPayerSession, DbPayerSession, DbVerificationAttempt, EmailVerificationAttempt,
    ExchangeClientSecretError, PAYER_SESSION_TTL, PayerSessionRepository,
    StartEmailVerificationError, VerificationCompletion, WalletChallenge, payer_ref,
};
pub use webhooks::{
    DeliveryClaim, WebhookAttempt, WebhookDelivery, WebhookEndpoint, WebhookEvent,
    WebhookRepository,
};
pub use withdrawals::{
    AuthorizeOutcome, CancelOutcome, CreateWithdrawalError, DbWithdrawal, DbWithdrawalLeg, LegKind,
    LegState, NewWithdrawal, NewWithdrawalLeg, OpenStep, RelayStats, StepApplied, StepError,
    StepPolicy, WithdrawalRepository,
};

#[cfg(any(test, feature = "test-helpers"))]
pub mod test_helpers;

/// The database schema lives in `gum-schema`; re-exported so
/// `#[sqlx::test(migrator = "gum_ledger::MIGRATOR")]` keeps working and every
/// test runs against the exact production schema.
pub use gum_schema::MIGRATOR;

/// Open a pool. Migrations are *not* applied here: `gum-server migrate` runs
/// them as an explicit deployment step so that a service restart can never
/// race another instance on schema changes.
pub use gum_schema::connect;
