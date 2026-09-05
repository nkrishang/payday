use std::fmt;
use std::str::FromStr;

use alloy_primitives::{Address, B256, U256};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    ATTRIBUTION_VERSION, Amount, AttributionError, BeneficiaryAddress, CANONICALIZATION,
    CanonicalIssuanceSnapshot, ChainId, FactoryAddress, PayerAttestationError,
    PayerAttestationScope, PayerWalletAttestation, PaymentAddress, RecoveryAddress,
    SNAPSHOT_SCHEMA, Salt, TokenAddress, derive_attribution, predict_payment_address,
    recompute_salt, verify_payer_attestation,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InvoiceId(pub Uuid);

impl fmt::Display for InvoiceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "dr_{}", self.0)
    }
}

/// Validate a complete merchant-facing `dr_…` deposit request ID and return the UUID used by
/// the account-scoped database lookup. Only the canonical form the API emits is
/// accepted: partial IDs and other UUID spellings are rejected so a lookup can
/// never resolve to more than one deposit request.
pub fn deposit_request_id(value: &str) -> Option<Uuid> {
    let suffix = value.strip_prefix("dr_")?;
    let uuid = Uuid::try_parse(suffix).ok()?;
    let mut canonical = [0u8; uuid::fmt::Hyphenated::LENGTH];
    (uuid.hyphenated().encode_lower(&mut canonical) == suffix).then_some(uuid)
}

pub fn generate_invoice_id() -> InvoiceId {
    InvoiceId(Uuid::now_v7())
}

/// Invoice lifecycle.
///
/// A `created` invoice has no payment address until a payer binds a wallet
/// (see [`Invoice::bind_payer_wallet`]); funds can only start arriving after
/// that, so the statuses below never depend on whether a binding exists.
///
/// ```text
/// created ──(finalized credit ≥ amount)──▶ funded ──(claimed)──▶ deploying ──▶ fulfilled
///    │                                       │                        │
///    └──(chain time > expiration)──▶ expired ◀┘                       └──▶ recovered
///                                       │                                    ▲
///                                       └──(balance swept after expiry)──────┘
/// ```
///
/// `blocked` is reached from `deploying` or `expired` when a sweep fails for
/// a reason the worker classifies as permanent. Funds arriving after
/// `fulfilled`/`recovered` are forwarded to the recovery address (the payer's
/// attested wallet) without changing the status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InvoiceStatus {
    Created,
    Funded,
    Deploying,
    Fulfilled,
    Recovered,
    Expired,
    Blocked,
}

/// Error returned when parsing an [`InvoiceStatus`] from its string form.
#[derive(Debug, Error, PartialEq, Eq)]
#[error("unknown invoice status: {0}")]
pub struct InvoiceStatusParseError(pub String);

impl InvoiceStatus {
    pub const ALL: [InvoiceStatus; 7] = [
        InvoiceStatus::Created,
        InvoiceStatus::Funded,
        InvoiceStatus::Deploying,
        InvoiceStatus::Fulfilled,
        InvoiceStatus::Recovered,
        InvoiceStatus::Expired,
        InvoiceStatus::Blocked,
    ];

    /// The canonical lowercase string form used on the wire and in the database.
    /// This is the single source of truth for the string mapping; [`Display`],
    /// [`FromStr`], and the DB CHECK constraint all agree with it.
    pub fn as_str(&self) -> &'static str {
        match self {
            InvoiceStatus::Created => "created",
            InvoiceStatus::Funded => "funded",
            InvoiceStatus::Deploying => "deploying",
            InvoiceStatus::Fulfilled => "fulfilled",
            InvoiceStatus::Recovered => "recovered",
            InvoiceStatus::Expired => "expired",
            InvoiceStatus::Blocked => "blocked",
        }
    }

    /// Whether the invoice's own payment has reached its final destination.
    /// Late transfers can still be collected from a terminal invoice.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            InvoiceStatus::Fulfilled | InvoiceStatus::Recovered | InvoiceStatus::Blocked
        )
    }
}

impl fmt::Display for InvoiceStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for InvoiceStatus {
    type Err = InvoiceStatusParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        InvoiceStatus::ALL
            .into_iter()
            .find(|status| status.as_str() == s)
            .ok_or_else(|| InvoiceStatusParseError(s.to_string()))
    }
}

