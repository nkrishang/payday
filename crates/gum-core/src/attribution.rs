//! Verifiable attribution: the canonical issuance snapshot, its hash, and the
//! CREATE3 salt derived from it (product plan §5.2).
//!
//! ```text
//! canonical_bytes    = JCS(canonical_issuance_snapshot)          (RFC 8785)
//! attribution_hash   = keccak256("GUM_ATTRIBUTION_V5" || canonical_bytes)
//! salt               = keccak256("GUM_SALT_V5" || issuance_nonce || attribution_hash [+ attestation_digest])
//! ```
//!
//! The salt exists from the moment the payment address does. The issuance
//! nonce is a fresh 32-byte value generated at creation and kept undisclosed
//! until the address is registered, so nobody can derive and prefund an
//! address the service is not watching yet. When the request attaches wallet
//! attestation, the salt also commits to the EIP-712 digest of the payer's
//! attestation (see [`crate::PayerAttestation`]). The API never accepts a
//! client-supplied salt or nonce: [`recompute_salt`] over the stored nonce —
//! plus a verified attestation, when one is required — is the only way one
//! comes into existence.

use alloy_primitives::{B256, keccak256};
use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{Amount, BeneficiaryAddress, Currency, NetworkTerms, RecoveryAddress, Salt};

pub const ATTRIBUTION_VERSION: u16 = 5;
pub const ATTRIBUTION_DOMAIN: &[u8] = b"GUM_ATTRIBUTION_V5";
pub const SALT_DOMAIN: &[u8] = b"GUM_SALT_V5";
/// `CanonicalIssuanceSnapshot::schema`; a new schema means a new version.
/// v3 commits to the list of networks the request may be paid on instead of
/// one chain: the payer's attestation selects one of them. v4 names the
/// currency and its decimals. v5 replaces the exclusive `payer_policy` with
/// independent verification add-ons and commits to the payment contract's
/// recovery term, which is always Gum's own recovery wallet.
pub const SNAPSHOT_SCHEMA: &str = "gum.invoice.v5";
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

/// The three verification add-ons a deposit request can attach, independently
/// of one another. The default is none of them: a fully permissionless
/// request whose address exists as soon as its network is known and whose
/// deposits are welcome from any wallet.
///
/// `email` asks the payer to prove ownership of an expected mailbox; it is
/// the old `verified_email` mode. `merchant_auth` is the old
/// `merchant_session` mode: the merchant's own application has already
/// authenticated the payer, names them by `payer_reference` (its own user
/// id), and opens the hosted checkout for them with a single-use client
/// secret; Gum performs no check of its own. `wallet_attestation` asks the
/// payer to sign an EIP-712 attestation from the wallet they will pay from;
/// without it, deposits from any wallet are good.
///
/// The recovery term is deliberately absent: it is always Gum's own
/// recovery wallet, snapshotted into the issuance document, and never a
/// request field.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PayerVerification {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<EmailVerification>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merchant_auth: Option<MerchantAuth>,
    /// Serialized even when false, so the canonical form is stable. Response
    /// payloads omit the whole field when no add-on is attached.
    #[serde(default)]
    pub wallet_attestation: bool,
}

/// Prove ownership of exactly this mailbox with a one-time code.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EmailVerification {
    pub expected_email: String,
}

/// The merchant's own authentication: its application signed the payer in,
/// names them by `payer_reference`, and released the single-use client
/// secret that opens the checkout.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MerchantAuth {
    pub payer_reference: String,
}

impl<'de> Deserialize<'de> for PayerVerification {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// The wire shape, so the defaults above apply only at the field
        /// level and unknown fields are still refused.
        #[derive(Deserialize, Default)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            #[serde(default)]
            email: Option<EmailVerification>,
            #[serde(default)]
            merchant_auth: Option<MerchantAuth>,
            #[serde(default)]
            wallet_attestation: bool,
        }
        let wire = Wire::deserialize(deserializer)?;
        Ok(Self {
            email: wire.email,
            merchant_auth: wire.merchant_auth,
            wallet_attestation: wire.wallet_attestation,
        })
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

impl PayerVerification {
    /// Whether invoice content and payment mechanics are withheld from the
    /// payer until identity verification completes: any attached identity
    /// add-on gates the content, whether or not wallet attestation is
    /// attached.
    pub fn is_gated(&self) -> bool {
        self.email.is_some() || self.merchant_auth.is_some()
    }

