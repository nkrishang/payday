//! Verifiable attribution: the canonical issuance snapshot, its hash, and the
//! CREATE3 salt derived from it (product plan §5.2).
//!
//! ```text
//! canonical_bytes    = JCS(canonical_issuance_snapshot)          (RFC 8785)
//! attribution_hash   = keccak256("PAYDAY_ATTRIBUTION_V3" || canonical_bytes)
//! attestation_digest = EIP-712 signing hash of the payer's PayerAttestation
//! salt               = keccak256("PAYDAY_SALT_V3" || attribution_hash || attestation_digest)
//! ```
//!
//! The salt exists only once a payer has attested a wallet for the request
//! (see [`crate::PayerAttestation`]). The attestation carries a one-time
//! nonce issued to the payer's session, so identical requests land at
//! distinct addresses and nobody can enumerate addresses from guessable
//! request contents. The API never accepts a client-supplied salt:
//! [`recompute_salt`] over a verified attestation is the only way one comes
//! into existence.

use std::fmt;
use std::str::FromStr;

use alloy_primitives::{B256, keccak256};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{Amount, BeneficiaryAddress, NetworkTerms, Salt};

pub const ATTRIBUTION_VERSION: u16 = 3;
pub const ATTRIBUTION_DOMAIN: &[u8] = b"PAYDAY_ATTRIBUTION_V3";
pub const SALT_DOMAIN: &[u8] = b"PAYDAY_SALT_V3";
/// `CanonicalIssuanceSnapshot::schema`; a new schema means a new version.
/// v3 commits to the list of networks the request may be paid on instead of
/// one chain: the payer's attestation selects one of them.
pub const SNAPSHOT_SCHEMA: &str = "payday.invoice.v3";
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

/// The three public payer modes (product plan §3.2). Deserialization enforces
/// the mode rules of §4.5 (each assertion is required exactly where its mode
/// needs it and forbidden elsewhere), so an unrepresentable policy can never
/// reach validation.
///
/// `merchant_session` is the API-first mode: the merchant's own application
/// has already authenticated the payer, names them by `payer_reference` (its
/// own user id), and opens the hosted checkout for them with a single-use
/// client secret. Payday performs no check of its own; it records that the
/// merchant's server released the secret and binds the session to it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum PayerPolicy {
    Permissionless,
    VerifiedEmail { expected_email: String },
    MerchantSession { payer_reference: String },
}

/// The wire shape of every mode. A derived internally tagged enum would let a
/// unit variant swallow extra fields, so the mode rules are applied by hand.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PayerPolicyWire {
    mode: PayerPolicyMode,
    #[serde(default)]
    expected_email: Option<String>,
    #[serde(default)]
    payer_reference: Option<String>,
}