/// The payer's wallet, attested in their session, and everything derived
/// from it: the recovery term, the salt, and the CREATE3 payment address.
/// Immutable once set; the address commits to all of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentBinding {
    pub payer_wallet: Address,
    pub attestation: PayerWalletAttestation,
    /// Always the payer's wallet: excess and late funds return to the payer.
    pub recovery: RecoveryAddress,
    pub salt: Salt,
    pub payment_address: PaymentAddress,
    /// RFC 3339, when the attestation was accepted.
    pub bound_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invoice {
    pub id: InvoiceId,
    pub chain_id: ChainId,
    pub token: TokenAddress,
    pub beneficiary: BeneficiaryAddress,
    pub expiration_timestamp: u64,
    pub factory: FactoryAddress,
    pub amount: Amount,
    /// `None` until a payer has attested a wallet; only then does the
    /// request have an address anyone can pay.
    pub binding: Option<PaymentBinding>,
    pub status: InvoiceStatus,
    /// Finalized transfers credited toward `amount`, in base units.
    pub received: Amount,
    /// Transaction that deployed the `Payment` contract, when this service sent it.
    pub execute_tx_hash: Option<B256>,
    /// Block at which the invoice reached `fulfilled` or `recovered`.
    pub resolved_at_block: Option<u64>,
    /// Timestamp of that finalized block.
    pub settled_at_timestamp: Option<u64>,
    /// Why automatic sweeping stopped for this invoice, if it did.
    pub blocked_reason: Option<String>,
    /// When the customer asked clients to stop using this invoice. This is
    /// advisory metadata and does not alter the immutable payment contract.
    pub cancellation_requested_at: Option<String>,
    /// Proof material (product plan §5): the commitment scheme version, the
    /// hash the salt is derived from, and the document that was hashed. None
    /// of it is a settlement dependency.
    pub attribution_version: u16,
    pub attribution_hash: B256,
    pub issuance_snapshot: CanonicalIssuanceSnapshot,
}

impl Invoice {
    /// Issue an invoice: commit to `snapshot` and hash it. The typed
    /// parameters are the ones the address will be derived from, so the
    /// snapshot must describe exactly those; otherwise a proof would verify
    /// against terms nobody was paid under. No address exists yet: see
    /// [`Invoice::bind_payer_wallet`].
    #[allow(clippy::too_many_arguments)]
    pub fn issue(
        factory: FactoryAddress,
        chain_id: ChainId,
        token: TokenAddress,
        beneficiary: BeneficiaryAddress,
        amount: Amount,
        expiration_timestamp: u64,
        snapshot: CanonicalIssuanceSnapshot,
    ) -> Result<Self, AttributionError> {
        let expected = CanonicalIssuanceSnapshot::new(
            snapshot.issuer.clone(),
            snapshot.bill_to.clone(),
            snapshot.payer_policy.clone(),
            factory,
            chain_id,
            token,
            beneficiary,
            amount,
            expiration_timestamp,
        );
        for (field, actual, wanted) in [
            ("schema", &snapshot.schema, SNAPSHOT_SCHEMA),
            (
                "canonicalization",
                &snapshot.canonicalization,
                CANONICALIZATION,
            ),
            ("chain_id", &snapshot.chain_id, &expected.chain_id),
            (
                "token_address",
                &snapshot.token_address,
                &expected.token_address,
            ),
            (
                "receiver_address",
                &snapshot.receiver_address,
                &expected.receiver_address,
            ),
            (
                "factory_address",
                &snapshot.factory_address,
                &expected.factory_address,
            ),
            (
                "amount_base_units",
                &snapshot.amount_base_units,
                &expected.amount_base_units,
            ),
            (
                "expiration_timestamp",
                &snapshot.expiration_timestamp,
                &expected.expiration_timestamp,
            ),
        ] {
            if actual != wanted {
                return Err(AttributionError::SnapshotMismatch { field });
            }
        }

        let attribution = derive_attribution(&snapshot)?;
        Ok(Invoice {
            id: generate_invoice_id(),
            chain_id,
            token,
            beneficiary,
            expiration_timestamp,
            factory,
            amount,
            binding: None,
            status: InvoiceStatus::Created,
            received: Amount(U256::ZERO),
            execute_tx_hash: None,
            resolved_at_block: None,
            settled_at_timestamp: None,
            blocked_reason: None,
            cancellation_requested_at: None,
            attribution_version: ATTRIBUTION_VERSION,
            attribution_hash: attribution.attribution_hash,
            issuance_snapshot: snapshot,
        })
    }