    pub fn expected_email(&self) -> Option<&str> {
        self.email
            .as_ref()
            .map(|email| email.expected_email.as_str())
    }

    /// The merchant's own identifier for the authenticated payer, in the
    /// merchant-auth add-on only.
    pub fn payer_reference(&self) -> Option<&str> {
        self.merchant_auth
            .as_ref()
            .map(|auth| auth.payer_reference.as_str())
    }

    /// The add-ons attached, by name: the summary form used in list
    /// responses and webhook payloads.
    pub fn addons(&self) -> Vec<&'static str> {
        let mut addons = Vec::new();
        if self.email.is_some() {
            addons.push("email");
        }
        if self.merchant_auth.is_some() {
            addons.push("merchant_auth");
        }
        if self.wallet_attestation {
            addons.push("wallet_attestation");
        }
        addons
    }

    /// True when no add-on is attached: the permissionless default, which
    /// response payloads omit entirely.
    pub fn is_default(&self) -> bool {
        self.addons().is_empty()
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
        Self {
            email: self.email.as_ref().map(|email| EmailVerification {
                expected_email: email.expected_email.trim().to_lowercase(),
            }),
            merchant_auth: self.merchant_auth.as_ref().map(|auth| MerchantAuth {
                payer_reference: auth.payer_reference.trim().to_owned(),
            }),
            wallet_attestation: self.wallet_attestation,
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
/// The recovery address is always Gum's own recovery wallet: it is fixed
/// at issuance and committed here, never derived from a payer wallet. So is
/// the chain, when it is not pinned: the request commits to every network it
/// may be paid on, and the payer picks one — through a wallet attestation's
/// EIP-712 domain when wallet attestation is attached, or through the
/// payer's network-selection call when it is not.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CanonicalIssuanceSnapshot {
    pub schema: String,
    pub canonicalization: String,
    pub issuer: Party,
    pub bill_to: Party,
    /// The currency's wire code (`USDC`, `USDT`); every network below is
    /// that currency's contract on its chain.
    pub currency: String,
    /// Decimal: base units per whole unit, so `amount_base_units` reads
    /// without a registry.
    pub decimals: String,
    pub amount_base_units: String,
    pub notes: Option<String>,
    pub heading: Option<String>,
    pub reference: Option<String>,
    pub expiration_timestamp: String,
    pub payer_verification: PayerVerification,
    pub attachment: Option<AttachmentCommitment>,
    pub networks: Vec<SnapshotNetwork>,
    pub receiver_address: String,
    /// EIP-55 checksummed: Gum's recovery wallet, the payment contract's
    /// recovery term. Overpayments, expired balances, and late transfers go
    /// here, and Gum returns them to the payer manually.
    pub recovery_address: String,
}

impl CanonicalIssuanceSnapshot {
    /// A snapshot of the mandatory fields from typed payment parameters, with
    /// every optional field absent. This is the one place that renders chain
    /// parameters into their canonical string form, so
    /// [`crate::Invoice::issue`] can check a snapshot against the same rules.
    /// Networks are sorted by chain id whatever order they arrive in.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        issuer: Party,
        bill_to: Party,
        payer_verification: PayerVerification,
        currency: Currency,
        networks: &[NetworkTerms],
        receiver: BeneficiaryAddress,
        recovery: RecoveryAddress,
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
            currency: currency.code().into(),
            decimals: currency.decimals().to_string(),
            amount_base_units: amount.0.to_string(),
            notes: None,
            heading: None,
            reference: None,
            expiration_timestamp: expiration_timestamp.to_string(),
            payer_verification,
            attachment: None,
            networks: sorted.into_iter().map(SnapshotNetwork::from).collect(),
            receiver_address: receiver.0.to_checksum(None),
            recovery_address: recovery.0.to_checksum(None),
        }
    }

    /// The typed network terms this snapshot commits to, in canonical order.
    /// `None` when any entry is malformed: a snapshot is never edited after
    /// issuance, so that is a corrupted row, not a request.
    pub fn networks(&self) -> Option<Vec<NetworkTerms>> {
        self.networks.iter().map(SnapshotNetwork::terms).collect()
    }

    /// The typed currency, if the code is one this build knows. A snapshot
    /// naming another is a row from a newer build, not a request.
    pub fn currency(&self) -> Option<Currency> {
        self.currency.parse().ok()
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

/// `keccak256(SALT_DOMAIN || issuance_nonce || attribution_hash)` when the
/// request attaches no wallet attestation, and
/// `keccak256(SALT_DOMAIN || issuance_nonce || attribution_hash ||
/// attestation_digest)` when it does: what a verifier recomputes from a
/// proof, and what binding stores as the salt. The issuance nonce is the
/// fresh 32-byte value generated at creation, so identical requests land at
/// distinct addresses and nobody can derive an address before the service
/// registers it. `attestation_digest` is the EIP-712 signing hash of the
/// payer's verified wallet attestation, so the address also commits to the
/// exact statement the payer signed.
pub fn recompute_salt(
    issuance_nonce: B256,
    attribution_hash: B256,
    attestation_digest: Option<B256>,
) -> Salt {
    let mut input = Vec::with_capacity(4 * 32 + SALT_DOMAIN.len());
    input.extend_from_slice(SALT_DOMAIN);
    input.extend_from_slice(issuance_nonce.as_slice());
    input.extend_from_slice(attribution_hash.as_slice());
    if let Some(digest) = attestation_digest {
        input.extend_from_slice(digest.as_slice());
    }
    Salt(keccak256(input))
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
    const NONCE: B256 = B256::repeat_byte(0x22);

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

    pub(crate) fn snapshot() -> CanonicalIssuanceSnapshot {
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
            PayerVerification {
                email: Some(EmailVerification {
                    expected_email: "alice@example.com".into(),
                }),
                merchant_auth: None,
                wallet_attestation: false,
            },
            Currency::Usdc,
            &networks(),
            BeneficiaryAddress(address!("0x70997970C51812dc3A010C7d01b50e0d17dc79C8")),
            RecoveryAddress(WALLET),
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
            "schema": "gum.invoice.v5", "canonicalization": "RFC8785",
            "issuer": {"name": "Acme Corp", "email": "billing@acme.example"},
            "bill_to": {"name": "Globex", "details": "1 Main St"},
            "currency": "USDC", "decimals": "6",
            "amount_base_units": "1000000", "notes": "Thanks", "heading": null,
            "reference": "INV-1", "expiration_timestamp": "1900000000",
            "payer_verification": {"email": {"expected_email": "alice@example.com"}, "wallet_attestation": false},
            "attachment": {"id": "0198f80c-8d2f-7dc1-a369-90556a64f700", "byte_length": "1234",
                "sha256": "0xabababababababababababababababababababababababababababababababab"},
            "networks": [
                {"chain_id": "143", "token_address": "0x754704Bc059F8C67012fEd69BC8A327a5aafb603",
                 "factory_address": "0x5FbDB2315678afecb367f032d93F642f64180aa3"},
                {"chain_id": "8453", "token_address": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
                 "factory_address": "0x5FbDB2315678afecb367f032d93F642f64180aa3"}
            ],
            "receiver_address": "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
            "recovery_address": "0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc"
        }"#;
        let reversed = r#"{"recovery_address":"0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc",
            "receiver_address":"0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
            "networks":[
                {"factory_address":"0x5FbDB2315678afecb367f032d93F642f64180aa3","chain_id":"143",
                 "token_address":"0x754704Bc059F8C67012fEd69BC8A327a5aafb603"},
                {"token_address":"0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
                 "factory_address":"0x5FbDB2315678afecb367f032d93F642f64180aa3","chain_id":"8453"}],
            "attachment":{"sha256":"0xabababababababababababababababababababababababababababababababab",
                "byte_length":"1234","id":"0198f80c-8d2f-7dc1-a369-90556a64f700"},
            "payer_verification":{"wallet_attestation":false,
                "email":{"expected_email":"alice@example.com"}},
            "expiration_timestamp":"1900000000","reference":"INV-1","heading":null,
            "notes":"\u0054hanks","amount_base_units":"1000000","decimals":"6",
            "bill_to":{"details":"1 Main St","email":null,"name":"Globex"},
            "issuer":{"details":null,"email":"billing@acme.example","name":"Acme Corp"},
            "currency":"USDC",
            "canonicalization":"RFC8785","schema":"gum.invoice.v5"}"#;
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
            r#""canonicalization":"RFC8785","currency":"USDC","decimals":"6","#,
            r#""expiration_timestamp":"1900000000","heading":null,"#,
            r#""issuer":{"email":"billing@acme.example","name":"Acme Corp"},"#,
            r#""networks":[{"chain_id":"143","factory_address":"0x5FbDB2315678afecb367f032d93F642f64180aa3","#,
            r#""token_address":"0x754704Bc059F8C67012fEd69BC8A327a5aafb603"},"#,
            r#"{"chain_id":"8453","factory_address":"0x5FbDB2315678afecb367f032d93F642f64180aa3","#,
            r#""token_address":"0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"}],"notes":"Thanks","#,
            r#""payer_verification":{"email":{"expected_email":"alice@example.com"},"wallet_attestation":false},"#,
            r#""receiver_address":"0x70997970C51812dc3A010C7d01b50e0d17dc79C8","#,
            r#""recovery_address":"0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc","#,
            r#""reference":"INV-1","schema":"gum.invoice.v5"}"#,
        );
        let bytes = canonical_bytes(&snapshot()).unwrap();
        assert_eq!(std::str::from_utf8(&bytes).unwrap(), expected);
        assert_eq!(
            attribution_hash(&bytes).to_string(),
            "0x5c23e19f826b027f3e42e0ebea21635c6ba60cbf59d1a0caaf753b59471925ae"
        );
    }

    #[test]
    fn the_currency_is_part_of_the_document() {
        // Same amount, same networks, USDT instead of USDC: a different
        // request, a different hash, a different address.
        let usdc = snapshot();
        let mut usdt = snapshot();
        usdt.currency = "USDT".into();
        assert_eq!(usdc.currency(), Some(Currency::Usdc));
        assert_eq!(usdt.currency(), Some(Currency::Usdt));
        assert_ne!(
            derive_attribution(&usdc).unwrap().attribution_hash,
            derive_attribution(&usdt).unwrap().attribution_hash
        );
        let mut unknown = snapshot();
        unknown.currency = "AUSD".into();
        assert_eq!(unknown.currency(), None);
    }

    #[test]
    fn one_field_change_changes_attribution_hash_and_address() {
        let original = snapshot();
        let mut edited = snapshot();
        edited.notes = Some("Thanks!".into());
        let a = derive_attribution(&original).unwrap();
        let b = derive_attribution(&edited).unwrap();
        assert_ne!(a.attribution_hash, b.attribution_hash);
        // Same nonce and attestation digest, different content: the salt and
        // address still move.
        let digest = B256::repeat_byte(0x11);
        let salt_a = recompute_salt(NONCE, a.attribution_hash, Some(digest));
        let salt_b = recompute_salt(NONCE, b.attribution_hash, Some(digest));
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
    fn the_nonce_separates_identical_requests() {
        // Two requests with identical text hash identically; what separates
        // their addresses is the fresh issuance nonce each carries.
        let snapshot = snapshot();
        let a = derive_attribution(&snapshot).unwrap();
        let b = derive_attribution(&snapshot).unwrap();
        assert_eq!(a, b);
        let salt_a = recompute_salt(NONCE, a.attribution_hash, None);
        let salt_b = recompute_salt(B256::repeat_byte(0x23), a.attribution_hash, None);
        assert_ne!(salt_a, salt_b);
        assert_ne!(
            address_for(&snapshot, salt_a),
            address_for(&snapshot, salt_b)
        );
    }

    #[test]
    fn the_attestation_digest_branch_is_distinct_from_the_unsigned_one() {
        // The same request and nonce, with and without an attestation digest
        // in the salt: different branches, different addresses.
        let hash = derive_attribution(&snapshot()).unwrap().attribution_hash;
        let unsigned = recompute_salt(NONCE, hash, None);
        let attested = recompute_salt(NONCE, hash, Some(B256::repeat_byte(0x11)));
        assert_ne!(unsigned, attested);
        assert_ne!(
            address_for(&snapshot(), unsigned),
            address_for(&snapshot(), attested)
        );
    }

    #[test]
    fn every_add_on_combination_round_trips() {
        let cases = [
            (serde_json::json!({}), PayerVerification::default()),
            (
                serde_json::json!({"wallet_attestation": true}),
                PayerVerification {
                    wallet_attestation: true,
                    ..PayerVerification::default()
                },
            ),
            (
                serde_json::json!({"email": {"expected_email": "alice@example.com"}}),
                PayerVerification {
                    email: Some(EmailVerification {
                        expected_email: "alice@example.com".into(),
                    }),
                    ..PayerVerification::default()
                },
            ),
            (
                serde_json::json!({
                    "email": {"expected_email": "alice@example.com"},
                    "merchant_auth": {"payer_reference": "user_123"},
                    "wallet_attestation": true
                }),
                PayerVerification {
                    email: Some(EmailVerification {
                        expected_email: "alice@example.com".into(),
                    }),
                    merchant_auth: Some(MerchantAuth {
                        payer_reference: "user_123".into(),
                    }),
                    wallet_attestation: true,
                },
            ),
        ];
        for (json, verification) in &cases {
            let parsed: PayerVerification = serde_json::from_value(json.clone()).unwrap();
            assert_eq!(parsed, *verification);
            parsed.validate().unwrap();
        }
        // The full form round-trips verbatim; sparser forms serialize with
        // absent add-ons omitted.
        let full = cases[3].1.clone();
        assert_eq!(serde_json::to_value(&full).unwrap(), cases[3].0.clone());
        assert_eq!(
            PayerVerification::default().addons(),
            Vec::<&'static str>::new()
        );
        assert_eq!(
            full.addons(),
            vec!["email", "merchant_auth", "wallet_attestation"]
        );
    }

    #[test]
    fn add_ons_reject_unknown_fields_and_bad_values() {
        for rejected in [
            serde_json::json!({"mode": "permissionless"}),
            serde_json::json!({"expected_email": "a@b.co"}),
            serde_json::json!({"email": {"expected_email": "a@b.co", "extra": 1}}),
            serde_json::json!({"merchant_auth": {}}),
            serde_json::json!({"email": {"expected_email": "alice@example.com", "merchant_auth": {"payer_reference": "u"}}}),
        ] {
            assert!(
                serde_json::from_value::<PayerVerification>(rejected.clone()).is_err(),
                "{rejected}"
            );
        }
    }

    #[test]
    fn gating_is_any_identity_add_on() {
        assert!(!PayerVerification::default().is_gated());
        assert!(
            !PayerVerification {
                wallet_attestation: true,
                ..PayerVerification::default()
            }
            .is_gated()
        );
        assert!(
            PayerVerification {
                email: Some(EmailVerification {
                    expected_email: "a@b.co".into(),
                }),
                ..PayerVerification::default()
            }
            .is_gated()
        );
        assert!(
            PayerVerification {
                merchant_auth: Some(MerchantAuth {
                    payer_reference: "u".into(),
                }),
                wallet_attestation: true,
                ..PayerVerification::default()
            }
            .is_gated()
        );
    }

    #[test]
    fn validation_rejects_bad_emails() {
        for email in ["not-an-email", "a@b", " alice@example.com", "a@.com", ""] {
            let verification = PayerVerification {
                email: Some(EmailVerification {
                    expected_email: email.into(),
                }),
                ..PayerVerification::default()
            };
            assert_eq!(
                verification.validate(),
                Err(PayerPolicyError::InvalidExpectedEmail),
                "{email:?}"
            );
        }
        let long = "x".repeat(250) + "@e.com";
        assert!(
            PayerVerification {
                email: Some(EmailVerification {
                    expected_email: long
                }),
                ..PayerVerification::default()
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
            let verification = PayerVerification {
                merchant_auth: Some(MerchantAuth {
                    payer_reference: reference.to_owned(),
                }),
                ..PayerVerification::default()
            };
            assert_eq!(
                verification.validate(),
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
            PayerVerification {
                merchant_auth: Some(MerchantAuth {
                    payer_reference: reference.to_owned(),
                }),
                ..PayerVerification::default()
            }
            .validate()
            .unwrap_or_else(|error| panic!("{reference:?}: {error}"));
        }
        // Trimmed, never lowercased: the merchant's identifier is opaque.
        let normalized = PayerVerification {
            merchant_auth: Some(MerchantAuth {
                payer_reference: "  User_ABC ".into(),
            }),
            ..PayerVerification::default()
        }
        .normalized();
        assert_eq!(normalized.payer_reference(), Some("User_ABC"));
        normalized.validate().unwrap();
    }

    #[test]
    fn normalization_trims_and_lowercases_the_expected_email() {
        let verification = PayerVerification {
            email: Some(EmailVerification {
                expected_email: "  Alice@Example.COM ".into(),
            }),
            ..PayerVerification::default()
        };
        assert_eq!(
            verification.validate(),
            Err(PayerPolicyError::InvalidExpectedEmail)
        );
        let normalized = verification.normalized();
        assert_eq!(normalized.expected_email(), Some("alice@example.com"));
        normalized.validate().unwrap();
        assert_eq!(
            PayerVerification::default().normalized(),
            PayerVerification::default()
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
