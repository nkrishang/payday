//! Didit V3 behind [`PayerIdentityProvider`] (product plan §6.2).
//!
//! ```text
//! POST {base}/v3/session/                       open a hosted session
//! GET  {base}/v3/session/{id}/decision/         fetch its decision
//! x-api-key: <secret>
//! ```
//!
//! The decision is deserialized into allowlisted fields only: statuses,
//! warning risk codes, and the issuing country. Names, document numbers,
//! dates of birth, image links, and biometrics are absent from the types, so
//! serde drops them on the floor and nothing here can log or store them.
//! The raw response is never serialized, logged, or kept.
//!
//! Callbacks carry `X-Signature-V2`, an HMAC-SHA256 over Didit's canonical
//! JSON form of the body, and `X-Timestamp`, checked separately within
//! ±300 seconds. Didit's canonicalization is its own (sorted keys, compact
//! separators, unescaped Unicode, whole-valued floats as integers), so it is
//! implemented here rather than borrowed from the RFC 8785 code the proofs
//! use.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use axum::http::{HeaderMap, StatusCode};
use gateway_core::VerificationFactStatus;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Sha256;
use uuid::Uuid;

use super::{
    IdentityDecision, IdentityProviderError, IdentityProviderStatus, IdentitySession,
    IdentitySessionRequest, PayerIdentityProvider, VerifiedIdentityCallback,
};

pub const DEFAULT_BASE_URL: &str = "https://verification.didit.me";
/// The provider name in `payer_verifications.provider` and webhook receipts.
pub const PROVIDER: &str = "didit";
/// How far a callback's `X-Timestamp` may sit from now, either way.
pub const CALLBACK_TOLERANCE: Duration = Duration::from_secs(300);
/// Risk codes are short upper-case tokens; anything else is not a category
/// and is dropped rather than stored.
const MAX_RISK_CODES: usize = 32;
const MAX_RISK_CODE_LEN: usize = 64;

pub struct DiditConfig {
    pub api_key: String,
    pub workflow_id: String,
    pub webhook_secret: String,
    pub base_url: String,
}

pub struct DiditProvider {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    workflow_id: String,
    webhook_secret: Vec<u8>,
}

#[derive(Serialize)]
struct DiditCreateSessionRequest {
    workflow_id: String,
    vendor_data: String,
    callback: String,
    callback_method: &'static str,
    metadata: DiditMetadata,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_details: Option<DiditExpectedDetails>,
}

#[derive(Serialize)]
struct DiditMetadata {
    verification_id: Uuid,
}

#[derive(Serialize)]
struct DiditExpectedDetails {
    first_name: String,
    last_name: String,
}

#[derive(Deserialize)]
struct DiditCreateSessionResponse {
    session_id: String,
    url: String,
    status: String,
}

/// The allowlisted shape of a decision. Every other field Didit returns is
/// ignored by construction.
#[derive(Deserialize)]
struct DiditDecision {
    status: String,
    #[serde(default)]
    id_verifications: Vec<DiditFeature>,
    #[serde(default)]
    liveness_checks: Vec<DiditFeature>,
    #[serde(default)]
    face_matches: Vec<DiditFeature>,
}

#[derive(Deserialize)]
struct DiditFeature {
    status: String,
    #[serde(default)]
    warnings: Vec<DiditWarning>,
    /// The document's issuing country, on ID verifications only.
    #[serde(default)]
    issuing_state: Option<String>,
}

#[derive(Deserialize)]
struct DiditWarning {
    risk: String,
}

/// The two fields of a callback Payday acts on.
#[derive(Deserialize)]
struct DiditCallback {
    event_id: String,
    session_id: String,
}

