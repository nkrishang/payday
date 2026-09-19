use std::fmt;
use std::str::FromStr;

use alloy_primitives::{Address, B256, U256};
use rand::Rng;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    ATTRIBUTION_VERSION, Amount, AttributionError, BeneficiaryAddress, CANONICALIZATION,
    CanonicalIssuanceSnapshot, ChainId, Currency, NetworkTerms, PayerAttestationError,
    PayerAttestationScope, PayerWalletAttestation, PaymentAddress, RecoveryAddress,
    SNAPSHOT_SCHEMA, Salt, derive_attribution, predict_payment_address, recompute_salt,
    verify_payer_attestation,
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
    crate::parse_prefixed("dr_", value)
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
/// created ──(finalized credit ≥ amount)──▶ funded ──(sweep settles)──▶ fulfilled
///    │                                       │
///    └──(chain time > expiration)──▶ expired ◀┘
///                                       │
///                                       └──(balance swept after expiry)──▶ recovered
/// ```
///
/// Sweep membership (`sweep_job_id`) and operator attention
/// (`attention_reason`) are orthogonal to the status: a `funded` request in
/// a sweep job stays `funded`, and a request that needs an operator keeps
/// the lifecycle status it had while `attention_reason` explains why
/// automation stopped. Funds arriving after `fulfilled`/`recovered` are
/// forwarded to the recovery address (the payer's attested wallet) without
/// changing the status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InvoiceStatus {
    Created,
    Funded,
    Fulfilled,
    Recovered,
    Expired,
}

/// Error returned when parsing an [`InvoiceStatus`] from its string form.
#[derive(Debug, Error, PartialEq, Eq)]
#[error("unknown invoice status: {0}")]
pub struct InvoiceStatusParseError(pub String);

impl InvoiceStatus {
    pub const ALL: [InvoiceStatus; 5] = [
        InvoiceStatus::Created,
        InvoiceStatus::Funded,
        InvoiceStatus::Fulfilled,
        InvoiceStatus::Recovered,
        InvoiceStatus::Expired,
    ];

    /// The canonical lowercase string form used on the wire and in the database.
    /// This is the single source of truth for the string mapping; [`Display`],
    /// [`FromStr`], and the DB CHECK constraint all agree with it.
    pub fn as_str(&self) -> &'static str {
        match self {
            InvoiceStatus::Created => "created",
            InvoiceStatus::Funded => "funded",
            InvoiceStatus::Fulfilled => "fulfilled",
            InvoiceStatus::Recovered => "recovered",
            InvoiceStatus::Expired => "expired",
        }
    }

    /// Whether the invoice's own payment has reached its final destination.
    /// Late transfers can still be collected from a terminal invoice.
    pub fn is_terminal(&self) -> bool {
        matches!(self, InvoiceStatus::Fulfilled | InvoiceStatus::Recovered)
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

/// The payer's wallet, attested in their session on the network they chose —
/// present only when the request attaches the wallet-attestation add-on — and
/// everything the address commits to beyond the issuance document. The
/// recovery term is never here: it is Gum's own recovery wallet, fixed in
/// the issuance snapshot. Immutable once set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletEvidence {
    pub payer_wallet: Address,
    pub attestation: PayerWalletAttestation,
    /// RFC 3339, when the attestation was accepted.
    pub bound_at: String,
}