impl<'de> Deserialize<'de> for PayerPolicy {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = PayerPolicyWire::deserialize(deserializer)?;
        let mode = wire.mode;
        if wire.expected_email.is_some() && mode != PayerPolicyMode::VerifiedEmail {
            return Err(D::Error::custom(format!(
                "expected_email is not allowed for {mode}"
            )));
        }
        if wire.payer_reference.is_some() && mode != PayerPolicyMode::MerchantSession {
            return Err(D::Error::custom(format!(
                "payer_reference is not allowed for {mode}"
            )));
        }
        match mode {
            PayerPolicyMode::Permissionless => Ok(PayerPolicy::Permissionless),
            PayerPolicyMode::VerifiedEmail => wire
                .expected_email
                .map(|expected_email| PayerPolicy::VerifiedEmail { expected_email })
                .ok_or_else(|| D::Error::custom("expected_email is required for verified_email")),
            PayerPolicyMode::MerchantSession => wire
                .payer_reference
                .map(|payer_reference| PayerPolicy::MerchantSession { payer_reference })
                .ok_or_else(|| {
                    D::Error::custom("payer_reference is required for merchant_session")
                }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PayerPolicyMode {
    Permissionless,
    VerifiedEmail,
    MerchantSession,
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("unknown payer policy mode: {0}")]
pub struct PayerPolicyModeParseError(pub String);

impl PayerPolicyMode {
    pub const ALL: [PayerPolicyMode; 3] = [
        PayerPolicyMode::Permissionless,
        PayerPolicyMode::VerifiedEmail,
        PayerPolicyMode::MerchantSession,
    ];

    /// The canonical string used on the wire and in `invoices.payer_policy_mode`.
    /// This is the single source of truth; the DB CHECK constraint agrees with it.
    pub fn as_str(self) -> &'static str {
        match self {
            PayerPolicyMode::Permissionless => "permissionless",
            PayerPolicyMode::VerifiedEmail => "verified_email",
            PayerPolicyMode::MerchantSession => "merchant_session",
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

/// The longest `payer_reference` accepted: room for any vendor's user id or a
/// compound key, never for free text.
pub const MAX_PAYER_REFERENCE_BYTES: usize = 128;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PayerPolicyError {
    #[error("expected_email must be a valid email address of at most 254 bytes")]
    InvalidExpectedEmail,
    #[error(
        "payer_reference must be 1 to 128 bytes of printable text with no whitespace or control characters"
    )]
    InvalidPayerReference,
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

/// The `payer_reference` rule: an opaque identifier, so it is bounded and must
/// be a single printable token. Case is preserved; identifiers are often
/// case-sensitive on the merchant's side.
pub fn valid_payer_reference(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_PAYER_REFERENCE_BYTES
        && !value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
}

impl PayerPolicy {
    pub fn mode(&self) -> PayerPolicyMode {
        match self {
            PayerPolicy::Permissionless => PayerPolicyMode::Permissionless,
            PayerPolicy::VerifiedEmail { .. } => PayerPolicyMode::VerifiedEmail,
            PayerPolicy::MerchantSession { .. } => PayerPolicyMode::MerchantSession,
        }
    }

    pub fn expected_email(&self) -> Option<&str> {
        match self {
            PayerPolicy::VerifiedEmail { expected_email } => Some(expected_email),
            PayerPolicy::Permissionless | PayerPolicy::MerchantSession { .. } => None,
        }
    }

    /// The merchant's own identifier for the authenticated payer, in the
    /// merchant-session mode only.
    pub fn payer_reference(&self) -> Option<&str> {
        match self {
            PayerPolicy::MerchantSession { payer_reference } => Some(payer_reference),
            PayerPolicy::Permissionless | PayerPolicy::VerifiedEmail { .. } => None,
        }
    }

    pub fn validate(&self) -> Result<(), PayerPolicyError> {
        if self
            .expected_email()
            .is_some_and(|email| !valid_email(email))
        {
            return Err(PayerPolicyError::InvalidExpectedEmail);
        }
        if self
            .payer_reference()
            .is_some_and(|reference| !valid_payer_reference(reference))
        {
            return Err(PayerPolicyError::InvalidPayerReference);
        }
        Ok(())
    }

    /// The form that is stored and committed: the expected email trimmed and
    /// lowercased so a retry that only differs in case is the same request,
    /// and the payer reference trimmed but otherwise as the merchant wrote it.
    pub fn normalized(&self) -> Self {
        match self {
            PayerPolicy::Permissionless => PayerPolicy::Permissionless,
            PayerPolicy::VerifiedEmail { expected_email } => PayerPolicy::VerifiedEmail {
                expected_email: expected_email.trim().to_lowercase(),
            },
            PayerPolicy::MerchantSession { payer_reference } => PayerPolicy::MerchantSession {
                payer_reference: payer_reference.trim().to_owned(),
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

/// One network a request may be paid on, in canonical string form: decimal
/// chain id, EIP-55 token and factory. The list is ordered by chain id.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SnapshotNetwork {
    pub chain_id: String,
    pub token_address: String,
    pub factory_address: String,
}

impl From<&NetworkTerms> for SnapshotNetwork {
    fn from(network: &NetworkTerms) -> Self {
        Self {
            chain_id: network.chain_id.0.to_string(),
            token_address: network.token.0.to_checksum(None),
            factory_address: network.factory.0.to_checksum(None),
        }
    }
}

impl SnapshotNetwork {
    /// The typed terms, if every field parses.
    pub fn terms(&self) -> Option<NetworkTerms> {
        Some(NetworkTerms {
            chain_id: crate::ChainId(self.chain_id.parse().ok()?),
            token: self.token_address.parse().ok()?,
            factory: self.factory_address.parse().ok()?,
        })
    }
}

/// Everything an issued invoice commits to. Numbers are decimal strings and
/// addresses are EIP-55 checksummed so the canonical form is unambiguous.
///
/// The recovery address is deliberately absent: it is the payer's attested
/// wallet, known only after issuance, and it enters the payment address
/// through the attestation the salt is derived from rather than through
/// this document. So is the chain: the request commits to every network it
/// may be paid on, and the attestation's EIP-712 domain names the one the
/// payer chose.
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
    pub networks: Vec<SnapshotNetwork>,
    pub receiver_address: String,
}

impl CanonicalIssuanceSnapshot {
    /// A snapshot of the mandatory fields from typed payment parameters, with
    /// every optional field absent. This is the one place that renders chain
    /// parameters into their canonical string form, so
    /// [`crate::Invoice::issue`] can check a snapshot against the same rules.
    /// Networks are sorted by chain id whatever order they arrive in.
    pub fn new(
        issuer: Party,
        bill_to: Party,
        payer_policy: PayerPolicy,
        networks: &[NetworkTerms],
        receiver: BeneficiaryAddress,
        amount: Amount,
        expiration_timestamp: u64,
    ) -> Self {
        let mut sorted: Vec<&NetworkTerms> = networks.iter().collect();
        sorted.sort_by_key(|network| network.chain_id);
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
            networks: sorted.into_iter().map(SnapshotNetwork::from).collect(),
            receiver_address: receiver.0.to_checksum(None),
        }
    }

    /// The typed network terms this snapshot commits to, in canonical order.
    /// `None` when any entry is malformed: a snapshot is never edited after
    /// issuance, so that is a corrupted row, not a request.
    pub fn networks(&self) -> Option<Vec<NetworkTerms>> {
        self.networks.iter().map(SnapshotNetwork::terms).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributionMaterial {
    pub canonical_bytes: Vec<u8>,
    pub attribution_hash: B256,
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

/// `keccak256(SALT_DOMAIN || attribution_hash || attestation_digest)`: what a
/// verifier recomputes from a proof, and what binding stores as the salt.
/// `attestation_digest` is the EIP-712 signing hash of the payer's verified
/// wallet attestation, so the address commits to the request text and to the
/// exact statement the payer signed.
pub fn recompute_salt(attribution_hash: B256, attestation_digest: B256) -> Salt {
    Salt(keccak256(
        [
            SALT_DOMAIN,
            attribution_hash.as_slice(),
            attestation_digest.as_slice(),
        ]
        .concat(),
    ))
}

/// Canonicalize and hash a snapshot.
pub fn derive_attribution(
    snapshot: &CanonicalIssuanceSnapshot,
) -> Result<AttributionMaterial, AttributionError> {
    let canonical_bytes = canonical_bytes(snapshot)?;
    let attribution_hash = attribution_hash(&canonical_bytes);
    Ok(AttributionMaterial {
        canonical_bytes,
        attribution_hash,
    })
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{Address, U256, address};

    use super::*;
    use crate::{ChainId, FactoryAddress, RecoveryAddress, TokenAddress, predict_payment_address};

    const WALLET: Address = address!("0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc");

    fn networks() -> Vec<NetworkTerms> {
        vec![
            NetworkTerms {
                chain_id: ChainId(8453),
                token: TokenAddress(address!("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913")),
                factory: FactoryAddress(address!("0x5FbDB2315678afecb367f032d93F642f64180aa3")),
            },
            NetworkTerms {
                chain_id: ChainId(143),
                token: TokenAddress(address!("0x754704Bc059F8C67012fEd69BC8A327a5aafb603")),
                factory: FactoryAddress(address!("0x5FbDB2315678afecb367f032d93F642f64180aa3")),
            },
        ]
    }

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
            &networks(),
            BeneficiaryAddress(address!("0x70997970C51812dc3A010C7d01b50e0d17dc79C8")),
            Amount(U256::from(1_000_000)),
            1_900_000_000,
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
        let network = snapshot.networks().unwrap()[0];
        predict_payment_address(
            network.factory,
            network.token,
            Amount(U256::from_str_radix(&snapshot.amount_base_units, 10).unwrap()),
            BeneficiaryAddress(snapshot.receiver_address.parse().unwrap()),
            snapshot.expiration_timestamp.parse().unwrap(),
            RecoveryAddress(WALLET),
            salt,
            network.chain_id,
        )
        .0
    }

    #[test]
    fn jcs_is_stable_across_input_field_order() {
        // The same document written twice: struct order with pretty
        // whitespace, then reversed keys, an escaped character, and the
        // absent party fields spelled out as null.
        let natural = r#"{
            "schema": "payday.invoice.v3", "canonicalization": "RFC8785",
            "issuer": {"name": "Acme Corp", "email": "billing@acme.example"},
            "bill_to": {"name": "Globex", "details": "1 Main St"},
            "amount_base_units": "1000000", "notes": "Thanks", "heading": null,
            "reference": "INV-1", "expiration_timestamp": "1900000000",
            "payer_policy": {"mode": "verified_email", "expected_email": "alice@example.com"},
            "attachment": {"id": "0198f80c-8d2f-7dc1-a369-90556a64f700", "byte_length": "1234",
                "sha256": "0xabababababababababababababababababababababababababababababababab"},
            "networks": [
                {"chain_id": "143", "token_address": "0x754704Bc059F8C67012fEd69BC8A327a5aafb603",
                 "factory_address": "0x5FbDB2315678afecb367f032d93F642f64180aa3"},
                {"chain_id": "8453", "token_address": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
                 "factory_address": "0x5FbDB2315678afecb367f032d93F642f64180aa3"}
            ],
            "receiver_address": "0x70997970C51812dc3A010C7d01b50e0d17dc79C8"
        }"#;
        let reversed = r#"{"receiver_address":"0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
            "networks":[
                {"factory_address":"0x5FbDB2315678afecb367f032d93F642f64180aa3","chain_id":"143",
                 "token_address":"0x754704Bc059F8C67012fEd69BC8A327a5aafb603"},
                {"token_address":"0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
                 "factory_address":"0x5FbDB2315678afecb367f032d93F642f64180aa3","chain_id":"8453"}],
            "attachment":{"sha256":"0xabababababababababababababababababababababababababababababababab",
                "byte_length":"1234","id":"0198f80c-8d2f-7dc1-a369-90556a64f700"},
            "payer_policy":{"expected_email":"alice@example.com","mode":"verified_email"},
            "expiration_timestamp":"1900000000","reference":"INV-1","heading":null,
            "notes":"\u0054hanks","amount_base_units":"1000000",
            "bill_to":{"details":"1 Main St","email":null,"name":"Globex"},
            "issuer":{"details":null,"email":"billing@acme.example","name":"Acme Corp"},
            "canonicalization":"RFC8785","schema":"payday.invoice.v3"}"#;
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
            r#""canonicalization":"RFC8785","expiration_timestamp":"1900000000","heading":null,"#,
            r#""issuer":{"email":"billing@acme.example","name":"Acme Corp"},"#,
            r#""networks":[{"chain_id":"143","factory_address":"0x5FbDB2315678afecb367f032d93F642f64180aa3","#,
            r#""token_address":"0x754704Bc059F8C67012fEd69BC8A327a5aafb603"},"#,
            r#"{"chain_id":"8453","factory_address":"0x5FbDB2315678afecb367f032d93F642f64180aa3","#,
            r#""token_address":"0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"}],"notes":"Thanks","#,
            r#""payer_policy":{"expected_email":"alice@example.com","mode":"verified_email"},"#,
            r#""receiver_address":"0x70997970C51812dc3A010C7d01b50e0d17dc79C8","#,
            r#""reference":"INV-1","schema":"payday.invoice.v3"}"#,
        );
        let bytes = canonical_bytes(&snapshot()).unwrap();
        assert_eq!(std::str::from_utf8(&bytes).unwrap(), expected);
        assert_eq!(
            attribution_hash(&bytes).to_string(),
            "0x6f71820539f6876e12aa81e36e09d9d5b8d0c87c03c73ba82a464765a9f82235"
        );
        let attestation_digest = B256::repeat_byte(0x11);
        assert_eq!(
            recompute_salt(attribution_hash(&bytes), attestation_digest)
                .0
                .to_string(),
            "0x2ad64ccc40637d28f2b9a0b4c800254ef46970b20156e444f935900c7689b7ff"
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
        // Same attestation digest, different content: the salt and address
        // still move.
        let digest = B256::repeat_byte(0x11);
        let salt_a = recompute_salt(a.attribution_hash, digest);
        let salt_b = recompute_salt(b.attribution_hash, digest);
        assert_ne!(salt_a, salt_b);
        assert_ne!(address_for(&original, salt_a), address_for(&edited, salt_b));
    }

    #[test]
    fn networks_are_canonicalized_by_chain_id_and_round_trip_as_terms() {
        // `networks()` above lists Base before Monad; the snapshot sorts.
        let snapshot = snapshot();
        assert_eq!(snapshot.networks[0].chain_id, "143");
        assert_eq!(snapshot.networks[1].chain_id, "8453");
        let mut terms = networks();
        terms.sort_by_key(|network| network.chain_id);
        assert_eq!(snapshot.networks().unwrap(), terms);
        let mut corrupt = snapshot.clone();
        corrupt.networks[0].chain_id = "monad".into();
        assert_eq!(corrupt.networks(), None);
    }

    #[test]
    fn identical_snapshots_differ_by_attestation_digest_only() {
        // Two requests with identical text hash identically; what separates
        // their addresses is the attestation each payer signs, whose nonce is
        // fresh per session.
        let snapshot = snapshot();
        let a = derive_attribution(&snapshot).unwrap();
        let b = derive_attribution(&snapshot).unwrap();
        assert_eq!(a, b);
        let salt_a = recompute_salt(a.attribution_hash, B256::repeat_byte(0x11));
        let salt_b = recompute_salt(a.attribution_hash, B256::repeat_byte(0x12));
        assert_ne!(salt_a, salt_b);
        assert_ne!(
            address_for(&snapshot, salt_a),
            address_for(&snapshot, salt_b)
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
            (
                serde_json::json!({"mode": "merchant_session", "payer_reference": "user_123"}),
                PayerPolicy::MerchantSession {
                    payer_reference: "user_123".into(),
                },
                PayerPolicyMode::MerchantSession,
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
                mode == PayerPolicyMode::VerifiedEmail
            );
            assert_eq!(
                parsed.payer_reference().is_some(),
                mode == PayerPolicyMode::MerchantSession
            );
            parsed.validate().unwrap();
        }
        assert_eq!(PayerPolicyMode::ALL.len(), 3);
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
            serde_json::json!({"mode": "merchant_session"}),
            serde_json::json!({"mode": "merchant_session", "expected_email": "a@b.co"}),
            serde_json::json!({"mode": "merchant_session", "payer_reference": "u1", "expected_email": "a@b.co"}),
            serde_json::json!({"mode": "permissionless", "payer_reference": "u1"}),
            serde_json::json!({"mode": "verified_email", "expected_email": "a@b.co", "payer_reference": "u1"}),
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
    fn validation_bounds_the_payer_reference_to_one_printable_token() {
        for reference in [
            "",
            " ",
            "user 123",
            "user\n1",
            "user\u{0}",
            &"x".repeat(129),
        ] {
            let policy = PayerPolicy::MerchantSession {
                payer_reference: reference.to_owned(),
            };
            assert_eq!(
                policy.validate(),
                Err(PayerPolicyError::InvalidPayerReference),
                "{reference:?}"
            );
        }
        for reference in [
            "user_123",
            "a",
            "cus_9f8E|tenant:7",
            &"x".repeat(128),
            "ürsula",
        ] {
            PayerPolicy::MerchantSession {
                payer_reference: reference.to_owned(),
            }
            .validate()
            .unwrap_or_else(|error| panic!("{reference:?}: {error}"));
        }
        // Trimmed, never lowercased: the merchant's identifier is opaque.
        let normalized = PayerPolicy::MerchantSession {
            payer_reference: "  User_ABC ".into(),
        }
        .normalized();
        assert_eq!(normalized.payer_reference(), Some("User_ABC"));
        normalized.validate().unwrap();
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
