//! The payer identity provider boundary (product plan §6.2).
//!
//! Deliberately thin: one trait with the three things Payday needs from a
//! document-and-liveness vendor (open a hosted session, fetch its decision,
//! verify a callback) and status-only types to carry the answers. Didit is
//! the only implementation and lives in [`didit`]; nothing vendor-specific
//! leaves that module. The reconciler in [`reconciler`] polls open sessions
//! so a lost webhook never strands a payer.
//!
//! No type here can hold an extracted name, document number, date of birth,
//! image, or biometric. That is the privacy boundary, enforced by shape.

pub mod didit;
pub mod reconciler;

use alloy_primitives::B256;
use async_trait::async_trait;
use axum::http::HeaderMap;
use chrono::{DateTime, Utc};
use gateway_core::{ExpectedIdentity, VerificationFactStatus};
use gateway_db::IdentityStatus;
use uuid::Uuid;

/// What a hosted session is opened for.
#[derive(Debug, Clone)]
pub struct IdentitySessionRequest {
    /// Payday's own attempt id: the only thing sent as metadata.
    pub verification_id: Uuid,
    /// The merchant-scoped payer reference, which is what the vendor may
    /// use for duplicate detection; it names no mailbox.
    pub payer_ref: B256,
    /// The merchant's assertion for matched mode; absent for unattributed.
    pub expected_identity: Option<ExpectedIdentity>,
    /// Where the vendor sends the payer afterwards.
    pub callback_url: String,
}

#[derive(Debug, Clone)]
pub struct IdentitySession {
    pub provider_reference: String,
    pub hosted_url: String,
    pub status: IdentityProviderStatus,
}

/// Provider statuses use the ledger's canonical representation. Adapters
/// never produce `ReviewRequired`; that remains Payday's own verdict.
pub type IdentityProviderStatus = IdentityStatus;

/// The decision, reduced to statuses and allowlisted risk categories.
#[derive(Debug, Clone)]
pub struct IdentityDecision {
    pub status: IdentityProviderStatus,
    pub document: VerificationFactStatus,
    pub liveness: VerificationFactStatus,
    pub face_match: VerificationFactStatus,
    pub expected_identity_match: VerificationFactStatus,
    pub risk_codes: Vec<String>,
    pub country_code: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

/// A callback whose signature checked out: only its identity survives.
#[derive(Debug, Clone)]
pub struct VerifiedIdentityCallback {
    pub event_id: String,
    pub provider_reference: String,
}

#[derive(Debug, thiserror::Error)]
pub enum IdentityProviderError {
    /// The vendor could not be reached or answered outside its contract.
    /// Never a statement about the payer.
    #[error("identity provider unavailable: {0}")]
    Unavailable(String),
    /// The vendor refused the request itself (bad credentials, unknown
    /// workflow, unknown session).
    #[error("identity provider rejected the request: {0}")]
    Rejected(String),
    /// A callback that does not prove it came from the vendor. The reason is
    /// a fixed token, never derived from the body.
    #[error("identity callback rejected: {0}")]
    InvalidCallback(&'static str),
}

#[async_trait]
pub trait PayerIdentityProvider: Send + Sync {
    async fn create_session(
        &self,
        request: IdentitySessionRequest,
    ) -> Result<IdentitySession, IdentityProviderError>;

    async fn fetch_decision(
        &self,
        provider_reference: &str,
    ) -> Result<IdentityDecision, IdentityProviderError>;

    fn verify_callback(
        &self,
        headers: &HeaderMap,
        raw_body: &[u8],
    ) -> Result<VerifiedIdentityCallback, IdentityProviderError>;
}

#[cfg(test)]
pub mod testing {
    //! A scripted vendor for route and reconciler tests: it hands out
    //! sequential session references, answers `fetch_decision` from a queue
    //! per session, and accepts callbacks whose body is `{"event_id", "session_id"}`
    //! when the fixed header is present.

    use std::collections::{HashMap, VecDeque};
    use std::sync::Mutex;

    use super::*;

    pub const CALLBACK_HEADER: &str = "x-fake-signature";
    pub const CALLBACK_SECRET: &str = "fake-secret";

    #[derive(Default)]
    pub struct FakeIdentityProvider {
        pub sessions: Mutex<Vec<IdentitySessionRequest>>,
        pub decisions: Mutex<HashMap<String, VecDeque<Result<IdentityDecision, String>>>>,
        pub fetches: Mutex<Vec<String>>,
        /// When set, `create_session` fails as an outage.
        pub outage: Mutex<bool>,
    }

    impl FakeIdentityProvider {
        pub fn queue(&self, reference: &str, decision: Result<IdentityDecision, &str>) {
            self.decisions
                .lock()
                .unwrap()
                .entry(reference.to_owned())
                .or_default()
                .push_back(decision.map_err(str::to_owned));
        }
    }

    pub fn decision(status: IdentityProviderStatus) -> IdentityDecision {
        let fact = |approved: bool| {
            if approved {
                VerificationFactStatus::Approved
            } else {
                VerificationFactStatus::Declined
            }
        };
        let approved = status == IdentityProviderStatus::Approved;
        IdentityDecision {
            status,
            document: fact(approved),
            liveness: fact(approved),
            face_match: fact(approved),
            expected_identity_match: fact(approved),
            risk_codes: if approved {
                vec![]
            } else {
                vec!["DOCUMENT_EXPIRED".into()]
            },
            country_code: Some("DEU".into()),
            expires_at: None,
        }
    }

    #[async_trait]
    impl PayerIdentityProvider for FakeIdentityProvider {
        async fn create_session(
            &self,
            request: IdentitySessionRequest,
        ) -> Result<IdentitySession, IdentityProviderError> {
            if *self.outage.lock().unwrap() {
                return Err(IdentityProviderError::Unavailable("outage".into()));
            }
            let mut sessions = self.sessions.lock().unwrap();
            sessions.push(request);
            let reference = format!("fake-session-{}", sessions.len());
            Ok(IdentitySession {
                hosted_url: format!("https://verify.example/session/{reference}"),
                provider_reference: reference,
                status: IdentityProviderStatus::Pending,
            })
        }

        async fn fetch_decision(
            &self,
            provider_reference: &str,
        ) -> Result<IdentityDecision, IdentityProviderError> {
            self.fetches
                .lock()
                .unwrap()
                .push(provider_reference.to_owned());
            let next = self
                .decisions
                .lock()
                .unwrap()
                .get_mut(provider_reference)
                .and_then(VecDeque::pop_front);
            match next {
                Some(Ok(decision)) => Ok(decision),
                Some(Err(reason)) => Err(IdentityProviderError::Unavailable(reason)),
                None => Ok(decision(IdentityProviderStatus::Pending)),
            }
        }

        fn verify_callback(
            &self,
            headers: &HeaderMap,
            raw_body: &[u8],
        ) -> Result<VerifiedIdentityCallback, IdentityProviderError> {
            if headers.get(CALLBACK_HEADER).and_then(|v| v.to_str().ok()) != Some(CALLBACK_SECRET) {
                return Err(IdentityProviderError::InvalidCallback("signature"));
            }
            let body: serde_json::Value = serde_json::from_slice(raw_body)
                .map_err(|_| IdentityProviderError::InvalidCallback("body"))?;
            let field = |name: &str| {
                body.get(name)
                    .and_then(|value| value.as_str())
                    .map(str::to_owned)
                    .ok_or(IdentityProviderError::InvalidCallback("fields"))
            };
            Ok(VerifiedIdentityCallback {
                event_id: field("event_id")?,
                provider_reference: field("session_id")?,
            })
        }
    }
}