    /// What the payer's attestation must be for: this deployment and this
    /// request's commitment.
    pub fn attestation_scope(&self) -> PayerAttestationScope {
        PayerAttestationScope {
            chain_id: self.chain_id.0,
            factory: self.factory.0,
            attribution_hash: self.attribution_hash,
        }
    }

    /// Derive the binding a verified attestation produces: the payer's wallet
    /// is the recovery term, the salt commits to the attribution hash and the
    /// attestation digest, and the address follows from both. The attestation
    /// is verified here; a binding never exists for a signature that does
    /// not recover to its wallet.
    pub fn bind_payer_wallet(
        &self,
        attestation: PayerWalletAttestation,
        bound_at: String,
    ) -> Result<PaymentBinding, PayerAttestationError> {
        let verified = verify_payer_attestation(&attestation, self.attestation_scope())?;
        let recovery = RecoveryAddress(verified.wallet);
        let salt = recompute_salt(self.attribution_hash, verified.digest);
        let payment_address = predict_payment_address(
            self.factory,
            self.token,
            self.amount,
            self.beneficiary,
            self.expiration_timestamp,
            recovery,
            salt,
        );
        Ok(PaymentBinding {
            payer_wallet: verified.wallet,
            attestation,
            recovery,
            salt,
            payment_address,
            bound_at,
        })
    }

    /// The address payers were given, once a wallet is bound.
    pub fn payment_address(&self) -> Option<PaymentAddress> {
        self.binding.as_ref().map(|binding| binding.payment_address)
    }

    /// Recompute the counterfactual address from the stored parameters. A
    /// mismatch means the row no longer describes the address payers were
    /// given and must never be swept. An unbound invoice has no address to
    /// mismatch.
    pub fn address_matches_parameters(&self) -> bool {
        self.binding.as_ref().is_none_or(|binding| {
            predict_payment_address(
                self.factory,
                self.token,
                self.amount,
                self.beneficiary,
                self.expiration_timestamp,
                binding.recovery,
                binding.salt,
            ) == binding.payment_address
        })
    }
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::Duration;

    use super::*;

    #[test]
    fn public_payment_ids_round_trip_only_in_canonical_complete_form() {
        let uuid = Uuid::parse_str("0198f80c-8d2f-7dc1-a369-90556a64f700").unwrap();
        let id = InvoiceId(uuid);
        assert_eq!(id.to_string(), "dr_0198f80c-8d2f-7dc1-a369-90556a64f700");
        assert_eq!(deposit_request_id(&id.to_string()), Some(uuid));
        for invalid in [
            "",
            "dr_",
            "0198f80c-8d2f-7dc1-a369-90556a64f700",
            "dr_0198f80c",
            "dr_0198f80c-8d2f",
            "dr_0198F80C-8D2F-7DC1-A369-90556A64F700",
            "dr_0198f80c8d2f7dc1a36990556a64f700",
            "dr_{0198f80c-8d2f-7dc1-a369-90556a64f700}",
            "dr_%",
        ] {
            assert_eq!(deposit_request_id(invalid), None, "{invalid}");
        }
    }
    use uuid::Version;

    #[test]
    fn two_ids_differ() {
        let a = generate_invoice_id();
        let b = generate_invoice_id();

        assert_ne!(a, b);
    }

    #[test]
    fn ids_are_version_7() {
        let id = generate_invoice_id();
        assert_eq!(id.0.get_version(), Some(Version::SortRand));
    }

    #[test]
    fn ids_are_chronologically_sortable() {
        // The whole point of UUIDv7 over v4: later timestamps sort after
        // earlier ones. Sleep a few ms to guarantee a timestamp boundary.
        let a = generate_invoice_id();
        thread::sleep(Duration::from_millis(5));
        let b = generate_invoice_id();

        assert!(a.0 < b.0, "earlier ID should sort before later ID");
    }

    use crate::{
        Amount, BeneficiaryAddress, ChainId, FactoryAddress, Party, PayerAttestation, PayerPolicy,
        RecoveryAddress, TokenAddress, predict_payment_address, recompute_salt,
        sign_payer_attestation, wallet_of,
    };
    use alloy_primitives::{U256, address};