impl DiditProvider {
    pub fn new(config: DiditConfig) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|error| format!("failed to build Didit HTTP client: {error}"))?;
        let base_url = config.base_url.trim_end_matches('/').to_owned();
        let parsed = reqwest::Url::parse(&base_url)
            .map_err(|error| format!("invalid PAYDAY_DIDIT_BASE_URL: {error}"))?;
        let local = matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
        if parsed.scheme() != "https" && !(parsed.scheme() == "http" && local) {
            return Err("PAYDAY_DIDIT_BASE_URL must be an HTTPS origin".into());
        }
        if config.api_key.trim().is_empty()
            || config.workflow_id.trim().is_empty()
            || config.webhook_secret.trim().is_empty()
        {
            return Err(
                "PAYDAY_DIDIT_API_KEY, PAYDAY_DIDIT_WORKFLOW_ID, and PAYDAY_DIDIT_WEBHOOK_SECRET must not be blank"
                    .into(),
            );
        }
        Ok(Self {
            http,
            base_url,
            api_key: config.api_key,
            workflow_id: config.workflow_id,
            webhook_secret: config.webhook_secret.into_bytes(),
        })
    }

    /// Reduce a raw decision to what Payday keeps.
    fn reduce(decision: DiditDecision) -> IdentityDecision {
        let status = session_status(&decision.status);
        let document = feature_status(&decision.id_verifications);
        let liveness = feature_status(&decision.liveness_checks);
        let face_match = feature_status(&decision.face_matches);
        let mismatch = decision
            .id_verifications
            .iter()
            .flat_map(|feature| feature.warnings.iter())
            .any(|warning| is_expected_details_mismatch(&warning.risk));
        let expected_identity_match = if mismatch {
            VerificationFactStatus::Declined
        } else if status == IdentityProviderStatus::Approved
            && document == VerificationFactStatus::Approved
        {
            VerificationFactStatus::Approved
        } else {
            VerificationFactStatus::Pending
        };
        let mut risk_codes: Vec<String> = Vec::new();
        for warning in decision
            .id_verifications
            .iter()
            .chain(&decision.liveness_checks)
            .chain(&decision.face_matches)
            .flat_map(|feature| feature.warnings.iter())
        {
            if risk_codes.len() >= MAX_RISK_CODES {
                break;
            }
            if is_risk_code(&warning.risk) && !risk_codes.contains(&warning.risk) {
                risk_codes.push(warning.risk.clone());
            }
        }
        let country_code = decision
            .id_verifications
            .iter()
            .filter_map(|feature| feature.issuing_state.as_deref())
            .find(|code| is_country_code(code))
            .map(str::to_owned);
        IdentityDecision {
            status,
            document,
            liveness,
            face_match,
            expected_identity_match,
            risk_codes,
            country_code,
            // Didit's session `expires_at` is the hosted link's validity, not
            // the approval's; Payday applies its own credential lifetime.
            expires_at: None,
        }
    }
}

/// Didit's session statuses (docs: verification statuses), folded to the
/// six Payday distinguishes.
fn session_status(status: &str) -> IdentityProviderStatus {
    match status {
        "Approved" => IdentityProviderStatus::Approved,
        "Declined" => IdentityProviderStatus::Declined,
        "In Review" => IdentityProviderStatus::InReview,
        "Expired" | "Kyc Expired" => IdentityProviderStatus::Expired,
        "Abandoned" => IdentityProviderStatus::Abandoned,
        // Not Started, In Progress, Awaiting User, Resubmitted, and anything
        // new: the payer is not done.
        _ => IdentityProviderStatus::Pending,
    }
}

/// A feature is established only by an explicit approval; a check the
/// workflow never ran is pending, never approved by omission.
fn feature_status(features: &[DiditFeature]) -> VerificationFactStatus {
    if features.is_empty() {
        return VerificationFactStatus::Pending;
    }
    if features.iter().any(|feature| feature.status == "Declined") {
        VerificationFactStatus::Declined
    } else if features.iter().all(|feature| feature.status == "Approved") {
        VerificationFactStatus::Approved
    } else {
        VerificationFactStatus::Pending
    }
}

fn is_expected_details_mismatch(risk: &str) -> bool {
    matches!(
        risk,
        "FULL_NAME_MISMATCH_WITH_PROVIDED" | "EXPECTED_DETAILS_MISMATCH"
    )
}

fn is_risk_code(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_RISK_CODE_LEN
        && value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

fn is_country_code(value: &str) -> bool {
    (2..=3).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_uppercase())
}

