//! Verifiable attribution: the canonical issuance snapshot, its hash, and the
//! CREATE3 salt derived from it (product plan §5.2).
//!
//! ```text
//! canonical_bytes  = JCS(canonical_issuance_snapshot)          (RFC 8785)
//! attribution_hash = keccak256("PAYDAY_ATTRIBUTION_V1" || canonical_bytes)
//! salt             = keccak256("PAYDAY_SALT_V1" || nonce || attribution_hash)
//! ```
//!
//! The random nonce keeps identical invoices at distinct addresses and stops
//! anyone enumerating addresses from guessable invoice contents. The API never
//! accepts a client-supplied nonce or salt: [`derive_attribution`] is the only
//! way an issuance salt comes into existence.

use std::fmt;
use std::str::FromStr;

use alloy_primitives::{B256, keccak256};
use rand::Rng;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    Amount, BeneficiaryAddress, ChainId, FactoryAddress, RecoveryAddress, Salt, TokenAddress,
};

pub const ATTRIBUTION_VERSION: u16 = 1;
pub const ATTRIBUTION_DOMAIN: &[u8] = b"PAYDAY_ATTRIBUTION_V1";
pub const SALT_DOMAIN: &[u8] = b"PAYDAY_SALT_V1";
/// `CanonicalIssuanceSnapshot::schema`; a new schema means a new version.
pub const SNAPSHOT_SCHEMA: &str = "payday.invoice";
/// `CanonicalIssuanceSnapshot::canonicalization`: RFC 8785 JSON Canonicalization Scheme.
pub const CANONICALIZATION: &str = "RFC8785";
/// One side of an invoice: bounded free text rendered verbatim, never parsed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Party {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

/// The two public payer modes (product plan §3.2). Deserialization enforces
/// the mode rules of §4.5 (the expected email is required exactly where its
/// mode needs it and forbidden elsewhere), so an unrepresentable policy can
/// never reach validation.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum PayerPolicy {
    Permissionless,
    VerifiedEmail { expected_email: String },
}

/// The wire shape of every mode. A derived internally tagged enum would let a
/// unit variant swallow extra fields, so the mode rules are applied by hand.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PayerPolicyWire {
    mode: PayerPolicyMode,
    #[serde(default)]
    expected_email: Option<String>,
}