    const PAYER_KEY: [u8; 32] = [7u8; 32];

    fn party(name: &str) -> Party {
        Party {
            name: name.into(),
            email: None,
            details: None,
        }
    }

    /// Issue with a minimal permissionless snapshot built from the same terms.
    fn issue(
        factory: FactoryAddress,
        chain_id: ChainId,
        token: TokenAddress,
        beneficiary: BeneficiaryAddress,
        amount: Amount,
        expiration_timestamp: u64,
    ) -> Invoice {
        let snapshot = CanonicalIssuanceSnapshot::new(
            party("Acme"),
            party("Globex"),
            PayerPolicy::Permissionless,
            factory,
            chain_id,
            token,
            beneficiary,
            amount,
            expiration_timestamp,
        );
        Invoice::issue(
            factory,
            chain_id,
            token,
            beneficiary,
            amount,
            expiration_timestamp,
            snapshot,
        )
        .unwrap()
    }

    fn sample_invoice() -> Invoice {
        issue(
            FactoryAddress(address!("0x0000000000000000000000000000000000000001")),
            ChainId(1),
            TokenAddress(address!("0xA0b86a91E6Dc7c5bE5d7B8f9cC2D9eF1a3B4c5D6")),
            BeneficiaryAddress(address!("0x70997970C51812dc3A010C7d01b50e0d17dc7C01")),
            Amount(U256::from(100)),
            1_900_000_000,
        )
    }

    /// The attestation a payer's wallet would sign for `invoice` under a
    /// given session nonce.
    fn attest(invoice: &Invoice, key: &[u8; 32], nonce: u8) -> PayerWalletAttestation {
        let message = PayerAttestation::new(
            invoice.attribution_hash,
            wallet_of(key),
            B256::repeat_byte(nonce),
            invoice.expiration_timestamp,
        );
        sign_payer_attestation(key, &message, invoice.chain_id.0, invoice.factory.0)
    }

    fn bound_invoice() -> Invoice {
        let mut invoice = sample_invoice();
        let binding = invoice
            .bind_payer_wallet(
                attest(&invoice, &PAYER_KEY, 0x11),
                "2026-09-06T00:00:00Z".into(),
            )
            .unwrap();
        invoice.binding = Some(binding);
        invoice
    }

    #[test]
    fn status_str_roundtrips_for_all_variants() {
        for status in InvoiceStatus::ALL {
            let parsed = status.as_str().parse::<InvoiceStatus>().unwrap();
            assert_eq!(parsed, status);
            // Display and as_str agree.
            assert_eq!(status.to_string(), status.as_str());
        }
    }

    #[test]
    fn status_from_str_rejects_unknown_and_retired_values() {
        for retired in ["bogus", "failed", "FULFILLED"] {
            assert_eq!(
                retired.parse::<InvoiceStatus>().unwrap_err(),
                InvoiceStatusParseError(retired.to_string())
            );
        }
    }

    #[test]
    fn terminal_statuses_are_exactly_the_settled_and_given_up_states() {
        let terminal: Vec<_> = InvoiceStatus::ALL
            .into_iter()
            .filter(InvoiceStatus::is_terminal)
            .collect();
        assert_eq!(
            terminal,
            [
                InvoiceStatus::Fulfilled,
                InvoiceStatus::Recovered,
                InvoiceStatus::Blocked
            ]
        );
    }

    #[test]
    fn new_invoice_has_created_status_no_address_and_no_operational_state() {
        let invoice = sample_invoice();
        assert_eq!(invoice.status, InvoiceStatus::Created);
        assert_eq!(invoice.binding, None);
        assert_eq!(invoice.payment_address(), None);
        assert!(invoice.address_matches_parameters());
        assert_eq!(invoice.received, Amount(U256::ZERO));
        assert_eq!(invoice.execute_tx_hash, None);
        assert_eq!(invoice.resolved_at_block, None);
        assert_eq!(invoice.blocked_reason, None);
    }

    #[test]
    fn new_invoice_has_valid_uuidv7_id() {
        let invoice = sample_invoice();
        assert_eq!(invoice.id.0.get_version(), Some(Version::SortRand));
    }

