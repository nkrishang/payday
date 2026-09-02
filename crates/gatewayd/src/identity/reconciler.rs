//! Reconciliation of open identity sessions (product plan §6.2).
//!
//! Didit retries a webhook only twice, so the webhook is a hint, not the
//! source of truth: every open attempt is polled until the provider settles
//! it. A callback merely brings the next poll forward. Provider outages defer
//! the poll with backoff and never count against the payer.

use std::sync::Arc;
use std::time::Duration;

use gateway_core::VerificationFactStatus;
use gateway_db::{ClaimedVerification, DecisionRecord, IdentityStatus, VerificationRepository};
use tokio::sync::watch;

use super::{
    IdentityDecision, IdentityProviderError, IdentityProviderStatus, PayerIdentityProvider,
};

/// Attempts claimed per pass.
const BATCH: i64 = 20;
/// How long a claim is held while the provider is asked.
const LEASE: Duration = Duration::from_secs(60);
/// How long to sleep when nothing is due.
const IDLE: Duration = Duration::from_secs(5);
/// Consecutive provider failures after which the attempt is reported at
/// error level. Polling continues regardless, at the capped backoff.
const FAILURE_ALERT_THRESHOLD: i32 = 10;
/// The risk category Payday itself attaches when the provider approved a
/// session without running every check the policy needs.
pub const INCOMPLETE_CHECKS_RISK: &str = "PAYDAY_INCOMPLETE_CHECKS";

#[derive(Debug, thiserror::Error)]
pub enum ReconciliationError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

pub async fn run(
    repo: VerificationRepository,
    provider: Arc<dyn PayerIdentityProvider>,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        let pause = match repo.claim_due(BATCH, LEASE).await {
            Ok(claims) if !claims.is_empty() => {
                for claim in claims {
                    let id = claim.id;
                    if let Err(error) = reconcile_one(&repo, provider.as_ref(), claim).await {
                        tracing::error!(verification_id = %id, %error, "identity reconciliation failed");
                    }
                }
                Duration::ZERO
            }
            Ok(_) => IDLE,
            Err(error) => {
                tracing::error!(%error, "identity reconciliation claim failed");
                IDLE
            }
        };
        tokio::select! {
            _ = tokio::time::sleep(pause) => {}
            _ = shutdown.changed() => break,
        }
    }
}

/// Ask the provider about one claimed attempt and record the answer.
pub async fn reconcile_one(
    repo: &VerificationRepository,
    provider: &dyn PayerIdentityProvider,
    verification: ClaimedVerification,
) -> Result<(), ReconciliationError> {
    match provider
        .fetch_decision(&verification.provider_reference)
        .await
    {
        Ok(decision) => {
            let record = record_from(&decision, verification.matched);
            if record.status == IdentityStatus::ReviewRequired {
                tracing::warn!(
                    verification_id = %verification.id,
                    "identity provider approved a session without every required check; held for review"
                );
            }
            let completion = repo.apply_decision(verification.id, &record).await?;
            if record.status != verification.status {
                tracing::info!(
                    verification_id = %verification.id,
                    status = record.status.as_str(),
                    invoice_completed = completion
                        .as_ref()
                        .is_some_and(|completion| completion.invoice_completed_at.is_some()),
                    "identity verification status changed"
                );
            }
        }
        Err(IdentityProviderError::InvalidCallback(_)) => {
            unreachable!("fetching a decision never verifies a callback")
        }
        Err(error) => {
            // Unavailable or rejected alike: the payer is not declined for
            // the provider's trouble. Keep polling, and shout once it has
            // gone on long enough that an operator should look.
            let failures = verification.provider_failures + 1;
            if failures >= FAILURE_ALERT_THRESHOLD {
                tracing::error!(verification_id = %verification.id, failures, %error, "identity provider keeps failing");
            } else {
                tracing::warn!(verification_id = %verification.id, failures, %error, "identity provider fetch failed");
            }
            repo.defer_after_failure(verification.id).await?;
        }
    }
    Ok(())
}

/// A freshly opened session's status, before any decision exists.
pub fn provider_status(status: IdentityProviderStatus) -> IdentityStatus {
    match status {
        IdentityProviderStatus::Pending => IdentityStatus::Pending,
        IdentityProviderStatus::Approved => IdentityStatus::Approved,
        IdentityProviderStatus::Declined => IdentityStatus::Declined,
        IdentityProviderStatus::InReview => IdentityStatus::InReview,
        IdentityProviderStatus::Expired => IdentityStatus::Expired,
        IdentityProviderStatus::Abandoned => IdentityStatus::Abandoned,
    }
}