impl<'de> Deserialize<'de> for PayerPolicy {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = PayerPolicyWire::deserialize(deserializer)?;
        match (wire.mode, wire.expected_email) {
            (PayerPolicyMode::Permissionless, None) => Ok(PayerPolicy::Permissionless),
            (PayerPolicyMode::VerifiedEmail, Some(expected_email)) => {
                Ok(PayerPolicy::VerifiedEmail { expected_email })
            }
            (PayerPolicyMode::Permissionless, Some(_)) => Err(D::Error::custom(
                "expected_email is not allowed for permissionless",
            )),
            (mode, None) => Err(D::Error::custom(format!(
                "expected_email is required for {mode}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PayerPolicyMode {
    Permissionless,
    VerifiedEmail,
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("unknown payer policy mode: {0}")]
pub struct PayerPolicyModeParseError(pub String);

impl PayerPolicyMode {
    pub const ALL: [PayerPolicyMode; 2] = [
        PayerPolicyMode::Permissionless,
        PayerPolicyMode::VerifiedEmail,
    ];

    /// The canonical string used on the wire and in `invoices.payer_policy_mode`.
    /// This is the single source of truth; the DB CHECK constraint agrees with it.
    pub fn as_str(self) -> &'static str {
        match self {
            PayerPolicyMode::Permissionless => "permissionless",
            PayerPolicyMode::VerifiedEmail => "verified_email",
        }
    }

    /// Whether invoice content and payment mechanics are withheld from the
    /// payer until verification completes (product plan §4.3).
    pub fn is_gated(self) -> bool {
        self != PayerPolicyMode::Permissionless
    }
}

impl fmt::Display for PayerPolicyMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for PayerPolicyMode {
    type Err = PayerPolicyModeParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        PayerPolicyMode::ALL
            .into_iter()
            .find(|mode| mode.as_str() == s)
            .ok_or_else(|| PayerPolicyModeParseError(s.to_string()))
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PayerPolicyError {
    #[error("expected_email must be a valid email address of at most 254 bytes")]
    InvalidExpectedEmail,
}

/// The email syntax rule shared with merchant sign-in: bounded, no whitespace,
/// one `@`, and a dotted domain. Anything stricter would reject real mailboxes.
pub fn valid_email(value: &str) -> bool {
    value.len() <= 254
        && !value.chars().any(char::is_whitespace)
        && value.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty()
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
        })
}

impl PayerPolicy {
    pub fn mode(&self) -> PayerPolicyMode {
        match self {
            PayerPolicy::Permissionless => PayerPolicyMode::Permissionless,
            PayerPolicy::VerifiedEmail { .. } => PayerPolicyMode::VerifiedEmail,
        }
    }

    pub fn expected_email(&self) -> Option<&str> {
        match self {
            PayerPolicy::Permissionless => None,
            PayerPolicy::VerifiedEmail { expected_email } => Some(expected_email),
        }
    }

    pub fn validate(&self) -> Result<(), PayerPolicyError> {
        if self
            .expected_email()
            .is_some_and(|email| !valid_email(email))
        {
            return Err(PayerPolicyError::InvalidExpectedEmail);
        }
        Ok(())
    }

    /// The form that is stored and committed: the expected email trimmed and
    /// lowercased so a retry that only differs in case is the same request.
    pub fn normalized(&self) -> Self {
        match self {
            PayerPolicy::Permissionless => PayerPolicy::Permissionless,
            PayerPolicy::VerifiedEmail { expected_email } => PayerPolicy::VerifiedEmail {
                expected_email: expected_email.trim().to_lowercase(),
            },
        }
    }
}

/// What the invoice commits to about its attached PDF. The bytes themselves
/// are verified against `sha256` by whoever holds them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttachmentCommitment {
    pub id: Uuid,
    /// Decimal byte count.
    pub byte_length: String,
    /// `0x`-prefixed lowercase hex.
    pub sha256: String,
}

/// Everything an issued invoice commits to. Numbers are decimal strings and
/// addresses are EIP-55 checksummed so the canonical form is unambiguous.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CanonicalIssuanceSnapshot {
    pub schema: String,
    pub canonicalization: String,
    pub issuer: Party,
    pub bill_to: Party,
    pub amount_base_units: String,
    pub notes: Option<String>,
    pub heading: Option<String>,
    pub reference: Option<String>,
    pub expiration_timestamp: String,
    pub payer_policy: PayerPolicy,
    pub attachment: Option<AttachmentCommitment>,
    pub chain_id: String,
    pub token_address: String,
    pub receiver_address: String,
    pub recovery_address: String,
    pub factory_address: String,
}