    #[test]
    fn new_invoice_stores_all_input_fields() {
        let factory = FactoryAddress(address!("0x0000000000000000000000000000000000000001"));
        let chain_id = ChainId(1);
        let token = TokenAddress(address!("0xA0b86a91E6Dc7c5bE5d7B8f9cC2D9eF1a3B4c5D6"));
        let beneficiary =
            BeneficiaryAddress(address!("0x70997970C51812dc3A010C7d01b50e0d17dc7C01"));
        let amount = Amount(U256::from(100));
        let expiration_timestamp = 1_900_000_000;

        let invoice = issue(
            factory,
            chain_id,
            token,
            beneficiary,
            amount,
            expiration_timestamp,
        );

        assert_eq!(invoice.factory, factory);
        assert_eq!(invoice.chain_id, chain_id);
        assert_eq!(invoice.token, token);
        assert_eq!(invoice.beneficiary, beneficiary);
        assert_eq!(invoice.amount, amount);
        assert_eq!(invoice.expiration_timestamp, expiration_timestamp);
    }

    #[test]
    fn binding_derives_recovery_salt_and_address_from_the_attestation() {
        // The integration test that ties derive_attribution, the payer
        // attestation, recompute_salt, and predict_payment_address together.
        let invoice = bound_invoice();
        let binding = invoice.binding.as_ref().unwrap();
        assert_eq!(binding.payer_wallet, wallet_of(&PAYER_KEY));
        assert_eq!(binding.recovery, RecoveryAddress(wallet_of(&PAYER_KEY)));
        let digest: B256 = binding.attestation.digest.parse().unwrap();
        assert_eq!(
            binding.salt,
            recompute_salt(invoice.attribution_hash, digest)
        );
        let expected = predict_payment_address(
            invoice.factory,
            invoice.token,
            invoice.amount,
            invoice.beneficiary,
            invoice.expiration_timestamp,
            binding.recovery,
            binding.salt,
        );
        assert_eq!(binding.payment_address, expected);
        assert_eq!(invoice.payment_address(), Some(expected));
        assert!(invoice.address_matches_parameters());
    }

    #[test]
    fn binding_refuses_an_attestation_for_another_request_or_wallet() {
        let invoice = sample_invoice();
        let other = sample_invoice();
        assert_eq!(invoice.attribution_hash, other.attribution_hash);
        let mut foreign = issue(
            invoice.factory,
            invoice.chain_id,
            invoice.token,
            invoice.beneficiary,
            Amount(U256::from(101)),
            invoice.expiration_timestamp,
        );
        foreign.binding = None;
        assert_eq!(
            invoice
                .bind_payer_wallet(attest(&foreign, &PAYER_KEY, 0x11), String::new())
                .unwrap_err(),
            PayerAttestationError::AttributionHashMismatch
        );
        let mut forged = attest(&invoice, &PAYER_KEY, 0x11);
        forged.signature = attest(&invoice, &[9u8; 32], 0x11).signature;
        assert_eq!(
            invoice
                .bind_payer_wallet(forged, String::new())
                .unwrap_err(),
            PayerAttestationError::SignerMismatch
        );
    }

    #[test]
    fn tampered_parameters_no_longer_match_the_stored_address() {
        let mut invoice = bound_invoice();
        invoice.beneficiary =
            BeneficiaryAddress(address!("0x0000000000000000000000000000000000000009"));
        assert!(!invoice.address_matches_parameters());
    }

    #[test]
    fn same_request_and_wallet_with_different_nonces_get_different_addresses() {
        // Identical terms hash identically; the session nonce inside the
        // attestation is what keeps their addresses apart, and a payer who
        // signs twice for the same request produces two distinct salts.
        let invoice = sample_invoice();
        let a = invoice
            .bind_payer_wallet(attest(&invoice, &PAYER_KEY, 0x11), String::new())
            .unwrap();
        let b = invoice
            .bind_payer_wallet(attest(&invoice, &PAYER_KEY, 0x12), String::new())
            .unwrap();
        assert_ne!(a.salt, b.salt, "salts must differ");
        assert_ne!(
            a.payment_address, b.payment_address,
            "addresses must differ"
        );
        // A different wallet moves both the salt (through the digest) and
        // the recovery term.
        let c = invoice
            .bind_payer_wallet(attest(&invoice, &[9u8; 32], 0x11), String::new())
            .unwrap();
        assert_ne!(a.salt, c.salt);
        assert_ne!(a.recovery, c.recovery);
        assert_ne!(a.payment_address, c.payment_address);
    }