/// One request, one chosen network, one address. The wallet evidence is
/// `Some` exactly when the request attached wallet attestation and the payer
/// completed it; a request without that add-on binds its network and derives
/// its address with no payer signature at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentBinding {
    /// The network the payer chose: one of the request's committed networks.
    pub network: NetworkTerms,
    /// Always the issuance snapshot's recovery term: Gum's recovery wallet.
    pub recovery: RecoveryAddress,
    pub salt: Salt,
    pub payment_address: PaymentAddress,
    /// RFC 3339, when the address was registered.
    pub ready_at: String,
    /// The attested wallet and its statement, when wallet attestation is
    /// attached.
    pub wallet: Option<WalletEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invoice {
    pub id: InvoiceId,
    /// Every network the request may be paid on, in canonical order. The
    /// payer picks one when they bind their wallet, or — without the
    /// wallet-attestation add-on — through the network-selection call.
    pub networks: Vec<NetworkTerms>,
    pub beneficiary: BeneficiaryAddress,
    pub expiration_timestamp: u64,
    pub amount: Amount,
    /// Fresh 32-byte value generated at issuance, kept undisclosed until the
    /// address is registered: nobody can derive the address before the
    /// service is watching for its funds.
    pub issuance_nonce: B256,
    /// `None` until the request has an address: after a wallet attestation
    /// when that add-on is attached, otherwise at creation (pinned network)
    /// or network selection.
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
    pub attention_reason: Option<String>,
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

/// Why a binding could not be made for a chain.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum BindError {
    #[error("chain {0} is not one of the request's networks")]
    UnsupportedChain(ChainId),
    #[error(transparent)]
    Attestation(#[from] PayerAttestationError),
    /// The wallet step exists only for a request that attached the
    /// wallet-attestation add-on; every other request binds without one.
    #[error("this request does not attach wallet attestation")]
    WalletAttestationNotAttached,
}

impl Invoice {
    /// Issue an invoice: commit to `snapshot` and hash it, and draw the fresh
    /// issuance nonce the salt will be derived from. The typed parameters are
    /// the ones an address will be derived from, so the snapshot must
    /// describe exactly those; otherwise a proof would verify against terms
    /// nobody was paid under. No address exists yet: see
    /// [`Invoice::bind_network`] and [`Invoice::bind_payer_wallet`].
    pub fn issue(
        currency: Currency,
        networks: &[NetworkTerms],
        beneficiary: BeneficiaryAddress,
        recovery: RecoveryAddress,
        amount: Amount,
        expiration_timestamp: u64,
        snapshot: CanonicalIssuanceSnapshot,
    ) -> Result<Self, AttributionError> {
        let expected = CanonicalIssuanceSnapshot::new(
            snapshot.issuer.clone(),
            snapshot.bill_to.clone(),
            snapshot.payer_verification.clone(),
            currency,
            networks,
            beneficiary,
            recovery,
            amount,
            expiration_timestamp,
        );
        for (field, matches) in [
            ("schema", snapshot.schema == SNAPSHOT_SCHEMA),
            (
                "canonicalization",
                snapshot.canonicalization == CANONICALIZATION,
            ),
            ("currency", snapshot.currency == expected.currency),
            ("decimals", snapshot.decimals == expected.decimals),
            ("networks", snapshot.networks == expected.networks),
            (
                "receiver_address",
                snapshot.receiver_address == expected.receiver_address,
            ),
            (
                "recovery_address",
                snapshot.recovery_address == expected.recovery_address,
            ),
            (
                "amount_base_units",
                snapshot.amount_base_units == expected.amount_base_units,
            ),
            (
                "expiration_timestamp",
                snapshot.expiration_timestamp == expected.expiration_timestamp,
            ),
        ] {
            if !matches {
                return Err(AttributionError::SnapshotMismatch { field });
            }
        }
        if networks.is_empty() {
            return Err(AttributionError::SnapshotMismatch { field: "networks" });
        }

        let attribution = derive_attribution(&snapshot)?;
        Ok(Invoice {
            id: generate_invoice_id(),
            networks: expected
                .networks()
                .expect("networks rendered from typed terms parse back"),
            beneficiary,
            expiration_timestamp,
            amount,
            issuance_nonce: B256::from(rand::rng().random::<[u8; 32]>()),
            binding: None,
            status: InvoiceStatus::Created,
            received: Amount(U256::ZERO),
            execute_tx_hash: None,
            resolved_at_block: None,
            settled_at_timestamp: None,
            attention_reason: None,
            cancellation_requested_at: None,
            attribution_version: ATTRIBUTION_VERSION,
            attribution_hash: attribution.attribution_hash,
            issuance_snapshot: snapshot,
        })
    }

    /// The currency the request is denominated in. Every issued snapshot
    /// names one this build knows; the fallback only guards a row written by
    /// a newer build, which the service does not otherwise handle.
    pub fn currency(&self) -> Currency {
        self.issuance_snapshot.currency().unwrap_or(Currency::Usdc)
    }

    /// The committed terms for `chain_id`, if the request may be paid there.
    pub fn network_for(&self, chain_id: ChainId) -> Option<&NetworkTerms> {
        self.networks
            .iter()
            .find(|network| network.chain_id == chain_id)
    }

    /// The network the payer chose, once an address exists.
    pub fn network(&self) -> Option<&NetworkTerms> {
        self.binding.as_ref().map(|binding| &binding.network)
    }

    /// Whether the request attaches the wallet-attestation add-on.
    pub fn wallet_attestation_required(&self) -> bool {
        self.issuance_snapshot.payer_verification.wallet_attestation
    }

    /// The issuance snapshot's recovery term: always Gum's own recovery
    /// wallet, never a payer's.
    pub fn recovery(&self) -> RecoveryAddress {
        RecoveryAddress(
            self.issuance_snapshot
                .recovery_address
                .parse()
                .expect("a stored snapshot's recovery address parses"),
        )
    }

    /// What the payer's attestation must be for: the chosen network's chain
    /// and factory, and this request's commitment.
    pub fn attestation_scope(&self, chain_id: ChainId) -> Option<PayerAttestationScope> {
        self.network_for(chain_id)
            .map(|network| PayerAttestationScope {
                chain_id: network.chain_id.0,
                factory: network.factory.0,
                attribution_hash: self.attribution_hash,
            })
    }

    /// The address-only binding a request without the wallet-attestation
    /// add-on produces: the salt commits to the issuance nonce and the
    /// document, nothing else. Any wallet may pay the address.
    pub fn bind_network(
        &self,
        chain_id: ChainId,
        ready_at: String,
    ) -> Result<PaymentBinding, BindError> {
        if self.wallet_attestation_required() {
            return Err(BindError::WalletAttestationNotAttached);
        }
        let network = *self
            .network_for(chain_id)
            .ok_or(BindError::UnsupportedChain(chain_id))?;
        let recovery = self.recovery();
        let salt = recompute_salt(self.issuance_nonce, self.attribution_hash, None);
        let payment_address = predict_payment_address(
            network.factory,
            network.token,
            self.amount,
            self.beneficiary,
            self.expiration_timestamp,
            recovery,
            salt,
            network.chain_id,
        );
        Ok(PaymentBinding {
            network,
            recovery,
            salt,
            payment_address,
            ready_at,
            wallet: None,
        })
    }

    /// Derive the binding a verified attestation produces on `chain_id`: the
    /// salt commits to the issuance nonce, the attribution hash, and the
    /// attestation digest, and the address follows from both plus the chosen
    /// network's token, factory, and chain id. The recovery term stays the
    /// snapshot's — Gum's recovery wallet — whatever wallet the payer
    /// attests. The attestation is verified here under that network's domain;
    /// a binding never exists for a signature that does not recover to its
    /// wallet or that was made for another chain. Only a request that
    /// attached the wallet-attestation add-on can bind this way.
    pub fn bind_payer_wallet(
        &self,
        chain_id: ChainId,
        attestation: PayerWalletAttestation,
        bound_at: String,
    ) -> Result<PaymentBinding, BindError> {
        if !self.wallet_attestation_required() {
            return Err(BindError::WalletAttestationNotAttached);
        }
        let network = *self
            .network_for(chain_id)
            .ok_or(BindError::UnsupportedChain(chain_id))?;
        let scope = self
            .attestation_scope(chain_id)
            .expect("network_for succeeded");
        let verified = verify_payer_attestation(&attestation, scope)?;
        let recovery = self.recovery();
        let salt = recompute_salt(
            self.issuance_nonce,
            self.attribution_hash,
            Some(verified.digest),
        );
        let payment_address = predict_payment_address(
            network.factory,
            network.token,
            self.amount,
            self.beneficiary,
            self.expiration_timestamp,
            recovery,
            salt,
            network.chain_id,
        );
        Ok(PaymentBinding {
            network,
            recovery,
            salt,
            payment_address,
            ready_at: bound_at.clone(),
            wallet: Some(WalletEvidence {
                payer_wallet: verified.wallet,
                attestation,
                bound_at,
            }),
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
                binding.network.factory,
                binding.network.token,
                self.amount,
                self.beneficiary,
                self.expiration_timestamp,
                binding.recovery,
                binding.salt,
                binding.network.chain_id,
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
        Amount, BeneficiaryAddress, ChainId, FactoryAddress, Party, PayerAttestation,
        PayerVerification, RecoveryAddress, TokenAddress, predict_payment_address, recompute_salt,
        sign_payer_attestation, wallet_of,
    };
    use alloy_primitives::{U256, address};

    const PAYER_KEY: [u8; 32] = [7u8; 32];
    const MONAD: ChainId = ChainId(143);
    const BASE: ChainId = ChainId(8453);
    /// The snapshot's recovery term: Gum's own recovery wallet in tests.
    const RECOVERY: Address = address!("0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc");

    fn party(name: &str) -> Party {
        Party {
            name: name.into(),
            email: None,
            details: None,
        }
    }

    fn networks() -> Vec<NetworkTerms> {
        vec![
            NetworkTerms {
                chain_id: MONAD,
                token: TokenAddress(address!("0x754704Bc059F8C67012fEd69BC8A327a5aafb603")),
                factory: FactoryAddress(address!("0x0000000000000000000000000000000000000001")),
            },
            NetworkTerms {
                chain_id: BASE,
                token: TokenAddress(address!("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913")),
                factory: FactoryAddress(address!("0x0000000000000000000000000000000000000001")),
            },
        ]
    }

    /// Issue with a minimal snapshot built from the same terms.
    fn issue_with(
        verification: PayerVerification,
        networks: &[NetworkTerms],
        beneficiary: BeneficiaryAddress,
        amount: Amount,
        expiration_timestamp: u64,
    ) -> Invoice {
        let snapshot = CanonicalIssuanceSnapshot::new(
            party("Acme"),
            party("Globex"),
            verification,
            Currency::Usdc,
            networks,
            beneficiary,
            RecoveryAddress(RECOVERY),
            amount,
            expiration_timestamp,
        );
        Invoice::issue(
            Currency::Usdc,
            networks,
            beneficiary,
            RecoveryAddress(RECOVERY),
            amount,
            expiration_timestamp,
            snapshot,
        )
        .unwrap()
    }

    fn issue(
        networks: &[NetworkTerms],
        beneficiary: BeneficiaryAddress,
        amount: Amount,
        expiration_timestamp: u64,
    ) -> Invoice {
        issue_with(
            PayerVerification::default(),
            networks,
            beneficiary,
            amount,
            expiration_timestamp,
        )
    }

    fn sample_invoice() -> Invoice {
        issue(
            &networks(),
            BeneficiaryAddress(address!("0x70997970C51812dc3A010C7d01b50e0d17dc7C01")),
            Amount(U256::from(100)),
            1_900_000_000,
        )
    }

    fn attesting_invoice() -> Invoice {
        issue_with(
            PayerVerification {
                wallet_attestation: true,
                ..PayerVerification::default()
            },
            &networks(),
            BeneficiaryAddress(address!("0x70997970C51812dc3A010C7d01b50e0d17dc7C01")),
            Amount(U256::from(100)),
            1_900_000_000,
        )
    }

    /// The attestation a payer's wallet would sign for `invoice` on `chain`
    /// under a given session nonce.
    fn attest(
        invoice: &Invoice,
        chain: ChainId,
        key: &[u8; 32],
        nonce: u8,
    ) -> PayerWalletAttestation {
        let message = PayerAttestation::new(
            invoice.attribution_hash,
            wallet_of(key),
            B256::repeat_byte(nonce),
            invoice.expiration_timestamp,
        );
        let network = invoice.network_for(chain).unwrap();
        sign_payer_attestation(key, &message, chain.0, network.factory.0)
    }

    fn bound_invoice() -> Invoice {
        let mut invoice = attesting_invoice();
        let binding = invoice
            .bind_payer_wallet(
                MONAD,
                attest(&invoice, MONAD, &PAYER_KEY, 0x11),
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
            [InvoiceStatus::Fulfilled, InvoiceStatus::Recovered]
        );
    }

    #[test]
    fn new_invoice_has_created_status_no_address_and_no_operational_state() {
        let invoice = sample_invoice();
        assert_eq!(invoice.status, InvoiceStatus::Created);
        assert_eq!(invoice.binding, None);
        assert_eq!(invoice.payment_address(), None);
        assert_eq!(invoice.network(), None);
        assert!(invoice.address_matches_parameters());
        assert_eq!(invoice.received, Amount(U256::ZERO));
        assert_eq!(invoice.execute_tx_hash, None);
        assert_eq!(invoice.resolved_at_block, None);
        assert_eq!(invoice.attention_reason, None);
    }

    #[test]
    fn new_invoice_has_valid_uuidv7_id() {
        let invoice = sample_invoice();
        assert_eq!(invoice.id.0.get_version(), Some(Version::SortRand));
    }

    #[test]
    fn new_invoice_stores_all_input_fields_with_networks_in_canonical_order() {
        let beneficiary =
            BeneficiaryAddress(address!("0x70997970C51812dc3A010C7d01b50e0d17dc7C01"));
        let amount = Amount(U256::from(100));
        let expiration_timestamp = 1_900_000_000;
        let mut reversed = networks();
        reversed.reverse();

        let invoice = issue(&reversed, beneficiary, amount, expiration_timestamp);

        assert_eq!(invoice.networks, networks(), "sorted by chain id");
        assert_eq!(invoice.network_for(MONAD), Some(&networks()[0]));
        assert_eq!(invoice.network_for(ChainId(1)), None);
        assert_eq!(invoice.beneficiary, beneficiary);
        assert_eq!(invoice.amount, amount);
        assert_eq!(invoice.expiration_timestamp, expiration_timestamp);
        assert_eq!(invoice.recovery(), RecoveryAddress(RECOVERY));
    }

    #[test]
    fn a_request_without_wallet_attestation_binds_its_network_without_a_signature() {
        let invoice = sample_invoice();
        let binding = invoice
            .bind_network(MONAD, "2026-09-06T00:00:00Z".into())
            .unwrap();
        assert_eq!(binding.network, networks()[0]);
        assert_eq!(binding.recovery, RecoveryAddress(RECOVERY));
        assert_eq!(binding.wallet, None);
        assert_eq!(
            binding.salt,
            recompute_salt(invoice.issuance_nonce, invoice.attribution_hash, None)
        );
        let expected = predict_payment_address(
            binding.network.factory,
            binding.network.token,
            invoice.amount,
            invoice.beneficiary,
            invoice.expiration_timestamp,
            binding.recovery,
            binding.salt,
            MONAD,
        );
        assert_eq!(binding.payment_address, expected);
        // A chain the request does not offer is refused.
        assert_eq!(
            invoice.bind_network(ChainId(1), String::new()).unwrap_err(),
            BindError::UnsupportedChain(ChainId(1))
        );
    }

    #[test]
    fn the_wallet_step_belongs_only_to_requests_that_attach_it() {
        let invoice = sample_invoice();
        assert_eq!(
            invoice
                .bind_payer_wallet(
                    MONAD,
                    attest(&invoice, MONAD, &PAYER_KEY, 0x11),
                    String::new()
                )
                .unwrap_err(),
            BindError::WalletAttestationNotAttached
        );
        let attesting = attesting_invoice();
        assert_eq!(
            attesting.bind_network(MONAD, String::new()).unwrap_err(),
            BindError::WalletAttestationNotAttached
        );
    }

    #[test]
    fn binding_derives_salt_and_address_from_the_attestation_on_the_chosen_chain() {
        // The integration test that ties derive_attribution, the payer
        // attestation, recompute_salt, and predict_payment_address together.
        let invoice = bound_invoice();
        let binding = invoice.binding.as_ref().unwrap();
        let wallet = binding.wallet.as_ref().unwrap();
        assert_eq!(wallet.payer_wallet, wallet_of(&PAYER_KEY));
        // The recovery term is the snapshot's, never the payer's wallet.
        assert_eq!(binding.recovery, RecoveryAddress(RECOVERY));
        assert_ne!(RecoveryAddress(wallet.payer_wallet), binding.recovery);
        assert_eq!(binding.network, networks()[0]);
        assert_eq!(invoice.network(), Some(&networks()[0]));
        let digest: B256 = wallet.attestation.digest.parse().unwrap();
        assert_eq!(
            binding.salt,
            recompute_salt(
                invoice.issuance_nonce,
                invoice.attribution_hash,
                Some(digest)
            )
        );
        let expected = predict_payment_address(
            binding.network.factory,
            binding.network.token,
            invoice.amount,
            invoice.beneficiary,
            invoice.expiration_timestamp,
            binding.recovery,
            binding.salt,
            MONAD,
        );
        assert_eq!(binding.payment_address, expected);
        assert_eq!(invoice.payment_address(), Some(expected));
        assert!(invoice.address_matches_parameters());
    }

    #[test]
    fn the_chain_the_payer_chooses_is_committed_by_the_attestation_domain() {
        let invoice = attesting_invoice();
        let on_monad = invoice
            .bind_payer_wallet(
                MONAD,
                attest(&invoice, MONAD, &PAYER_KEY, 0x11),
                String::new(),
            )
            .unwrap();
        let on_base = invoice
            .bind_payer_wallet(
                BASE,
                attest(&invoice, BASE, &PAYER_KEY, 0x11),
                String::new(),
            )
            .unwrap();
        assert_eq!(on_monad.network.chain_id, MONAD);
        assert_eq!(on_base.network.chain_id, BASE);
        assert_eq!(on_base.network.token, networks()[1].token);
        // Same wallet, same nonce, another chain: another digest, salt, and address.
        assert_ne!(on_monad.salt, on_base.salt);
        assert_ne!(on_monad.payment_address, on_base.payment_address);

        // An attestation signed under one chain's domain cannot bind another.
        assert_eq!(
            invoice
                .bind_payer_wallet(
                    BASE,
                    attest(&invoice, MONAD, &PAYER_KEY, 0x11),
                    String::new()
                )
                .unwrap_err(),
            BindError::Attestation(PayerAttestationError::DomainMismatch)
        );
        // A chain the request does not offer is refused before any verification.
        assert_eq!(
            invoice
                .bind_payer_wallet(
                    ChainId(1),
                    attest(&invoice, MONAD, &PAYER_KEY, 0x11),
                    String::new()
                )
                .unwrap_err(),
            BindError::UnsupportedChain(ChainId(1))
        );
        assert_eq!(invoice.attestation_scope(ChainId(1)), None);
    }

    #[test]
    fn binding_refuses_an_attestation_for_another_request_or_wallet() {
        let invoice = attesting_invoice();
        let other = attesting_invoice();
        assert_eq!(invoice.attribution_hash, other.attribution_hash);
        // The nonce is what makes the otherwise identical requests distinct.
        assert_ne!(invoice.issuance_nonce, other.issuance_nonce);
        let foreign = issue_with(
            PayerVerification {
                wallet_attestation: true,
                ..PayerVerification::default()
            },
            &networks(),
            invoice.beneficiary,
            Amount(U256::from(101)),
            invoice.expiration_timestamp,
        );
        assert_eq!(
            invoice
                .bind_payer_wallet(
                    MONAD,
                    attest(&foreign, MONAD, &PAYER_KEY, 0x11),
                    String::new()
                )
                .unwrap_err(),
            BindError::Attestation(PayerAttestationError::AttributionHashMismatch)
        );
        let mut forged = attest(&invoice, MONAD, &PAYER_KEY, 0x11);
        forged.signature = attest(&invoice, MONAD, &[9u8; 32], 0x11).signature;
        assert_eq!(
            invoice
                .bind_payer_wallet(MONAD, forged, String::new())
                .unwrap_err(),
            BindError::Attestation(PayerAttestationError::SignerMismatch)
        );
    }

    #[test]
    fn tampered_parameters_no_longer_match_the_stored_address() {
        let mut invoice = bound_invoice();
        invoice.beneficiary =
            BeneficiaryAddress(address!("0x0000000000000000000000000000000000000009"));
        assert!(!invoice.address_matches_parameters());
        let mut moved = bound_invoice();
        moved.binding.as_mut().unwrap().network.chain_id = BASE;
        assert!(!moved.address_matches_parameters());
    }

    #[test]
    fn the_attested_wallet_changes_salt_and_address_but_not_recovery() {
        let invoice = attesting_invoice();
        let a = invoice
            .bind_payer_wallet(
                MONAD,
                attest(&invoice, MONAD, &PAYER_KEY, 0x11),
                String::new(),
            )
            .unwrap();
        let c = invoice
            .bind_payer_wallet(
                MONAD,
                attest(&invoice, MONAD, &[9u8; 32], 0x11),
                String::new(),
            )
            .unwrap();
        assert_ne!(a.salt, c.salt);
        // Whatever wallet attests, funds recover to Gum's wallet.
        assert_eq!(a.recovery, c.recovery);
        assert_eq!(a.recovery, RecoveryAddress(RECOVERY));
        assert_ne!(a.payment_address, c.payment_address);
    }

    #[test]
    fn expiration_recovery_and_chain_are_address_parameters() {
        let invoice = bound_invoice();
        let binding = invoice.binding.as_ref().unwrap();
        let derive = |expiration: u64, recovery: RecoveryAddress, chain: ChainId| {
            predict_payment_address(
                binding.network.factory,
                binding.network.token,
                invoice.amount,
                invoice.beneficiary,
                expiration,
                recovery,
                binding.salt,
                chain,
            )
        };
        let later_expiration = derive(invoice.expiration_timestamp + 1, binding.recovery, MONAD);
        let other_recovery = derive(
            invoice.expiration_timestamp,
            RecoveryAddress(address!("0x0000000000000000000000000000000000000003")),
            MONAD,
        );
        let other_chain = derive(invoice.expiration_timestamp, binding.recovery, BASE);

        assert_ne!(binding.payment_address, later_expiration);
        assert_ne!(binding.payment_address, other_recovery);
        assert_ne!(binding.payment_address, other_chain);
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
        assert_eq!(invoice.issuance_snapshot.networks[0].chain_id, "143");
        assert_eq!(invoice.issuance_snapshot.networks[1].chain_id, "8453");
        assert_eq!(
            invoice.issuance_snapshot.receiver_address,
            invoice.beneficiary.0.to_checksum(None)
        );
        assert_eq!(
            invoice.issuance_snapshot.recovery_address,
            RECOVERY.to_checksum(None)
        );
    }

    #[test]
    fn issue_refuses_a_snapshot_that_describes_other_terms() {
        let beneficiary =
            BeneficiaryAddress(address!("0x70997970C51812dc3A010C7d01b50e0d17dc7C01"));
        let amount = Amount(U256::from(100));
        let snapshot = || {
            CanonicalIssuanceSnapshot::new(
                party("Acme"),
                party("Globex"),
                PayerVerification::default(),
                Currency::Usdc,
                &networks(),
                beneficiary,
                RecoveryAddress(RECOVERY),
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
                s.receiver_address = s.networks[0].factory_address.clone();
                s
            }),
            ("recovery_address", {
                let mut s = snapshot();
                s.recovery_address = s.networks[0].factory_address.clone();
                s
            }),
            ("networks", {
                let mut s = snapshot();
                s.networks[0].token_address = s.networks[0].token_address.to_lowercase();
                s
            }),
            ("networks", {
                let mut s = snapshot();
                s.networks.pop();
                s
            }),
            ("schema", {
                let mut s = snapshot();
                s.schema = "gum.invoice".into();
                s
            }),
        ];
        for (field, snapshot) in mismatches {
            let error = Invoice::issue(
                Currency::Usdc,
                &networks(),
                beneficiary,
                RecoveryAddress(RECOVERY),
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
        // No network at all is not a request anyone could pay.
        let empty = CanonicalIssuanceSnapshot::new(
            party("Acme"),
            party("Globex"),
            PayerVerification::default(),
            Currency::Usdc,
            &[],
            beneficiary,
            RecoveryAddress(RECOVERY),
            amount,
            1_900_000_000,
        );
        assert!(matches!(
            Invoice::issue(
                Currency::Usdc,
                &[],
                beneficiary,
                RecoveryAddress(RECOVERY),
                amount,
                1_900_000_000,
                empty
            )
            .unwrap_err(),
            AttributionError::SnapshotMismatch { field: "networks" }
        ));
    }

    #[test]
    fn address_matches_parameters_uses_stored_binding_only() {
        // Attribution metadata is proof material. Losing or corrupting it
        // must never make a funded address look unsafe to sweep.
        let mut invoice = bound_invoice();
        invoice.attribution_hash = B256::repeat_byte(0xFF);
        invoice.issuance_snapshot.notes = Some("rewritten after the fact".into());
        invoice.issuance_snapshot.amount_base_units = "999".into();
        invoice.attribution_version = 7;
        invoice.networks.clear();
        assert!(invoice.address_matches_parameters());

        invoice.binding.as_mut().unwrap().salt = Salt(B256::repeat_byte(0x01));
        assert!(!invoice.address_matches_parameters());
    }
}