impl CanonicalIssuanceSnapshot {
    /// A snapshot of the mandatory fields from typed payment parameters, with
    /// every optional field absent. This is the one place that renders chain
    /// parameters into their canonical string form, so
    /// [`crate::Invoice::issue`] can check a snapshot against the same rules.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        issuer: Party,
        bill_to: Party,
        payer_policy: PayerPolicy,
        factory: FactoryAddress,
        chain_id: ChainId,
        token: TokenAddress,
        receiver: BeneficiaryAddress,
        amount: Amount,
        expiration_timestamp: u64,
        recovery: RecoveryAddress,
    ) -> Self {
        Self {
            schema: SNAPSHOT_SCHEMA.into(),
            canonicalization: CANONICALIZATION.into(),
            issuer,
            bill_to,
            amount_base_units: amount.0.to_string(),
            notes: None,
            heading: None,
            reference: None,
            expiration_timestamp: expiration_timestamp.to_string(),
            payer_policy,
            attachment: None,
            chain_id: chain_id.0.to_string(),
            token_address: token.0.to_checksum(None),
            receiver_address: receiver.0.to_checksum(None),
            recovery_address: recovery.0.to_checksum(None),
            factory_address: factory.0.to_checksum(None),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributionMaterial {
    pub canonical_bytes: Vec<u8>,
    pub nonce: B256,
    pub attribution_hash: B256,
    pub salt: Salt,
}

#[derive(Debug, Error)]
pub enum AttributionError {
    #[error("issuance snapshot could not be canonicalized: {0}")]
    Canonicalization(#[from] serde_json::Error),
    #[error("issuance snapshot {field} does not match the payment parameters")]
    SnapshotMismatch { field: &'static str },
}

/// RFC 8785 bytes of a snapshot: the input to the attribution hash.
pub fn canonical_bytes(snapshot: &CanonicalIssuanceSnapshot) -> Result<Vec<u8>, AttributionError> {
    Ok(serde_jcs::to_vec(snapshot)?)
}

/// `keccak256(ATTRIBUTION_DOMAIN || canonical_bytes)`.
pub fn attribution_hash(canonical_bytes: &[u8]) -> B256 {
    keccak256([ATTRIBUTION_DOMAIN, canonical_bytes].concat())
}

/// `keccak256(SALT_DOMAIN || nonce || attribution_hash)`: what a verifier
/// recomputes from a proof, and what issuance stores as the invoice salt.
pub fn recompute_salt(nonce: B256, attribution_hash: B256) -> Salt {
    Salt(keccak256(
        [SALT_DOMAIN, nonce.as_slice(), attribution_hash.as_slice()].concat(),
    ))
}

/// Canonicalize and hash a snapshot, draw a fresh 32-byte OS-CSPRNG nonce,
/// and derive the issuance salt from both.
pub fn derive_attribution(
    snapshot: &CanonicalIssuanceSnapshot,
) -> Result<AttributionMaterial, AttributionError> {
    let canonical_bytes = canonical_bytes(snapshot)?;
    let attribution_hash = attribution_hash(&canonical_bytes);
    let nonce = B256::from(rand::rng().random::<[u8; 32]>());
    let salt = recompute_salt(nonce, attribution_hash);
    Ok(AttributionMaterial {
        canonical_bytes,
        nonce,
        attribution_hash,
        salt,
    })
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{U256, address};

    use super::*;
    use crate::predict_payment_address;

    pub(crate) fn party(name: &str) -> Party {
        Party {
            name: name.into(),
            email: None,
            details: None,
        }
    }

    fn snapshot() -> CanonicalIssuanceSnapshot {
        let mut snapshot = CanonicalIssuanceSnapshot::new(
            Party {
                name: "Acme Corp".into(),
                email: Some("billing@acme.example".into()),
                details: None,
            },
            Party {
                name: "Globex".into(),
                email: None,
                details: Some("1 Main St".into()),
            },
            PayerPolicy::VerifiedEmail {
                expected_email: "alice@example.com".into(),
            },
            FactoryAddress(address!("0x5FbDB2315678afecb367f032d93F642f64180aa3")),
            ChainId(143),
            TokenAddress(address!("0x754704Bc059F8C67012fEd69BC8A327a5aafb603")),
            BeneficiaryAddress(address!("0x70997970C51812dc3A010C7d01b50e0d17dc79C8")),
            Amount(U256::from(1_000_000)),
            1_900_000_000,
            RecoveryAddress(address!("0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc")),
        );
        snapshot.notes = Some("Thanks".into());
        snapshot.reference = Some("INV-1".into());
        snapshot.attachment = Some(AttachmentCommitment {
            id: Uuid::parse_str("0198f80c-8d2f-7dc1-a369-90556a64f700").unwrap(),
            byte_length: "1234".into(),
            sha256: format!("0x{}", "ab".repeat(32)),
        });
        snapshot
    }

    fn address_for(snapshot: &CanonicalIssuanceSnapshot, salt: Salt) -> alloy_primitives::Address {
        predict_payment_address(
            FactoryAddress(snapshot.factory_address.parse().unwrap()),
            TokenAddress(snapshot.token_address.parse().unwrap()),
            Amount(U256::from_str_radix(&snapshot.amount_base_units, 10).unwrap()),
            BeneficiaryAddress(snapshot.receiver_address.parse().unwrap()),
            snapshot.expiration_timestamp.parse().unwrap(),
            RecoveryAddress(snapshot.recovery_address.parse().unwrap()),
            salt,
        )
        .0
    }

    #[test]
    fn jcs_is_stable_across_input_field_order() {
        // The same document written twice: struct order with pretty
        // whitespace, then reversed keys, an escaped character, and the
        // absent party fields spelled out as null.
        let natural = r#"{
            "schema": "payday.invoice", "canonicalization": "RFC8785",
            "issuer": {"name": "Acme Corp", "email": "billing@acme.example"},
            "bill_to": {"name": "Globex", "details": "1 Main St"},
            "amount_base_units": "1000000", "notes": "Thanks", "heading": null,
            "reference": "INV-1", "expiration_timestamp": "1900000000",
            "payer_policy": {"mode": "verified_email", "expected_email": "alice@example.com"},
            "attachment": {"id": "0198f80c-8d2f-7dc1-a369-90556a64f700", "byte_length": "1234",
                "sha256": "0xabababababababababababababababababababababababababababababababab"},
            "chain_id": "143",
            "token_address": "0x754704Bc059F8C67012fEd69BC8A327a5aafb603",
            "receiver_address": "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
            "recovery_address": "0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc",
            "factory_address": "0x5FbDB2315678afecb367f032d93F642f64180aa3"
        }"#;
        let reversed = r#"{"factory_address":"0x5FbDB2315678afecb367f032d93F642f64180aa3",
            "recovery_address":"0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc",
            "receiver_address":"0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
            "token_address":"0x754704Bc059F8C67012fEd69BC8A327a5aafb603","chain_id":"143",
            "attachment":{"sha256":"0xabababababababababababababababababababababababababababababababab",
                "byte_length":"1234","id":"0198f80c-8d2f-7dc1-a369-90556a64f700"},
            "payer_policy":{"expected_email":"alice@example.com","mode":"verified_email"},
            "expiration_timestamp":"1900000000","reference":"INV-1","heading":null,
            "notes":"\u0054hanks","amount_base_units":"1000000",
            "bill_to":{"details":"1 Main St","email":null,"name":"Globex"},
            "issuer":{"details":null,"email":"billing@acme.example","name":"Acme Corp"},
            "canonicalization":"RFC8785","schema":"payday.invoice"}"#;
        let a: CanonicalIssuanceSnapshot = serde_json::from_str(natural).unwrap();
        let b: CanonicalIssuanceSnapshot = serde_json::from_str(reversed).unwrap();
        assert_eq!(a, snapshot());
        assert_eq!(canonical_bytes(&a).unwrap(), canonical_bytes(&b).unwrap());
        assert_eq!(
            attribution_hash(&canonical_bytes(&a).unwrap()),
            attribution_hash(&canonical_bytes(&b).unwrap())
        );
    }

    #[test]
    fn canonical_form_and_attribution_hash_are_pinned() {
        // Hand-written RFC 8785 form: keys sorted, no whitespace, absent
        // optional party fields omitted, absent snapshot fields as null.
        let expected = concat!(
            r#"{"amount_base_units":"1000000","#,
            r#""attachment":{"byte_length":"1234","id":"0198f80c-8d2f-7dc1-a369-90556a64f700","#,
            r#""sha256":"0xabababababababababababababababababababababababababababababababab"},"#,
            r#""bill_to":{"details":"1 Main St","name":"Globex"},"#,
            r#""canonicalization":"RFC8785","chain_id":"143","expiration_timestamp":"1900000000","#,
            r#""factory_address":"0x5FbDB2315678afecb367f032d93F642f64180aa3","heading":null,"#,
            r#""issuer":{"email":"billing@acme.example","name":"Acme Corp"},"notes":"Thanks","#,
            r#""payer_policy":{"expected_email":"alice@example.com","mode":"verified_email"},"#,
            r#""receiver_address":"0x70997970C51812dc3A010C7d01b50e0d17dc79C8","#,
            r#""recovery_address":"0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc","#,
            r#""reference":"INV-1","schema":"payday.invoice","#,
            r#""token_address":"0x754704Bc059F8C67012fEd69BC8A327a5aafb603"}"#,
        );
        let bytes = canonical_bytes(&snapshot()).unwrap();
        assert_eq!(std::str::from_utf8(&bytes).unwrap(), expected);
        assert_eq!(
            attribution_hash(&bytes).to_string(),
            "0x8ae09bb2907133efa348bc0d4b62b04546232597b040a78f01c7761fc6ce8d73"
        );
        let nonce = B256::repeat_byte(0x11);
        assert_eq!(
            recompute_salt(nonce, attribution_hash(&bytes))
                .0
                .to_string(),
            "0xdd594e4e792bf8c6e893153d819d6c668dc9591ea753d649019443776c97951b"
        );
    }

    #[test]
    fn one_field_change_changes_attribution_hash_and_address() {
        let original = snapshot();
        let mut edited = snapshot();
        edited.notes = Some("Thanks!".into());
        let a = derive_attribution(&original).unwrap();
        let b = derive_attribution(&edited).unwrap();
        assert_ne!(a.attribution_hash, b.attribution_hash);
        // Same nonce, different content: the salt and address still move.
        let salt = recompute_salt(a.nonce, b.attribution_hash);
        assert_ne!(salt, a.salt);
        assert_ne!(address_for(&original, a.salt), address_for(&edited, salt));
    }

    #[test]
    fn identical_snapshots_receive_distinct_nonce_salt_and_address() {
        let snapshot = snapshot();
        let a = derive_attribution(&snapshot).unwrap();
        let b = derive_attribution(&snapshot).unwrap();
        assert_eq!(a.attribution_hash, b.attribution_hash);
        assert_eq!(a.canonical_bytes, b.canonical_bytes);
        assert_ne!(a.nonce, b.nonce);
        assert_ne!(a.salt, b.salt);
        assert_eq!(a.salt, recompute_salt(a.nonce, a.attribution_hash));
        assert_ne!(
            address_for(&snapshot, a.salt),
            address_for(&snapshot, b.salt)
        );
    }

    #[test]
    fn every_mode_round_trips_and_exposes_its_assertions() {
        let cases = [
            (
                serde_json::json!({"mode": "permissionless"}),
                PayerPolicy::Permissionless,
                PayerPolicyMode::Permissionless,
            ),
            (
                serde_json::json!({"mode": "verified_email", "expected_email": "alice@example.com"}),
                PayerPolicy::VerifiedEmail {
                    expected_email: "alice@example.com".into(),
                },
                PayerPolicyMode::VerifiedEmail,
            ),
        ];
        for (json, policy, mode) in cases {
            let parsed: PayerPolicy = serde_json::from_value(json.clone()).unwrap();
            assert_eq!(parsed, policy);
            assert_eq!(serde_json::to_value(&parsed).unwrap(), json);
            assert_eq!(parsed.mode(), mode);
            assert_eq!(mode.as_str().parse::<PayerPolicyMode>().unwrap(), mode);
            assert_eq!(mode.is_gated(), mode != PayerPolicyMode::Permissionless);
            assert_eq!(
                parsed.expected_email().is_some(),
                mode != PayerPolicyMode::Permissionless
            );
            parsed.validate().unwrap();
        }
    }

    #[test]
    fn assertions_are_only_accepted_where_the_mode_allows_them() {
        let identity = serde_json::json!({"first_name": "Alice", "last_name": "Smith"});
        for rejected in [
            serde_json::json!({"mode": "verified_email", "expected_email": "a@b.co", "expected_identity": identity}),
            serde_json::json!({"mode": "permissionless", "expected_email": "a@b.co"}),
            serde_json::json!({"mode": "verified_identity", "expected_email": "a@b.co", "expected_identity": identity}),
            serde_json::json!({"mode": "verified_identity_unattributed", "expected_email": "a@b.co"}),
            serde_json::json!({"mode": "verified_email"}),
            serde_json::json!({"mode": "kyc"}),
            serde_json::json!({}),
        ] {
            assert!(
                serde_json::from_value::<PayerPolicy>(rejected.clone()).is_err(),
                "{rejected}"
            );
        }
    }

    #[test]
    fn validation_rejects_bad_emails() {
        for email in ["not-an-email", "a@b", " alice@example.com", "a@.com", ""] {
            let policy = PayerPolicy::VerifiedEmail {
                expected_email: email.into(),
            };
            assert_eq!(
                policy.validate(),
                Err(PayerPolicyError::InvalidExpectedEmail),
                "{email:?}"
            );
        }
        let long = "x".repeat(250) + "@e.com";
        assert!(
            PayerPolicy::VerifiedEmail {
                expected_email: long
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn normalization_trims_and_lowercases_the_expected_email() {
        let policy = PayerPolicy::VerifiedEmail {
            expected_email: "  Alice@Example.COM ".into(),
        };
        assert_eq!(
            policy.validate(),
            Err(PayerPolicyError::InvalidExpectedEmail)
        );
        let normalized = policy.normalized();
        assert_eq!(normalized.expected_email(), Some("alice@example.com"));
        normalized.validate().unwrap();
        assert_eq!(
            PayerPolicy::Permissionless.normalized(),
            PayerPolicy::Permissionless
        );
    }

    #[test]
    fn party_rejects_unknown_fields_and_omits_absent_ones() {
        assert!(
            serde_json::from_value::<Party>(serde_json::json!({"name": "Acme", "tax_id": "1"}))
                .is_err()
        );
        assert_eq!(
            serde_json::to_value(party("Acme")).unwrap(),
            serde_json::json!({"name": "Acme"})
        );
    }
}