/// Didit's canonical JSON: keys sorted by code point, `,` and `:` with no
/// whitespace, Unicode left unescaped, whole-valued floats as integers.
pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(number) => {
            if let Some(float) = number
                .as_f64()
                .filter(|_| !number.is_i64() && !number.is_u64())
            {
                if float.is_finite() && float.fract() == 0.0 {
                    if float == 0.0 {
                        out.push('0');
                    } else {
                        out.push_str(&format!("{float:.0}"));
                    }
                } else {
                    out.push_str(&float.to_string());
                }
            } else {
                out.push_str(&number.to_string());
            }
        }
        Value::String(string) => {
            out.push_str(&serde_json::to_string(string).expect("strings always serialize"));
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (index, key) in keys.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(key).expect("strings always serialize"));
                out.push(':');
                write_canonical(&map[*key], out);
            }
            out.push('}');
        }
    }
}

/// The V2 signature of `body` under `secret`, as Didit sends it.
#[cfg(test)]
pub fn sign_v2(secret: &[u8], body: &Value) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(canonical_json(body).as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or_default()
}

/// Check a callback against `secret` at time `now`, without touching
/// anything but the two identifying fields.
pub fn verify_callback_at(
    secret: &[u8],
    headers: &HeaderMap,
    raw_body: &[u8],
    now: i64,
) -> Result<VerifiedIdentityCallback, IdentityProviderError> {
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    let signature = header("x-signature-v2")
        .ok_or(IdentityProviderError::InvalidCallback("missing signature"))?;
    let timestamp: i64 = header("x-timestamp")
        .and_then(|value| value.parse().ok())
        .ok_or(IdentityProviderError::InvalidCallback("missing timestamp"))?;
    if (now - timestamp).unsigned_abs() > CALLBACK_TOLERANCE.as_secs() {
        return Err(IdentityProviderError::InvalidCallback("stale timestamp"));
    }
    let body: Value = serde_json::from_slice(raw_body)
        .map_err(|_| IdentityProviderError::InvalidCallback("malformed body"))?;
    let supplied =
        hex::decode(signature).map_err(|_| IdentityProviderError::InvalidCallback("signature"))?;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(canonical_json(&body).as_bytes());
    mac.verify_slice(&supplied)
        .map_err(|_| IdentityProviderError::InvalidCallback("signature"))?;
    let callback: DiditCallback = serde_json::from_value(body)
        .map_err(|_| IdentityProviderError::InvalidCallback("fields"))?;
    Ok(VerifiedIdentityCallback {
        event_id: callback.event_id,
        provider_reference: callback.session_id,
    })
}

#[async_trait]
impl PayerIdentityProvider for DiditProvider {
    async fn create_session(
        &self,
        request: IdentitySessionRequest,
    ) -> Result<IdentitySession, IdentityProviderError> {
        let body = DiditCreateSessionRequest {
            workflow_id: self.workflow_id.clone(),
            // Merchant-specific by construction: so is the payer reference.
            vendor_data: hex::encode(request.payer_ref),
            callback: request.callback_url,
            callback_method: "both",
            metadata: DiditMetadata {
                verification_id: request.verification_id,
            },
            expected_details: request
                .expected_identity
                .map(|identity| DiditExpectedDetails {
                    first_name: identity.first_name,
                    last_name: identity.last_name,
                }),
        };
        let response = self
            .http
            .post(format!("{}/v3/session/", self.base_url))
            .header("x-api-key", &self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|error| {
                IdentityProviderError::Unavailable(format!("create session: {error}"))
            })?;
        let status = response.status();
        if status.is_success() {
            let created: DiditCreateSessionResponse = response.json().await.map_err(|_| {
                IdentityProviderError::Unavailable("create session returned no session".into())
            })?;
            Ok(IdentitySession {
                provider_reference: created.session_id,
                hosted_url: created.url,
                status: session_status(&created.status),
            })
        } else if status.is_client_error() && status != StatusCode::TOO_MANY_REQUESTS {
            Err(IdentityProviderError::Rejected(format!(
                "create session answered {status}"
            )))
        } else {
            Err(IdentityProviderError::Unavailable(format!(
                "create session answered {status}"
            )))
        }
    }