/// Translate a provider decision into what the ledger records. Approval
/// counts only when every check the policy needs was actually run and
/// passed; an approval short of that is held for a human rather than trusted
/// or declined.
pub fn record_from(decision: &IdentityDecision, matched: bool) -> DecisionRecord {
    let liveness = match (decision.liveness, decision.face_match) {
        (VerificationFactStatus::Declined, _) | (_, VerificationFactStatus::Declined) => {
            VerificationFactStatus::Declined
        }
        (VerificationFactStatus::Approved, VerificationFactStatus::Approved) => {
            VerificationFactStatus::Approved
        }
        _ => VerificationFactStatus::Pending,
    };
    let identity_match = if matched {
        decision.expected_identity_match
    } else {
        VerificationFactStatus::NotRequired
    };
    let mut risk_codes = decision.risk_codes.clone();
    let status = match decision.status {
        IdentityProviderStatus::Approved => {
            let complete = decision.document == VerificationFactStatus::Approved
                && liveness == VerificationFactStatus::Approved
                && (!matched || identity_match == VerificationFactStatus::Approved);
            if complete {
                IdentityStatus::Approved
            } else if decision.document == VerificationFactStatus::Declined
                || liveness == VerificationFactStatus::Declined
                || identity_match == VerificationFactStatus::Declined
            {
                IdentityStatus::Declined
            } else {
                risk_codes.push(INCOMPLETE_CHECKS_RISK.into());
                IdentityStatus::ReviewRequired
            }
        }
        IdentityProviderStatus::Declined => IdentityStatus::Declined,
        IdentityProviderStatus::InReview => IdentityStatus::InReview,
        IdentityProviderStatus::Pending => IdentityStatus::Pending,
        IdentityProviderStatus::Expired => IdentityStatus::Expired,
        IdentityProviderStatus::Abandoned => IdentityStatus::Abandoned,
    };
    DecisionRecord {
        status,
        document: decision.document,
        liveness,
        identity_match,
        risk_codes,
        country_code: decision.country_code.clone(),
        expires_at: decision.expires_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::testing::decision;

    #[test]
    fn records_require_every_check_the_mode_needs() {
        let approved = record_from(&decision(IdentityProviderStatus::Approved), true);
        assert_eq!(approved.status, IdentityStatus::Approved);
        assert_eq!(approved.identity_match, VerificationFactStatus::Approved);

        let unattributed = record_from(&decision(IdentityProviderStatus::Approved), false);
        assert_eq!(unattributed.status, IdentityStatus::Approved);
        assert_eq!(
            unattributed.identity_match,
            VerificationFactStatus::NotRequired
        );

        let mut mismatch = decision(IdentityProviderStatus::Approved);
        mismatch.expected_identity_match = VerificationFactStatus::Declined;
        assert_eq!(
            record_from(&mismatch, true).status,
            IdentityStatus::Declined
        );
        assert_eq!(
            record_from(&mismatch, false).status,
            IdentityStatus::Approved
        );

        let mut no_liveness = decision(IdentityProviderStatus::Approved);
        no_liveness.liveness = VerificationFactStatus::Pending;
        let held = record_from(&no_liveness, false);
        assert_eq!(held.status, IdentityStatus::ReviewRequired);
        assert_eq!(held.liveness, VerificationFactStatus::Pending);
        assert_eq!(held.risk_codes, [INCOMPLETE_CHECKS_RISK]);

        let declined = record_from(&decision(IdentityProviderStatus::Declined), true);
        assert_eq!(declined.status, IdentityStatus::Declined);
        assert_eq!(declined.liveness, VerificationFactStatus::Declined);
        assert_eq!(
            record_from(&decision(IdentityProviderStatus::InReview), true).status,
            IdentityStatus::InReview
        );
        assert_eq!(
            record_from(&decision(IdentityProviderStatus::Expired), true).status,
            IdentityStatus::Expired
        );
        assert_eq!(
            record_from(&decision(IdentityProviderStatus::Abandoned), true).status,
            IdentityStatus::Abandoned
        );
        assert_eq!(
            record_from(&decision(IdentityProviderStatus::Pending), true).status,
            IdentityStatus::Pending
        );
    }
}