    #[test]
    fn expiration_and_recovery_are_address_parameters() {
        let invoice = bound_invoice();
        let binding = invoice.binding.as_ref().unwrap();
        let later_expiration = predict_payment_address(
            invoice.factory,
            invoice.token,
            invoice.amount,
            invoice.beneficiary,
            invoice.expiration_timestamp + 1,
            binding.recovery,
            binding.salt,
        );
        let other_recovery = predict_payment_address(
            invoice.factory,
            invoice.token,
            invoice.amount,
            invoice.beneficiary,
            invoice.expiration_timestamp,
            RecoveryAddress(address!("0x0000000000000000000000000000000000000003")),
            binding.salt,
        );

        assert_ne!(binding.payment_address, later_expiration);
        assert_ne!(binding.payment_address, other_recovery);
    }

    #[test]
    fn issued_invoice_commits_to_its_snapshot() {
        let invoice = sample_invoice();
        assert_eq!(invoice.attribution_version, ATTRIBUTION_VERSION);
        assert_eq!(
            invoice.attribution_hash,
            derive_attribution(&invoice.issuance_snapshot)
                .unwrap()
                .attribution_hash
        );
        assert_eq!(invoice.issuance_snapshot.amount_base_units, "100");
        assert_eq!(invoice.issuance_snapshot.chain_id, "1");
        assert_eq!(
            invoice.issuance_snapshot.receiver_address,
            invoice.beneficiary.0.to_checksum(None)
        );
    }

    #[test]
    fn issue_refuses_a_snapshot_that_describes_other_terms() {
        let factory = FactoryAddress(address!("0x0000000000000000000000000000000000000001"));
        let chain_id = ChainId(1);
        let token = TokenAddress(address!("0xA0b86a91E6Dc7c5bE5d7B8f9cC2D9eF1a3B4c5D6"));
        let beneficiary =
            BeneficiaryAddress(address!("0x70997970C51812dc3A010C7d01b50e0d17dc7C01"));
        let amount = Amount(U256::from(100));
        let snapshot = || {
            CanonicalIssuanceSnapshot::new(
                party("Acme"),
                party("Globex"),
                PayerPolicy::Permissionless,
                factory,
                chain_id,
                token,
                beneficiary,
                amount,
                1_900_000_000,
            )
        };
        let mismatches: Vec<(&str, CanonicalIssuanceSnapshot)> = vec![
            ("amount_base_units", {
                let mut s = snapshot();
                s.amount_base_units = "101".into();
                s
            }),
            ("expiration_timestamp", {
                let mut s = snapshot();
                s.expiration_timestamp = "1900000001".into();
                s
            }),
            ("receiver_address", {
                let mut s = snapshot();
                s.receiver_address = s.factory_address.clone();
                s
            }),
            ("token_address", {
                let mut s = snapshot();
                s.token_address = s.token_address.to_lowercase();
                s
            }),
            ("schema", {
                let mut s = snapshot();
                s.schema = "payday.invoice.v2".into();
                s
            }),
        ];
        for (field, snapshot) in mismatches {
            let error = Invoice::issue(
                factory,
                chain_id,
                token,
                beneficiary,
                amount,
                1_900_000_000,
                snapshot,
            )
            .unwrap_err();
            assert!(
                matches!(error, AttributionError::SnapshotMismatch { field: f } if f == field),
                "{field}: {error}"
            );
        }
    }

    #[test]
    fn address_matches_parameters_uses_stored_salt_only() {
        // Attribution metadata is proof material. Losing or corrupting it
        // must never make a funded address look unsafe to sweep.
        let mut invoice = bound_invoice();
        invoice.attribution_hash = B256::repeat_byte(0xFF);
        invoice.issuance_snapshot.notes = Some("rewritten after the fact".into());
        invoice.issuance_snapshot.amount_base_units = "999".into();
        invoice.attribution_version = 7;
        assert!(invoice.address_matches_parameters());

        invoice.binding.as_mut().unwrap().salt = Salt(B256::repeat_byte(0x01));
        assert!(!invoice.address_matches_parameters());
    }
}