    async fn fetch_decision(
        &self,
        provider_reference: &str,
    ) -> Result<IdentityDecision, IdentityProviderError> {
        if !provider_reference
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(IdentityProviderError::Rejected(
                "malformed session id".into(),
            ));
        }
        let response = self
            .http
            .get(format!(
                "{}/v3/session/{provider_reference}/decision/",
                self.base_url
            ))
            .header("x-api-key", &self.api_key)
            .send()
            .await
            .map_err(|error| {
                IdentityProviderError::Unavailable(format!("fetch decision: {error}"))
            })?;
        let status = response.status();
        if status.is_success() {
            let decision: DiditDecision = response
                .json()
                .await
                .map_err(|_| IdentityProviderError::Unavailable("decision did not parse".into()))?;
            Ok(Self::reduce(decision))
        } else if status == StatusCode::NOT_FOUND {
            Err(IdentityProviderError::Rejected(format!(
                "fetch decision answered {status}"
            )))
        } else {
            Err(IdentityProviderError::Unavailable(format!(
                "fetch decision answered {status}"
            )))
        }
    }

    fn verify_callback(
        &self,
        headers: &HeaderMap,
        raw_body: &[u8],
    ) -> Result<VerifiedIdentityCallback, IdentityProviderError> {
        verify_callback_at(&self.webhook_secret, headers, raw_body, unix_now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use serde_json::json;

    /// Recorded, redacted decisions. Every extracted-identity field carries a
    /// sentinel so a test can prove it never leaves the deserializer.
    pub const APPROVED_MATCHED: &str = include_str!("fixtures/decision_approved_matched.json");
    pub const DECLINED_NAME_MISMATCH: &str =
        include_str!("fixtures/decision_declined_name_mismatch.json");
    pub const APPROVED_UNATTRIBUTED: &str =
        include_str!("fixtures/decision_approved_unattributed.json");
    pub const APPROVED_MISSING_LIVENESS: &str =
        include_str!("fixtures/decision_approved_missing_liveness.json");
    pub const IN_REVIEW: &str = include_str!("fixtures/decision_in_review.json");
    pub const EXPIRED: &str = include_str!("fixtures/decision_expired.json");

    pub const SENTINELS: [&str; 6] = [
        "SENTINEL_FIRST_NAME",
        "SENTINEL_LAST_NAME",
        "SENTINEL_DOC_NUMBER",
        "1900-01-01",
        "sentinel-images.example",
        "SENTINEL_FULL_NAME",
    ];

    fn reduce(fixture: &str) -> IdentityDecision {
        DiditProvider::reduce(serde_json::from_str(fixture).unwrap())
    }

    #[test]
    fn fixtures_carry_sentinels_that_never_reach_the_decision() {
        for fixture in [
            APPROVED_MATCHED,
            DECLINED_NAME_MISMATCH,
            APPROVED_UNATTRIBUTED,
            APPROVED_MISSING_LIVENESS,
            IN_REVIEW,
        ] {
            assert!(SENTINELS.iter().any(|sentinel| fixture.contains(sentinel)));
            let decision = reduce(fixture);
            let debug = format!("{decision:?}");
            for sentinel in SENTINELS {
                assert!(!debug.contains(sentinel), "{sentinel} leaked into {debug}");
            }
        }
    }

    #[test]
    fn approved_matched_establishes_every_fact() {
        let decision = reduce(APPROVED_MATCHED);
        assert_eq!(decision.status, IdentityProviderStatus::Approved);
        assert_eq!(decision.document, VerificationFactStatus::Approved);
        assert_eq!(decision.liveness, VerificationFactStatus::Approved);
        assert_eq!(decision.face_match, VerificationFactStatus::Approved);
        assert_eq!(
            decision.expected_identity_match,
            VerificationFactStatus::Approved
        );
        assert_eq!(decision.country_code.as_deref(), Some("ESP"));
        assert!(decision.risk_codes.is_empty());
        assert!(decision.expires_at.is_none());
    }

    #[test]
    fn expected_name_mismatch_declines_the_match_and_keeps_only_risk_categories() {
        let decision = reduce(DECLINED_NAME_MISMATCH);
        assert_eq!(decision.status, IdentityProviderStatus::Declined);
        assert_eq!(decision.document, VerificationFactStatus::Declined);
        assert_eq!(
            decision.expected_identity_match,
            VerificationFactStatus::Declined
        );
        assert_eq!(
            decision.risk_codes,
            ["EXPECTED_DETAILS_MISMATCH", "DOCUMENT_EXPIRED"]
        );
    }

    #[test]
    fn unrelated_mismatch_warning_does_not_decline_the_expected_identity() {
        let raw = APPROVED_MATCHED.replace(
            "\"warnings\": []",
            "\"warnings\": [{\"risk\": \"FACE_MISMATCH\"}]",
        );
        let decision = reduce(&raw);
        assert_eq!(decision.status, IdentityProviderStatus::Approved);
        assert_eq!(
            decision.expected_identity_match,
            VerificationFactStatus::Approved
        );
        assert_eq!(decision.risk_codes, ["FACE_MISMATCH"]);
    }

    #[test]
    fn documented_full_name_mismatch_never_approves_the_expected_identity() {
        let raw = APPROVED_MATCHED.replace(
            "\"warnings\": []",
            "\"warnings\": [{\"risk\": \"FULL_NAME_MISMATCH_WITH_PROVIDED\"}]",
        );
        let decision = reduce(&raw);
        assert_eq!(decision.status, IdentityProviderStatus::Approved);
        assert_eq!(
            decision.expected_identity_match,
            VerificationFactStatus::Declined
        );
        assert_eq!(decision.risk_codes, ["FULL_NAME_MISMATCH_WITH_PROVIDED"]);
    }

    #[test]
    fn approved_unattributed_does_not_assert_a_match_it_never_checked() {
        let decision = reduce(APPROVED_UNATTRIBUTED);
        assert_eq!(decision.status, IdentityProviderStatus::Approved);
        assert_eq!(decision.document, VerificationFactStatus::Approved);
        assert_eq!(decision.liveness, VerificationFactStatus::Approved);
        assert_eq!(decision.face_match, VerificationFactStatus::Approved);
        // Approved with no mismatch warning reads as a match; the caller
        // ignores it in unattributed mode, where it is not required.
        assert_eq!(
            decision.expected_identity_match,
            VerificationFactStatus::Approved
        );
        // A lower-case or free-text warning is not a risk category.
        assert_eq!(decision.risk_codes, ["POSSIBLE_SCREEN_CAPTURE"]);
        assert_eq!(decision.country_code.as_deref(), Some("DEU"));
    }

    #[test]
    fn a_check_the_workflow_never_ran_is_pending_not_approved() {
        let decision = reduce(APPROVED_MISSING_LIVENESS);
        assert_eq!(decision.status, IdentityProviderStatus::Approved);
        assert_eq!(decision.document, VerificationFactStatus::Approved);
        assert_eq!(decision.liveness, VerificationFactStatus::Pending);
        assert_eq!(decision.face_match, VerificationFactStatus::Pending);
    }

    #[test]
    fn in_review_and_expired_statuses_fold_correctly() {
        let review = reduce(IN_REVIEW);
        assert_eq!(review.status, IdentityProviderStatus::InReview);
        assert_eq!(review.document, VerificationFactStatus::Pending);
        assert_eq!(review.risk_codes, ["POSSIBLE_TAMPERING"]);
        let expired = reduce(EXPIRED);
        assert_eq!(expired.status, IdentityProviderStatus::Expired);
        assert_eq!(expired.document, VerificationFactStatus::Pending);
        assert_eq!(
            session_status("Kyc Expired"),
            IdentityProviderStatus::Expired
        );
        assert_eq!(
            session_status("Abandoned"),
            IdentityProviderStatus::Abandoned
        );
        assert_eq!(
            session_status("Resubmitted"),
            IdentityProviderStatus::Pending
        );
        assert_eq!(
            session_status("Awaiting User"),
            IdentityProviderStatus::Pending
        );
        assert_eq!(
            session_status("Something New"),
            IdentityProviderStatus::Pending
        );
    }

    #[test]
    fn canonical_form_matches_didits_rules() {
        let body = json!({
            "b": 100.0,
            "a": "José \"q\" \n",
            "n": {"y": [1, 2.5, -0.0, 1e20, true, null], "x": null},
            "z": -3,
            "u": "\u{1F600}"
        });
        assert_eq!(
            canonical_json(&body),
            "{\"a\":\"José \\\"q\\\" \\n\",\"b\":100,\"n\":{\"x\":null,\"y\":[1,2.5,0,100000000000000000000,true,null]},\"u\":\"😀\",\"z\":-3}"
        );
    }

    fn headers(signature: &str, timestamp: i64) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("x-signature-v2", HeaderValue::from_str(signature).unwrap());
        headers.insert(
            "x-timestamp",
            HeaderValue::from_str(&timestamp.to_string()).unwrap(),
        );
        headers
    }

    #[test]
    fn callbacks_need_a_fresh_timestamp_and_a_matching_v2_signature() {
        let secret = b"shared-secret";
        let now = 1_800_000_000;
        // Pretty-printed with unsorted keys, as a re-encoding proxy might send it.
        let raw = br#"{
            "webhook_type": "status.updated",
            "status": "Approved",
            "session_id": "9f1c0f6e-1111-4c1a-9c1e-000000000001",
            "event_id": "evt-1",
            "timestamp": 1800000000,
            "decision": {"id_verifications": [{"first_name": "SENTINEL_FIRST_NAME", "score": 99.0}]}
        }"#;
        let signature = sign_v2(secret, &serde_json::from_slice(raw).unwrap());

        let verified = verify_callback_at(secret, &headers(&signature, now), raw, now).unwrap();
        assert_eq!(verified.event_id, "evt-1");
        assert_eq!(
            verified.provider_reference,
            "9f1c0f6e-1111-4c1a-9c1e-000000000001"
        );
        assert!(!format!("{verified:?}").contains("SENTINEL"));

        let tampered = raw
            .iter()
            .copied()
            .map(|b| if b == b'1' { b'2' } else { b })
            .collect::<Vec<_>>();
        assert!(matches!(
            verify_callback_at(secret, &headers(&signature, now), &tampered, now),
            Err(IdentityProviderError::InvalidCallback("signature"))
        ));
        assert!(matches!(
            verify_callback_at(secret, &headers(&signature, now - 301), raw, now),
            Err(IdentityProviderError::InvalidCallback("stale timestamp"))
        ));
        assert!(matches!(
            verify_callback_at(secret, &headers(&signature, now + 301), raw, now),
            Err(IdentityProviderError::InvalidCallback("stale timestamp"))
        ));
        assert!(matches!(
            verify_callback_at(b"other-secret", &headers(&signature, now), raw, now),
            Err(IdentityProviderError::InvalidCallback("signature"))
        ));
        assert!(matches!(
            verify_callback_at(secret, &HeaderMap::new(), raw, now),
            Err(IdentityProviderError::InvalidCallback("missing signature"))
        ));
        let mut no_timestamp = HeaderMap::new();
        no_timestamp.insert("x-signature-v2", HeaderValue::from_str(&signature).unwrap());
        assert!(matches!(
            verify_callback_at(secret, &no_timestamp, raw, now),
            Err(IdentityProviderError::InvalidCallback("missing timestamp"))
        ));
        assert!(matches!(
            verify_callback_at(secret, &headers("zz", now), raw, now),
            Err(IdentityProviderError::InvalidCallback("signature"))
        ));
        assert!(matches!(
            verify_callback_at(secret, &headers(&signature, now), b"not json", now),
            Err(IdentityProviderError::InvalidCallback("malformed body"))
        ));
        let without_ids = json!({"status": "Approved"});
        let signature = sign_v2(secret, &without_ids);
        assert!(matches!(
            verify_callback_at(
                secret,
                &headers(&signature, now),
                without_ids.to_string().as_bytes(),
                now
            ),
            Err(IdentityProviderError::InvalidCallback("fields"))
        ));
    }

    #[test]
    fn configuration_is_validated() {
        let config = |base_url: &str, api_key: &str| DiditConfig {
            api_key: api_key.into(),
            workflow_id: "wf".into(),
            webhook_secret: "secret".into(),
            base_url: base_url.into(),
        };
        assert!(DiditProvider::new(config(DEFAULT_BASE_URL, "key")).is_ok());
        assert!(DiditProvider::new(config("http://127.0.0.1:4010/", "key")).is_ok());
        assert!(DiditProvider::new(config("http://didit.example", "key")).is_err());
        assert!(DiditProvider::new(config(DEFAULT_BASE_URL, " ")).is_err());
    }
}
